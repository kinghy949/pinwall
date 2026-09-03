//! PinWall 原型 1c：NSPanel 路线验证
//!
//! 背景：原型 1a/1b 已证明 winit 创建的 NSWindow 配合任意
//! level × collectionBehavior 组合，均无法进入他人全屏应用的 Space
//! （isVisible=true 但 isOnActiveSpace=false）。
//!
//! 本原型绕开 winit，直接用 objc2 创建 NSPanel，验证结构性假设：
//! 覆盖全屏所需的是 **NSPanel + NonactivatingPanel**，而非 NSWindow。
//! 这是 Raycast / Alfred / CleanShot X 一类工具的通行做法。

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEventMask,
    NSPanel, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize};

fn main() {
    let mtm = MainThreadMarker::new().expect("必须在主线程运行");
    let app = NSApplication::sharedApplication(mtm);
    // 无 Dock 图标；.app bundle 的 LSUIElement 亦会固化该身份
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let rect = NSRect::new(NSPoint::new(200.0, 400.0), NSSize::new(360.0, 220.0));

    // 关键：NonactivatingPanel —— 面板获得点击时不激活所属应用，
    // 这正是覆盖层窗口能出现在他人全屏 Space 上的结构前提。
    let style = NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Borderless;

    let panel: Retained<NSPanel> = unsafe {
        NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };

    unsafe {
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setBackgroundColor(Some(&NSColor::systemRedColor()));
        panel.setOpaque(true);
        panel.setLevel(1000); // NSScreenSaverWindowLevel
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.orderFrontRegardless();
    }

    println!(
        r#"
╔════════════════════════════════════════════════════════════╗
║  PinWall 原型 1c · NSPanel 路线验证                        ║
╠════════════════════════════════════════════════════════════╣
║  屏幕上应出现一个红色方块（无边框，约 360x220）            ║
║  请把任意 App 切到全屏，观察它是否仍然可见                 ║
║  每秒打印一次探针，Ctrl-C 退出                             ║
╚════════════════════════════════════════════════════════════╝
"#
    );

    app.finishLaunching();

    // 手动泵事件循环，以便每秒插入一次状态探针（AppKit 调用须在主线程）
    let mut tick: u64 = 0;
    loop {
        let until = unsafe { NSDate::dateWithTimeIntervalSinceNow(1.0) };
        let ev = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        };
        if let Some(ev) = ev {
            unsafe { app.sendEvent(&ev) };
        }

        tick += 1;
        let visible = unsafe { panel.isVisible() };
        let on_space = unsafe { panel.isOnActiveSpace() };
        let level = unsafe { panel.level() };
        println!(
            "[{tick:>3}s] NSPanel: isVisible={visible} isOnActiveSpace={on_space} level={level}"
        );
    }
}
