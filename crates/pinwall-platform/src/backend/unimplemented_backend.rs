//! 尚未实现的平台的占位后端。
//!
//! Windows 侧需以 `WS_EX_LAYERED | WS_EX_TRANSPARENT` + `HWND_TOPMOST` 实现；
//! Linux 侧受 Wayland 协议限制（客户端不得设置窗口位置），仅计划支持 X11 / XWayland。

use crate::geom::Rect;
use crate::{Error, Overlay, Platform, PinWindow, Result, ScreenInfo};

pub struct StubPlatform;

impl Platform for StubPlatform {
    fn screens(&self) -> Result<Vec<ScreenInfo>> {
        Err(Error::Unsupported("screens"))
    }
    fn create_pin(&self, _frame: Rect) -> Result<Box<dyn PinWindow>> {
        Err(Error::Unsupported("create_pin"))
    }
    fn create_overlay(&self, _screen: &ScreenInfo) -> Result<Box<dyn Overlay>> {
        Err(Error::Unsupported("create_overlay"))
    }
}
