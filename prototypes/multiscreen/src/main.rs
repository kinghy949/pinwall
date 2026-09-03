//! PinWall 原型 4：多显示器坐标与跨屏行为（R4）
//!
//! 用法： multiscreen <screen0|screen1|union>
//!   screenN —— 把贴图面板放到第 N 块屏中央，报告其所在屏与 backingScaleFactor
//!   union   —— 建一个覆盖**所有屏并集**的遮罩，验证负坐标与跨屏铺满
//!
//! 关键点：macOS 全局坐标系原点在主屏左下角，外接屏可位于负坐标区。
//! 任何「从 (0,0) 开始」的假设都会在多屏下出错。

use std::env;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEventMask,
    NSPanel, NSScreen, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize};

fn make_panel(mtm: MainThreadMarker, frame: NSRect, alpha: f64) -> Retained<NSPanel> {
    let p: Retained<NSPanel> = unsafe {
        NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        p.setFloatingPanel(true);
        p.setHidesOnDeactivate(false);
        p.setOpaque(false);
        p.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            1.0, 0.23, 0.19, alpha,
        )));
        p.setLevel(1000);
        p.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        p.orderFrontRegardless();
    }
    p
}

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "screen0".into());
    let mtm = MainThreadMarker::new().expect("须主线程");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let screens = NSScreen::screens(mtm);
    println!("显示器 {} 块：", screens.len());
    for (i, s) in screens.iter().enumerate() {
        let f = s.frame();
        println!(
            "  [{i}] {} frame=({:.0},{:.0}) {:.0}x{:.0} scale={}",
            s.localizedName(), f.origin.x, f.origin.y, f.size.width, f.size.height,
            s.backingScaleFactor()
        );
    }

    // 计算所有屏的并集 —— 这是全屏捕获遮罩必须覆盖的范围
    let mut min_x = f64::MAX; let mut min_y = f64::MAX;
    let mut max_x = f64::MIN; let mut max_y = f64::MIN;
    for s in screens.iter() {
        let f = s.frame();
        min_x = min_x.min(f.origin.x);
        min_y = min_y.min(f.origin.y);
        max_x = max_x.max(f.origin.x + f.size.width);
        max_y = max_y.max(f.origin.y + f.size.height);
    }
    println!(
        "\n所有屏并集: 原点=({min_x:.0},{min_y:.0}) 尺寸={:.0}x{:.0}",
        max_x - min_x, max_y - min_y
    );
    if min_x < 0.0 || min_y < 0.0 {
        println!("  ⚠ 并集原点为负 —— 遮罩不能假定从 (0,0) 起算");
    }

    // perscreen：每块屏各建一个遮罩。
    // 因「显示器各自拥有独立空间」为 macOS 默认，单个窗口无法跨屏，
    // 全屏捕获遮罩必须按屏拆分。
    if mode == "perscreen" {
        println!("\n模式: perscreen —— 每块屏各建一个遮罩");
        let mut panels = Vec::new();
        for (i, s) in screens.iter().enumerate() {
            let f = s.frame();
            println!("  屏[{i}] 遮罩 origin=({:.0},{:.0}) size={:.0}x{:.0}", f.origin.x, f.origin.y, f.size.width, f.size.height);
            panels.push(make_panel(mtm, f, 0.30));
        }
        app.finishLaunching();
        for tick in 1..=4 {
            let until = unsafe { NSDate::dateWithTimeIntervalSinceNow(1.0) };
            if let Some(e) = unsafe {
                app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any, Some(&until), NSDefaultRunLoopMode, true)
            } { unsafe { app.sendEvent(&e) }; }
            let where_: Vec<String> = panels.iter().map(|p| match p.screen() {
                Some(s) => s.localizedName().to_string(),
                None => "<无>".into(),
            }).collect();
            println!("[{tick}s] 各遮罩所在屏: {where_:?}");
        }
        return;
    }

    let panel = match mode.as_str() {
        "union" => {
            let r = NSRect::new(
                NSPoint::new(min_x, min_y),
                NSSize::new(max_x - min_x, max_y - min_y),
            );
            println!("\n模式: union —— 建立覆盖全部显示器的遮罩");
            make_panel(mtm, r, 0.30)
        }
        m => {
            let idx: usize = m.trim_start_matches("screen").parse().unwrap_or(0);
            let s = screens.iter().nth(idx).expect("显示器索引越界");
            let f = s.frame();
            let r = NSRect::new(
                NSPoint::new(
                    f.origin.x + f.size.width / 2.0 - 180.0,
                    f.origin.y + f.size.height / 2.0 - 110.0,
                ),
                NSSize::new(360.0, 220.0),
            );
            println!("\n模式: {m} —— 面板置于该屏中央 origin=({:.0},{:.0})", r.origin.x, r.origin.y);
            make_panel(mtm, r, 0.95)
        }
    };

    app.finishLaunching();

    // 报告面板实际落在哪块屏、其 scale 为多少
    let mut tick = 0;
    loop {
        let until = unsafe { NSDate::dateWithTimeIntervalSinceNow(1.0) };
        if let Some(e) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any, Some(&until), NSDefaultRunLoopMode, true)
        } { unsafe { app.sendEvent(&e) }; }

        tick += 1;
        let on = panel.screen();
        let (name, scale) = match &on {
            Some(s) => (s.localizedName().to_string(), s.backingScaleFactor()),
            None => ("<不在任何屏上>".to_string(), 0.0),
        };
        let bsf = panel.backingScaleFactor();
        println!(
            "[{tick:>2}s] 面板所在屏={name} 屏scale={scale} 窗口backingScale={bsf} visible={} onActiveSpace={}",
            panel.isVisible(), panel.isOnActiveSpace()
        );
        if tick >= 4 { break; }
    }
}
