//! 端到端验证：跨屏框选 → 分屏捕获 → 拼接 → 存 PNG。
//!
//! 用合成的拖拽事件驱动选区状态机（不依赖 UI），再走真实捕获链路。
//!
//! 运行： cargo run -p pinwall-capture --example cross_screen -- <输出目录>

use pinwall_capture::{capture_selection, current_capturer, permission_status, CapturedImage, Permission};
use pinwall_core::{Event, Outcome, SelectionMachine};
use pinwall_platform::current_platform;
use pinwall_platform::geom::Point;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    if permission_status() == Permission::Denied {
        eprintln!("未获屏幕录制权限");
        return Ok(());
    }

    let platform = current_platform()?;
    let capturer = current_capturer()?;
    let screens = platform.screens()?;
    if screens.len() < 2 {
        println!("仅检测到 {} 块显示器，跨屏用例需要至少两块", screens.len());
        return Ok(());
    }

    for s in &screens {
        println!(
            "屏[{}] {:22} ({:>6.0},{:>4.0}) {:.0}x{:.0} scale={}",
            s.id.0, s.name, s.frame.origin.x, s.frame.origin.y,
            s.frame.size.width, s.frame.size.height, s.scale
        );
    }

    // 构造一个横跨 x=0 边界的选区：从副屏拖到主屏
    let from = Point::new(-400.0, 200.0);
    let to = Point::new(400.0, 500.0);
    println!("\n模拟拖拽 ({:.0},{:.0}) -> ({:.0},{:.0})", from.x, from.y, to.x, to.y);

    let mut machine = SelectionMachine::new(screens.clone());
    machine.handle(Event::Down(from));
    machine.handle(Event::Move(to));
    let Outcome::Committed(sel) = machine.handle(Event::Up(to)) else {
        eprintln!("选区未提交");
        return Ok(());
    };

    println!(
        "选区 ({:.0},{:.0}) {:.0}x{:.0} 逻辑点，跨屏={}，输出倍率={}",
        sel.rect.origin.x, sel.rect.origin.y, sel.rect.size.width, sel.rect.size.height,
        sel.is_cross_screen(), sel.output_scale()
    );
    for p in &sel.parts {
        let (pw, ph) = p.pixel_size();
        println!(
            "  部分 屏[{}] ({:>6.0},{:>4.0}) {:.0}x{:.0} 逻辑点 -> {pw}x{ph} 像素 (scale={})",
            p.screen_id.0, p.rect.origin.x, p.rect.origin.y,
            p.rect.size.width, p.rect.size.height, p.scale
        );
    }

    let t0 = std::time::Instant::now();
    let img = capture_selection(capturer.as_ref(), &sel)?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let (ew, eh) = sel.output_pixel_size();
    println!(
        "\n拼接结果 {}x{} 像素（预期 {ew}x{eh}）{}  耗时 {ms:.1}ms",
        img.width, img.height,
        if img.width == ew && img.height == eh { "✓" } else { "✗" }
    );

    let path = format!("{out_dir}/cross_screen.png");
    write_png(&img, &path)?;
    println!("已保存 {path}");
    Ok(())
}

fn write_png(img: &CapturedImage, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut rgba = Vec::with_capacity(img.bgra.len());
    for p in img.bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    let f = std::fs::File::create(path)?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), img.width, img.height);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&rgba)?;
    Ok(())
}
