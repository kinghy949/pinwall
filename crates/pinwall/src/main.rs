//! PinWall —— 把截图钉在屏幕上。
//!
//! 当前实现的最小闭环：
//!   全局热键 → 每屏遮罩 → 框选（可跨屏）→ 分屏捕获并拼接 → 贴为置顶浮窗
//!
//! 尚未接入：标注编辑、历史库、上传工作流。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

use pinwall_capture::{capture_selection, current_capturer, permission_status, Capturer, Permission};
use pinwall_core::{Event, Outcome, Selection, SelectionMachine};
use pinwall_platform::geom::Rect;
use pinwall_platform::{current_platform, OverlaySet, PinImage, PinWindow, Platform, PointerEvent};

fn main() {
    let mtm = MainThreadMarker::new().expect("须在主线程运行");
    let app = NSApplication::sharedApplication(mtm);
    // 无 Dock 图标；正式打包时应由 Info.plist 的 LSUIElement 固化
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    if permission_status() == Permission::Denied {
        eprintln!("未获得「屏幕录制」权限。");
        eprintln!("请在 系统设置 → 隐私与安全性 → 屏幕录制 中授权，然后重新启动本程序。");
        std::process::exit(1);
    }

    let platform = match current_platform() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("初始化窗口层失败: {e}");
            std::process::exit(1);
        }
    };
    let capturer = match current_capturer() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("初始化捕获层失败: {e}");
            std::process::exit(1);
        }
    };

    let manager = GlobalHotKeyManager::new().expect("热键管理器创建失败");
    let capture_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyA);
    let clear_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyX);
    manager.register(capture_key).expect("注册 ⌘⇧A 失败");
    manager.register(clear_key).expect("注册 ⌘⇧X 失败");

    println!(
        r#"
PinWall  —— 把截图钉在屏幕上

  ⌘⇧A   截图并贴到屏幕上（拖拽框选，右键取消）
  ⌘⇧X   关闭所有贴图
  Ctrl-C 退出

贴图上：拖拽移动，双击或右键关闭

已就绪。
"#
    );

    app.finishLaunching();

    let rx = GlobalHotKeyEvent::receiver();
    let mut pins: Vec<Box<dyn PinWindow>> = Vec::new();

    loop {
        let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
        if let Some(e) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            app.sendEvent(&e);
        }

        // 回收用户自行关掉的贴图（双击 / 右键）。窗口关闭由用户在窗口上
        // 直接触发，上层无从感知，只能轮询回收，否则 Box 会一直堆着。
        pins.retain(|p: &Box<dyn PinWindow>| !p.is_closed());

        while let Ok(ev) = rx.try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if ev.id == capture_key.id() {
                match capture_and_pin(&app, platform.as_ref(), capturer.as_ref()) {
                    Ok(Some(pin)) => {
                        pins.push(pin);
                        println!("已贴图，当前共 {} 张（⌘⇧X 全部关闭）", pins.len());
                    }
                    Ok(None) => println!("已取消"),
                    Err(e) => eprintln!("失败: {e}"),
                }
            } else if ev.id == clear_key.id() {
                let n = pins.len();
                for p in pins.drain(..) {
                    p.close();
                }
                println!("已关闭 {n} 张贴图");
            }
        }
    }
}

/// 走一遍完整流程：铺遮罩 → 等框选 → 捕获 → 贴图。
///
/// 返回 `Ok(None)` 表示用户取消。
///
/// # 为什么用事件队列而不是在回调里直接驱动状态机
///
/// 遮罩持有回调，若回调再捕获 `Rc<OverlaySet>`，就构成
/// `OverlaySet → Overlay → view → 闭包 → OverlaySet` 的循环引用，
/// 引用计数永远降不到零，遮罩无法释放。
///
/// 改为「回调只入队、主循环消费」后：所有权是单向的
/// （overlays 持有闭包，闭包持有队列），无环；状态机成为主循环的
/// 局部变量，也不再需要 `RefCell`，顺带消除了嵌套借用 panic 的可能。
fn capture_and_pin(
    app: &NSApplication,
    platform: &dyn Platform,
    capturer: &dyn Capturer,
) -> Result<Option<Box<dyn PinWindow>>, Box<dyn std::error::Error>> {
    // 每次都重新枚举 —— 显示器可能在两次截图之间发生热插拔
    let screens = platform.screens()?;
    let overlays = OverlaySet::covering_all_screens(platform)?;

    let queue: Rc<RefCell<VecDeque<PointerEvent>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = queue.clone();
        overlays.set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            q.borrow_mut().push_back(ev);
        }));
    }

    overlays.show();

    let mut machine = SelectionMachine::new(screens);
    let mut result: Option<Selection> = None;

    'session: loop {
        let until = NSDate::dateWithTimeIntervalSinceNow(0.01);
        if let Some(e) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            app.sendEvent(&e);
        }

        loop {
            // 先取出再处理，不要在持有借用时调用状态机
            let next = queue.borrow_mut().pop_front();
            let Some(ev) = next else { break };
            let e = match ev {
                PointerEvent::Down(p) => Event::Down(p),
                PointerEvent::Moved(p) => Event::Move(p),
                PointerEvent::Up(p) => Event::Up(p),
                PointerEvent::Cancel => Event::Cancel,
            };
            match machine.handle(e) {
                // 选区可能跨屏，必须广播给全部遮罩，各自求交后绘制
                Outcome::Redraw => overlays.set_selection(machine.current_rect()),
                Outcome::Committed(sel) => {
                    result = Some(sel);
                    break 'session;
                }
                Outcome::Cancelled => break 'session,
                Outcome::Idle => {}
            }
        }
    }

    overlays.hide();
    // 必须显式关闭：遮罩每次截图都会重建，只隐藏会持续累积
    overlays.close();

    let Some(sel) = result else {
        return Ok(None);
    };

    let img = capture_selection(capturer, &sel)?;
    // 贴在原位置：视觉上就像把那块画面「冻结」在了原地
    let pin = platform.create_pin(Rect::from_xywh(
        sel.rect.origin.x,
        sel.rect.origin.y,
        sel.rect.size.width,
        sel.rect.size.height,
    ))?;
    pin.set_image(&PinImage {
        width: img.width,
        height: img.height,
        scale: img.scale,
        bgra: &img.bgra,
    })?;
    pin.show();
    Ok(Some(pin))
}
