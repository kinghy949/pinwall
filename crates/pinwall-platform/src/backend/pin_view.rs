//! 贴图浮窗的交互视图（macOS）。
//!
//! 贴图不能是惰性的图片：用户会立刻想去拖它、关它。
//! 本视图负责绘制图像并处理这两件事。

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSImage, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};

pub struct PinViewIvars {
    pub image: RefCell<Option<Retained<NSImage>>>,
    /// 按下时鼠标在窗口内的偏移，用于拖动时保持抓取点不变。
    pub grab_offset: Cell<Option<NSPoint>>,
    /// 窗口是否已被用户关闭。上层据此回收对应的 PinWindow。
    pub closed: Cell<bool>,
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
        }
    }
);

impl PinView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, image: Retained<NSImage>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PinViewIvars {
            image: RefCell::new(Some(image)),
            grab_offset: Cell::new(None),
            closed: Cell::new(false),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn is_closed(&self) -> bool {
        self.ivars().closed.get()
    }

    fn close_self(&self) {
        self.ivars().closed.set(true);
        if let Some(w) = self.window() {
            w.orderOut(None);
        }
    }
}
