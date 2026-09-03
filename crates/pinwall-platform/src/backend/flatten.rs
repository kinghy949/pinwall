//! 把标注烧进位图。
//!
//! # 为什么需要它
//!
//! 标注在屏幕上只是**叠加显示**——窗口画完图像再画标注，两者从未合并。
//! 存盘和复制走的却是原始像素，结果就是：屏幕上画了一圈红框，复制出去
//! 却是干干净净的原图。这种落差很难自查，用户只会觉得标注功能坏了。
//!
//! 本模块在导出前把两者合成一张新位图。合成用的是与屏幕**同一份**
//! 绘制代码（[`super::annot_draw`]），所见即所得由此得到保证。

use std::ptr;

use objc2::AnyThread;
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::annot_draw;
use super::image::ns_image_from_bgra;
use crate::{DrawCommand, Error, PinImage, Result};

/// 返回叠加了标注的 BGRA8 位图，尺寸与输入一致。
///
/// 无标注时直接返回原数据 —— 免去一次无谓的重绘与拷贝，
/// 也顺带保证「没标注就是原图」这件事绝对成立。
pub fn flatten(image: &PinImage<'_>, commands: &[DrawCommand]) -> Result<Vec<u8>> {
    if commands.is_empty() {
        return Ok(image.bgra.to_vec());
    }

    let src = ns_image_from_bgra(image)?;
    let (w, h) = (image.width as isize, image.height as isize);
    let (lw, lh) = (
        image.width as f64 / image.scale,
        image.height as f64 / image.scale,
    );

    // 传空的行指针，由 AppKit 自行分配后备存储；随后再读回。
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            w,
            h,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            // 无标志即 RGBA。NSBitmapImageRep 在 8 位/通道下给不出 BGRA，
            // 故读回时再换一次通道（见 read_back）。
            NSBitmapFormat(0),
            w * 4,
            32,
        )
    }
    .ok_or_else(|| Error::WindowCreation("创建导出位图失败".into()))?;

    // 位图按物理像素分配，但把它的**逻辑尺寸**设为点数，
    // 由 AppKit 自动在上下文里乘上倍率。于是绘制代码只需按点计算，
    // 与屏幕上完全一致 —— 这也是这里传 scale = 1.0 的原因。
    rep.setSize(NSSize::new(lw, lh));

    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)
        .ok_or_else(|| Error::WindowCreation("创建位图绘制上下文失败".into()))?;

    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    src.drawInRect(NSRect::new(NSPoint::ZERO, NSSize::new(lw, lh)));
    annot_draw::draw(&ctx.CGContext(), commands, lh, 1.0);
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    read_back(&rep, image.width as usize, image.height as usize)
}

/// 从位图表示里拷出紧凑排列的 BGRA 数据。
///
/// 两处不能图省事：
/// - **行距**。AppKit 分配的行距通常大于 `宽 * 4`（按对齐补齐），
///   必须逐行按实际行距取，不能整块 memcpy。
/// - **通道序**。位图是 RGBA，而本项目对外一律是 BGRA，取的同时换回来。
fn read_back(rep: &NSBitmapImageRep, width: usize, height: usize) -> Result<Vec<u8>> {
    let data = rep.bitmapData();
    if data.is_null() {
        return Err(Error::WindowCreation("导出位图无数据".into()));
    }
    let stride = rep.bytesPerRow() as usize;
    let row = width * 4;
    if stride < row {
        return Err(Error::WindowCreation("导出位图行距异常".into()));
    }

    let mut out = Vec::with_capacity(row * height);
    for y in 0..height {
        // SAFETY: rep 在本函数内保持存活，且 y < height、stride 由 rep 自报，
        // 故 [y*stride, y*stride+row) 落在其后备存储内。
        let line = unsafe { std::slice::from_raw_parts(data.add(y * stride), row) };
        for px in line.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    /// 各通道互不相同，任何一处排列错误都会被这组值抓住。
    fn solid(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|_| [10u8, 20, 200, 255]).collect()
    }

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = (y * w + x) as usize * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn 无标注时原样返回() {
        let bgra = solid(4, 4);
        let img = PinImage { width: 4, height: 4, scale: 1.0, bgra: &bgra };
        assert_eq!(flatten(&img, &[]).unwrap(), bgra);
    }

    /// 这条曾经是真 bug：BGRA 数据被当成 RGBA 交给 NSBitmapImageRep，
    /// 红蓝互换。往返后必须逐字节相同。
    #[test]
    fn 往返不改变未标注像素的通道序() {
        let (w, h) = (40u32, 40u32);
        let bgra = solid(w, h);
        let img = PinImage { width: w, height: h, scale: 1.0, bgra: &bgra };
        // 标注画在右下角，左上角应完好如初
        let cmds = [DrawCommand::Redact { rect: Rect::from_xywh(20.0, 20.0, 15.0, 15.0) }];
        let out = flatten(&img, &cmds).unwrap();

        assert_eq!(out.len(), bgra.len());
        assert_eq!(px(&out, w, 2, 2), [10, 20, 200, 255], "未标注处不应被改动");
    }

    #[test]
    fn 打码块被真的画进了像素里() {
        let (w, h) = (40u32, 40u32);
        let bgra = solid(w, h);
        let img = PinImage { width: w, height: h, scale: 1.0, bgra: &bgra };
        let cmds = [DrawCommand::Redact { rect: Rect::from_xywh(20.0, 20.0, 15.0, 15.0) }];
        let out = flatten(&img, &cmds).unwrap();

        // 0.12 * 255 ≈ 31，三通道相同的深灰
        let c = px(&out, w, 27, 27);
        assert_eq!(c[3], 255, "打码块应完全不透明");
        for ch in &c[..3] {
            assert!((*ch as i32 - 31).abs() <= 2, "打码块颜色异常: {c:?}");
        }
    }

    /// Retina 贴图的标注坐标以**逻辑点**计，输出仍是物理像素。
    #[test]
    fn 二倍屏下标注按逻辑点定位而输出物理像素() {
        let (w, h) = (40u32, 40u32); // 物理 40x40，逻辑 20x20
        let bgra = solid(w, h);
        let img = PinImage { width: w, height: h, scale: 2.0, bgra: &bgra };
        // 逻辑坐标 (10,10)-(18,18) → 物理 (20,20)-(36,36)
        let cmds = [DrawCommand::Redact { rect: Rect::from_xywh(10.0, 10.0, 8.0, 8.0) }];
        let out = flatten(&img, &cmds).unwrap();

        assert_eq!(out.len(), (w * h * 4) as usize, "输出应为物理像素尺寸");
        assert_eq!(px(&out, w, 2, 2), [10, 20, 200, 255], "块外不应被改动");
        let c = px(&out, w, 28, 28);
        assert!((c[0] as i32 - 31).abs() <= 2, "块内应已打码: {c:?}");
    }
}
