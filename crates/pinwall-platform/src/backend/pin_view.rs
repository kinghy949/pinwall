//! 贴图浮窗的交互视图（macOS）。
//!
//! 贴图不能是惰性的图片：用户会立刻想去拖它、关它。
//! 本视图负责绘制图像并处理这两件事。

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSGraphicsContext, NSImage, NSView};
use objc2_core_graphics::CGContext;
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSStringDrawing,
};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};

use crate::{DrawCommand, PointerEvent, PointerHandler, Rgba};

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
    /// 叠加在图像之上的绘制指令。
    pub commands: RefCell<Vec<DrawCommand>>,
    /// 标注模式：拖拽用于绘制而非移动窗口。
    pub annotating: Cell<bool>,
    pub handler: RefCell<Option<PointerHandler>>,
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
            if self.ivars().annotating.get() {
                self.emit(PointerEvent::Down(self.local_point(event)));
                return;
            }
            // 双击关闭 —— 与 Snipaste 一致。标注模式下不生效，
            // 否则画图时双击会误关整张贴图。
            if event.clickCount() >= 2 {
                self.close_self();
                return;
            }
            self.ivars().grab_offset.set(Some(event.locationInWindow()));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            if self.ivars().annotating.get() {
                self.emit(PointerEvent::Moved(self.local_point(_event)));
                return;
            }
            let Some(grab) = self.ivars().grab_offset.get() else { return };
            let Some(window) = self.window() else { return };
            // 用全局鼠标位置减去抓取点偏移，得到窗口新原点。
            // 不用事件里的增量，避免快速拖动时累积误差。
            let mouse = NSEvent::mouseLocation();
            window.setFrameOrigin(NSPoint::new(mouse.x - grab.x, mouse.y - grab.y));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if self.ivars().annotating.get() {
                self.emit(PointerEvent::Up(self.local_point(_event)));
                return;
            }
            self.ivars().grab_offset.set(None);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            if self.ivars().annotating.get() {
                // 标注模式下右键用于取消当前操作，而非关闭贴图
                self.emit(PointerEvent::Cancel);
                return;
            }
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
            self.draw_commands(bounds);
            self.draw_border(bounds);
        }
    }
);

impl PinView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, image: Retained<NSImage>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PinViewIvars {
            image: RefCell::new(Some(image)),
            commands: RefCell::new(Vec::new()),
            annotating: Cell::new(false),
            handler: RefCell::new(None),
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
        self.reposition_children();
    }

    pub fn set_commands(&self, commands: &[DrawCommand]) {
        *self.ivars().commands.borrow_mut() = commands.to_vec();
        self.setNeedsDisplay(true);
    }

    pub fn set_annotating(&self, on: bool) {
        self.ivars().annotating.set(on);
        // 退出标注模式时清掉半途的拖拽状态，否则下次点击会误判为继续拖动
        self.ivars().grab_offset.set(None);
        self.setNeedsDisplay(true);
    }

    pub fn is_annotating(&self) -> bool {
        self.ivars().annotating.get()
    }

    pub fn set_handler(&self, handler: PointerHandler) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// 缩放后重新摆放子窗口（工具栏）。
    ///
    /// 子窗口在父窗口移动时会保持相对偏移，但父窗口**改变尺寸**时不会，
    /// 缩放后必须重新定位。此处直接向系统查询子窗口列表，
    /// 避免在 Rust 侧让视图与工具栏互相持有引用而构成循环。
    fn reposition_children(&self) {
        let Some(window) = self.window() else { return };
        let pin = window.frame();
        // 无子窗口时 childWindows 返回 None
        let Some(children) = window.childWindows() else { return };
        for i in 0..children.count() {
            let child = children.objectAtIndex(i);
            let size = child.frame().size;
            let x = pin.origin.x + (pin.size.width - size.width) / 2.0;
            // Cocoa 的 y 向上，「下方」即更小的 y
            let y = pin.origin.y - size.height - 5.0;
            child.setFrameOrigin(NSPoint::new(x, y));
        }
    }

    fn emit(&self, event: PointerEvent) {
        // 先取出再调用，避免回调里回头 borrow 造成 panic
        let handler = self.ivars().handler.borrow().clone();
        if let Some(h) = handler {
            h(event);
        }
    }

    /// 事件位置换算为贴图局部坐标（左上角原点、y 向下）。
    ///
    /// 视图自身是 Cocoa 坐标（左下角原点、y 向上），需翻转。
    fn local_point(&self, event: &NSEvent) -> crate::geom::Point {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let h = self.bounds().size.height;
        // 除以缩放系数，换算回**图像原始坐标**。
        // 标注存在图像坐标系中，才能在缩放时随图像一起变化而不漂移。
        let z = self.ivars().zoom.get().max(f64::EPSILON);
        crate::geom::Point::new(p.x / z, (h - p.y) / z)
    }

    pub fn is_closed(&self) -> bool {
        self.ivars().closed.get()
    }

    /// 绘制叠加的标注图元。
    ///
    /// 指令坐标为贴图局部（左上角原点、y 向下），而 CG 上下文是
    /// 左下角原点、y 向上，故每条指令都要翻转 y。
    fn draw_commands(&self, bounds: NSRect) {
        let cmds = self.ivars().commands.borrow();
        if cmds.is_empty() {
            return;
        }
        let Some(nsctx) = NSGraphicsContext::currentContext() else { return };
        let ctx = nsctx.CGContext();

        // 标注坐标存于图像原始尺寸下，绘制时整体按缩放系数放大，
        // 使线宽、字号也随之变化 —— 若只缩放坐标，放大后线条会显得过细。
        let z = self.ivars().zoom.get().max(f64::EPSILON);
        CGContext::save_g_state(Some(&ctx));
        CGContext::scale_ctm(Some(&ctx), z, z);
        // 翻转 y 时用的是**图像原始高度**，而非已缩放的视图高度
        let h = bounds.size.height / z;
        let fy = |y: f64| h - y;

        for c in cmds.iter() {
            match c {
                DrawCommand::Rect { rect, color, width } => {
                    set_stroke(&ctx, *color);
                    CGContext::set_line_width(Some(&ctx), *width);
                    CGContext::stroke_rect(
                        Some(&ctx),
                        NSRect::new(
                            NSPoint::new(rect.origin.x, fy(rect.origin.y + rect.size.height)),
                            NSSize::new(rect.size.width, rect.size.height),
                        ),
                    );
                }
                DrawCommand::Arrow { from, to, color, width } => {
                    let (a, b) = (
                        NSPoint::new(from.x, fy(from.y)),
                        NSPoint::new(to.x, fy(to.y)),
                    );
                    set_stroke(&ctx, *color);
                    CGContext::set_line_width(Some(&ctx), *width);
                    CGContext::begin_path(Some(&ctx));
                    CGContext::move_to_point(Some(&ctx), a.x, a.y);
                    CGContext::add_line_to_point(Some(&ctx), b.x, b.y);
                    CGContext::stroke_path(Some(&ctx));

                    // 箭头头部：以线段方向为轴的等腰三角形
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    let len = (dx * dx + dy * dy).sqrt();
                    if len > f64::EPSILON {
                        let (ux, uy) = (dx / len, dy / len);
                        let (nx, ny) = (-uy, ux);
                        let back = 8.0 + width * 2.0;
                        let half = 4.0 + width;
                        set_fill(&ctx, *color);
                        CGContext::begin_path(Some(&ctx));
                        CGContext::move_to_point(Some(&ctx), b.x, b.y);
                        CGContext::add_line_to_point(
                            Some(&ctx),
                            b.x - ux * back + nx * half,
                            b.y - uy * back + ny * half,
                        );
                        CGContext::add_line_to_point(
                            Some(&ctx),
                            b.x - ux * back - nx * half,
                            b.y - uy * back - ny * half,
                        );
                        CGContext::close_path(Some(&ctx));
                        CGContext::fill_path(Some(&ctx));
                    }
                }
                DrawCommand::Redact { rect } => {
                    // 以不透明纯色遮蔽。真正的马赛克需要读回底图像素，
                    // 而纯色遮挡在防泄露上更彻底 —— 马赛克有被复原的先例。
                    CGContext::set_rgb_fill_color(Some(&ctx), 0.12, 0.12, 0.12, 1.0);
                    CGContext::fill_rect(
                        Some(&ctx),
                        NSRect::new(
                            NSPoint::new(rect.origin.x, fy(rect.origin.y + rect.size.height)),
                            NSSize::new(rect.size.width, rect.size.height),
                        ),
                    );
                }
                DrawCommand::SelectionBox { rect } => {
                    let r = NSRect::new(
                        NSPoint::new(rect.origin.x, fy(rect.origin.y + rect.size.height)),
                        NSSize::new(rect.size.width, rect.size.height),
                    );
                    CGContext::set_rgb_stroke_color(Some(&ctx), 0.0, 0.48, 1.0, 1.0);
                    CGContext::set_line_width(Some(&ctx), 1.0);
                    CGContext::stroke_rect(Some(&ctx), inset(r, -3.0));
                    // 两个角手柄，与模型中的 a / b 对应
                    for (hx, hy) in [
                        (r.origin.x, r.origin.y + r.size.height),
                        (r.origin.x + r.size.width, r.origin.y),
                    ] {
                        let d = 3.5;
                        let hr = NSRect::new(
                            NSPoint::new(hx - d, hy - d),
                            NSSize::new(d * 2.0, d * 2.0),
                        );
                        CGContext::set_rgb_fill_color(Some(&ctx), 0.0, 0.48, 1.0, 1.0);
                        CGContext::fill_rect(Some(&ctx), hr);
                        CGContext::set_rgb_stroke_color(Some(&ctx), 1.0, 1.0, 1.0, 1.0);
                        CGContext::stroke_rect(Some(&ctx), hr);
                    }
                }
                DrawCommand::Text { origin, text, color, size } => {
                    let s = NSString::from_str(text);
                    let font = NSFont::systemFontOfSize(*size);
                    let fg = NSColor::colorWithSRGBRed_green_blue_alpha(
                        color.r, color.g, color.b, color.a,
                    );
                    let attrs = NSDictionary::from_slices(
                        &[unsafe { NSFontAttributeName }, unsafe { NSForegroundColorAttributeName }],
                        &[&*font as &objc2::runtime::AnyObject, &*fg],
                    );
                    // NSString 绘制以左下角为基准，故用文字高度回退
                    let point = NSPoint::new(origin.x, fy(origin.y) - size * 1.2);
                    unsafe { s.drawAtPoint_withAttributes(point, Some(&attrs)) };
                }
            }
        }
        CGContext::restore_g_state(Some(&ctx));
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

fn set_stroke(ctx: &CGContext, c: Rgba) {
    CGContext::set_rgb_stroke_color(Some(ctx), c.r, c.g, c.b, c.a);
}

fn set_fill(ctx: &CGContext, c: Rgba) {
    CGContext::set_rgb_fill_color(Some(ctx), c.r, c.g, c.b, c.a);
}
