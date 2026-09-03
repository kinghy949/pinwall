//! PinWall —— 把截图钉在屏幕上。
//!
//! 当前实现的最小闭环：
//!   全局热键 → 每屏遮罩 → 框选（可跨屏）→ 分屏捕获并拼接 → 贴为置顶浮窗
//!
//! 尚未接入：标注编辑、历史库、上传工作流。

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::VecDeque;
use std::rc::Rc;

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

use pinwall_capture::{
    capture_selection, current_capturer, encode_png, permission_status, CapturedImage, Capturer,
    Permission,
};
use pinwall_core::annotation::{AnnotationEditor, EditEvent, EditOutcome, Shape, Tool};
use pinwall_core::{Event, Outcome, Selection, SelectionMachine};
use pinwall_platform::geom::Rect;
use pinwall_platform::{
    copy_image_to_clipboard, current_platform, DrawCommand, OverlaySet, PinImage, PinWindow,
    Platform, PointerEvent, Rgba,
};

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
    let through_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyT);
    let save_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyS);
    let copy_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyC);
    let annotate_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyE);
    let undo_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyZ);
    let tool_keys = [
        (HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Digit1), Tool::Select),
        (HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Digit2), Tool::Rect),
        (HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Digit3), Tool::Arrow),
        (HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Digit4), Tool::Redact),
    ];
    manager.register(capture_key).expect("注册 ⌘⇧A 失败");
    manager.register(clear_key).expect("注册 ⌘⇧X 失败");
    manager.register(through_key).expect("注册 ⌘⇧T 失败");
    manager.register(save_key).expect("注册 ⌘⇧S 失败");
    manager.register(copy_key).expect("注册 ⌘⇧C 失败");
    manager.register(annotate_key).expect("注册 ⌘⇧E 失败");
    manager.register(undo_key).expect("注册 ⌘⇧Z 失败");
    for (k, _) in &tool_keys {
        manager.register(*k).expect("注册工具快捷键失败");
    }

    println!(
        r#"
PinWall  —— 把截图钉在屏幕上

  ⌘⇧A   截图并贴到屏幕上（拖拽框选，右键取消）
  ⌘⇧X   关闭所有贴图
  ⌘⇧T   切换所有贴图的鼠标穿透
  ⌘⇧S   把最近一张贴图存到桌面
  ⌘⇧C   把最近一张贴图复制到剪贴板
  ⌘⇧E   在最近一张贴图上进出标注模式
  ⌘⇧Z   撤销标注
  Ctrl-C 退出

标注模式下：⌘⇧1 选择  ⌘⇧2 矩形  ⌘⇧3 箭头  ⌘⇧4 打码
            拖拽绘制，右键删除选中

截图完成后会自动复制到剪贴板。

贴图上：
  拖拽        移动
  滚轮        缩放（以光标为锚点）
  Shift+滚轮  调透明度（Option+滚轮亦可）
  中键        切换鼠标穿透
  双击 / 右键 关闭

已就绪。
"#
    );

    app.finishLaunching();

    let rx = GlobalHotKeyEvent::receiver();
    // 图像与窗口一同保存：存盘与复制都需要原始像素，
    // 而窗口本身不保留可读回的位图
    let mut pins: Vec<Pin> = Vec::new();

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
        pins.retain(|p: &Pin| !p.window.is_closed());

        // 驱动处于标注模式的贴图
        for pin in pins.iter_mut() {
            if pin.pump_annotation() {
                let cmds = pin.draw_commands();
                pin.window.set_draw_commands(&cmds);
            }
        }

        while let Ok(ev) = rx.try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if ev.id == capture_key.id() {
                match capture_and_pin(&app, platform.as_ref(), capturer.as_ref()) {
                    Ok(Some(pin)) => {
                        // 截图即入剪贴板：多数使用场景下一步就是粘贴，
                        // 让用户不必再多按一次
                        match copy_image_to_clipboard(&pin.as_pin_image()) {
                            Ok(()) => println!(
                                "已贴图并复制到剪贴板，当前共 {} 张（⌘⇧X 全部关闭）",
                                pins.len() + 1
                            ),
                            Err(e) => println!("已贴图（复制到剪贴板失败: {e}）"),
                        }
                        pins.push(pin);
                    }
                    Ok(None) => println!("已取消"),
                    Err(e) => eprintln!("失败: {e}"),
                }
            } else if ev.id == clear_key.id() {
                let n = pins.len();
                for p in pins.drain(..) {
                    p.window.close();
                }
                println!("已关闭 {n} 张贴图");
            } else if ev.id == annotate_key.id() {
                match pins.last_mut() {
                    Some(p) => {
                        let on = !p.window.is_annotation_mode();
                        p.window.set_annotation_mode(on);
                        println!(
                            "{}标注模式（⌘⇧1 选择 / ⌘⇧2 矩形 / ⌘⇧3 箭头 / ⌘⇧4 打码，右键删除选中）",
                            if on { "已进入" } else { "已退出" }
                        );
                    }
                    None => println!("当前没有贴图"),
                }
            } else if ev.id == undo_key.id() {
                if let Some(p) = pins.last_mut() {
                    if p.editor.undo() {
                        let cmds = p.draw_commands();
                        p.window.set_draw_commands(&cmds);
                    }
                }
            } else if let Some((_, tool)) = tool_keys.iter().find(|(k, _)| ev.id == k.id()) {
                if let Some(p) = pins.last_mut() {
                    // 文字工具需原生输入控件，尚未接入，故未占用快捷键位
                    let t = *tool;
                    p.editor.set_tool(t);
                    let cmds = p.draw_commands();
                    p.window.set_draw_commands(&cmds);
                    println!("工具: {t:?}");
                }
            } else if ev.id == save_key.id() {
                match pins.last() {
                    Some(p) => match save_to_desktop(&p.image) {
                        Ok(path) => println!("已保存 {}", path.display()),
                        Err(e) => eprintln!("保存失败: {e}"),
                    },
                    None => println!("当前没有贴图可保存"),
                }
            } else if ev.id == copy_key.id() {
                match pins.last() {
                    Some(p) => match copy_image_to_clipboard(&p.as_pin_image()) {
                        Ok(()) => println!("已复制最近一张贴图到剪贴板"),
                        Err(e) => eprintln!("复制失败: {e}"),
                    },
                    None => println!("当前没有贴图可复制"),
                }
            } else if ev.id == through_key.id() {
                // 以「是否存在未穿透的贴图」决定统一开还是统一关，
                // 避免逐张取反导致状态参差不齐
                let turn_on = pins.iter().any(|p| !p.window.is_click_through());
                for p in &pins {
                    p.window.set_click_through(turn_on);
                }
                println!(
                    "{} {} 张贴图的鼠标穿透",
                    if turn_on { "已开启" } else { "已关闭" },
                    pins.len()
                );
            }
        }
    }
}


/// 一张贴图：窗口 + 其原始像素。
///
/// 必须同时保存像素 —— 窗口只负责显示，不提供读回位图的通路，
/// 而存盘与复制到剪贴板都需要原始数据。
struct Pin {
    window: Box<dyn PinWindow>,
    image: CapturedImage,
    editor: AnnotationEditor,
    /// 标注模式下的指针事件队列。
    ///
    /// 与遮罩同理：回调若直接驱动编辑器，就要捕获对编辑器的共享引用，
    /// 既构成循环也带来嵌套借用风险。改为只入队、主循环消费。
    events: Rc<RefCell<VecDeque<PointerEvent>>>,
}

impl Pin {
    fn as_pin_image(&self) -> PinImage<'_> {
        PinImage {
            width: self.image.width,
            height: self.image.height,
            scale: self.image.scale,
            bgra: &self.image.bgra,
        }
    }

    /// 把标注模型翻译成窗口层认识的绘制指令。
    ///
    /// 这层翻译是必要的：标注模型在 pinwall-core，而它依赖
    /// pinwall-platform 的几何类型，窗口层无法反向依赖它。
    fn draw_commands(&self) -> Vec<DrawCommand> {
        let mut out = Vec::with_capacity(self.editor.objects().len() + 1);
        for o in self.editor.objects() {
            let color = Rgba::new(o.color.r, o.color.g, o.color.b, o.color.a);
            out.push(match &o.shape {
                Shape::Rect => DrawCommand::Rect { rect: o.bounds(), color, width: o.width },
                Shape::Arrow => DrawCommand::Arrow {
                    from: o.a,
                    to: o.b,
                    color,
                    width: o.width,
                },
                Shape::Redact => DrawCommand::Redact { rect: o.bounds() },
                Shape::Text(t) => DrawCommand::Text {
                    origin: o.a,
                    text: t.clone(),
                    color,
                    size: 18.0,
                },
            });
        }
        if let Some(i) = self.editor.selected() {
            if let Some(o) = self.editor.objects().get(i) {
                out.push(DrawCommand::SelectionBox { rect: o.bounds() });
            }
        }
        out
    }

    /// 消费队列中的指针事件，返回是否需要重绘。
    fn pump_annotation(&mut self) -> bool {
        let mut dirty = false;
        loop {
            let next = self.events.borrow_mut().pop_front();
            let Some(ev) = next else { break };
            let outcome = match ev {
                PointerEvent::Down(p) => self.editor.handle(EditEvent::Down(p)),
                PointerEvent::Moved(p) => self.editor.handle(EditEvent::Move(p)),
                PointerEvent::Up(p) => self.editor.handle(EditEvent::Up(p)),
                // 标注模式下右键用于删除选中对象
                PointerEvent::Cancel => {
                    if self.editor.delete_selected() {
                        EditOutcome::Redraw
                    } else {
                        EditOutcome::Idle
                    }
                }
            };
            match outcome {
                EditOutcome::Redraw => dirty = true,
                // 文字输入需要原生输入控件，尚未接入
                EditOutcome::BeginTextInput(_) => dirty = true,
                EditOutcome::Idle => {}
            }
        }
        dirty
    }
}

/// 把图像存到桌面，文件名带时间戳。
///
/// 桌面是 macOS 截图的惯例位置；取不到主目录时退回当前工作目录。
fn save_to_desktop(img: &CapturedImage) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Desktop"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("."));

    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut path = dir.join(format!("PinWall-{stamp}.png"));
    // 同一秒内连续保存不应互相覆盖
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("PinWall-{stamp}-{n}.png"));
        n += 1;
    }

    std::fs::write(&path, encode_png(img)?)?;
    Ok(path)
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
) -> Result<Option<Pin>, Box<dyn std::error::Error>> {
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
    // 标注事件队列在建窗时就装好，进入标注模式时无需再改回调
    let events: Rc<RefCell<VecDeque<PointerEvent>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = events.clone();
        pin.set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            q.borrow_mut().push_back(ev);
        }));
    }

    Ok(Some(Pin {
        window: pin,
        image: img,
        editor: AnnotationEditor::new(),
        events,
    }))
}
