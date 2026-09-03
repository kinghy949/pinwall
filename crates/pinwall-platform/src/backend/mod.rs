//! 平台后端分发。

use crate::{Platform, Result};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod overlay_view;
#[cfg(target_os = "macos")]
mod pin_view;

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
