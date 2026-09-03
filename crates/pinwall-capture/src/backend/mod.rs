use crate::{Capturer, Permission, Result};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub fn current_capturer() -> Result<Box<dyn Capturer>> {
    Ok(Box::new(macos::MacCapturer))
}

#[cfg(target_os = "macos")]
pub fn permission_status() -> Permission {
    macos::permission_status()
}

/// 触发系统的屏幕录制授权流程。
///
/// 首次调用会弹出系统授权对话框；**用户授权后通常需要重启应用才会生效**，
/// 调用方应据此提示用户，而不是原地重试。
#[cfg(target_os = "macos")]
pub fn request_permission() -> bool {
    macos::request_permission()
}

#[cfg(not(target_os = "macos"))]
pub fn current_capturer() -> Result<Box<dyn Capturer>> {
    Err(crate::Error::Unsupported)
}

#[cfg(not(target_os = "macos"))]
pub fn permission_status() -> Permission {
    Permission::NotRequired
}

#[cfg(not(target_os = "macos"))]
pub fn request_permission() -> bool {
    true
}
