//! 剪贴板往返诊断：写进去一张已知尺寸与倍率的图，再读回来看还剩什么。
//!
//! 运行： cargo run -p pinwall-platform --example clipboard_roundtrip
//!
//! 关心的是**倍率是否活着穿过剪贴板**。贴图窗口按 `像素 / 倍率` 定尺寸，
//! 倍率一旦在往返中丢成 1.0，贴回来的图就会在 Retina 屏上正好大一倍。

use pinwall_platform::{copy_image_to_clipboard, read_clipboard_image, PinImage};

fn main() {
    // 800x400 像素、倍率 2 —— 即逻辑尺寸 400x200 点
    let (w, h, scale) = (800u32, 400u32, 2.0f64);
    let bgra = vec![128u8; (w * h * 4) as usize];

    println!("写入: {w}x{h} 像素, 倍率 {scale} → 逻辑 {}x{} 点", w as f64 / scale, h as f64 / scale);

    copy_image_to_clipboard(&PinImage { width: w, height: h, scale, bgra: &bgra })
        .expect("写剪贴板失败");

    match read_clipboard_image() {
        Some(img) => {
            println!(
                "读回: {}x{} 像素, 倍率 {} → 逻辑 {}x{} 点",
                img.width,
                img.height,
                img.scale,
                img.width as f64 / img.scale,
                img.height as f64 / img.scale
            );
            let kept = (img.scale - scale).abs() < 1e-6;
            if kept {
                println!("\n倍率保住了 —— 可以直接采信剪贴板给出的倍率");
            } else {
                println!(
                    "\n倍率丢失（放大 {:.2} 倍），与已知平台行为一致。",
                    (img.width as f64 / img.scale) / (w as f64 / scale)
                );
                println!("应用层据此改按目标屏的物理像素密度贴图，见 pin_from_clipboard。");
            }
        }
        None => println!("读回: 剪贴板里没有图像 ✗"),
    }
}
