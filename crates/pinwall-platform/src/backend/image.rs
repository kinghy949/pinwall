//! BGRA 位图到 NSImage 的转换。贴图显示与剪贴板写入共用。

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapFormat, NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage};
use objc2_foundation::NSSize;

use crate::{Error, PinImage, Result};

/// 由 BGRA8 数据构造 NSImage。
///
/// 返回的 NSImage 逻辑尺寸为 `像素 / 倍率`，从而在 Retina 屏上显示为
/// 原始大小而非两倍放大。
pub fn ns_image_from_bgra(image: &PinImage<'_>) -> Result<Retained<NSImage>> {
    if image.width == 0 || image.height == 0 {
        return Err(Error::WindowCreation("图像尺寸为零".into()));
    }
    let expected = image.width as usize * image.height as usize * 4;
    if image.bgra.len() < expected {
        return Err(Error::WindowCreation(format!(
            "像素数据长度不足：需要 {expected}，实得 {}",
            image.bgra.len()
        )));
    }

    // NSBitmapImageRep 需要可写的行指针。此处复制一份供其拷贝，
    // 直接借用调用方的切片会在其释放后留下悬垂指针。
    let mut buf = image.bgra.to_vec();
    let mut plane: *mut u8 = buf.as_mut_ptr();

    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            &mut plane as *mut *mut u8,
            image.width as isize,
            image.height as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            // BGRA 预乘 alpha、小端序 —— 与捕获层输出格式一致
            NSBitmapFormat::ThirtyTwoBitLittleEndian | NSBitmapFormat(0),
            image.width as isize * 4,
            32,
        )
    }
    .ok_or_else(|| Error::WindowCreation("创建位图表示失败".into()))?;

    let logical = NSSize::new(
        image.width as f64 / image.scale,
        image.height as f64 / image.scale,
    );
    let ns_image = NSImage::initWithSize(NSImage::alloc(), logical);
    ns_image.addRepresentation(&rep);
    drop(buf); // rep 已持有自己的副本
    Ok(ns_image)
}
