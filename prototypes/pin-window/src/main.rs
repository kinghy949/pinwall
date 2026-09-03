//! PinWall 原型 1：贴图窗口的置顶行为验证
//!
//! 已知结论：level=Status(25) + CanJoinAllSpaces|FullScreenAuxiliary + Regular 激活策略
//! **无法**盖住其他应用的全屏窗口（2026-09-03 实测）。
//!
//! 本版改为自动轮播参数矩阵，逐组排查到底哪个变量是决定性的。
//! 窗口左上角画 N 个方块表示当前组合编号（从 1 数起），全屏时可直接目视判断。

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

const DWELL: Duration = Duration::from_secs(4);

/// 一组待验证的窗口参数
#[derive(Clone, Copy)]
struct Combo {
    label: &'static str,
    level: isize,
    /// collectionBehavior 预设：0=全量 1=仅 CanJoinAllSpaces+Stationary 2=仅 FullScreenAuxiliary
    behavior: u8,
    /// true = NSApplicationActivationPolicy.Accessory（无 Dock 图标，不抢 Space）
    accessory: bool,
}

/// kCGMaximumWindowLevel
const MAX_LEVEL: isize = 2_147_483_631;

const COMBOS: &[Combo] = &[
    Combo { label: "ScreenSaver(1000) + FullScreenNone      【主推方案】", level: 1000,      behavior: 3, accessory: true },
    Combo { label: "ScreenSaver(1000) + FullScreenAuxiliary 【已知失败，对照】", level: 1000, behavior: 0, accessory: true },
    Combo { label: "ScreenSaver(1000) + 无fullscreen标志",                 level: 1000,      behavior: 1, accessory: true },
    Combo { label: "PopUpMenu(101)    + FullScreenNone",                   level: 101,       behavior: 3, accessory: true },
    Combo { label: "Maximum(2^31-17)  + FullScreenNone",                   level: MAX_LEVEL, behavior: 3, accessory: true },
];

// ---------------------------------------------------------------- macOS 实现

#[cfg(target_os = "macos")]
mod platform {
    use super::Combo;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSView, NSWindow, NSWindowCollectionBehavior};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return None;
        };
        // SAFETY: winit 保证该指针是有效 NSView
        let view: &NSView = unsafe { &*(h.ns_view.as_ptr() as *const NSView) };
        view.window()
    }

    pub fn apply(window: &Window, c: &Combo) {
        let Some(w) = ns_window(window) else {
            eprintln!("!! 拿不到 NSWindow");
            return;
        };

        w.setLevel(c.level);

        let behavior = match c.behavior {
            0 => {
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle
            }
            1 => {
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
            }
            3 => {
                // FullScreenAuxiliary 的语义是「作为全屏应用的附属窗口」，
                // 与「凌驾于他人全屏之上」方向相反。改用 FullScreenNone：
                // 声明本窗口不参与全屏机制，仅靠 CanJoinAllSpaces 跨 Space。
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle
                    | NSWindowCollectionBehavior::FullScreenNone
            }
            _ => NSWindowCollectionBehavior::FullScreenAuxiliary,
        };
        w.setCollectionBehavior(behavior);

        // 激活策略：Accessory 让 App 不占 Dock、不强制切 Space，
        // 这是覆盖他人全屏窗口时常被忽略的一环
        // 激活策略不再运行时切换：已由 .app bundle 的 LSUIElement=1 在启动时固化，
        // 运行时调用 setActivationPolicy 对 Space 参与行为不可靠。

        // 关键：把窗口重新排入当前 Space，且不激活本应用。
        // 仅设置 level / collectionBehavior 不会让一个已被 order out 的窗口
        // 出现在新切换到的全屏 Space 中 —— 上一版正是漏了这一步。
        w.orderFrontRegardless();
    }

    /// 回传窗口当前的真实状态，用于在日志里直接判定成败，
    /// 不必依赖人眼观察。
    pub fn probe(window: &Window) -> String {
        let Some(w) = ns_window(window) else {
            return "<无 NSWindow>".into();
        };
        let cb = w.collectionBehavior().0;
        // 位含义：1=CanJoinAllSpaces 2=MoveToActiveSpace 16=Stationary
        //         64=IgnoresCycle 128=FullScreenPrimary 256=FullScreenAuxiliary
        let mut flags = Vec::new();
        for (bit, name) in [
            (1usize, "CanJoinAllSpaces"), (2, "MoveToActiveSpace"), (4, "Managed"),
            (8, "Transient"), (16, "Stationary"), (32, "ParticipatesInCycle"),
            (64, "IgnoresCycle"), (128, "FullScreenPrimary"), (256, "FullScreenAuxiliary"),
            (512, "FullScreenNone"),
        ] {
            if cb & bit != 0 { flags.push(name); }
        }
        format!(
            "isVisible={} isOnActiveSpace={} level={} cb={cb}[{}]",
            w.isVisible(),
            w.isOnActiveSpace(),
            w.level(),
            flags.join("|"),
        )
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Combo;
    use winit::window::Window;
    pub fn apply(_w: &Window, _c: &Combo) {}
    pub fn probe(_w: &Window) -> String { "<非 macOS>".into() }
}

// ---------------------------------------------------------------- 应用

struct App {
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    ctx: Option<softbuffer::Context<Rc<Window>>>,
    idx: usize,
    next_switch: Instant,
    paused: bool,
    last_scale: f64,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            surface: None,
            ctx: None,
            idx: 0,
            next_switch: Instant::now() + DWELL,
            paused: false,
            last_scale: 0.0,
        }
    }

    fn apply_current(&mut self) {
        let c = COMBOS[self.idx];
        if let Some(w) = &self.window {
            platform::apply(w, &c);
            w.request_redraw();
        }
        println!(
            "\n▶ 组合 [{}/{}]  ■×{}  {}",
            self.idx + 1,
            COMBOS.len(),
            self.idx + 1,
            c.label
        );
        println!("   level={}  behavior_preset={}  accessory={}", c.level, c.behavior, c.accessory);
        if let Some(w) = &self.window {
            println!("   探针: {}", platform::probe(w));
        }
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
            .with_inner_size(PhysicalSize::new(460u32, 260u32))
            .with_position(PhysicalPosition::new(240i32, 240i32));

        let window = Rc::new(event_loop.create_window(attrs).expect("创建窗口失败"));
        let ctx = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&ctx, window.clone()).expect("softbuffer surface");

        self.last_scale = window.scale_factor();
        self.window = Some(window);
        self.ctx = Some(ctx);
        self.surface = Some(surface);
        self.next_switch = Instant::now() + DWELL;
        self.apply_current();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.paused {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let now = Instant::now();
        if now >= self.next_switch {
            self.idx = (self.idx + 1) % COMBOS.len();
            self.next_switch = now + DWELL;
            self.apply_current();
            if self.idx == 0 {
                println!("\n──────── 一轮结束，重新开始 ────────");
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_switch));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // 已知问题：启动时会收到前后值相同的假事件，必须比对去重
                if (scale_factor - self.last_scale).abs() > f64::EPSILON {
                    println!(">>> [DPI 真实变化] {} -> {}", self.last_scale, scale_factor);
                    self.last_scale = scale_factor;
                } else {
                    println!("(忽略：scale 未变的冗余事件 {scale_factor})");
                }
            }

            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, .. },
                ..
            } => match logical_key.as_ref() {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Named(NamedKey::Space) => {
                    self.paused = !self.paused;
                    println!("\n[{}]", if self.paused { "已暂停轮播" } else { "恢复轮播" });
                    if !self.paused {
                        self.next_switch = Instant::now() + DWELL;
                    }
                }
                Key::Character("n") => {
                    self.idx = (self.idx + 1) % COMBOS.len();
                    self.next_switch = Instant::now() + DWELL;
                    self.apply_current();
                }
                _ => {}
            },

            WindowEvent::RedrawRequested => {
                let (Some(surface), Some(window)) = (&mut self.surface, &self.window) else {
                    return;
                };
                let size = window.inner_size();
                let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
                    return;
                };
                surface.resize(w, h).unwrap();
                let mut buf = surface.buffer_mut().unwrap();
                draw(&mut buf, size.width, size.height, self.idx + 1);
                buf.present().unwrap();
            }
            _ => {}
        }
    }
}

/// 画面：红边框 + N 个大方块表示组合编号 + 下半部 1px 细线（DPI 判据）
fn draw(buf: &mut [u32], w: u32, h: u32, marker_count: usize) {
    const SQ: u32 = 44; // 方块边长
    const GAP: u32 = 14;
    const TOP: u32 = 40;
    const LEFT: u32 = 30;

    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let border = x < 4 || y < 4 || x >= w - 4 || y >= h - 4;

            // 组合编号方块
            let mut in_square = false;
            for k in 0..marker_count as u32 {
                let sx = LEFT + k * (SQ + GAP);
                if x >= sx && x < sx + SQ && y >= TOP && y < TOP + SQ {
                    in_square = true;
                    break;
                }
            }

            let hairline = (x % 2 == 0) && y > h - 60 && y < h - 10;

            buf[i] = if border {
                0x00FF3B30
            } else if in_square {
                0x00FFD60A // 亮黄方块，全屏背景下也醒目
            } else if hairline {
                0x00FFFFFF
            } else {
                0x00141414
            };
        }
    }
}

fn main() {
    println!(
        r#"
╔══════════════════════════════════════════════════════════════════╗
║  PinWall 原型 1b · 全屏覆盖参数矩阵排查                          ║
╠══════════════════════════════════════════════════════════════════╣
║  窗口每 4 秒自动切换一组参数，并用【黄色方块的个数】表示组合编号 ║
║                                                                  ║
║  用法：                                                          ║
║   1. 把任意 App 切到全屏 (Ctrl+Cmd+F)                            ║
║   2. 盯着屏幕，等红框窗口出现                                    ║
║   3. 数一下窗口里有几个黄方块 —— 那就是生效的组合编号           ║
║   4. 退出全屏，回终端看该编号对应的参数                          ║
║                                                                  ║
║  空格 暂停/恢复轮播    n 手动下一组    Esc 退出                  ║
╚══════════════════════════════════════════════════════════════════╝
"#
    );
    for (i, c) in COMBOS.iter().enumerate() {
        println!("  [{}] ■×{}  {}", i + 1, i + 1, c.label);
    }

    let event_loop = EventLoop::new().expect("创建事件循环失败");
    event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + DWELL));
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("事件循环异常");
}
