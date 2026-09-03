//! 系统剪贴板（macOS）。

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
use objc2_foundation::NSArray;

use super::image::ns_image_from_bgra;
use crate::{PinImage, Result};

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
