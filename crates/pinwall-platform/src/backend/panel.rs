//! 可获得键盘焦点的无边框面板（macOS）。
//!
//! # 为什么必须子类化
//!
//! `NSWindow` 只在窗口带标题栏或可缩放时才允许成为 key window，而 PinWall 的
//! 窗口一律是 `Borderless` —— 不覆写 `canBecomeKeyWindow`，贴图就永远拿不到
//! 键盘焦点，文字标注也就无从输入。
//!
//! 与 `NonactivatingPanel` 组合后，面板可以在不激活本应用的前提下参与
//! 键盘焦点，这正是浮动工具窗该有的行为。

use objc2::rc::Retained;
use objc2::{define_class, msg_send, MainThreadOnly};
use objc2_app_kit::{NSBackingStoreType, NSPanel, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSRect};

define_class!(
    // SAFETY:
    // - 父类 NSPanel 无特殊子类化要求。
    // - KeyablePanel 不实现 Drop。
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "PinWallPanel"]
    pub struct KeyablePanel;

    impl KeyablePanel {
        /// 无边框窗口默认不能成为 key window，须显式放行。
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// 但不做 main window —— 那是文档窗口的角色，
        /// 贴图抢过来只会让菜单栏跟着一起变。
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

impl KeyablePanel {
    /// 造一个本类的实例，但以父类型返回 —— 调用方只需要 `NSPanel` 的接口。
    pub fn make(
        mtm: MainThreadMarker,
        frame: NSRect,
        style: NSWindowStyleMask,
        backing: NSBackingStoreType,
    ) -> Retained<NSPanel> {
        let this = Self::alloc(mtm);
        // SAFETY: 参数类型与 NSWindow 的指定初始化方法一致，接收者为刚分配的实例。
        let panel: Retained<Self> = unsafe {
            msg_send![
                this,
                initWithContentRect: frame,
                styleMask: style,
                backing: backing,
                defer: false,
            ]
        };
        Retained::into_super(panel)
    }
}
