//! 遮罩上的交互视图（macOS）。
//!
//! 承担两件事：把鼠标事件换算成**全局逻辑坐标**后交给上层，
//! 以及绘制「暗色蒙版 + 选区镂空」。
//!
//! 坐标换算是本文件的核心复杂度。涉及三套坐标系：
//!   1. 视图局部     —— 原点在视图左下角，y 向上（Cocoa）
//!   2. Cocoa 全局   —— 原点在主屏左下角，y 向上
//!   3. 本项目全局   —— 原点在主屏左上角，y 向下
//! 换算全部收敛在这里，不外泄。

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSGraphicsContext, NSView};
use objc2_core_graphics::CGContext;
use objc2_foundation::{MainThreadMarker, NSPoint as CGPoint, NSRect, NSRect as CGRect, NSSize as CGSize};

use crate::geom::{Point, Rect};
use crate::{PointerEvent, PointerHandler};

pub struct OverlayViewIvars {
    /// 本遮罩所在屏的 frame（本项目全局坐标）。
    pub screen_frame: Rect,
    /// 主屏高度，用于 Cocoa 全局与本项目全局之间翻转 y。
    pub primary_height: f64,
    pub handler: RefCell<Option<PointerHandler>>,
    /// 当前选区（本项目全局坐标）。
    pub selection: Cell<Option<Rect>>,
}

define_class!(
    // SAFETY:
    // - 父类 NSView 无特殊子类化要求。
    // - OverlayView 不实现 Drop。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PinWallOverlayView"]
    #[ivars = OverlayViewIvars]
    pub struct OverlayView;

    impl OverlayView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.emit(PointerEvent::Down(self.global_point(event)));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.emit(PointerEvent::Moved(self.global_point(event)));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.emit(PointerEvent::Up(self.global_point(event)));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            // 右键取消。不用 Esc 是因为无边框 NonactivatingPanel 难以成为
            // key window，键盘事件不保证送达；鼠标事件则一定会到。
            self.emit(PointerEvent::Cancel);
        }

        /// 应用未激活时，首次点击也要直接生效，而不是先激活再点一次。
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: *mut NSEvent) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl OverlayView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, ivars: OverlayViewIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn set_handler(&self, handler: PointerHandler) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    pub fn set_selection(&self, rect: Option<Rect>) {
        self.ivars().selection.set(rect);
        self.setNeedsDisplay(true);
    }

    fn emit(&self, event: PointerEvent) {
        // 先取出再调用，避免回调里回头 borrow 造成 panic
        let handler = self.ivars().handler.borrow().clone();
        if let Some(h) = handler {
            h(event);
        }
    }

    /// 把鼠标事件的位置换算成本项目的全局逻辑坐标。
    fn global_point(&self, event: &NSEvent) -> Point {
        let iv = self.ivars();
        // locationInWindow：窗口坐标（原点在窗口左下角，y 向上）
        let in_window = event.locationInWindow();
        // 加上窗口原点，得到 Cocoa 全局坐标
        let (wx, wy) = match self.window() {
            Some(w) => {
                let f = w.frame();
                (f.origin.x, f.origin.y)
            }
            None => (0.0, 0.0),
        };
        let cocoa_x = wx + in_window.x;
        let cocoa_y = wy + in_window.y;
        // 翻转 y，转到本项目坐标系
        Point::new(cocoa_x, iv.primary_height - cocoa_y)
    }

    /// 把本项目全局坐标的矩形换算成视图局部坐标（Cocoa，y 向上）。
    fn to_local(&self, r: Rect) -> CGRect {
        let iv = self.ivars();
        let local_x = r.origin.x - iv.screen_frame.origin.x;
        let local_top = r.origin.y - iv.screen_frame.origin.y;
        // 视图内翻转 y
        let local_y = iv.screen_frame.size.height - (local_top + r.size.height);
        CGRect::new(
            CGPoint::new(local_x, local_y),
            CGSize::new(r.size.width, r.size.height),
        )
    }

    fn draw(&self) {
        
        let Some(nsctx) = NSGraphicsContext::currentContext() else { return };
        let ctx = nsctx.CGContext();
        let bounds = self.bounds();
        let full = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(bounds.size.width, bounds.size.height),
        );

        // 整屏压暗
        CGContext::set_rgb_fill_color(Some(&ctx), 0.0, 0.0, 0.0, 0.35);
        CGContext::fill_rect(Some(&ctx), full);

        // 选区镂空。选区可能跨屏，故先与本屏求交，只画落在本屏的部分。
        let iv = self.ivars();
        let Some(sel) = iv.selection.get() else { return };
        let Some(part) = sel.intersection(&iv.screen_frame) else { return };
        let local = self.to_local(part);

        // 清空为全透明，露出底下的真实画面
        CGContext::clear_rect(Some(&ctx), local);

        // 选区描边
        CGContext::set_rgb_stroke_color(Some(&ctx), 1.0, 0.23, 0.19, 1.0);
        CGContext::set_line_width(Some(&ctx), 1.0);
        CGContext::stroke_rect(Some(&ctx), local);
    }
}
