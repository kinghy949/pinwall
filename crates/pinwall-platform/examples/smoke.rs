//! 冒烟测试：枚举显示器、每屏建遮罩、建一个贴图浮窗并跨屏移动。
//!
//! 运行： cargo run -p pinwall-platform --example smoke

use pinwall_platform::geom::{Point, Rect};
use pinwall_platform::{current_platform, OverlaySet};

fn main() -> pinwall_platform::Result<()> {
    let platform = current_platform()?;

    // ---- 1. 显示器枚举（本 crate 坐标系：主屏左上角为原点，y 向下）----
    let screens = platform.screens()?;
    println!("显示器 {} 块：", screens.len());
    let mut all = None::<Rect>;
    for s in &screens {
        let (pw, ph) = s.pixel_size();
        println!(
            "  [{}] {:24} frame=({:>6.0},{:>5.0}) {:.0}x{:.0}  scale={}  像素={}x{}{}",
            s.id.0, s.name,
            s.frame.origin.x, s.frame.origin.y, s.frame.size.width, s.frame.size.height,
            s.scale, pw, ph,
            if s.is_primary { "  [主屏]" } else { "" }
        );
        all = Some(match all {
            Some(a) => a.union(&s.frame),
            None => s.frame,
        });
    }
    let all = all.expect("至少应有一块显示器");
    println!(
        "\n桌面并集: ({:.0},{:.0}) {:.0}x{:.0}",
        all.origin.x, all.origin.y, all.size.width, all.size.height
    );
    if all.origin.x < 0.0 || all.origin.y < 0.0 {
        println!("  注意：并集原点为负，坐标计算不可假定从 (0,0) 起算");
    }

    // ---- 2. 每屏一个遮罩 ----
    println!("\n创建遮罩集合…");
    let overlays = OverlaySet::covering_all_screens(platform.as_ref())?;
    println!("  共 {} 个遮罩（每屏一个，不是一个跨屏窗口）", overlays.len());
    for o in overlays.iter() {
        let f = o.frame();
        println!("    屏[{}] ({:.0},{:.0}) {:.0}x{:.0}", o.screen_id().0, f.origin.x, f.origin.y, f.size.width, f.size.height);
    }

    // 命中测试：各屏中心应落在各自的遮罩里
    println!("\n命中测试（各屏中心点落在哪个遮罩）：");
    for s in &screens {
        let c = Point::new(
            s.frame.origin.x + s.frame.size.width / 2.0,
            s.frame.origin.y + s.frame.size.height / 2.0,
        );
        match overlays.overlay_at(c) {
            Some(o) => println!("    ({:>6.0},{:>5.0}) -> 屏[{}] {}", c.x, c.y, o.screen_id().0,
                if o.screen_id() == s.id { "✓" } else { "✗ 不匹配！" }),
            None => println!("    ({:>6.0},{:>5.0}) -> 无遮罩命中  ✗", c.x, c.y),
        }
    }

    overlays.show();
    pump(2.0);
    overlays.hide();
    overlays.close();

    // ---- 3. 贴图浮窗：建在主屏，再移到最后一块屏 ----
    println!("\n创建贴图浮窗…");
    let first = &screens[0];
    let pin = platform.create_pin(Rect::from_xywh(
        first.frame.origin.x + 120.0,
        first.frame.origin.y + 120.0,
        320.0,
        200.0,
    ))?;
    println!("  初始 frame={:?}  所在屏={:?}", pin.frame(), pin.current_screen());
    pump(1.5);

    if let Some(last) = screens.last() {
        if last.id != first.id {
            let target = Point::new(last.frame.origin.x + 150.0, last.frame.origin.y + 150.0);
            println!("  移动到屏[{}] 的 ({:.0},{:.0})…", last.id.0, target.x, target.y);
            pin.move_to(target);
            pump(1.5);
            println!("  移动后 frame={:?}  所在屏={:?}", pin.frame(), pin.current_screen());
        }
    }

    pin.set_opacity(0.5);
    pump(1.0);
    pin.close();
    println!("\n完成");
    Ok(())
}

/// 简易主线程事件泵，让窗口有机会绘制。
fn pump(seconds: f64) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds);
    while std::time::Instant::now() < end {
        std::thread::yield_now();
    }
}
