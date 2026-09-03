//! macOS 后端。
//!
//! 两个关键实现约束，均来自原型实测（见 `docs/mvp-risks.md`）：
//!
//! 1. 窗口一律使用 `NSPanel` + `NonactivatingPanel`，**不能用 `NSWindow`** ——
//!    后者无法进入其他应用的全屏 Space。
//! 2. 遮罩每屏一个，不能用并集构造单个跨屏窗口。

use objc2::rc::Retained;
use std::cell::{Cell, RefCell};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::geom::{Point, Rect};
use super::overlay_view::{OverlayView, OverlayViewIvars};
use super::image::ns_image_from_bgra;
use super::pin_view::PinView;
use crate::{Error, Overlay, PinImage, PinWindow, Platform, PointerHandler, Result, ScreenId, ScreenInfo};

/// `NSScreenSaverWindowLevel`。实测该层级配合 NonactivatingPanel 可覆盖全屏应用。
const OVERLAY_LEVEL: isize = 1000;

pub struct MacPlatform {
    mtm: MainThreadMarker,
}

impl MacPlatform {
    pub fn new() -> Result<Self> {
        let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;
        Ok(Self { mtm })
    }

    /// 主屏高度（逻辑点），用于 Cocoa 与本 crate 坐标系之间的换算。
    ///
    /// Cocoa 全局坐标原点在主屏**左下角**且 y 轴向上；本 crate 约定原点在主屏
    /// **左上角**且 y 轴向下。换算只需翻转 y。
    fn primary_height(&self) -> f64 {
        NSScreen::screens(self.mtm)
            .iter()
            .next()
            .map(|s| s.frame().size.height)
            .unwrap_or(0.0)
    }

    fn to_cocoa(&self, r: Rect) -> NSRect {
        let h = self.primary_height();
        NSRect::new(
            NSPoint::new(r.origin.x, h - (r.origin.y + r.size.height)),
            NSSize::new(r.size.width, r.size.height),
        )
    }

    fn from_cocoa(&self, r: NSRect) -> Rect {
        let h = self.primary_height();
        Rect::from_xywh(
            r.origin.x,
            h - (r.origin.y + r.size.height),
            r.size.width,
            r.size.height,
        )
    }

    fn make_panel(&self, cocoa_frame: NSRect, opaque: bool) -> Retained<NSPanel> {
        let panel: Retained<NSPanel> = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(self.mtm),
            cocoa_frame,
            // NonactivatingPanel 是能覆盖他人全屏窗口的结构前提
            NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        );
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setOpaque(opaque);
        panel.setLevel(OVERLAY_LEVEL);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel
    }
}

impl Platform for MacPlatform {
    fn screens(&self) -> Result<Vec<ScreenInfo>> {
        let screens = NSScreen::screens(self.mtm);
        Ok(screens
            .iter()
            .enumerate()
            .map(|(i, s)| ScreenInfo {
                id: ScreenId(i as u32),
                name: s.localizedName().to_string(),
                frame: self.from_cocoa(s.frame()),
                scale: s.backingScaleFactor(),
                // NSScreen.screens 的首个元素即带菜单栏的主屏
                is_primary: i == 0,
            })
            .collect())
    }

    fn create_pin(&self, frame: Rect) -> Result<Box<dyn PinWindow>> {
        let panel = self.make_panel(self.to_cocoa(frame), true);
        // 投影进一步把贴图与其下方的真实界面区分开
        panel.setHasShadow(true);
        panel.orderFrontRegardless();
        Ok(Box::new(MacPin {
            panel,
            view: RefCell::new(None),
            primary_height: self.primary_height(),
            mtm: self.mtm,
        }))
    }

    fn create_overlay(&self, screen: &ScreenInfo) -> Result<Box<dyn Overlay>> {
        let cocoa = self.to_cocoa(screen.frame);
        let panel = self.make_panel(cocoa, false);
        // 背景交给视图绘制（需要在压暗层上镂空选区），窗口本身保持透明
        panel.setBackgroundColor(Some(&NSColor::clearColor()));

        let view = OverlayView::new(
            self.mtm,
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(cocoa.size.width, cocoa.size.height),
            ),
            OverlayViewIvars {
                screen_frame: screen.frame,
                primary_height: self.primary_height(),
                handler: RefCell::new(None),
                selection: Cell::new(None),
            },
        );
        panel.setContentView(Some(&view));

        Ok(Box::new(MacOverlay {
            panel,
            view,
            screen_id: screen.id,
            frame: screen.frame,
        }))
    }
}

// ---------------------------------------------------------------- 贴图浮窗

struct MacPin {
    panel: Retained<NSPanel>,
    /// 视图在 set_image 时才创建，故用 RefCell 延迟填入。
    view: RefCell<Option<Retained<PinView>>>,
    primary_height: f64,
    mtm: MainThreadMarker,
}

impl PinWindow for MacPin {
    fn set_image(&self, image: &PinImage<'_>) -> Result<()> {
        let ns_image = ns_image_from_bgra(image)?;
        let logical_w = image.width as f64 / image.scale;
        let logical_h = image.height as f64 / image.scale;

        let view = PinView::new(
            self.mtm,
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(logical_w, logical_h)),
            ns_image,
        );
        self.panel.setContentView(Some(&view));
        *self.view.borrow_mut() = Some(view);

        // 保持左上角不动地调整尺寸 —— Cocoa 的 origin 在左下角，
        // 直接改 size 会让窗口视觉上向下生长
        let old = self.panel.frame();
        let new_origin = NSPoint::new(old.origin.x, old.origin.y + old.size.height - logical_h);
        self.panel.setFrame_display(
            NSRect::new(new_origin, NSSize::new(logical_w, logical_h)),
            true,
        );
        Ok(())
    }

    fn show(&self) {
        self.panel.orderFrontRegardless();
    }

    fn hide(&self) {
        self.panel.orderOut(None);
    }

    fn close(self: Box<Self>) {
        self.panel.close();
    }

    fn set_opacity(&self, alpha: f64) {
        self.panel.setAlphaValue(alpha.clamp(0.0, 1.0));
    }

    fn set_click_through(&self, enabled: bool) {
        // 经视图设置，使视图内记录的状态与窗口实际行为保持一致；
        // 直接改窗口会让视图里的中键切换逻辑读到过期状态。
        match self.view.borrow().as_ref() {
            Some(v) => v.set_click_through(enabled),
            None => self.panel.setIgnoresMouseEvents(enabled),
        }
    }

    fn move_to(&self, origin: Point) {
        let size = self.panel.frame().size;
        self.panel.setFrameOrigin(NSPoint::new(
            origin.x,
            self.primary_height - (origin.y + size.height),
        ));
    }

    fn frame(&self) -> Rect {
        let f = self.panel.frame();
        Rect::from_xywh(
            f.origin.x,
            self.primary_height - (f.origin.y + f.size.height),
            f.size.width,
            f.size.height,
        )
    }

    fn is_click_through(&self) -> bool {
        self.view.borrow().as_ref().is_some_and(|v| v.is_click_through())
    }

    fn is_closed(&self) -> bool {
        self.view.borrow().as_ref().is_some_and(|v| v.is_closed())
    }

    fn current_screen(&self) -> Option<ScreenId> {
        let on = self.panel.screen()?;
        let mtm = MainThreadMarker::new()?;
        NSScreen::screens(mtm)
            .iter()
            .position(|s| s.frame() == on.frame())
            .map(|i| ScreenId(i as u32))
    }
}

// ---------------------------------------------------------------- 捕获遮罩

struct MacOverlay {
    panel: Retained<NSPanel>,
    view: Retained<OverlayView>,
    screen_id: ScreenId,
    frame: Rect,
}

impl Overlay for MacOverlay {
    fn show(&self) {
        self.panel.orderFrontRegardless();
    }

    fn hide(&self) {
        self.panel.orderOut(None);
    }

    fn close(self: Box<Self>) {
        self.panel.close();
    }

    fn screen_id(&self) -> ScreenId {
        self.screen_id
    }

    fn frame(&self) -> Rect {
        self.frame
    }

    fn set_pointer_handler(&self, handler: PointerHandler) {
        self.view.set_handler(handler);
    }

    fn set_selection(&self, rect: Option<Rect>) {
        self.view.set_selection(rect);
    }
}
