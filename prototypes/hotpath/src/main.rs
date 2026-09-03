//! PinWall 原型 3：热路径延迟实测
//!
//! 目标指标：**按下快捷键 → 选区遮罩首帧 < 50ms**（理想 16ms，一帧）。
//!
//! 同时验证技术选型中的一条断言：
//!   「遮罩窗口预创建但不显示 —— 这是唯一值得的预热」
//! 因此冷热两条路径都测，用数据判断预热到底买到了多少。
//!
//! 遮罩使用 NSPanel（原型 1 结论：NSWindow 无法覆盖他人全屏应用）。
//!
//! 测量口径说明：计时起点是**热键回调触发**，而非物理按键按下。
//! 从按键到回调之间的系统事件派发耗时无法从进程内测量，未计入。

use std::time::{Duration, Instant};

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEventMask,
    NSPanel, NSScreen, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRect};

const ITERS: usize = 12;

/// 按原型 1 的结论构造遮罩面板：NSPanel + NonactivatingPanel。
fn make_overlay(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSPanel> {
    let panel: Retained<NSPanel> = unsafe {
        NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setOpaque(false);
        // 半透明压暗，模拟真实选区遮罩
        panel.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.0, 0.0, 0.0, 0.35,
        )));
        panel.setLevel(1000);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
    }
    panel
}

fn stats(mut v: Vec<Duration>) -> (f64, f64, f64) {
    v.sort();
    let ms = |d: &Duration| d.as_secs_f64() * 1000.0;
    (ms(&v[0]), ms(&v[v.len() / 2]), ms(&v[v.len() - 1]))
}

fn main() {
    let mtm = MainThreadMarker::new().expect("须在主线程");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let screen_frame = NSScreen::mainScreen(mtm)
        .expect("拿不到主屏")
        .frame();

    // —— 预热路径：启动时就把遮罩建好，隐藏待命 ——
    let warm = make_overlay(mtm, screen_frame);

    let manager = GlobalHotKeyManager::new().expect("热键管理器创建失败");
    let hotkey = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyA);
    manager.register(hotkey).expect("热键注册失败");

    println!(
        r#"
╔══════════════════════════════════════════════════════════════╗
║  PinWall 原型 3 · 热路径延迟实测                             ║
╠══════════════════════════════════════════════════════════════╣
║  按 ⌘⇧A 触发一轮测量（冷 / 热 各 {ITERS} 次）                     ║
║  屏幕会闪烁数次，属正常现象                                  ║
║                                                              ║
║  目标：热路径 < 50ms（理想 16ms）                            ║
║  Ctrl-C 退出                                                 ║
╚══════════════════════════════════════════════════════════════╝

主屏尺寸: {:.0}x{:.0}
等待热键…
"#,
        screen_frame.size.width, screen_frame.size.height
    );

    app.finishLaunching();
    let rx = GlobalHotKeyEvent::receiver();

    loop {
        let until = unsafe { NSDate::dateWithTimeIntervalSinceNow(0.02) };
        if let Some(ev) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            unsafe { app.sendEvent(&ev) };
        }

        while let Ok(e) = rx.try_recv() {
            if e.state != global_hotkey::HotKeyState::Pressed {
                continue;
            }

            let win_before = app.windows().len();

            // ---------- 热路径：面板已存在，只需排入并强制绘制 ----------
            let mut warm_times = Vec::with_capacity(ITERS);
            for _ in 0..ITERS {
                objc2::rc::autoreleasepool(|_| {
                    let t0 = Instant::now();
                    unsafe {
                        warm.orderFrontRegardless();
                        warm.display(); // 强制同步绘制，逼出首帧
                    }
                    warm_times.push(t0.elapsed());
                    unsafe { warm.orderOut(None) };
                });
            }

            // ---------- 冷路径：每次现建面板 ----------
            let mut cold_times = Vec::with_capacity(ITERS);
            for _ in 0..ITERS {
                objc2::rc::autoreleasepool(|_| {
                    let t0 = Instant::now();
                    let p = make_overlay(mtm, screen_frame);
                    unsafe {
                        p.orderFrontRegardless();
                        p.display();
                    }
                    cold_times.push(t0.elapsed());
                    // 关键：必须 close()。仅 orderOut() 只是隐藏，
                    // 面板仍留在 NSApp 的窗口列表中，累积后会拖慢
                    // 所有窗口的排序操作 —— 首版基准正是栽在这里。
                    unsafe { p.close() };
                });
            }

            let win_after = app.windows().len();

            let (wmin, wmed, wmax) = stats(warm_times);
            let (cmin, cmed, cmax) = stats(cold_times);

            println!("\n──────────── 一轮测量（各 {ITERS} 次）────────────");
            println!("  路径      最小      中位      最大");
            println!("  预热   {wmin:7.2}ms {wmed:7.2}ms {wmax:7.2}ms   <- 产品应走这条");
            println!("  冷建   {cmin:7.2}ms {cmed:7.2}ms {cmax:7.2}ms");
            println!("  预热省下（中位）: {:.2}ms", cmed - wmed);
            let verdict = if wmed < 16.0 {
                "达标 —— 优于一帧(16ms)"
            } else if wmed < 50.0 {
                "达标 —— 在 50ms 预算内"
            } else {
                "未达标 —— 超出 50ms 预算"
            };
            println!("  结论: {verdict}");
            println!("  窗口列表: {win_before} -> {win_after}  (增长即为泄漏)");
            println!("  注：计时起点为热键回调，不含系统事件派发耗时\n");
        }
    }
}
