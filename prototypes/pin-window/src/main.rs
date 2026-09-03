//! PinWall 原型 1：贴图窗口的置顶行为验证
//!
//! 验证目标：
//!   A. macOS 上浮窗能否盖在**全屏应用**之上（NSWindow level × collectionBehavior 组合）
//!   B. 多显示器 + 混合 DPI 下，跨屏拖动时 scale factor 变化是否被正确处理
//!   C. 鼠标穿透与透明度调节是否可用
//!
//! 这是一次性验证代码，不是产品代码。

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

// ---------------------------------------------------------------- 平台层级定义

/// macOS NSWindow level 常量。数值越大越靠上。
/// 关键：仅靠 level 无法盖住全屏应用，必须配合 collectionBehavior。
#[cfg(target_os = "macos")]
const LEVELS: &[(&str, isize)] = &[
    ("Normal (0)", 0),
    ("Floating (3)", 3),
    ("Status (25)", 25),
    ("PopUpMenu (101)", 101),
    ("ScreenSaver (1000)", 1000),
];

#[cfg(not(target_os = "macos"))]
const LEVELS: &[(&str, isize)] = &[("Normal", 0), ("AlwaysOnTop", 1)];

struct PinState {
    opacity: f64,
    click_through: bool,
    level_idx: usize,
    /// macOS: collectionBehavior 是否包含 CanJoinAllSpaces | FullScreenAuxiliary
    join_all_spaces: bool,
}

impl Default for PinState {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            click_through: false,
            // 默认直接用能盖住全屏应用的组合，方便一上来就看到结论
            level_idx: 2, // Status (25)
            join_all_spaces: true,
        }
    }
}

// ---------------------------------------------------------------- macOS 实现

#[cfg(target_os = "macos")]
mod platform {
    use super::PinState;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSView, NSWindow, NSWindowCollectionBehavior};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return None;
        };
        // SAFETY: winit 保证 AppKit handle 中的 ns_view 是有效的 NSView 指针
        let view: &NSView = unsafe { &*(h.ns_view.as_ptr() as *const NSView) };
        view.window()
    }

    pub fn apply(window: &Window, st: &PinState) {
        let Some(w) = ns_window(window) else {
            eprintln!("!! 拿不到 NSWindow，macOS 特化设置未生效");
            return;
        };

        let level = super::LEVELS[st.level_idx].1;
        w.setLevel(level);
        w.setAlphaValue(st.opacity);
        w.setIgnoresMouseEvents(st.click_through);

        // 这一步才是能否盖住全屏应用的关键：
        //   CanJoinAllSpaces    —— 窗口出现在所有 Space 上
        //   FullScreenAuxiliary —— 允许与全屏应用共存于同一 Space
        let behavior = if st.join_all_spaces {
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
        } else {
            NSWindowCollectionBehavior::Default
        };
        w.setCollectionBehavior(behavior);
    }

    pub fn describe() -> &'static str {
        "macOS: NSWindow.level + collectionBehavior"
    }
}

// ---------------------------------------------------------------- Windows 实现
// 注：本机为 macOS，以下代码**未经实机验证**，仅为对照实现。

#[cfg(target_os = "windows")]
mod platform {
    use super::PinState;
    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE,
        HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_LAYERED,
        WS_EX_TRANSPARENT,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    fn hwnd(window: &Window) -> Option<HWND> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return None;
        };
        Some(HWND(h.hwnd.get() as *mut _))
    }

    pub fn apply(window: &Window, st: &PinState) {
        let Some(h) = hwnd(window) else { return };
        unsafe {
            let mut ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            ex |= WS_EX_LAYERED.0;
            if st.click_through {
                ex |= WS_EX_TRANSPARENT.0;
            } else {
                ex &= !WS_EX_TRANSPARENT.0;
            }
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex as isize);

            let alpha = (st.opacity.clamp(0.0, 1.0) * 255.0) as u8;
            let _ = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA);

            if st.level_idx > 0 {
                let _ = SetWindowPos(
                    h, Some(HWND_TOPMOST), 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    pub fn describe() -> &'static str {
        "Windows: WS_EX_LAYERED/TRANSPARENT + HWND_TOPMOST（未实机验证）"
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::PinState;
    use winit::window::Window;
    pub fn apply(_w: &Window, _s: &PinState) {}
    pub fn describe() -> &'static str {
        "此平台无特化实现（Wayland 下贴图无法定位，见 docs/mvp-risks.md R1）"
    }
}

// ---------------------------------------------------------------- 应用

struct App {
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    ctx: Option<softbuffer::Context<Rc<Window>>>,
    st: PinState,
    last_scale: f64,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            surface: None,
            ctx: None,
            st: PinState::default(),
            last_scale: 0.0,
        }
    }

    fn sync(&mut self) {
        if let Some(w) = &self.window {
            platform::apply(w, &self.st);
            self.print_status();
        }
    }

    fn print_status(&self) {
        let (name, val) = LEVELS[self.st.level_idx];
        println!(
            "  level={name}({val})  opacity={:.2}  穿透={}  跨Space={}",
            self.st.opacity,
            if self.st.click_through { "开" } else { "关" },
            if self.st.join_all_spaces { "开" } else { "关" },
        );
    }

    fn diagnostics(&self) {
        let Some(w) = &self.window else { return };
        println!("\n──────── 诊断 ────────");
        println!("  scale_factor : {}", w.scale_factor());
        if let Ok(p) = w.outer_position() {
            println!("  窗口位置(物理): {:?}", p);
        }
        println!("  窗口尺寸(物理): {:?}", w.inner_size());
        if let Some(m) = w.current_monitor() {
            println!(
                "  当前显示器   : {:?}  尺寸={:?}  scale={}",
                m.name().unwrap_or_else(|| "<未命名>".into()),
                m.size(),
                m.scale_factor()
            );
        }
        println!("  ── 所有显示器 ──");
        if let Some(w2) = &self.window {
            for (i, m) in w2.available_monitors().enumerate() {
                println!(
                    "   [{i}] {:?} 尺寸={:?} 位置={:?} scale={}",
                    m.name().unwrap_or_else(|| "<未命名>".into()),
                    m.size(),
                    m.position(),
                    m.scale_factor()
                );
            }
        }
        println!("──────────────────────\n");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("PinWall 原型 · 贴图窗口")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(PhysicalSize::new(420u32, 300u32))
            .with_position(PhysicalPosition::new(200i32, 200i32));

        let window = Rc::new(event_loop.create_window(attrs).expect("创建窗口失败"));
        let ctx = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&ctx, window.clone()).expect("softbuffer surface");

        self.last_scale = window.scale_factor();
        self.window = Some(window);
        self.ctx = Some(ctx);
        self.surface = Some(surface);

        println!("平台特化：{}", platform::describe());
        self.sync();
        self.diagnostics();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            // ---- 关键验证点 B：跨屏拖动时 DPI 变化 ----
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                println!(
                    "\n>>> [DPI 变化] {} -> {}  （贴图内容需在此重新采样，否则会糊）",
                    self.last_scale, scale_factor
                );
                self.last_scale = scale_factor;
                self.diagnostics();
            }

            WindowEvent::Moved(pos) => {
                if let Some(w) = &self.window {
                    if let Some(m) = w.current_monitor() {
                        println!(
                            "[移动] 位置={:?} 所在屏={:?} scale={}",
                            pos,
                            m.name().unwrap_or_else(|| "<未命名>".into()),
                            m.scale_factor()
                        );
                    }
                }
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => match logical_key.as_ref() {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Character("l") => {
                    self.st.level_idx = (self.st.level_idx + 1) % LEVELS.len();
                    println!("\n[切换层级]");
                    self.sync();
                }
                Key::Character("s") => {
                    self.st.join_all_spaces = !self.st.join_all_spaces;
                    println!("\n[切换 collectionBehavior]");
                    self.sync();
                }
                Key::Character("c") => {
                    self.st.click_through = !self.st.click_through;
                    println!("\n[切换鼠标穿透] 注意：开启后本窗口不再接收键盘/鼠标，需切回终端操作");
                    self.sync();
                }
                Key::Character("[") => {
                    self.st.opacity = (self.st.opacity - 0.1).max(0.1);
                    self.sync();
                }
                Key::Character("]") => {
                    self.st.opacity = (self.st.opacity + 0.1).min(1.0);
                    self.sync();
                }
                Key::Character("p") => self.diagnostics(),
                _ => {}
            },

            WindowEvent::RedrawRequested => {
                let (Some(surface), Some(window)) = (&mut self.surface, &self.window) else {
                    return;
                };
                let size = window.inner_size();
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
                    return;
                };
                surface.resize(w, h).unwrap();
                let mut buf = surface.buffer_mut().unwrap();
                draw_test_pattern(&mut buf, size.width, size.height);
                buf.present().unwrap();
            }
            _ => {}
        }
    }
}

/// 测试图案：1 物理像素的交替细线 + 边框 + 十字线。
/// 若窗口被系统缩放（而非按物理像素渲染），细线会立刻糊成灰块 —— 这就是 DPI 验证的判据。
fn draw_test_pattern(buf: &mut [u32], w: u32, h: u32) {
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let border = x < 3 || y < 3 || x >= w - 3 || y >= h - 3;
            let cross = x == w / 2 || y == h / 2;
            let hairline = (x % 2 == 0) && (y > h / 2);
            let checker = ((x / 16) + (y / 16)) % 2 == 0;

            buf[i] = if border {
                0x00FF3B30 // 红边框：确认窗口边界无系统装饰
            } else if cross {
                0x0000C853 // 绿十字：确认中心点定位
            } else if hairline {
                0x00FFFFFF // 下半部 1px 白细线：糊了就说明缩放有问题
            } else if y > h / 2 {
                0x00101010
            } else if checker {
                0x00303030
            } else {
                0x00202020
            };
        }
    }
}

fn main() {
    println!(
        r#"
╔════════════════════════════════════════════════════════════╗
║  PinWall 原型 1 · 贴图窗口置顶行为验证                     ║
╠════════════════════════════════════════════════════════════╣
║  l  切换窗口层级（Normal→Floating→Status→PopUp→ScreenSaver）║
║  s  切换 collectionBehavior（跨 Space / 全屏共存）         ║
║  c  切换鼠标穿透                                           ║
║ [ ] 调整透明度                                             ║
║  p  打印诊断（位置 / DPI / 所有显示器）                    ║
║ Esc 退出                                                   ║
╠════════════════════════════════════════════════════════════╣
║  验证 A：把某个 App 切到全屏，看本窗口是否仍浮在其上       ║
║  验证 B：把窗口拖到另一块不同缩放的屏，看 DPI 日志与清晰度 ║
╚════════════════════════════════════════════════════════════╝
"#
    );

    let event_loop = EventLoop::new().expect("创建事件循环失败");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("事件循环异常");
}
