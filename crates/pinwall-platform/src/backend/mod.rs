//! 平台后端分发。

use crate::{Platform, Result};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod overlay_view;
#[cfg(target_os = "macos")]
mod pin_view;
#[cfg(target_os = "macos")]
mod toolbar_view;
#[cfg(target_os = "macos")]
mod image;
#[cfg(target_os = "macos")]
mod clipboard;
#[cfg(target_os = "macos")]
mod annot_draw;
#[cfg(target_os = "macos")]
mod flatten;

#[cfg(not(target_os = "macos"))]
mod unimplemented_backend;

/// 取得当前平台的后端实现。
///
/// **必须在主线程调用** —— macOS 的窗口 API 有此硬性要求。
#[cfg(target_os = "macos")]
pub fn current_platform() -> Result<Box<dyn Platform>> {
    Ok(Box::new(macos::MacPlatform::new()?))
}

#[cfg(not(target_os = "macos"))]
pub fn current_platform() -> Result<Box<dyn Platform>> {
    Ok(Box::new(unimplemented_backend::StubPlatform))
}

/// 把图像写入系统剪贴板。
#[cfg(target_os = "macos")]
pub fn copy_image_to_clipboard(image: &crate::PinImage<'_>) -> Result<()> {
    clipboard::copy_image(image)
}

#[cfg(not(target_os = "macos"))]
pub fn copy_image_to_clipboard(_image: &crate::PinImage<'_>) -> Result<()> {
    Err(crate::Error::Unsupported("clipboard"))
}

/// 把标注烧进图像，返回新的 BGRA8 数据（尺寸不变）。
#[cfg(target_os = "macos")]
pub fn flatten_annotations(
    image: &crate::PinImage<'_>,
    commands: &[crate::DrawCommand],
) -> Result<Vec<u8>> {
    flatten::flatten(image, commands)
}

#[cfg(not(target_os = "macos"))]
pub fn flatten_annotations(
    image: &crate::PinImage<'_>,
    _commands: &[crate::DrawCommand],
) -> Result<Vec<u8>> {
    Ok(image.bgra.to_vec())
}
