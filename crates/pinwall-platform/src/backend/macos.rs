//! macOS 后端。
//!
//! 两个关键实现约束，均来自原型实测（见 `docs/mvp-risks.md`）：
//!
//! 1. 窗口一律使用 `NSPanel` + `NonactivatingPanel`，**不能用 `NSWindow`** ——
//!    后者无法进入其他应用的全屏 Space。
//! 2. 遮罩每屏一个，不能用并集构造单个跨屏窗口。

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::geom::{Point, Rect};
use crate::{Error, Overlay, PinWindow, Platform, Result, ScreenId, ScreenInfo};

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
        panel.orderFrontRegardless();
        Ok(Box::new(MacPin {
            panel,
            primary_height: self.primary_height(),
        }))
    }

    fn create_overlay(&self, screen: &ScreenInfo) -> Result<Box<dyn Overlay>> {
        let panel = self.make_panel(self.to_cocoa(screen.frame), false);
        panel.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.0, 0.0, 0.0, 0.35,
        )));
        Ok(Box::new(MacOverlay {
            panel,
            screen_id: screen.id,
            frame: screen.frame,
        }))
    }
}

// ---------------------------------------------------------------- 贴图浮窗

struct MacPin {
    panel: Retained<NSPanel>,
    primary_height: f64,
}

impl PinWindow for MacPin {
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
        self.panel.setIgnoresMouseEvents(enabled);
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
}
