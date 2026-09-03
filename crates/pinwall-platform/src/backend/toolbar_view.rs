//! 浮动工具栏视图（macOS）。
//!
//! 以贴图窗口的**子窗口**形式存在 —— Cocoa 的 child window 会随父窗口
//! 一同移动，省去手工同步位置的麻烦，也保证两者层级关系稳定。

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSColor, NSEvent, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSGraphicsContext, NSStringDrawing, NSView,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString};

use crate::{ToolbarHandler, ToolbarItem};

/// 单个按钮的尺寸与间距（逻辑点）。
pub const BUTTON_W: f64 = 52.0;
pub const BUTTON_H: f64 = 26.0;
pub const PADDING: f64 = 5.0;
const CORNER: f64 = 5.0;
const FONT_SIZE: f64 = 12.0;

/// 按给定按钮数算出工具栏所需尺寸。
pub fn toolbar_size(count: usize) -> NSSize {
    NSSize::new(
        count as f64 * BUTTON_W + (count as f64 + 1.0) * PADDING,
        BUTTON_H + PADDING * 2.0,
    )
}

pub struct ToolbarViewIvars {
    pub items: RefCell<Vec<ToolbarItem>>,
    pub handler: RefCell<Option<ToolbarHandler>>,
}

define_class!(
    // SAFETY:
    // - 父类 NSView 无特殊子类化要求。
    // - ToolbarView 不实现 Drop。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PinWallToolbarView"]
    #[ivars = ToolbarViewIvars]
    pub struct ToolbarView;

    impl ToolbarView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let Some(id) = self.hit_test_button(p) else { return };
            let handler = self.ivars().handler.borrow().clone();
            if let Some(h) = handler {
                h(id);
            }
        }

        /// 应用未激活时首次点击也应直接选中工具，而非先激活再点一次。
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

impl ToolbarView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ToolbarViewIvars {
            items: RefCell::new(Vec::new()),
            handler: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn set_items(&self, items: &[ToolbarItem]) {
        *self.ivars().items.borrow_mut() = items.to_vec();
        self.setNeedsDisplay(true);
    }

    pub fn set_handler(&self, handler: ToolbarHandler) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// 第 `i` 个按钮的矩形（视图局部，Cocoa 坐标）。
    fn button_rect(&self, i: usize) -> NSRect {
        NSRect::new(
            NSPoint::new(PADDING + i as f64 * (BUTTON_W + PADDING), PADDING),
            NSSize::new(BUTTON_W, BUTTON_H),
        )
    }

    fn hit_test_button(&self, p: NSPoint) -> Option<u32> {
        let items = self.ivars().items.borrow();
        items.iter().enumerate().find_map(|(i, item)| {
            let r = self.button_rect(i);
            let inside = p.x >= r.origin.x
                && p.x < r.origin.x + r.size.width
                && p.y >= r.origin.y
                && p.y < r.origin.y + r.size.height;
            inside.then_some(item.id)
        })
    }

    fn draw(&self) {
        let Some(nsctx) = NSGraphicsContext::currentContext() else { return };
        let ctx = nsctx.CGContext();
        let bounds = self.bounds();

        // 底板：深色半透明，圆角。深色是为了在任何截图内容之上都能看清
        CGContext::set_rgb_fill_color(Some(&ctx), 0.11, 0.11, 0.12, 0.94);
        fill_round_rect(&ctx, bounds, CORNER + 2.0);
        CGContext::set_rgb_stroke_color(Some(&ctx), 1.0, 1.0, 1.0, 0.14);
        CGContext::set_line_width(Some(&ctx), 1.0);
        stroke_round_rect(&ctx, inset(bounds, 0.5), CORNER + 2.0);

        let items = self.ivars().items.borrow();
        let font = NSFont::systemFontOfSize(FONT_SIZE);
        for (i, item) in items.iter().enumerate() {
            let r = self.button_rect(i);
            if item.selected {
                CGContext::set_rgb_fill_color(Some(&ctx), 0.0, 0.48, 1.0, 1.0);
                fill_round_rect(&ctx, r, CORNER);
            }
            // 选中态用白字，未选中用浅灰，保证两种底色下都够对比度
            let fg = if item.selected {
                NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0)
            } else {
                NSColor::colorWithSRGBRed_green_blue_alpha(0.82, 0.82, 0.85, 1.0)
            };
            let s = NSString::from_str(&item.label);
            let attrs = NSDictionary::from_slices(
                &[unsafe { NSFontAttributeName }, unsafe { NSForegroundColorAttributeName }],
                &[&*font as &objc2::runtime::AnyObject, &*fg],
            );
            let size = unsafe { s.sizeWithAttributes(Some(&attrs)) };
            let point = NSPoint::new(
                r.origin.x + (r.size.width - size.width) / 2.0,
                r.origin.y + (r.size.height - size.height) / 2.0,
            );
            unsafe { s.drawAtPoint_withAttributes(point, Some(&attrs)) };
        }
    }
}

fn inset(r: NSRect, d: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(r.origin.x + d, r.origin.y + d),
        NSSize::new(
            (r.size.width - d * 2.0).max(0.0),
            (r.size.height - d * 2.0).max(0.0),
        ),
    )
}

/// 圆角矩形路径。CoreGraphics 没有现成的圆角矩形绘制函数，需自行拼接。
fn round_rect_path(ctx: &CGContext, r: NSRect, radius: f64) {
    let radius = radius.min(r.size.width / 2.0).min(r.size.height / 2.0);
    let (x, y, w, h) = (r.origin.x, r.origin.y, r.size.width, r.size.height);
    CGContext::begin_path(Some(ctx));
    CGContext::move_to_point(Some(ctx), x + radius, y);
    CGContext::add_arc_to_point(Some(ctx), x + w, y, x + w, y + h, radius);
    CGContext::add_arc_to_point(Some(ctx), x + w, y + h, x, y + h, radius);
    CGContext::add_arc_to_point(Some(ctx), x, y + h, x, y, radius);
    CGContext::add_arc_to_point(Some(ctx), x, y, x + w, y, radius);
    CGContext::close_path(Some(ctx));
}

fn fill_round_rect(ctx: &CGContext, r: NSRect, radius: f64) {
    round_rect_path(ctx, r, radius);
    CGContext::fill_path(Some(ctx));
}

fn stroke_round_rect(ctx: &CGContext, r: NSRect, radius: f64) {
    round_rect_path(ctx, r, radius);
    CGContext::stroke_path(Some(ctx));
}
