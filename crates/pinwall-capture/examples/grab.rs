//! 捕获每块显示器并存为 PNG。
//!
//! 运行： cargo run -p pinwall-capture --example grab -- <输出目录>

use std::time::Instant;

use pinwall_capture::{current_capturer, permission_status, CapturedImage, Permission};
use pinwall_platform::current_platform;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());

    match permission_status() {
        Permission::Granted => println!("屏幕录制权限：已授权"),
        Permission::Denied => {
            eprintln!("屏幕录制权限：未授权");
            eprintln!("请在 系统设置 → 隐私与安全性 → 屏幕录制 中授权后重启终端再试");
            return Ok(());
        }
        Permission::NotRequired => println!("屏幕录制权限：本平台无需"),
    }

    let platform = current_platform()?;
    let capturer = current_capturer()?;
    let screens = platform.screens()?;

    println!("\n显示器 {} 块", screens.len());
    for s in &screens {
        let (pw, ph) = s.pixel_size();
        print!(
            "  [{}] {:24} 逻辑 {:.0}x{:.0} @({:.0},{:.0}) scale={} 预期像素 {}x{} … ",
            s.id.0, s.name, s.frame.size.width, s.frame.size.height,
            s.frame.origin.x, s.frame.origin.y, s.scale, pw, ph
        );

        let t0 = Instant::now();
        match capturer.capture_screen(s) {
            Ok(img) => {
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                let ok = img.width == pw && img.height == ph;
                println!(
                    "实得 {}x{} {} 耗时 {ms:.1}ms",
                    img.width, img.height,
                    if ok { "✓ 与预期一致" } else { "✗ 与预期不符" }
                );
                let path = format!("{out_dir}/screen{}.png", s.id.0);
                write_png(&img, &path)?;
                println!("       已保存 {path}  ({} KB)", std::fs::metadata(&path)?.len() / 1024);
            }
            Err(e) => println!("失败: {e}"),
        }
    }

    // 局部区域捕获：主屏左上角 400x300 逻辑点
    if let Some(p) = screens.iter().find(|s| s.is_primary) {
        let r = pinwall_platform::geom::Rect::from_xywh(
            p.frame.origin.x, p.frame.origin.y, 400.0, 300.0,
        );
        let t0 = Instant::now();
        let img = capturer.capture_rect(r, p.scale)?;
        println!(
            "\n局部捕获 400x300 逻辑点 -> {}x{} 像素，耗时 {:.1}ms",
            img.width, img.height, t0.elapsed().as_secs_f64() * 1000.0
        );
        let path = format!("{out_dir}/region.png");
        write_png(&img, &path)?;
        println!("  已保存 {path}");
    }

    Ok(())
}

/// BGRA8 -> RGBA8 并写 PNG。
fn write_png(img: &CapturedImage, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut rgba = Vec::with_capacity(img.bgra.len());
    for px in img.bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    Ok(())
}
