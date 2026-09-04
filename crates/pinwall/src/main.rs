//! PinWall —— 把截图钉在屏幕上。
//!
//! 当前实现的闭环：
//!   全局热键 → 每屏遮罩 → 框选（可跨屏）→ 分屏捕获并拼接 → 贴为置顶浮窗
//!   → 标注（工具栏 / 快捷键，文字走原生输入框）→ 烧进像素后存盘或复制
//!
//! 尚未接入：历史库、上传工作流。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
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
use pinwall_platform::geom::Rect;
use pinwall_platform::{
    ask_save_path, copy_image_to_clipboard, current_platform, flatten_annotations, DrawCommand,
    KeyPress, OverlaySet, PinImage, PinWindow, Platform, PointerEvent, Rgba, ToolbarItem,
};

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
    // 标注模式的保底通路。正常应按空格（窗口内），但万一贴图窗口拿不到
    // 键盘焦点，没有它就完全进不去标注模式 —— 而工具栏只在标注模式下才出现。
    let annotate_key = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyE);
    manager.register(capture_key).expect("注册 F1 失败");
    manager.register(clear_key).expect("注册 ⌘⇧X 失败");
    manager.register(through_key).expect("注册 ⌘⇧T 失败");
    manager.register(annotate_key).expect("注册 ⌘⇧E 失败");

    println!(
        r#"
PinWall  —— 把截图钉在屏幕上

全局快捷键（在任何地方都生效）
  F1     截图并贴到屏幕上（拖拽框选，右键取消）
  ⌘⇧X   关闭所有贴图
  ⌘⇧T   切换所有贴图的鼠标穿透
  ⌘⇧E   进出标注模式（正常用空格，这是拿不到焦点时的保底通路）
  Ctrl-C 退出

贴图窗口内（先点一下贴图使其取得焦点）
  空格   显隐标注工具栏（进出标注模式）
  V      选择      R  矩形      A  箭头
  B      打码      T  文字
  ⌘Z     撤销标注
  ⌘C     复制到剪贴板（含标注）
  ⌘S     存储为…（弹对话框选位置，含标注）
  ⌘⇧S    快速保存到桌面（不打断，含标注）
  Esc    标注模式下退出标注；否则关闭该贴图

  拖拽        移动
  滚轮        缩放（以光标为锚点）
  Shift+滚轮  调透明度（Option+滚轮亦可）
  中键        切换鼠标穿透
  双击 / 右键 关闭

标注模式下贴图下方会浮出工具栏，可直接点击切换工具，不依赖键盘。
文字工具：点一下即就地弹出输入框（支持输入法），回车或点别处提交。
拖拽绘制，右键删除选中。截图完成后会自动复制到剪贴板。

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
        pins.retain(|p: &Pin| !p.window.is_closed());

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
        }

        // 按键触发的外部动作单独处理：它们要动到贴图集合或文件系统，
        // 在遍历 pins 的过程中做不了
        for (i, a) in actions.drain(..) {
            match a {
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
                pins.remove(i).window.close();
            }
            println!("已关闭贴图，当前剩 {} 张", pins.len());
        }

        while let Ok(ev) = rx.try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if ev.id == capture_key.id() {
                match capture_and_pin(&app, platform.as_ref(), capturer.as_ref()) {
                    Ok(Some(pin)) => {
                        // 截图即入剪贴板：多数使用场景下一步就是粘贴，
                        // 让用户不必再多按一次
                        match copy_image_to_clipboard(&pin.as_pin_image()) {
                            Ok(()) => println!(
                                "已贴图并复制到剪贴板，当前共 {} 张（⌘⇧X 全部关闭）",
                                pins.len() + 1
                            ),
                            Err(e) => println!("已贴图（复制到剪贴板失败: {e}）"),
                        }
                        pins.push(pin);
                    }
                    Ok(None) => println!("已取消"),
                    Err(e) => eprintln!("失败: {e}"),
                }
            } else if ev.id == clear_key.id() {
                let n = pins.len();
                for p in pins.drain(..) {
                    p.window.close();
                }
                println!("已关闭 {n} 张贴图");
            } else if ev.id == annotate_key.id() {
                match pins.last_mut() {
                    Some(p) => {
                        let on = !p.window.is_annotation_mode();
                        p.set_annotation_mode(on);
                        println!(
                            "{}标注模式（工具栏可点击，或用 V/R/A/B/T；右键删除选中）",
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
                Shape::Arrow => DrawCommand::Arrow {
                    from: o.a,
                    to: o.b,
                    color,
                    width: o.width,
                },
                Shape::Redact => DrawCommand::Redact { rect: o.bounds() },
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
        const TOOLS: [(u32, &str, Tool); 5] = [
            (0, "选择", Tool::Select),
            (1, "矩形", Tool::Rect),
            (2, "箭头", Tool::Arrow),
            (3, "打码", Tool::Redact),
            (4, "文字", Tool::Text),
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
            2 => Some(Tool::Arrow),
            3 => Some(Tool::Redact),
            4 => Some(Tool::Text),
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
                PointerEvent::Down(p) => self.editor.handle(EditEvent::Down(p)),
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
                KeyPress::Command('c') => actions.push(PinAction::Copy),
                // ⌘S 存储为、⌘⇧S 快速保存 —— 对齐 Snipaste 的既有分工。
                // 无提示地丢进固定目录，从用户视角与「快捷键坏了」无法区分。
                KeyPress::Command('s') => actions.push(PinAction::SaveAs),
                KeyPress::CommandShift('s') => actions.push(PinAction::QuickSave),
                KeyPress::Command(_) | KeyPress::CommandShift(_) => {}
                // 对齐 Snipaste：Esc 先收标注，再按才关窗
                KeyPress::Escape => {
                    if self.window.is_annotation_mode() {
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
            'a' => Some(Tool::Arrow),
            'b' => Some(Tool::Redact),
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

/// 走一遍完整流程：铺遮罩 → 等框选 → 捕获 → 贴图。
///
/// 返回 `Ok(None)` 表示用户取消。
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
fn capture_and_pin(
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

    overlays.hide();
    // 必须显式关闭：遮罩每次截图都会重建，只隐藏会持续累积
    overlays.close();

    let Some(sel) = result else {
        return Ok(None);
    };

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
    pin.show();
    // 标注事件队列在建窗时就装好，进入标注模式时无需再改回调
    let events: Rc<RefCell<VecDeque<PointerEvent>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = events.clone();
        pin.set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            q.borrow_mut().push_back(ev);
        }));
    }

    let keys: Rc<RefCell<VecDeque<KeyPress>>> = Rc::new(RefCell::new(VecDeque::new()));
    {
        let q = keys.clone();
        pin.set_key_handler(Rc::new(move |k: KeyPress| {
            q.borrow_mut().push_back(k);
        }));
    }

    let pending_tool: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    {
        let slot = pending_tool.clone();
        pin.set_toolbar_handler(Rc::new(move |id: u32| slot.set(Some(id))));
    }

    Ok(Some(Pin {
        window: pin,
        image: img,
        editor: AnnotationEditor::new(),
        events,
        pending_tool,
        editing_text: None,
        editing_last: String::new(),
        keys,
    }))
}
