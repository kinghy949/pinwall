//! PinWall 的跨平台窗口层。
//!
//! # 为什么需要这一层
//!
//! 本层的存在是原型验证的直接结果，而非预先的架构偏好：
//!
//! - **不能用 winit 建窗**（macOS）。实测 winit 创建的 `NSWindow` 无论如何配置
//!   window level 与 collectionBehavior，都无法进入其他应用的全屏 Space；
//!   必须使用 `NSPanel` 且 styleMask 含 `NonactivatingPanel`。
//!   而「盯着全屏设计稿写代码」正是贴图的核心场景，故此项不可妥协。
//!
//! - **遮罩必须每屏一个**。macOS 默认「显示器各自拥有独立空间」，
//!   一个窗口只能属于一个 Space，无法跨屏铺开。按所有屏并集构造的单个窗口
//!   实测只覆盖一块屏。
//!
//! 详见仓库 `docs/mvp-risks.md`。

pub mod geom;

use geom::{Point, Rect};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("必须在主线程调用")]
    NotMainThread,
    #[error("未找到显示器: {0:?}")]
    ScreenNotFound(ScreenId),
    #[error("窗口创建失败: {0}")]
    WindowCreation(String),
    #[error("当前平台尚未实现: {0}")]
    Unsupported(&'static str),
}

/// 显示器标识。跨热插拔不保证稳定，每次枚举后应重新获取。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScreenId(pub u32);

#[derive(Debug, Clone)]
pub struct ScreenInfo {
    pub id: ScreenId,
    pub name: String,
    /// 该屏在全局坐标系中的位置与大小（逻辑点，左上角原点，y 向下）。
    /// **副屏的 origin 可能为负。**
    pub frame: Rect,
    /// 逻辑点到物理像素的倍率。Retina 通常为 2.0。
    /// 各屏可能不同（混合 DPI），跨屏移动窗口时必须按目标屏的倍率重新采样。
    pub scale: f64,
    pub is_primary: bool,
}

impl ScreenInfo {
    /// 该屏的物理像素尺寸。截图取像素时应以此为准，而非逻辑尺寸。
    pub fn pixel_size(&self) -> (u32, u32) {
        (
            (self.frame.size.width * self.scale).round() as u32,
            (self.frame.size.height * self.scale).round() as u32,
        )
    }
}

/// 贴图浮窗：置顶、可跨 Space、可覆盖其他应用的全屏窗口。
pub trait PinWindow {
    fn show(&self);
    fn hide(&self);
    /// 关闭并释放。**不要只调 `hide()` 就丢弃** —— 浮窗会长期累积。
    fn close(self: Box<Self>);
    fn set_opacity(&self, alpha: f64);
    /// 鼠标穿透。开启后窗口不再接收任何鼠标事件，点击会落到其下方的窗口。
    fn set_click_through(&self, enabled: bool);
    fn move_to(&self, origin: Point);
    fn frame(&self) -> Rect;
    /// 当前所在显示器。跨屏拖动后会变化，据此判断是否需要按新倍率重采样。
    fn current_screen(&self) -> Option<ScreenId>;
}

/// 单块显示器上的捕获遮罩。
///
/// 全屏捕获需要为**每块显示器**各建一个，由 [`OverlaySet`] 统一管理。
pub trait Overlay {
    fn show(&self);
    fn hide(&self);
    fn close(self: Box<Self>);
    fn screen_id(&self) -> ScreenId;
    fn frame(&self) -> Rect;
}

/// 平台后端。
pub trait Platform {
    /// 枚举当前所有显示器。显示器可能热插拔，每次进入捕获流程前应重新枚举。
    fn screens(&self) -> Result<Vec<ScreenInfo>>;

    fn create_pin(&self, frame: Rect) -> Result<Box<dyn PinWindow>>;

    fn create_overlay(&self, screen: &ScreenInfo) -> Result<Box<dyn Overlay>>;
}

/// 覆盖全部显示器的遮罩集合。
///
/// 这是「一个全屏遮罩」这一错误假设的替代物：实际是 N 个窗口，
/// 每屏一个，生命周期统一管理。
pub struct OverlaySet {
    overlays: Vec<Box<dyn Overlay>>,
}

impl OverlaySet {
    /// 为当前每一块显示器各创建一个遮罩。
    pub fn covering_all_screens(platform: &dyn Platform) -> Result<Self> {
        let screens = platform.screens()?;
        let mut overlays = Vec::with_capacity(screens.len());
        for s in &screens {
            overlays.push(platform.create_overlay(s)?);
        }
        Ok(Self { overlays })
    }

    pub fn show(&self) {
        for o in &self.overlays {
            o.show();
        }
    }

    pub fn hide(&self) {
        for o in &self.overlays {
            o.hide();
        }
    }

    pub fn len(&self) -> usize {
        self.overlays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Box<dyn Overlay>> {
        self.overlays.iter()
    }

    /// 找出包含指定全局坐标的遮罩。跨屏框选时用于判断鼠标当前落在哪块屏。
    pub fn overlay_at(&self, p: Point) -> Option<&Box<dyn Overlay>> {
        self.overlays.iter().find(|o| o.frame().contains(p))
    }

    pub fn close(self) {
        for o in self.overlays {
            o.close();
        }
    }
}

mod backend;
pub use backend::current_platform;
