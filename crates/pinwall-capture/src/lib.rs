//! PinWall 的屏幕捕获层。
//!
//! # 设计要点
//!
//! - **按物理像素捕获**。逻辑点尺寸乘以该屏的 `scale` 才是实际像素数。
//!   混合 DPI 下各屏 scale 不同，跨屏区域必须分屏捕获后再拼接。
//! - **坐标系恰好对齐**。CoreGraphics 的全局显示坐标与本项目约定一致
//!   （原点在主屏左上角、y 向下），故 [`pinwall_platform::geom::Rect`]
//!   可直接用于捕获，无需换算。这与 Cocoa 的窗口坐标系不同 —— 后者
//!   原点在左下角，仅在窗口层做转换。

use pinwall_platform::ScreenInfo;
use pinwall_platform::geom::Rect;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("未获得「屏幕录制」权限。请在 系统设置 → 隐私与安全性 → 屏幕录制 中授权后重启本应用")]
    PermissionDenied,
    #[error("捕获失败: {0}")]
    CaptureFailed(String),
    #[error("捕获超时（{0:?}）")]
    Timeout(std::time::Duration),
    #[error("捕获区域为空: {0:?}")]
    EmptyRect(Rect),
    #[error("当前平台尚未实现屏幕捕获")]
    Unsupported,
}

/// 一张捕获到的位图，像素格式为 **BGRA8**（预乘 alpha，小端序）。
///
/// 选择 BGRA8 是因为它同时是 CoreGraphics 与 wgpu 的常用原生格式，
/// 可避免在捕获与上传纹理之间做一次全图转换。
#[derive(Clone)]
pub struct CapturedImage {
    /// 物理像素宽。
    pub width: u32,
    /// 物理像素高。
    pub height: u32,
    /// 捕获时所用的逻辑点→像素倍率。写文件或贴图时需据此还原显示尺寸。
    pub scale: f64,
    /// BGRA8 像素数据，长度为 `width * height * 4`。
    pub bgra: Vec<u8>,
}

impl CapturedImage {
    pub fn bytes_per_row(&self) -> usize {
        self.width as usize * 4
    }

    /// 以逻辑点表示的显示尺寸。
    pub fn logical_size(&self) -> (f64, f64) {
        (self.width as f64 / self.scale, self.height as f64 / self.scale)
    }
}

impl std::fmt::Debug for CapturedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .field("bytes", &self.bgra.len())
            .finish()
    }
}

pub trait Capturer {
    /// 捕获全局坐标下的一块矩形区域。
    ///
    /// `scale` 应取该区域所在显示器的 `ScreenInfo::scale`。
    /// **跨屏区域不要整块传入** —— 各屏倍率可能不同，应分屏捕获后拼接。
    fn capture_rect(&self, rect: Rect, scale: f64) -> Result<CapturedImage>;

    /// 捕获整块显示器。
    fn capture_screen(&self, screen: &ScreenInfo) -> Result<CapturedImage> {
        self.capture_rect(screen.frame, screen.scale)
    }
}

/// 屏幕录制权限状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    /// 该平台无需此权限。
    NotRequired,
}

mod compose;
pub use compose::capture_selection;

mod backend;
pub use backend::{current_capturer, permission_status, request_permission};
