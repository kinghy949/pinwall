//! 贴图浮窗的交互视图（macOS）。
//!
//! 贴图不能是惰性的图片：用户会立刻想去拖它、关它。
//! 本视图负责绘制图像并处理这两件事。

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSColor, NSEvent, NSEventModifierFlags, NSFocusRingType, NSFont,
    NSFontAttributeName, NSGraphicsContext, NSImage, NSStringDrawing, NSTextField, NSView,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString};

use super::annot_draw::{self, inset};
use crate::geom::{Rect, Size};
use crate::{DrawCommand, KeyHandler, KeyPress, PointerEvent, PointerHandler, Rgba, TextInput};

/// 缩放范围。下限保证贴图不会小到点不中，上限避免无意义的糊放大。
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 8.0;
const OPACITY_MIN: f64 = 0.1;

/// 输入框相对文字锚点的左移量。NSTextField 会给文字留一点内边距，
/// 不抵消掉，输入时看到的位置就与提交后画出来的位置错开几个点 ——
/// 而「所见即所得」正是就地输入的全部意义。
const FIELD_INSET: f64 = 2.0;
/// 输入框在内容之外多留的宽度，供光标停靠。
const FIELD_SLACK: f64 = 8.0;


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
    /// 正在编辑文字时的原生输入框。
    pub text_field: RefCell<Option<Retained<NSTextField>>>,
    /// 该文字对象的锚点（**图像原始坐标**）与 100% 缩放下的字号。
    pub text_anchor: Cell<Option<(crate::geom::Point, f64)>>,
    /// 用户已明确结束输入（回车，或点击了画面别处）。
    pub text_done: Cell<bool>,
    /// 窗口内按键的回调。
    pub key_handler: RefCell<Option<KeyHandler>>,
    /// 输入框是否曾真正拿到过焦点。
    ///
    /// 少了这一位，创建当帧「还没有字段编辑器」就会被误判成「已失焦」，
    /// 输入框刚弹出就自己关掉。
    pub text_focused: Cell<bool>,
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
            // 先取键盘焦点：工具键、空格、⌘Z 全部由本窗口消费，没有焦点就收不到
            self.focus_window();
            // 文字输入进行中：点击画面别处即为提交，而不是顺手再开一个新文字框。
            // 落在输入框自身范围内的点击根本到不了这里，由输入框先行接管。
            if self.ivars().text_field.borrow().is_some() {
                self.ivars().text_done.set(true);
                return;
            }
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

        /// 贴图要接收按键，就必须能成为第一响应者。NSView 默认返回 false。
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            // 文字输入进行中时按键属于输入框。正常情况下按键根本到不了这里
            // （第一响应者是字段编辑器），留这道判断是防着焦点错乱。
            if self.ivars().text_field.borrow().is_some() {
                return;
            }
            let Some(press) = key_press_from(event) else { return };
            let handler = self.ivars().key_handler.borrow().clone();
            if let Some(h) = handler {
                h(press);
            }
        }

        /// 吞掉按键的默认处理，免得未识别的键触发系统提示音。
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            // ⌘ 组合走的是 key equivalent 通路而非 keyDown:，须单独接住
            if self.ivars().text_field.borrow().is_some() {
                return Bool::NO;
            }
            let press = match key_press_from(event) {
                Some(p @ (KeyPress::Command(_) | KeyPress::CommandShift(_))) => p,
                _ => return Bool::NO,
            };
            let handler = self.ivars().key_handler.borrow().clone();
            match handler {
                Some(h) => {
                    h(press);
                    Bool::YES
                }
                None => Bool::NO,
            }
        }

        /// 输入框的 action：用户按下回车。
        ///
        /// 由本视图直接充当 target，省掉一个只为转发一次回调而存在的
        /// delegate 对象。NSControl 的 target 是不持有的弱引用，
        /// 而输入框是本视图的子视图，生命周期严格更短，不会悬垂。
        #[unsafe(method(pinwallTextCommitted:))]
        fn text_committed(&self, _sender: *mut AnyObject) {
            self.ivars().text_done.set(true);
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
            text_field: RefCell::new(None),
            text_anchor: Cell::new(None),
            text_done: Cell::new(false),
            text_focused: Cell::new(false),
            key_handler: RefCell::new(None),
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

        // 缩放会改变输入框该有的位置与字号。与其同步这一堆状态，
        // 不如就此提交 —— 打字打到一半去滚滚轮本就少见。
        if iv.text_field.borrow().is_some() {
            iv.text_done.set(true);
        }

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
        if !on {
            // 退出标注模式时输入框必须一并收走，否则它会孤零零地悬在贴图上，
            // 而此时已经没人在轮询它了
            self.end_text_input();
        }
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

    pub fn set_key_handler(&self, handler: KeyHandler) {
        *self.ivars().key_handler.borrow_mut() = Some(handler);
    }

    /// 让本窗口取得键盘焦点。
    ///
    /// 必须连带激活本应用：系统只把键盘事件投递给当前活跃的应用，
    /// 光让窗口成为 key window 是不够的。代价是点击贴图会抢走用户
    /// 原本应用的焦点 —— 这是把工具键从全局热键里摘出来所必须付的账，
    /// 也是 Snipaste 一类竞品的一致做法。
    fn focus_window(&self) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(w) = self.window() else { return };
        if !w.isKeyWindow() {
            // 见 begin_text_input 中同样的注释：不能用 macOS 14 才有的 activate()
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            w.makeKeyAndOrderFront(None);
        }
        // 文字输入进行中时第一响应者归输入框，不要抢回来
        if self.ivars().text_field.borrow().is_none() {
            w.makeFirstResponder(Some(self));
        }
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

    /// 就地弹出原生输入框。
    ///
    /// `rect` 与 `font_size` 均为**图像原始坐标/字号**，按当前缩放换算后落位。
    pub fn begin_text_input(&self, rect: Rect, initial: &str, font_size: f64, color: Rgba) {
        // 上一个输入框若还在，先收掉；同一时刻只允许编辑一个文字对象
        self.end_text_input();
        let Some(mtm) = MainThreadMarker::new() else { return };
        let iv = self.ivars();
        let z = iv.zoom.get().max(f64::EPSILON);

        let field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            self.field_frame(rect, z, font_size),
        );
        field.setStringValue(&NSString::from_str(initial));
        field.setFont(Some(&NSFont::systemFontOfSize(font_size * z)));
        field.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            color.r, color.g, color.b, color.a,
        )));
        // 去掉一切边框与聚焦环，让输入框尽量接近文字最终的样子；
        // 但保留浅色底 —— 红字落在深色截图上会看不见自己在打什么
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(true);
        field.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            1.0, 1.0, 1.0, 0.88,
        )));
        field.setFocusRingType(NSFocusRingType::None);
        field.setEditable(true);
        field.setSelectable(true);
        unsafe {
            field.setTarget(Some(self));
            field.setAction(Some(sel!(pinwallTextCommitted:)));
        }
        self.addSubview(&field);

        // 键盘焦点。本应用是 Accessory 策略（无 Dock 图标、不出现在 ⌘Tab 中），
        // 激活它不改变用户的窗口布局；而不激活就拿不到按键事件 ——
        // 系统只把键盘事件投递给当前活跃的应用。
        if let Some(w) = self.window() {
            // 用 activateIgnoringOtherApps 而非 macOS 14 才有的 activate()：
            // 本项目的下限由 ScreenCaptureKit 定在 12.3，在 12/13 上调
            // activate() 会因选择子不存在直接崩掉。等下限抬到 14 再换。
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            w.makeKeyAndOrderFront(None);
            w.makeFirstResponder(Some(&field));
        }
        let _ = mtm;

        iv.text_anchor.set(Some((rect.origin, font_size)));
        iv.text_done.set(false);
        iv.text_focused.set(false);
        *iv.text_field.borrow_mut() = Some(field);
        self.setNeedsDisplay(true);
    }

    /// 读取输入框当前内容。无输入进行中时返回 `None`。
    pub fn poll_text_input(&self) -> Option<TextInput> {
        let iv = self.ivars();
        let field = iv.text_field.borrow().clone()?;
        let z = iv.zoom.get().max(f64::EPSILON);
        let text = field.stringValue().to_string();

        // 按实际字体度量文字占多大，供上层确定包围盒
        let measured = measure(&text, field.font().as_deref());
        // 输入框随内容加宽，否则超出初始宽度的字会滚出可视区，
        // 用户只能看见自己刚打的最后几个字
        let want = measured.width + FIELD_SLACK;
        let frame = field.frame();
        if want > frame.size.width {
            field.setFrameSize(NSSize::new(want, frame.size.height));
        }

        // 拿到过焦点之后又失去 —— 说明用户点去了别处，视同提交
        let editing = field.currentEditor().is_some();
        if editing {
            iv.text_focused.set(true);
        }
        let finished = iv.text_done.get() || (iv.text_focused.get() && !editing);

        Some(TextInput {
            text,
            extent: Size::new(measured.width / z, measured.height / z),
            finished,
        })
    }

    /// 收起输入框。内容的去留由上层决定，此处只撤掉控件。
    pub fn end_text_input(&self) {
        let iv = self.ivars();
        let field = iv.text_field.borrow_mut().take();
        if let Some(f) = field {
            if let Some(w) = self.window() {
                // 先交还第一响应者，否则字段编辑器会连着一个已被移除的视图
                w.makeFirstResponder(None);
            }
            f.removeFromSuperview();
        }
        iv.text_anchor.set(None);
        iv.text_done.set(false);
        iv.text_focused.set(false);
        self.setNeedsDisplay(true);
    }

    /// 输入框在视图局部坐标（Cocoa 左下原点）下的位置。
    fn field_frame(&self, rect: Rect, z: f64, font_size: f64) -> NSRect {
        let view_h = self.bounds().size.height;
        // 行高按字号估算，保证输入框至少装得下一行字
        let h = (rect.size.height * z).max(font_size * z * 1.4);
        let w = (rect.size.width * z).max(font_size * z * 4.0);
        NSRect::new(
            NSPoint::new(
                rect.origin.x * z - FIELD_INSET,
                view_h - rect.origin.y * z - h,
            ),
            NSSize::new(w, h),
        )
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
    /// 实际画法在 [`annot_draw`]，与导出时共用同一份代码。此处只负责
    /// 把「当前缩放」这一屏幕特有的信息喂进去：标注存在**图像原始坐标**
    /// 中，随图像一起缩放，才不会在放大后漂移或显得过细。
    fn draw_commands(&self, bounds: NSRect) {
        let cmds = self.ivars().commands.borrow();
        if cmds.is_empty() {
            return;
        }
        let Some(nsctx) = NSGraphicsContext::currentContext() else { return };
        let z = self.ivars().zoom.get().max(f64::EPSILON);
        // 翻转 y 时用的是**图像原始高度**，而非已缩放的视图高度
        annot_draw::draw(&nsctx.CGContext(), &cmds, bounds.size.height / z, z);
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

/// 用给定字体量出文字占据的尺寸（视图坐标，即已含当前缩放）。
///
/// 与工具栏用的是同一套 `sizeWithAttributes:` —— 也与 `annot_draw`
/// 最终绘制文字所用的度量一致，两处若各自估算，包围盒就会与字对不齐。
fn measure(text: &str, font: Option<&NSFont>) -> NSSize {
    let Some(font) = font else { return NSSize::new(0.0, 0.0) };
    let s = NSString::from_str(text);
    let attrs = NSDictionary::from_slices(
        &[unsafe { NSFontAttributeName }],
        &[font as &AnyObject],
    );
    unsafe { s.sizeWithAttributes(Some(&attrs)) }
}

/// 把 Cocoa 的按键事件翻成 [`KeyPress`]。不关心的组合返回 `None`。
///
/// 用 `charactersIgnoringModifiers` 而非 `characters`：后者在按住 ⌘ 或切到
/// 非拉丁输入法时给出的字符会变，工具键会随输入法失灵。
fn key_press_from(event: &NSEvent) -> Option<KeyPress> {
    /// Esc 的虚拟键码。它没有可打印字符，只能按键码认。
    const KEYCODE_ESCAPE: u16 = 53;
    if event.keyCode() == KEYCODE_ESCAPE {
        return Some(KeyPress::Escape);
    }
    let flags = event.modifierFlags();
    // 带 ⌃ / ⌥ 的组合一律不接，留给系统和输入法
    if flags.contains(NSEventModifierFlags::Control)
        || flags.contains(NSEventModifierFlags::Option)
    {
        return None;
    }
    let chars = event.charactersIgnoringModifiers()?;
    let c = chars.to_string().chars().next()?.to_ascii_lowercase();
    if flags.contains(NSEventModifierFlags::Command) {
        if flags.contains(NSEventModifierFlags::Shift) {
            Some(KeyPress::CommandShift(c))
        } else {
            Some(KeyPress::Command(c))
        }
    } else if c.is_control() {
        None
    } else {
        Some(KeyPress::Plain(c))
    }
}
