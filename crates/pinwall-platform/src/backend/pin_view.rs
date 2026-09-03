//! 贴图浮窗的交互视图（macOS）。
//!
//! 贴图不能是惰性的图片：用户会立刻想去拖它、关它。
//! 本视图负责绘制图像并处理这两件事。

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSGraphicsContext, NSImage, NSView};
use objc2_core_graphics::CGContext;
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

/// 缩放范围。下限保证贴图不会小到点不中，上限避免无意义的糊放大。
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 8.0;
const OPACITY_MIN: f64 = 0.1;

/// 矩形四边同时内缩。
fn inset(r: NSRect, d: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(r.origin.x + d, r.origin.y + d),
        NSSize::new(
            (r.size.width - d * 2.0).max(0.0),
            (r.size.height - d * 2.0).max(0.0),
        ),
    )
}

pub struct PinViewIvars {
    pub image: RefCell<Option<Retained<NSImage>>>,
    /// 按下时鼠标在窗口内的偏移，用于拖动时保持抓取点不变。
    pub grab_offset: Cell<Option<NSPoint>>,
    /// 窗口是否已被用户关闭。上层据此回收对应的 PinWindow。
    pub closed: Cell<bool>,
    /// 100% 时的逻辑尺寸。缩放始终以它为基准计算，
    /// 而非在当前尺寸上累乘，避免反复缩放后累积误差。
    pub base_size: Cell<NSSize>,
    pub zoom: Cell<f64>,
    pub opacity: Cell<f64>,
    pub click_through: Cell<bool>,
}

define_class!(
    // SAFETY:
    // - 父类 NSView 无特殊子类化要求。
    // - PinView 不实现 Drop。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PinWallPinView"]
    #[ivars = PinViewIvars]
    pub struct PinView;

    impl PinView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // 双击关闭 —— 与 Snipaste 一致
            if event.clickCount() >= 2 {
                self.close_self();
                return;
            }
            self.ivars().grab_offset.set(Some(event.locationInWindow()));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            let Some(grab) = self.ivars().grab_offset.get() else { return };
            let Some(window) = self.window() else { return };
            // 用全局鼠标位置减去抓取点偏移，得到窗口新原点。
            // 不用事件里的增量，避免快速拖动时累积误差。
            let mouse = NSEvent::mouseLocation();
            window.setFrameOrigin(NSPoint::new(mouse.x - grab.x, mouse.y - grab.y));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().grab_offset.set(None);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            // 右键也关闭。双击对触控板用户不总是顺手，留个备选。
            self.close_self();
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, _event: &NSEvent) {
            // 中键切换鼠标穿透。开启后本窗口不再接收任何鼠标事件，
            // 无法再靠点击自己关掉——故上层须提供全局快捷键兜底。
            let on = !self.ivars().click_through.get();
            self.ivars().click_through.set(on);
            if let Some(w) = self.window() {
                w.setIgnoresMouseEvents(on);
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let dy = event.scrollingDeltaY();
            let dx = event.scrollingDeltaX();
            let flags = event.modifierFlags();

            // macOS 会把 Shift+滚轮**转换成横向滚动**，此时 deltaY 恒为 0、
            // 值跑到 deltaX 上。只读 deltaY 会导致 Shift 组合完全失效。
            if flags.contains(NSEventModifierFlags::Shift)
                || flags.contains(NSEventModifierFlags::Option)
            {
                let delta = if dy != 0.0 { dy } else { dx };
                if delta != 0.0 {
                    self.adjust_opacity(delta);
                }
            } else if dy != 0.0 {
                self.adjust_zoom(dy);
            }
        }

        /// 应用未激活时首次点击也应直接拖动，而不是先激活再点一次。
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: *mut NSEvent) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            if let Some(img) = self.ivars().image.borrow().as_ref() {
                img.drawInRect(bounds);
            }
            self.draw_border(bounds);
        }
    }
);

impl PinView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, image: Retained<NSImage>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PinViewIvars {
            image: RefCell::new(Some(image)),
            grab_offset: Cell::new(None),
            closed: Cell::new(false),
            base_size: Cell::new(frame.size),
            zoom: Cell::new(1.0),
            opacity: Cell::new(1.0),
            click_through: Cell::new(false),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn is_click_through(&self) -> bool {
        self.ivars().click_through.get()
    }

    /// 供上层的全局快捷键调用。鼠标穿透一旦开启，窗口就收不到鼠标事件，
    /// 无法自行关闭，必须留这条外部通路。
    pub fn set_click_through(&self, on: bool) {
        self.ivars().click_through.set(on);
        if let Some(w) = self.window() {
            w.setIgnoresMouseEvents(on);
        }
    }

    fn adjust_opacity(&self, dy: f64) {
        let iv = self.ivars();
        let next = (iv.opacity.get() + dy * 0.01).clamp(OPACITY_MIN, 1.0);
        iv.opacity.set(next);
        if let Some(w) = self.window() {
            w.setAlphaValue(next);
        }
    }

    /// 以光标位置为锚点缩放。
    ///
    /// 锚点不变是手感的关键：若固定左上角，用户想放大某个细节时
    /// 那个细节会跑掉，得反复拖回来。
    fn adjust_zoom(&self, dy: f64) {
        let iv = self.ivars();
        let Some(window) = self.window() else { return };

        let base = iv.base_size.get();
        // 指数步进，使每一格滚轮的视觉变化率一致
        let next_zoom = (iv.zoom.get() * (1.0 + dy * 0.01)).clamp(ZOOM_MIN, ZOOM_MAX);
        if (next_zoom - iv.zoom.get()).abs() < f64::EPSILON {
            return;
        }
        iv.zoom.set(next_zoom);

        let old = window.frame();
        let new_size = NSSize::new(base.width * next_zoom, base.height * next_zoom);

        // 求光标在窗口内的相对位置（0..1），缩放后令其保持不变
        let mouse = NSEvent::mouseLocation();
        let rx = if old.size.width > 0.0 {
            ((mouse.x - old.origin.x) / old.size.width).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let ry = if old.size.height > 0.0 {
            ((mouse.y - old.origin.y) / old.size.height).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let new_origin = NSPoint::new(
            mouse.x - rx * new_size.width,
            mouse.y - ry * new_size.height,
        );

        window.setFrame_display(NSRect::new(new_origin, new_size), true);
        self.setFrameSize(new_size);
        self.setNeedsDisplay(true);
    }

    pub fn is_closed(&self) -> bool {
        self.ivars().closed.get()
    }

    /// 描一圈双色细边。
    ///
    /// 贴图与其下方的真实界面在视觉上极易混淆——用户会对着一张静止的
    /// 截图反复点击，以为界面卡住了。加一圈边框是最低成本的区分手段。
    ///
    /// 外深内浅两条线是经典做法：单一颜色的边框在与之相近的背景上会消失，
    /// 深浅并置则在任何底色上都能看见其中一条。
    fn draw_border(&self, bounds: NSRect) {
        let Some(nsctx) = NSGraphicsContext::currentContext() else { return };
        let ctx = nsctx.CGContext();
        CGContext::set_line_width(Some(&ctx), 1.0);

        // 外圈深色。0.5 的内缩使 1 像素的线正好落在像素格上，不会被抗锯齿糊成两像素
        CGContext::set_rgb_stroke_color(Some(&ctx), 0.0, 0.0, 0.0, 0.45);
        CGContext::stroke_rect(Some(&ctx), inset(bounds, 0.5));

        // 内圈浅色
        CGContext::set_rgb_stroke_color(Some(&ctx), 1.0, 1.0, 1.0, 0.30);
        CGContext::stroke_rect(Some(&ctx), inset(bounds, 1.5));
    }

    fn close_self(&self) {
        self.ivars().closed.set(true);
        if let Some(w) = self.window() {
            w.orderOut(None);
        }
    }
}
