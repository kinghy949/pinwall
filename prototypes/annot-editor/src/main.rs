//! PinWall 原型 2：egui 矢量标注编辑器
//!
//! 验证 R3 的两个疑虑：
//!   A. egui 是 immediate mode，做「可选中 / 可拖拽 / 可改属性 / 可撤销」的
//!      **持久化矢量对象** 是否可行，以及实际工作量有多大。
//!   B. 中文 IME 输入在 egui 的文本框里是否可用（标注打字是刚需）。
//!
//! 一次性验证代码，不是产品代码。

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

// ------------------------------------------------------------------ 数据模型

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Select,
    Rect,
    Arrow,
    Text,
}

#[derive(Clone)]
enum Shape {
    Rect,
    Arrow,
    Text(String),
}

#[derive(Clone)]
struct Obj {
    shape: Shape,
    /// 用两个角点表达；箭头则是起点/终点
    a: Pos2,
    b: Pos2,
    color: Color32,
    width: f32,
}

impl Obj {
    fn bounds(&self) -> Rect {
        Rect::from_two_pos(self.a, self.b)
    }

    /// 命中测试。immediate mode 不替你做这件事，必须自己写 —— 这正是
    /// 「工作量被低估」的部分之一。
    fn hit(&self, p: Pos2) -> bool {
        match self.shape {
            Shape::Arrow => {
                // 点到线段的距离
                let (a, b) = (self.a, self.b);
                let ab = b - a;
                let len2 = ab.length_sq();
                let t = if len2 <= f32::EPSILON {
                    0.0
                } else {
                    (((p - a).dot(ab)) / len2).clamp(0.0, 1.0)
                };
                let proj = a + ab * t;
                (p - proj).length() <= self.width.max(6.0)
            }
            _ => self.bounds().expand(4.0).contains(p),
        }
    }
}

#[derive(Clone, Default)]
struct Doc {
    objs: Vec<Obj>,
    sel: Option<usize>,
}

/// 快照式撤销栈。对原型足够，产品化时应换成命令式 diff，
/// 否则大图标注会有明显内存开销。
struct History {
    stack: Vec<Doc>,
    idx: usize,
}

impl History {
    fn new(d: &Doc) -> Self {
        Self {
            stack: vec![d.clone()],
            idx: 0,
        }
    }
    fn push(&mut self, d: &Doc) {
        self.stack.truncate(self.idx + 1);
        self.stack.push(d.clone());
        self.idx = self.stack.len() - 1;
    }
    fn undo(&mut self, d: &mut Doc) {
        if self.idx > 0 {
            self.idx -= 1;
            *d = self.stack[self.idx].clone();
        }
    }
    fn redo(&mut self, d: &mut Doc) {
        if self.idx + 1 < self.stack.len() {
            self.idx += 1;
            *d = self.stack[self.idx].clone();
        }
    }
}

// ------------------------------------------------------------------ 拖拽状态

enum Drag {
    None,
    Creating,
    Moving { grab: Vec2 },
    /// 缩放中，握住的是哪个角（0=a, 1=b）
    Resizing { corner: u8 },
}

struct App {
    doc: Doc,
    hist: History,
    tool: Tool,
    drag: Drag,
    color: Color32,
    width: f32,
    /// 正在编辑文本的对象索引
    editing: Option<usize>,
    frame_ms: f32,
}

impl Default for App {
    fn default() -> Self {
        let doc = Doc::default();
        Self {
            hist: History::new(&doc),
            doc,
            tool: Tool::Select,
            drag: Drag::None,
            color: Color32::from_rgb(255, 59, 48),
            width: 3.0,
            editing: None,
            frame_ms: 0.0,
        }
    }
}

const HANDLE: f32 = 7.0;

impl eframe::App for App {
    // egui 0.36 起 eframe::App 的入口从 update(&Context) 改为 ui(&mut Ui)，
    // TopBottomPanel 亦被移除 —— 这类 API 变动正是 R9「生态仍在演进」的实例。
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        let t0 = std::time::Instant::now();
        let ctx = ui.ctx().clone();

        // ---------------- 工具栏 ----------------
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tool, Tool::Select, "选择");
            ui.selectable_value(&mut self.tool, Tool::Rect, "矩形");
            ui.selectable_value(&mut self.tool, Tool::Arrow, "箭头");
            ui.selectable_value(&mut self.tool, Tool::Text, "文字");
            ui.separator();
            ui.color_edit_button_srgba(&mut self.color);
            ui.add(egui::Slider::new(&mut self.width, 1.0..=12.0).text("粗细"));
            ui.separator();
            if ui.button("撤销 ⌘Z").clicked() {
                self.hist.undo(&mut self.doc);
                self.editing = None;
            }
            if ui.button("重做 ⌘⇧Z").clicked() {
                self.hist.redo(&mut self.doc);
                self.editing = None;
            }
            if ui.button("删除选中").clicked() {
                if let Some(i) = self.doc.sel.take() {
                    if i < self.doc.objs.len() {
                        self.doc.objs.remove(i);
                    }
                    self.editing = None;
                    self.hist.push(&self.doc);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "对象 {}  |  历史 {}/{}  |  帧 {:.2}ms",
                self.doc.objs.len(),
                self.hist.idx + 1,
                self.hist.stack.len(),
                self.frame_ms
            ));
        });
        ui.label(
            egui::RichText::new(
                "中文输入测试：选「文字」工具在画布点一下，切到中文输入法打字。\
                 观察候选词窗位置、拼音上屏、退格删词是否正常。选择工具下双击文本可再次编辑。",
            )
            .small()
            .color(Color32::from_gray(150)),
        );
        ui.separator();

        // ---------------- 快捷键 ----------------
        let editing_now = self.editing.is_some();
        let (do_undo, do_redo, do_del) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && i.key_pressed(egui::Key::Z) && !i.modifiers.shift,
                cmd && i.key_pressed(egui::Key::Z) && i.modifiers.shift,
                !editing_now
                    && (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)),
            )
        });
        if do_undo {
            self.hist.undo(&mut self.doc);
            self.editing = None;
        }
        if do_redo {
            self.hist.redo(&mut self.doc);
            self.editing = None;
        }
        if do_del {
            if let Some(i) = self.doc.sel.take() {
                if i < self.doc.objs.len() {
                    self.doc.objs.remove(i);
                    self.hist.push(&self.doc);
                }
            }
        }

        // ---------------- 画布 ----------------
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        painter.rect_filled(resp.rect, 0.0, Color32::from_gray(28));
        let pointer = resp.interact_pointer_pos();

        // 起手
        if resp.drag_started() {
            if let Some(p) = pointer {
                match self.tool {
                    Tool::Select => {
                        self.drag = Drag::None;
                        if let Some(i) = self.doc.sel {
                            if let Some(o) = self.doc.objs.get(i) {
                                if (p - o.a).length() <= HANDLE * 1.6 {
                                    self.drag = Drag::Resizing { corner: 0 };
                                } else if (p - o.b).length() <= HANDLE * 1.6 {
                                    self.drag = Drag::Resizing { corner: 1 };
                                }
                            }
                        }
                        if matches!(self.drag, Drag::None) {
                            let hit = self.doc.objs.iter().rposition(|o| o.hit(p));
                            self.doc.sel = hit;
                            self.editing = None;
                            if let Some(i) = hit {
                                self.drag = Drag::Moving { grab: p - self.doc.objs[i].a };
                            }
                        }
                    }
                    Tool::Rect | Tool::Arrow => {
                        let shape = if self.tool == Tool::Rect { Shape::Rect } else { Shape::Arrow };
                        self.doc.objs.push(Obj { shape, a: p, b: p, color: self.color, width: self.width });
                        self.doc.sel = Some(self.doc.objs.len() - 1);
                        self.drag = Drag::Creating;
                    }
                    Tool::Text => {}
                }
            }
        }

        // 拖拽中
        if resp.dragged() {
            if let Some(p) = pointer {
                match self.drag {
                    Drag::Creating => {
                        if let Some(i) = self.doc.sel {
                            if let Some(o) = self.doc.objs.get_mut(i) { o.b = p; }
                        }
                    }
                    Drag::Moving { grab } => {
                        if let Some(i) = self.doc.sel {
                            if let Some(o) = self.doc.objs.get_mut(i) {
                                let d = o.b - o.a;
                                o.a = p - grab;
                                o.b = o.a + d;
                            }
                        }
                    }
                    Drag::Resizing { corner } => {
                        if let Some(i) = self.doc.sel {
                            if let Some(o) = self.doc.objs.get_mut(i) {
                                if corner == 0 { o.a = p; } else { o.b = p; }
                            }
                        }
                    }
                    Drag::None => {}
                }
            }
        }

        if resp.drag_stopped() && !matches!(self.drag, Drag::None) {
            self.drag = Drag::None;
            self.hist.push(&self.doc);
        }

        // 文字工具落字
        if resp.clicked() && self.tool == Tool::Text {
            if let Some(p) = pointer {
                self.doc.objs.push(Obj {
                    shape: Shape::Text(String::new()),
                    a: p,
                    b: p + Vec2::new(220.0, 34.0),
                    color: self.color,
                    width: self.width,
                });
                let i = self.doc.objs.len() - 1;
                self.doc.sel = Some(i);
                self.editing = Some(i);
                self.hist.push(&self.doc);
            }
        }

        // 双击已有文本进入编辑
        if resp.double_clicked() && self.tool == Tool::Select {
            if let Some(p) = pointer {
                if let Some(i) = self.doc.objs.iter().rposition(|o| o.hit(p)) {
                    if matches!(self.doc.objs[i].shape, Shape::Text(_)) {
                        self.doc.sel = Some(i);
                        self.editing = Some(i);
                    }
                }
            }
        }

        // ---------------- 绘制 ----------------
        for (i, o) in self.doc.objs.iter().enumerate() {
            let stroke = Stroke::new(o.width, o.color);
            match &o.shape {
                Shape::Rect => {
                    painter.rect_stroke(o.bounds(), 2.0, stroke, StrokeKind::Middle);
                }
                Shape::Arrow => {
                    painter.line_segment([o.a, o.b], stroke);
                    let dir = (o.b - o.a).normalized();
                    if dir.length() > 0.0 {
                        let n = Vec2::new(-dir.y, dir.x);
                        let tip = o.b;
                        let back = tip - dir * (8.0 + o.width * 2.0);
                        let half = n * (4.0 + o.width);
                        painter.add(egui::Shape::convex_polygon(
                            vec![tip, back + half, back - half],
                            o.color,
                            Stroke::NONE,
                        ));
                    }
                }
                Shape::Text(s) => {
                    if self.editing != Some(i) {
                        if s.is_empty() {
                            painter.text(
                                o.a,
                                egui::Align2::LEFT_TOP,
                                "(空文本)",
                                egui::FontId::proportional(20.0),
                                Color32::from_gray(90),
                            );
                        } else {
                            painter.text(
                                o.a,
                                egui::Align2::LEFT_TOP,
                                s,
                                egui::FontId::proportional(20.0),
                                o.color,
                            );
                        }
                    }
                }
            }
        }

        // 选中框与手柄
        if let Some(i) = self.doc.sel {
            if let Some(o) = self.doc.objs.get(i) {
                painter.rect_stroke(
                    o.bounds().expand(3.0),
                    0.0,
                    Stroke::new(1.0, Color32::from_rgb(0, 122, 255)),
                    StrokeKind::Middle,
                );
                for c in [o.a, o.b] {
                    painter.circle_filled(c, HANDLE * 0.5, Color32::from_rgb(0, 122, 255));
                    painter.circle_stroke(c, HANDLE * 0.5, Stroke::new(1.0, Color32::WHITE));
                }
            }
        }

        // ---------------- 文本编辑：IME 验证点 ----------------
        if let Some(i) = self.editing {
            let cur = self.doc.objs.get(i).and_then(|o| match &o.shape {
                Shape::Text(s) => Some((o.a, s.clone(), o.color)),
                _ => None,
            });
            if let Some((a, mut text, color)) = cur {
                // 所见即所得的原位编辑需要三处补偿，缺一处文字就会在
                // 编辑态与非编辑态之间跳动：
                //   1. ui.put 用的是 centered_and_justified 布局，会把文字居中，
                //      须改用 UiBuilder + left_to_right(Align::Min) 保持左上对齐
                //   2. TextEdit 默认 margin 为 Margin::symmetric(4, 2)，须清零
                //   3. TextEdit 默认带 Frame（白底+边框），须设为 Frame::NONE
                // 这三点是 egui 做标注文字编辑的实际成本，非显然。
                let edit_rect = Rect::from_min_size(a, Vec2::new(280.0, 30.0));
                let r = ui
                    .scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(edit_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Min)),
                        |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut text)
                                    .frame(egui::Frame::NONE)
                                    .margin(egui::Margin::ZERO)
                                    .desired_width(280.0)
                                    .font(egui::FontId::proportional(20.0))
                                    .text_color(color),
                            )
                        },
                    )
                    .inner;
                if !r.has_focus() {
                    r.request_focus();
                }
                if let Some(o) = self.doc.objs.get_mut(i) {
                    o.shape = Shape::Text(text);
                }
                if r.lost_focus() {
                    // 内容为空的文本对象直接丢弃，否则误点会在画布上
                    // 堆积一堆不可见的空对象
                    let empty = self
                        .doc
                        .objs
                        .get(i)
                        .map(|o| matches!(&o.shape, Shape::Text(t) if t.trim().is_empty()))
                        .unwrap_or(false);
                    if empty {
                        self.doc.objs.remove(i);
                        if self.doc.sel == Some(i) {
                            self.doc.sel = None;
                        }
                    }
                    self.editing = None;
                    self.hist.push(&self.doc);
                }
            } else {
                self.editing = None;
            }
        }

        self.frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PinWall 原型 2 · egui 标注编辑器",
        opts,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx);
            Ok(Box::<App>::default())
        }),
    )
}

/// egui 默认字体不含中日韩字形，不装的话中文全是豆腐块。
/// 这本身就是一条结论：中文场景下必须自带或加载系统 CJK 字体。
fn install_cjk_font(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "cjk".into(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".into());
            ctx.set_fonts(fonts);
            println!("已加载中文字体: {path}");
            return;
        }
    }
    eprintln!("!! 未找到中文字体，界面中文将显示为方块");
}
