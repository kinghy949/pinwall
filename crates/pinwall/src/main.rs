//! PinWall —— 把截图钉在屏幕上。
//!
//! 当前实现的闭环：
//!   全局热键 → 每屏遮罩 → 框选（可跨屏）→ 分屏捕获并拼接 → **暂存**
//!   → 就地标注（工具栏 / 单字母键，文字走原生输入框）
//!   → Enter 烧进像素进剪贴板并收工，或 ⇧Enter 留成置顶浮窗
//!
//! 尚未接入：历史库、上传工作流。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::collections::VecDeque;
use std::rc::Rc;

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

use pinwall_capture::{
    capture_selection, current_capturer, encode_png, permission_status, CapturedImage, Capturer,
    Permission,
};
use pinwall_core::annotation::{AnnotationEditor, EditEvent, EditOutcome, Shape, Tool};
use pinwall_core::{Event, Outcome, Selection, SelectionMachine};
use pinwall_platform::geom::{Point, Rect};
use pinwall_platform::{
    ask_save_path, copy_image_to_clipboard, current_platform, flatten_annotations,
    read_clipboard_image, DrawCommand, KeyPress, OverlaySet, PinImage, PinWindow, Platform,
    PointerEvent, Rgba, ToolbarItem,
};

/// 撤下遮罩后等待窗口服务器完成一次合成的时长。
///
/// 这是个经验值：要盖住一帧的合成（60Hz 下约 17ms），又不能让用户觉出卡顿。
const OVERLAY_SETTLE: Duration = Duration::from_millis(60);

fn main() {
    let mtm = MainThreadMarker::new().expect("须在主线程运行");
    let app = NSApplication::sharedApplication(mtm);
    // 无 Dock 图标；正式打包时应由 Info.plist 的 LSUIElement 固化
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    if permission_status() == Permission::Denied {
        eprintln!("未获得「屏幕录制」权限。");
        eprintln!("请在 系统设置 → 隐私与安全性 → 屏幕录制 中授权，然后重新启动本程序。");
        std::process::exit(1);
    }

    let platform = match current_platform() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("初始化窗口层失败: {e}");
            std::process::exit(1);
        }
    };
    let capturer = match current_capturer() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("初始化捕获层失败: {e}");
            std::process::exit(1);
        }
    };

    // 全局热键只留下真正需要「从任何地方进入」的那几个。工具键、撤销、
    // 存盘、复制一律改由贴图窗口自己消费（见 [`Pin::pump_keys`]）。
    //
    // 全局热键有两笔代价：它会从**所有**应用手里独占那个键位；而撞上系统
    // 保留组合时（macOS 的 ⌘⇧3/4/5 归它自己的截图功能）`register()` 仍然
    // 返回成功，快捷键静悄悄失效，启动时毫无征兆。竞品（Snipaste、
    // CleanShot X）一律只把「捕获」放在全局，其余都是窗口内按键。
    let manager = GlobalHotKeyManager::new().expect("热键管理器创建失败");
    // F1 对齐 Snipaste 的默认截图键
    let capture_key = HotKey::new(None, Code::F1);
    let clear_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyX);
    // 鼠标穿透**必须**留在全局：穿透开启后窗口既收不到鼠标也收不到按键，
    // 没有这条外部通路就再也关不掉了
    let through_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyT);
    // F3 对齐 Snipaste：把剪贴板里的图贴成浮窗。来源不限于自家截图 ——
    // 从浏览器、聊天窗口复制来的图都能钉上去。
    let paste_key = HotKey::new(None, Code::F3);
    // 标注模式的保底通路。正常应按空格（窗口内），但万一贴图窗口拿不到
    // 键盘焦点，没有它就完全进不去标注模式 —— 而工具栏只在标注模式下才出现。
    let annotate_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyE);
    manager.register(capture_key).expect("注册 F1 失败");
    manager.register(paste_key).expect("注册 F3 失败");
    manager.register(clear_key).expect("注册 ⌘⇧X 失败");
    manager.register(through_key).expect("注册 ⌘⇧T 失败");
    manager.register(annotate_key).expect("注册 ⌘⇧E 失败");

    println!(
        r#"
PinWall  —— 把截图钉在屏幕上

全局快捷键（在任何地方都生效）
  F1     截图（拖拽框选 → 就地标注 → Enter 复制收工 / ⇧Enter 留在屏上）
  F3     把剪贴板里的图贴成浮窗
  ⌘⇧X   关闭所有贴图
  ⌘⇧T   切换所有贴图的鼠标穿透
  ⌘⇧E   进出标注模式（正常用空格，这是拿不到焦点时的保底通路）
  Ctrl-C 退出

贴图窗口内（刚截的图已自动取得焦点；旧贴图需先点一下）
  空格   显隐标注工具栏（进出标注模式）
  V 选择   R 矩形   O 椭圆   L 直线   A 箭头
  H 高亮   B 打码   N 序号   T 文字
  ⌘Z     撤销标注
  ⌘C     复制到剪贴板（含标注）；暂存期同时收工，见下
  ⌘S     存储为…（弹对话框选位置，含标注）
  ⌘⇧S    快速保存到桌面（不打断，含标注）
  Esc    标注模式下退出标注；否则关闭该贴图

刚框选完处于「暂存」状态，四周仍压暗：
  Enter   烧入标注 → 进剪贴板 → 收工，不留窗口（⌘C、空白处双击亦同）
  ⇧Enter  烧入标注 → 留成置顶浮窗
  Esc     放弃本次截图（在压暗区右键亦可）

  拖拽        移动
  滚轮        缩放（以光标为锚点）
  Shift+滚轮  调透明度（Option+滚轮亦可）
  中键        切换鼠标穿透
  双击        复制到剪贴板并关闭（含标注）
  右键        直接关闭，不复制

标注模式下贴图下方会浮出工具栏，可直接点击切换工具，不依赖键盘。
文字工具：点一下即就地弹出输入框（支持输入法），回车或点别处提交。
拖拽绘制，右键删除选中。截图不会自动进剪贴板，按 Enter 或 ⌘C 才复制。

已就绪。
"#
    );

    app.finishLaunching();

    let rx = GlobalHotKeyEvent::receiver();
    // 图像与窗口一同保存：存盘与复制都需要原始像素，
    // 而窗口本身不保留可读回的位图
    let mut pins: Vec<Pin> = Vec::new();
    // 复用缓冲区，避免每帧分配
    let mut actions: Vec<(usize, PinAction)> = Vec::new();
    let mut closing: Vec<usize> = Vec::new();

    loop {
        let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
        if let Some(e) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            app.sendEvent(&e);
        }

        // 回收用户自行关掉的贴图（双击 / 右键）。窗口关闭由用户在窗口上
        // 直接触发，上层无从感知，只能轮询回收，否则 Box 会一直堆着。
        //
        // 不能用 retain：那样只是 drop 掉 Pin，暂存期的遮罩不会被关闭，
        // 会留下一层盖住整个屏幕的暗色面板。
        let mut i = 0;
        while i < pins.len() {
            if pins[i].window.is_closed() {
                pins.remove(i).dispose();
            } else {
                i += 1;
            }
        }

        // 驱动处于标注模式的贴图
        for (i, pin) in pins.iter_mut().enumerate() {
            if let Some(id) = pin.pending_tool.take() {
                if let Some(t) = Pin::tool_from_id(id) {
                    pin.editor.set_tool(t);
                    pin.refresh();
                }
            }
            // 三个 pump 都要跑，不能短路 —— 指针那步可能刚弹出输入框，
            // 文字那步要把用户已经打进去的字取回来
            let (kdirty, acts) = pin.pump_keys();
            let dirty = pin.pump_annotation() | pin.pump_text() | kdirty;
            if dirty {
                pin.refresh();
            }
            for a in acts {
                actions.push((i, a));
            }
            // 双击 = 复制并关闭。顺序有意义：先复制，再关窗
            if pin.window.take_copy_request() {
                actions.push((i, PinAction::Copy));
                actions.push((i, PinAction::Close));
            }
            // 碰过遮罩就把贴图重新抬回最前并取回焦点
            if pin.staging && pin.staging_touch.replace(false) {
                pin.window.show();
                pin.window.focus();
            }
            // 遮罩上右键 = 放弃本次截图
            if pin.staging_cancelled() {
                actions.push((i, PinAction::Close));
            }
        }

        // 按键触发的外部动作单独处理：它们要动到贴图集合或文件系统，
        // 在遍历 pins 的过程中做不了
        for (i, a) in actions.drain(..) {
            match a {
                PinAction::Keep => {
                    pins[i].keep();
                    println!("已贴在屏幕上，当前共 {} 张（⌘⇧X 全部关闭）", pins.len());
                }
                PinAction::SaveAs => {
                    let r = pins[i].export_image().and_then(|img| {
                        match ask_save_path(&default_file_name()) {
                            Some(path) => {
                                std::fs::write(&path, encode_png(&img)?)?;
                                Ok(Some(path))
                            }
                            None => Ok(None),
                        }
                    });
                    match r {
                        Ok(Some(path)) => println!("已保存 {}", path.display()),
                        Ok(None) => println!("已取消保存"),
                        Err(e) => eprintln!("保存失败: {e}"),
                    }
                }
                PinAction::QuickSave => {
                    match pins[i].export_image().and_then(|img| save_to_desktop(&img)) {
                        Ok(path) => println!("已保存 {}", path.display()),
                        Err(e) => eprintln!("保存失败: {e}"),
                    }
                }
                PinAction::Copy => {
                    let r = pins[i].export_image().and_then(|img| {
                        copy_image_to_clipboard(&PinImage {
                            width: img.width,
                            height: img.height,
                            scale: img.scale,
                            bgra: &img.bgra,
                        })
                        .map_err(Into::into)
                    });
                    match r {
                        Ok(()) => println!("已复制到剪贴板（含标注）"),
                        Err(e) => eprintln!("复制失败: {e}"),
                    }
                }
                // 倒序删除，否则前面的删除会让后面的下标失效
                PinAction::Close => closing.push(i),
            }
        }
        if !closing.is_empty() {
            closing.sort_unstable();
            closing.dedup();
            for i in closing.drain(..).rev() {
                pins.remove(i).dispose();
            }
            println!("已收起，当前剩 {} 张贴图", pins.len());
        }

        while let Ok(ev) = rx.try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if ev.id == capture_key.id() {
                match capture_and_stage(&app, platform.as_ref(), capturer.as_ref()) {
                    Ok(Some(pin)) => {
                        pins.push(pin);
                        println!(
                            "已框选，可直接标注：Enter/⌘C 复制并收工 · ⇧Enter 贴在屏幕上 · Esc 放弃"
                        );
                    }
                    Ok(None) => println!("已取消"),
                    Err(e) => eprintln!("失败: {e}"),
                }
            } else if ev.id == paste_key.id() {
                match pin_from_clipboard(platform.as_ref()) {
                    Ok(Some(pin)) => {
                        pins.push(pin);
                        println!("已贴出剪贴板内容，当前共 {} 张（⌘⇧X 全部关闭）", pins.len());
                    }
                    Ok(None) => println!("剪贴板里没有图片"),
                    Err(e) => eprintln!("贴图失败: {e}"),
                }
            } else if ev.id == clear_key.id() {
                let n = pins.len();
                for p in pins.drain(..) {
                    p.dispose();
                }
                println!("已关闭 {n} 张贴图");
            } else if ev.id == annotate_key.id() {
                match pins.last_mut() {
                    Some(p) => {
                        let on = !p.window.is_annotation_mode();
                        p.set_annotation_mode(on);
                        println!(
                            "{}标注模式（工具栏可点击，或用 V/R/O/L/A/H/B/N/T；右键删除选中）",
                            if on { "已进入" } else { "已退出" }
                        );
                    }
                    None => println!("当前没有贴图"),
                }
            } else if ev.id == through_key.id() {
                // 以「是否存在未穿透的贴图」决定统一开还是统一关，
                // 避免逐张取反导致状态参差不齐
                let turn_on = pins.iter().any(|p| !p.window.is_click_through());
                for p in &pins {
                    p.window.set_click_through(turn_on);
                }
                println!(
                    "{} {} 张贴图的鼠标穿透",
                    if turn_on { "已开启" } else { "已关闭" },
                    pins.len()
                );
            }
        }
    }
}


/// 窗口内按键里，需要交回主循环处理的那部分。
///
/// 这几件事都要动到贴图集合本身或文件系统，[`Pin`] 自己做不了 ——
/// 它拿不到 `pins`，也不该拿到。
enum PinAction {
    /// 暂存期结束，落定为一张普通贴图（⇧Enter）。
    Keep,
    /// 弹出系统对话框选择保存位置（⌘S）。
    SaveAs,
    /// 直接存进快速目录，不打断用户（⌘⇧S）。
    QuickSave,
    Copy,
    Close,
}

/// 标注文字的字号（100% 缩放下的逻辑点）。
///
/// 输入框与最终绘制必须用同一个值，否则提交的瞬间字会跳一下大小。
const TEXT_FONT_SIZE: f64 = 18.0;

/// 一张贴图：窗口 + 其原始像素。
///
/// 必须同时保存像素 —— 窗口只负责显示，不提供读回位图的通路，
/// 而存盘与复制到剪贴板都需要原始数据。
struct Pin {
    window: Box<dyn PinWindow>,
    image: CapturedImage,
    editor: AnnotationEditor,
    /// 工具栏点击产生的待处理工具切换。
    ///
    /// 与指针事件同理：回调不能直接改编辑器（会捕获共享引用而成环），
    /// 只记录待处理项，由主循环消费。
    pending_tool: Rc<Cell<Option<u32>>>,
    /// 正在用原生输入框编辑的文字对象下标。
    editing_text: Option<usize>,
    /// 上一次取回的文字内容，用来避免每帧无谓重绘。
    editing_last: String,
    /// 暂存期：刚框选完、尚未决定去留。
    ///
    /// 此时 Enter（或 ⌘C）表示「复制并收工」，⇧Enter 表示「留成贴图」，Esc 放弃。
    /// 落定之后这三个键的含义都不一样了，故必须区分。
    staging: bool,
    /// 暂存期仍然铺着的遮罩。压暗周围是在提示「你还在一次截图流程里」，
    /// 少了它用户不知道此刻 Enter 有特殊含义。落定或放弃时一并撤掉。
    overlays: Option<OverlaySet>,
    /// 暂存期在遮罩上右键即放弃 —— 键盘焦点万一异常，这是唯一的逃生口。
    staging_cancel: Rc<Cell<bool>>,
    /// 暂存期用户碰过遮罩。据此把贴图重新置顶并取回焦点 ——
    /// 点一下遮罩会把它抬到贴图之上，贴图就沉到压暗层底下去了。
    staging_touch: Rc<Cell<bool>>,
    /// 窗口内按键的队列。
    ///
    /// 与指针事件同理：回调不能直接驱动编辑器，否则要捕获对它的共享引用，
    /// 既成环也有嵌套借用的风险。只入队，由主循环消费。
    keys: Rc<RefCell<VecDeque<KeyPress>>>,
    /// 标注模式下的指针事件队列。
    ///
    /// 与遮罩同理：回调若直接驱动编辑器，就要捕获对编辑器的共享引用，
    /// 既构成循环也带来嵌套借用风险。改为只入队、主循环消费。
    events: Rc<RefCell<VecDeque<PointerEvent>>>,
}

impl Pin {
    fn as_pin_image(&self) -> PinImage<'_> {
        PinImage {
            width: self.image.width,
            height: self.image.height,
            scale: self.image.scale,
            bgra: &self.image.bgra,
        }
    }

    /// 把标注模型翻译成窗口层认识的绘制指令。
    ///
    /// 这层翻译是必要的：标注模型在 pinwall-core，而它依赖
    /// pinwall-platform 的几何类型，窗口层无法反向依赖它。
    fn draw_commands(&self) -> Vec<DrawCommand> {
        self.commands(true, true)
    }

    /// 导出用的指令：不含选中框。
    ///
    /// 选中框是**编辑期的提示**，不是画面的一部分。烧进导出图里
    /// 就成了一个谁也解释不清的蓝框。
    fn export_commands(&self) -> Vec<DrawCommand> {
        // 正在编辑的文字**要**导出：用户看得见自己刚打的字，
        // 此刻按下 ⌘⇧S 却存出一张没有那行字的图，只会以为存错了
        self.commands(false, false)
    }

    /// `skip_editing` 只在**画到屏幕上**时为真：那一份由原生输入框自己显示，
    /// 再画一遍就成了两份对不齐的重影。导出时没有输入框，必须照常画。
    fn commands(&self, include_selection: bool, skip_editing: bool) -> Vec<DrawCommand> {
        let mut out = Vec::with_capacity(self.editor.objects().len() + 1);
        for (i, o) in self.editor.objects().iter().enumerate() {
            if skip_editing && self.editing_text == Some(i) {
                continue;
            }
            let color = Rgba::new(o.color.r, o.color.g, o.color.b, o.color.a);
            out.push(match &o.shape {
                Shape::Rect => DrawCommand::Rect { rect: o.bounds(), color, width: o.width },
                Shape::Ellipse => DrawCommand::Ellipse {
                    rect: o.bounds(),
                    color,
                    width: o.width,
                },
                Shape::Line => DrawCommand::Line {
                    from: o.a,
                    to: o.b,
                    color,
                    width: o.width,
                },
                Shape::Arrow => DrawCommand::Arrow {
                    from: o.a,
                    to: o.b,
                    color,
                    width: o.width,
                },
                Shape::Highlight => DrawCommand::Highlight { rect: o.bounds() },
                Shape::Redact => DrawCommand::Redact { rect: o.bounds() },
                Shape::Number(n) => DrawCommand::Number {
                    rect: o.bounds(),
                    value: *n,
                    color,
                },
                Shape::Text(t) => DrawCommand::Text {
                    origin: o.a,
                    text: t.clone(),
                    color,
                    size: TEXT_FONT_SIZE,
                },
            });
        }
        if include_selection {
            if let Some(i) = self.editor.selected() {
                // 编辑中的文字同理：输入框已经标明了「在编辑这里」，
                // 再套一个蓝框只是噪声
                if let Some(o) = self
                    .editor
                    .objects()
                    .get(i)
                    .filter(|_| !(skip_editing && self.editing_text == Some(i)))
                {
                    out.push(DrawCommand::SelectionBox { rect: o.bounds() });
                }
            }
        }
        out
    }

    /// 把标注烧进像素，得到可导出的图像。
    ///
    /// 屏幕上标注只是**叠加显示**，从未与底图合并。存盘和复制若直接用
    /// 原始像素，导出的就是没有标注的干净原图 —— 用户画了半天，
    /// 粘出去一看什么都没有。
    ///
    /// 无标注时 `flatten_annotations` 直接返回原数据，不多走一遍重绘。
    fn export_image(&self) -> Result<CapturedImage, Box<dyn std::error::Error>> {
        let cmds = self.export_commands();
        let bgra = flatten_annotations(&self.as_pin_image(), &cmds)?;
        Ok(CapturedImage {
            width: self.image.width,
            height: self.image.height,
            scale: self.image.scale,
            bgra,
        })
    }

    /// 工具栏按钮定义。id 与 [`Self::tool_from_id`] 对应。
    fn toolbar_items(&self) -> Vec<ToolbarItem> {
        // 顺序与工具栏按钮一致；id 亦即 [`Self::tool_from_id`] 的入参
        const TOOLS: [(u32, &str, Tool); 9] = [
            (0, "选择", Tool::Select),
            (1, "矩形", Tool::Rect),
            (2, "椭圆", Tool::Ellipse),
            (3, "直线", Tool::Line),
            (4, "箭头", Tool::Arrow),
            (5, "高亮", Tool::Highlight),
            (6, "打码", Tool::Redact),
            (7, "序号", Tool::Number),
            (8, "文字", Tool::Text),
        ];
        let current = self.editor.tool();
        TOOLS
            .iter()
            .map(|(id, label, tool)| ToolbarItem {
                id: *id,
                label: (*label).to_string(),
                selected: *tool == current,
            })
            .collect()
    }

    fn tool_from_id(id: u32) -> Option<Tool> {
        match id {
            0 => Some(Tool::Select),
            1 => Some(Tool::Rect),
            2 => Some(Tool::Ellipse),
            3 => Some(Tool::Line),
            4 => Some(Tool::Arrow),
            5 => Some(Tool::Highlight),
            6 => Some(Tool::Redact),
            7 => Some(Tool::Number),
            8 => Some(Tool::Text),
            _ => None,
        }
    }

    /// 刷新工具栏的选中态与画面。
    fn refresh(&self) {
        let cmds = self.draw_commands();
        self.window.set_draw_commands(&cmds);
        if self.window.is_annotation_mode() {
            self.window.set_toolbar(&self.toolbar_items());
        }
    }

    /// 消费队列中的指针事件，返回是否需要重绘。
    fn pump_annotation(&mut self) -> bool {
        let mut dirty = false;
        loop {
            let next = self.events.borrow_mut().pop_front();
            let Some(ev) = next else { break };
            let outcome = match ev {
                PointerEvent::Down(p) => {
                    let o = self.editor.handle(EditEvent::Down(p));
                    // Select 工具在空白处按下 —— 什么标注也没抓着，那这一拖
                    // 的意图就是挪贴图本身。窗口层判断不了这件事：它既不知道
                    // 有哪些标注，也不知道当前是什么工具，故由此处回填。
                    let move_window = self.editor.tool() == Tool::Select
                        && !self.editor.has_active_drag();
                    self.window.set_window_drag(move_window);
                    o
                }
                PointerEvent::Moved(p) => self.editor.handle(EditEvent::Move(p)),
                PointerEvent::Up(p) => self.editor.handle(EditEvent::Up(p)),
                // 标注模式下右键用于删除选中对象
                PointerEvent::Cancel => {
                    if self.editor.delete_selected() {
                        EditOutcome::Redraw
                    } else {
                        EditOutcome::Idle
                    }
                }
            };
            match outcome {
                EditOutcome::Redraw => dirty = true,
                EditOutcome::BeginTextInput(i) => {
                    self.begin_text(i);
                    dirty = true;
                }
                EditOutcome::Idle => {}
            }
        }
        dirty
    }

    /// 就地弹出原生输入框，编辑第 `i` 个文字对象。
    ///
    /// 输入交给平台控件是为了白拿输入法：预编辑、候选词、双拼、
    /// emoji 面板全都自动可用，自己实现一遍毫无胜算。
    fn begin_text(&mut self, i: usize) {
        let Some(o) = self.editor.objects().get(i) else { return };
        let Shape::Text(text) = o.shape.clone() else { return };
        let (rect, c) = (o.bounds(), o.color);
        self.window.begin_text_input(
            rect,
            &text,
            TEXT_FONT_SIZE,
            Rgba::new(c.r, c.g, c.b, c.a),
        );
        self.editing_text = Some(i);
        self.editing_last = text;
    }

    /// 暂存期结束，落定为一张普通贴图。
    fn keep(&mut self) {
        self.close_overlays();
        self.staging = false;
        // 退出标注模式，贴图恢复为可拖动；想接着标注按空格即可
        self.set_annotation_mode(false);
    }

    /// 用户是否在遮罩上右键放弃了本次截图。
    fn staging_cancelled(&self) -> bool {
        self.staging && self.staging_cancel.get()
    }

    fn close_overlays(&mut self) {
        if let Some(o) = self.overlays.take() {
            // 必须显式关闭：遮罩每次截图都会重建，只 drop 会留下悬着的面板
            o.close();
        }
    }

    /// 关闭并释放。遮罩要先撤，否则会留下一层盖住整个屏幕的暗色面板。
    fn dispose(mut self) {
        self.close_overlays();
        self.window.close();
    }

    /// 进出标注模式，并同步工具栏的显隐。
    ///
    /// 两件事必须一起做：工具栏只在标注模式下才有意义，而退出时若不撤掉，
    /// 它会孤零零地挂在贴图下面。
    fn set_annotation_mode(&mut self, on: bool) {
        if !on {
            self.end_text();
        }
        self.window.set_annotation_mode(on);
        if on {
            self.window.set_toolbar(&self.toolbar_items());
        } else {
            self.window.set_toolbar(&[]);
        }
    }

    /// 消费窗口内按键，返回（是否需要重绘，交回主循环的动作）。
    fn pump_keys(&mut self) -> (bool, Vec<PinAction>) {
        let mut dirty = false;
        let mut actions = Vec::new();
        loop {
            let next = self.keys.borrow_mut().pop_front();
            let Some(k) = next else { break };
            match k {
                // 空格显隐标注工具栏 —— 对齐 Snipaste
                KeyPress::Plain(' ') => {
                    let on = !self.window.is_annotation_mode();
                    self.set_annotation_mode(on);
                    dirty = true;
                }
                KeyPress::Plain(c) => {
                    // 单字母工具键对齐 CleanShot X：T 文字 / A 箭头 / R 矩形 / B 打码
                    let Some(tool) = Self::tool_from_key(c) else { continue };
                    // 直接按工具键即进入标注模式，省掉「先按空格」这一步
                    if !self.window.is_annotation_mode() {
                        self.set_annotation_mode(true);
                    }
                    self.editor.set_tool(tool);
                    dirty = true;
                }
                KeyPress::Command('z') => {
                    // 先收掉输入框：撤销会换掉整份文档快照，下标随之失效，
                    // 而输入框还对着旧下标，接着写就会改到别的对象头上
                    self.end_text();
                    if self.editor.undo() {
                        dirty = true;
                    }
                }
                // 暂存期的 ⌘C 与 Enter 同义：复制走人就是这次截图的终点，
                // 再要用户补按一次 Esc 收窗是多余的一步。落定之后的贴图则
                // 只复制不关 —— 那是用户特意留在屏幕上的，关掉才是意外。
                KeyPress::Command('c') => {
                    // 顺序有意义：先复制，再关窗
                    actions.push(PinAction::Copy);
                    if self.staging {
                        actions.push(PinAction::Close);
                    }
                }
                // ⌘S 存储为、⌘⇧S 快速保存 —— 对齐 Snipaste 的既有分工。
                // 无提示地丢进固定目录，从用户视角与「快捷键坏了」无法区分。
                KeyPress::Command('s') => actions.push(PinAction::SaveAs),
                KeyPress::CommandShift('s') => actions.push(PinAction::QuickSave),
                KeyPress::Command(_) | KeyPress::CommandShift(_) => {}
                // 暂存期的收尾：Enter 复制并收工，⇧Enter 留在屏幕上
                KeyPress::Enter { shift } if self.staging => {
                    if shift {
                        actions.push(PinAction::Keep);
                    } else {
                        // 顺序有意义：先复制，再关窗
                        actions.push(PinAction::Copy);
                        actions.push(PinAction::Close);
                    }
                }
                KeyPress::Enter { .. } => {}
                KeyPress::Escape => {
                    if self.staging {
                        // 暂存期的 Esc 是放弃整次截图，不是退出标注
                        actions.push(PinAction::Close);
                    } else if self.window.is_annotation_mode() {
                        // 落定之后对齐 Snipaste：先收标注，再按才关窗
                        self.set_annotation_mode(false);
                        dirty = true;
                    } else {
                        actions.push(PinAction::Close);
                    }
                }
            }
        }
        (dirty, actions)
    }

    /// 单字母到工具的映射。字母取自 CleanShot X 的既有约定，
    /// 用户从别的工具迁过来不必重新记。
    fn tool_from_key(c: char) -> Option<Tool> {
        match c {
            'v' => Some(Tool::Select),
            'r' => Some(Tool::Rect),
            'o' => Some(Tool::Ellipse),
            'l' => Some(Tool::Line),
            'a' => Some(Tool::Arrow),
            'h' => Some(Tool::Highlight),
            'b' => Some(Tool::Redact),
            'n' => Some(Tool::Number),
            't' => Some(Tool::Text),
            _ => None,
        }
    }

    /// 立即收掉进行中的文字输入：取回内容、撤下输入框、走一遍结束流程。
    ///
    /// 供撤销一类会打乱下标的操作在动手之前调用。无输入进行中时什么都不做。
    fn end_text(&mut self) {
        let Some(i) = self.editing_text.take() else { return };
        if let Some(input) = self.window.poll_text_input() {
            self.editor.set_text(i, input.text, Some(input.extent));
        }
        self.window.end_text_input();
        self.editor.finish_text(i);
        self.editing_last.clear();
    }

    /// 把原生输入框里的内容同步进标注模型，返回是否需要重绘。
    ///
    /// 尺寸也一并取回：核心层没有字体度量，文字的包围盒只能由这里喂进去，
    /// 否则用户刚打完的字既选不中也拖不动。
    fn pump_text(&mut self) -> bool {
        let Some(i) = self.editing_text else { return false };
        let Some(input) = self.window.poll_text_input() else {
            // 输入框已被窗口层收走（例如退出了标注模式），仍须走一遍结束流程 ——
            // 否则空文字对象会留在文档里，看不见却选得中
            self.editor.finish_text(i);
            self.editing_text = None;
            self.editing_last.clear();
            return true;
        };
        if !input.finished && input.text == self.editing_last {
            return false;
        }
        self.editing_last = input.text.clone();
        self.editor.set_text(i, input.text, Some(input.extent));
        if input.finished {
            self.window.end_text_input();
            self.editor.finish_text(i);
            self.editing_text = None;
            self.editing_last.clear();
        }
        true
    }
}

/// 给一个已建好的贴图窗口装上其余回调，组装成 [`Pin`]。
///
/// 指针队列由调用方传入 —— 它必须在**建窗时**就装好，故无法在此处补装。
fn make_pin(
    window: Box<dyn PinWindow>,
    image: CapturedImage,
    events: Rc<RefCell<VecDeque<PointerEvent>>>,
) -> Pin {
    let keys: Rc<RefCell<VecDeque<KeyPress>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = keys.clone();
        window.set_key_handler(Rc::new(move |k: KeyPress| {
            q.borrow_mut().push_back(k);
        }));
    }
    let pending_tool: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    {
        let slot = pending_tool.clone();
        window.set_toolbar_handler(Rc::new(move |id: u32| slot.set(Some(id))));
    }
    Pin {
        window,
        image,
        editor: AnnotationEditor::new(),
        events,
        pending_tool,
        editing_text: None,
        editing_last: String::new(),
        keys,
        staging: false,
        overlays: None,
        staging_cancel: Rc::new(Cell::new(false)),
        staging_touch: Rc::new(Cell::new(false)),
    }
}

/// 把剪贴板里的图贴成浮窗。剪贴板里没有图像时返回 `Ok(None)`。
fn pin_from_clipboard(platform: &dyn Platform) -> Result<Option<Pin>, Box<dyn std::error::Error>> {
    let Some(img) = read_clipboard_image() else {
        return Ok(None);
    };
    // 落在鼠标处：用户刚复制完，视线就在那儿
    let at = platform
        .cursor_position()
        .unwrap_or_else(|| Point::new(100.0, 100.0));

    // 剪贴板**不携带倍率**。实测（examples/clipboard_roundtrip）写入倍率 2 的
    // 图，读回恒为 1.0 —— 直接采信就会在 Retina 屏上正好贴大一倍。
    //
    // 改按目标屏的物理像素密度显示。这恰好让 PinWall 自己的截图与 macOS 系统
    // 截图都 1:1 还原，因为两者本来就是按屏幕倍率抓的；外部图片则以最清晰的
    // 方式呈现（一个位图像素对一个屏幕像素）。
    let screens = platform.screens()?;
    let screen_scale = screens
        .iter()
        .find(|s| s.frame.contains(at))
        .or_else(|| screens.first())
        .map(|s| s.scale)
        .unwrap_or(1.0);
    // 万一哪天剪贴板真给出了倍率，就采信它
    let scale = if img.scale > 1.0 { img.scale } else { screen_scale };

    let logical_w = img.width as f64 / scale;
    let logical_h = img.height as f64 / scale;

    let window = platform.create_pin(Rect::from_xywh(at.x, at.y, logical_w, logical_h))?;
    window.set_image(&PinImage {
        width: img.width,
        height: img.height,
        scale,
        bgra: &img.bgra,
    })?;
    window.show();

    let events: Rc<RefCell<VecDeque<PointerEvent>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = events.clone();
        window.set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            q.borrow_mut().push_back(ev);
        }));
    }
    Ok(Some(make_pin(
        window,
        CapturedImage {
            width: img.width,
            height: img.height,
            scale,
            bgra: img.bgra,
        },
        events,
    )))
}

/// 保存对话框里预填的文件名。
fn default_file_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("PinWall-{stamp}.png")
}

/// 把图像存到桌面，文件名带时间戳。
///
/// 桌面是 macOS 截图的惯例位置；取不到主目录时退回当前工作目录。
fn save_to_desktop(img: &CapturedImage) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Desktop"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("."));

    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut path = dir.join(format!("PinWall-{stamp}.png"));
    // 同一秒内连续保存不应互相覆盖
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("PinWall-{stamp}-{n}.png"));
        n += 1;
    }

    std::fs::write(&path, encode_png(img)?)?;
    Ok(path)
}

/// 把遮罩撤下屏，并等到窗口服务器真的把它从画面里去掉为止。
///
/// `orderOut:` 只是提交请求，合成是异步的。快门若抢在合成之前按下，拍到的
/// 依然是遮罩 —— 这正是「已经 hide 了却仍有红边」的成因，且因为赶上的是
/// 淡出动画的中途，红色还变成了半透明的粉（见 examples/edge_probe 的实测：
/// 纯红 rgba(255,64,54) 变成了 rgba(212,148,143)）。
///
/// 动画本身已在 make_panel 里关掉，此处再泵一小段事件，把余下的一次合成
/// 也等掉。代价是每次截图多出这点延迟，换取图边干净。
fn hide_and_settle(app: &NSApplication, overlays: &OverlaySet) {
    overlays.hide();
    let deadline = Instant::now() + OVERLAY_SETTLE;
    while Instant::now() < deadline {
        let until = NSDate::dateWithTimeIntervalSinceNow(0.005);
        if let Some(e) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            app.sendEvent(&e);
        }
    }
}

/// 走一遍捕获流程：铺遮罩 → 等框选 → 捕获 → 交出一张**暂存中**的贴图。
///
/// 返回 `Ok(None)` 表示用户取消。
///
/// # 为什么标注发生在贴图上，而不是遮罩上
///
/// 竞品（Snipaste）是在框选完的遮罩上直接标注的。此处换了个做法：框选一
/// 结束就立刻捕获，并把贴图**严丝合缝地摆在选区原位**，标注照旧发生在贴图
/// 上。用户看到的完全一样 —— 那块画面本就静止，贴的是它自己的截图。
///
/// 这样做省下了在遮罩层重写一整套标注渲染与交互（贴图那边已经有了），
/// 代价是选区框定后不能再拖边调整。
///
/// 遮罩**不撤**，继续压暗四周：那圈暗色是在提示「你还在一次截图流程里」，
/// 少了它，用户不会知道此刻 Enter 有特殊含义。
///
/// # 为什么用事件队列而不是在回调里直接驱动状态机
///
/// 遮罩持有回调，若回调再捕获 `Rc<OverlaySet>`，就构成
/// `OverlaySet → Overlay → view → 闭包 → OverlaySet` 的循环引用，
/// 引用计数永远降不到零，遮罩无法释放。
///
/// 改为「回调只入队、主循环消费」后：所有权是单向的
/// （overlays 持有闭包，闭包持有队列），无环；状态机成为主循环的
/// 局部变量，也不再需要 `RefCell`，顺带消除了嵌套借用 panic 的可能。
fn capture_and_stage(
    app: &NSApplication,
    platform: &dyn Platform,
    capturer: &dyn Capturer,
) -> Result<Option<Pin>, Box<dyn std::error::Error>> {
    // 每次都重新枚举 —— 显示器可能在两次截图之间发生热插拔
    let screens = platform.screens()?;
    let overlays = OverlaySet::covering_all_screens(platform)?;

    let queue: Rc<RefCell<VecDeque<PointerEvent>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = queue.clone();
        overlays.set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            q.borrow_mut().push_back(ev);
        }));
    }

    overlays.show();

    let mut machine = SelectionMachine::new(screens);
    let mut result: Option<Selection> = None;

    'session: loop {
        let until = NSDate::dateWithTimeIntervalSinceNow(0.01);
        if let Some(e) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            app.sendEvent(&e);
        }

        loop {
            // 先取出再处理，不要在持有借用时调用状态机
            let next = queue.borrow_mut().pop_front();
            let Some(ev) = next else { break };
            let e = match ev {
                PointerEvent::Down(p) => Event::Down(p),
                PointerEvent::Moved(p) => Event::Move(p),
                PointerEvent::Up(p) => Event::Up(p),
                PointerEvent::Cancel => Event::Cancel,
            };
            match machine.handle(e) {
                // 选区可能跨屏，必须广播给全部遮罩，各自求交后绘制
                Outcome::Redraw => overlays.set_selection(machine.current_rect()),
                Outcome::Committed(sel) => {
                    result = Some(sel);
                    break 'session;
                }
                Outcome::Cancelled => break 'session,
                Outcome::Idle => {}
            }
        }
    }

    let Some(sel) = result else {
        overlays.hide();
        // 必须显式关闭：遮罩每次截图都会重建，只隐藏会持续累积
        overlays.close();
        return Ok(None);
    };

    // 框选阶段的事件队列到此为止。换成只认「右键放弃」的回调 ——
    // 既避免事件在无人消费的队列里越积越多，也给键盘失灵时留一个逃生口。
    let staging_cancel: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let staging_touch: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    {
        let cancel = staging_cancel.clone();
        let touch = staging_touch.clone();
        overlays.set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            // 任何一次触碰都要记：点中遮罩会把它抬到贴图之上，
            // 贴图随之沉到压暗层底下 —— 看上去就是整屏变灰、按键全失灵。
            touch.set(true);
            if matches!(ev, PointerEvent::Cancel) {
                cancel.set(true);
            }
        }));
    }
    // 快门期间遮罩**必须真的离屏**。SCScreenshotManager 拍的是屏幕的合成
    // 结果，遮罩还铺着就会被一并拍进去。
    //
    // 光靠镂空躲不掉：捕获区域与选区并不严丝合缝 —— 实测（examples/edge_probe）
    // 选区描边有整整 2 物理像素落在图里，左、右、下三边都有而上边没有，可见
    // 捕获区是按像素边界外扩过的，并非简单地半条线骑在边界上。既然对不齐，
    // 就没有任何描边位置是安全的，唯一可靠的做法是让遮罩在快门时不在场。
    hide_and_settle(app, &overlays);
    let img = capture_selection(capturer, &sel)?;

    // 贴在原位置：视觉上就像把那块画面「冻结」在了原地
    let pin = platform.create_pin(Rect::from_xywh(
        sel.rect.origin.x,
        sel.rect.origin.y,
        sel.rect.size.width,
        sel.rect.size.height,
    ))?;
    pin.set_image(&PinImage {
        width: img.width,
        height: img.height,
        scale: img.scale,
        bgra: &img.bgra,
    })?;
    // 图已到手，遮罩回到屏上继续压暗四周，标示仍在暂存期。
    //
    // 但**镂空要撤掉**：贴图是不透明的，正好盖在选区上，镂空本就看不见；
    // 而暂存期的贴图可以拖动，一旦挪开，那个洞就会露出底下的实时画面，
    // 看着像个 bug。改为整屏均匀压暗，贴图浮在上面。
    overlays.set_selection(None);
    overlays.show();
    // 后于遮罩置顶，从而盖在镂空之上（两者同为 1000 层，靠顺序定胜负）
    pin.show();
    // 主动取焦点，不等用户点。刚框完选，手已经离开鼠标，此时最该能直接按
    // ⌘C / Enter / Esc —— 还要求先点一下贴图，等于把最顺的那一步堵上了。
    pin.focus();
    // 标注事件队列在建窗时就装好，进入标注模式时无需再改回调
    let events: Rc<RefCell<VecDeque<PointerEvent>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = events.clone();
        pin.set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            q.borrow_mut().push_back(ev);
        }));
    }

    let mut staged = make_pin(pin, img, events);
    staged.staging = true;
    staged.overlays = Some(overlays);
    staged.staging_cancel = staging_cancel;
    staged.staging_touch = staging_touch;
    // 框选完直接就能画，不必再按一次空格
    staged.set_annotation_mode(true);
    Ok(Some(staged))
}
