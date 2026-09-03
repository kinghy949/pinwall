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
///
/// # 为什么要转成 RGBA
///
/// `NSBitmapImageRep` 在 8 位/通道下只能表达 **RGBA** 或 **ARGB**
/// （由 `AlphaFirst` 决定），没有 BGRA 选项 —— 名字里带 LittleEndian 的
/// 那两个标志指的是 16/32 位**采样**的字节序，8 位采样时被忽略。
/// 而捕获层输出的是 BGRA（CoreGraphics 的原生格式）。两者对不上，
/// 只能在这里逐像素换一次红蓝通道。
///
/// 这次换通道不额外花内存：本来就必须拷贝一份（见下），顺手换掉即可。
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

    // 传空的行指针，让 NSBitmapImageRep 自行分配并**持有**后备存储。
    //
    // 不能改成传自己的缓冲区：那样 rep 只是借用指针、不做拷贝，
    // 函数返回后缓冲区释放，rep 就指向了已释放的内存。
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            image.width as isize,
            image.height as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            // 无标志即 RGBA（预乘 alpha）
            NSBitmapFormat(0),
            image.width as isize * 4,
            32,
        )
    }
    .ok_or_else(|| Error::WindowCreation("创建位图表示失败".into()))?;

    write_rgba(&rep, image.bgra, image.width as usize, image.height as usize)?;

    let logical = NSSize::new(
        image.width as f64 / image.scale,
        image.height as f64 / image.scale,
    );
    let ns_image = NSImage::initWithSize(NSImage::alloc(), logical);
    ns_image.addRepresentation(&rep);
    Ok(ns_image)
}

/// 把 BGRA 源数据写进位图表示，写入时换成 RGBA。
fn write_rgba(rep: &NSBitmapImageRep, bgra: &[u8], width: usize, height: usize) -> Result<()> {
    let dst = rep.bitmapData();
    if dst.is_null() {
        return Err(Error::WindowCreation("位图表示无后备存储".into()));
    }
    // AppKit 分配的行距通常大于 `宽 * 4`（按对齐补齐），必须逐行按其行距写
    let stride = rep.bytesPerRow() as usize;
    let row = width * 4;
    if stride < row {
        return Err(Error::WindowCreation("位图行距异常".into()));
    }

    for y in 0..height {
        let src = &bgra[y * row..y * row + row];
        // SAFETY: rep 在本函数内存活，y < height 且 stride 由 rep 自报，
        // 故写入范围落在其后备存储内。
        let out = unsafe { std::slice::from_raw_parts_mut(dst.add(y * stride), row) };
        for (s, d) in src.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
            d[0] = s[2]; // R ← BGRA 的第 3 字节
            d[1] = s[1]; // G
            d[2] = s[0]; // B
            d[3] = s[3]; // A
        }
    }
    Ok(())
}
