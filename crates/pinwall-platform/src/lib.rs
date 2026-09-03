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

/// RGBA 颜色，各分量 0..1。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }
}

/// 一条绘制指令。
///
/// 窗口层只认这些基本图元，不理解「标注」的语义 ——
/// 标注模型在 `pinwall-core`，而它依赖本 crate 的几何类型，
/// 反向依赖会成环。故由应用层做翻译：核心层产出标注对象，
/// 应用层转成绘制指令，窗口层照着画。
///
/// 坐标为**贴图局部逻辑点**（图像左上角为原点，y 向下）。
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    Rect { rect: Rect, color: Rgba, width: f64 },
    Arrow { from: Point, to: Point, color: Rgba, width: f64 },
    Text { origin: Point, text: String, color: Rgba, size: f64 },
    /// 对该区域做马赛克遮蔽。像素处理由窗口层完成，
    /// 因为只有它持有底图。
    Redact { rect: Rect },
    /// 选中态的虚线框与两个角手柄。
    SelectionBox { rect: Rect },
}

/// 工具栏上的一个按钮。
///
/// 窗口层不理解「工具」的语义，只按 `id` 回报点击，由应用层映射到
/// 具体工具 —— 与 [`DrawCommand`] 同理，标注模型不能被窗口层依赖。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolbarItem {
    pub id: u32,
    pub label: String,
    pub selected: bool,
}

/// 工具栏点击回调，参数为被点按钮的 `id`。
pub type ToolbarHandler = std::rc::Rc<dyn Fn(u32)>;

/// 一张待贴出的位图，像素格式为 BGRA8。
///
/// 定义在此处而非捕获层，是为了让窗口层不必反向依赖捕获层；
/// 捕获层的 `CapturedImage` 可零成本借用为本类型。
pub struct PinImage<'a> {
    pub width: u32,
    pub height: u32,
    /// 逻辑点→像素倍率。贴图窗口按 `width / scale` 的逻辑尺寸显示，
    /// 从而在 Retina 屏上呈现为原始大小而非两倍放大。
    pub scale: f64,
    pub bgra: &'a [u8],
}

/// 贴图浮窗：置顶、可跨 Space、可覆盖其他应用的全屏窗口。
pub trait PinWindow {
    /// 设置窗口显示的图像。窗口尺寸会按图像的逻辑尺寸调整。
    fn set_image(&self, image: &PinImage<'_>) -> Result<()>;

    /// 设置叠加在图像之上的绘制指令。传空切片即清除。
    fn set_draw_commands(&self, commands: &[DrawCommand]);

    /// 进入/退出标注模式。
    ///
    /// 标注模式下拖拽用于绘制图形而非移动窗口，指针事件转交回调；
    /// 退出后恢复为拖拽移动。
    fn set_annotation_mode(&self, enabled: bool);

    fn is_annotation_mode(&self) -> bool;

    /// 设置标注模式下的指针事件回调，坐标为贴图局部逻辑点。
    fn set_pointer_handler(&self, handler: PointerHandler);

    /// 设置浮动工具栏的按钮。传空切片即隐藏工具栏。
    ///
    /// 工具栏作为贴图窗口的子窗口存在，会自动跟随贴图移动。
    fn set_toolbar(&self, items: &[ToolbarItem]);

    fn set_toolbar_handler(&self, handler: ToolbarHandler);

    fn show(&self);
    fn hide(&self);
    /// 关闭并释放。**不要只调 `hide()` 就丢弃** —— 浮窗会长期累积。
    fn close(self: Box<Self>);
    fn set_opacity(&self, alpha: f64);
    /// 鼠标穿透。开启后窗口不再接收任何鼠标事件，点击会落到其下方的窗口。
    ///
    /// **调用方必须提供一条外部通路（如全局快捷键）来关闭它** ——
    /// 穿透开启后窗口自身收不到点击，无法再靠交互恢复。
    fn set_click_through(&self, enabled: bool);

    fn is_click_through(&self) -> bool;
    fn move_to(&self, origin: Point);
    fn frame(&self) -> Rect;
    /// 当前所在显示器。跨屏拖动后会变化，据此判断是否需要按新倍率重采样。
    fn current_screen(&self) -> Option<ScreenId>;

    /// 用户是否已经关掉了这张贴图（双击或右键）。
    ///
    /// 窗口的关闭由用户在窗口上直接触发，上层无从得知，
    /// 故需轮询本方法回收对应的 `PinWindow`，否则会持续累积。
    fn is_closed(&self) -> bool;
}

/// 遮罩上的指针事件。坐标均为**全局逻辑坐标**，已由后端换算完毕。
///
/// 换算发生在后端是有意为之：遮罩是每屏一个窗口，各窗口的局部坐标系
/// 互不相同，若把换算留给上层，跨屏框选的逻辑会被窗口边界污染。
#[derive(Debug, Clone, Copy)]
pub enum PointerEvent {
    Down(Point),
    Moved(Point),
    Up(Point),
    /// 取消，来自右键或 Esc。
    Cancel,
}

/// 遮罩指针事件的回调。所有遮罩共享同一个回调，从而汇入同一个选区状态机。
pub type PointerHandler = std::rc::Rc<dyn Fn(PointerEvent)>;

/// 单块显示器上的捕获遮罩。
///
/// 全屏捕获需要为**每块显示器**各建一个，由 [`OverlaySet`] 统一管理。
pub trait Overlay {
    fn show(&self);
    fn hide(&self);
    fn close(self: Box<Self>);
    fn screen_id(&self) -> ScreenId;
    fn frame(&self) -> Rect;

    /// 设置指针事件回调。N 个遮罩应共享同一个回调。
    fn set_pointer_handler(&self, handler: PointerHandler);

    /// 设置当前选区（全局坐标），用于在遮罩上绘制镂空。
    ///
    /// 选区可能跨屏，故**每次变化都要设给所有遮罩**，
    /// 由各遮罩自行判断与本屏是否相交。
    fn set_selection(&self, rect: Option<Rect>);
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

    /// 把同一个指针回调装到所有遮罩上。
    pub fn set_pointer_handler(&self, handler: PointerHandler) {
        for o in &self.overlays {
            o.set_pointer_handler(handler.clone());
        }
    }

    /// 广播当前选区。必须设给所有遮罩 —— 选区可能横跨其中数块。
    pub fn set_selection(&self, rect: Option<Rect>) {
        for o in &self.overlays {
            o.set_selection(rect);
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
pub use backend::{copy_image_to_clipboard, current_platform};
