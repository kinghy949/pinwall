//! 系统剪贴板（macOS）。

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::AnyThread;
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage,
    NSPasteboard, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSPoint, NSRect};

use super::image::ns_image_from_bgra;
use crate::{ClipboardImage, PinImage, Result};

/// 把图像写入系统剪贴板。
///
/// 会先清空剪贴板 —— macOS 的 `writeObjects` 是追加语义，
/// 不清空会与上一次的内容混在一起，粘贴时行为不确定。
pub fn copy_image(image: &PinImage<'_>) -> Result<()> {
    let ns_image = ns_image_from_bgra(image)?;
    let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
        ProtocolObject::from_retained(ns_image);
    let objects = NSArray::from_retained_slice(&[writer]);

    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.writeObjects(&objects);
    Ok(())
}

/// 从系统剪贴板读出一张位图。剪贴板里没有图像时返回 `None`。
///
/// 这条通路让「贴图」不再局限于自家截图 —— 从浏览器、聊天窗口、任何地方
/// 复制来的图都能钉到屏幕上，这正是 Snipaste 里 F3 最常被用到的方式。
pub fn read_image() -> Option<ClipboardImage> {
    let pb = NSPasteboard::generalPasteboard();
    let image = NSImage::initWithPasteboard(NSImage::alloc(), &pb)?;

    // NSImage 的 size 是**逻辑点**。真实分辨率要从它的表示里取，
    // 否则 Retina 截来的图会被当成一半大小，钉出来是糊的。
    let logical = image.size();
    let (mut pw, mut ph) = (0isize, 0isize);
    let reps = image.representations();
    for i in 0..reps.count() {
        let r = reps.objectAtIndex(i);
        pw = pw.max(r.pixelsWide());
        ph = ph.max(r.pixelsHigh());
    }
    // 矢量内容（如 PDF）的表示没有像素尺寸，退回按逻辑点栅格化
    if pw <= 0 || ph <= 0 {
        pw = logical.width.round() as isize;
        ph = logical.height.round() as isize;
    }
    if pw <= 0 || ph <= 0 || logical.width <= 0.0 {
        return None;
    }
    let scale = pw as f64 / logical.width;

    // 与 flatten 同一套路：建位图表示、把图画进去、再读回 BGRA
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            pw,
            ph,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat(0),
            pw * 4,
            32,
        )
    }?;
    rep.setSize(logical);

    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    image.drawInRect(NSRect::new(NSPoint::ZERO, logical));
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    let bgra = super::flatten::read_back(&rep, pw as usize, ph as usize).ok()?;
    Some(ClipboardImage {
        width: pw as u32,
        height: ph as u32,
        scale,
        bgra,
    })
}
