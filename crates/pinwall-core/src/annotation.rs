//! 标注模型与编辑状态机。
//!
//! 与 [`crate::selection`] 一样，本模块不含任何渲染或平台代码 ——
//! 它只维护「有哪些标注对象、当前选中谁、如何撤销」，
//! 具体怎么画由各平台的渲染层决定。
//!
//! # 坐标系
//!
//! 本模块使用**贴图局部坐标**：原点在图像左上角，y 向下，单位为逻辑点。
//! 这样标注随贴图一起移动、缩放时无需重算，只在渲染时乘以缩放系数。

use pinwall_platform::geom::{Point, Rect};

/// 命中判定的容差（逻辑点）。细线条若严格按几何判定会极难点中。
const HIT_TOLERANCE: f64 = 6.0;
/// 小于此尺寸的图形视为误触，不予保留。
const MIN_SIZE: f64 = 3.0;
/// 缩放手柄的抓取半径。
const HANDLE_RADIUS: f64 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Rect,
    Arrow,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub const fn rgb(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b, a: 1.0 }
    }
    /// 默认标注色：醒目的红，在多数截图内容上都能看清。
    pub const RED: Self = Self::rgb(1.0, 0.23, 0.19);
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect,
    Arrow,
    Text(String),
    /// 马赛克/模糊打码。渲染层据此对该区域做像素处理。
    Redact,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub shape: Shape,
    /// 起点。矩形与打码为一个角，箭头为尾部，文字为左上角。
    pub a: Point,
    /// 终点。矩形与打码为对角，箭头为箭头尖端，文字为右下角。
    pub b: Point,
    pub color: Color,
    pub width: f64,
}

impl Annotation {
    /// 归一化后的包围盒。`a`/`b` 的相对方位任意。
    pub fn bounds(&self) -> Rect {
        Rect::from_points(self.a, self.b)
    }

    /// 命中测试。
    ///
    /// 立即模式渲染不会替你做命中判定，必须自己实现 ——
    /// 这是把 immediate mode 用于可编辑矢量对象时的主要成本。
    pub fn hit(&self, p: Point) -> bool {
        match self.shape {
            // 箭头是一条线段，按包围盒判定会让斜箭头旁边一大片空白都算命中
            Shape::Arrow => {
                distance_to_segment(p, self.a, self.b) <= self.width.max(HIT_TOLERANCE)
            }
            _ => {
                let b = self.bounds();
                Rect::from_xywh(
                    b.origin.x - HIT_TOLERANCE / 2.0,
                    b.origin.y - HIT_TOLERANCE / 2.0,
                    b.size.width + HIT_TOLERANCE,
                    b.size.height + HIT_TOLERANCE,
                )
                .contains(p)
            }
        }
    }

    fn is_degenerate(&self) -> bool {
        match self.shape {
            // 文字对象只要有内容就有意义，不按尺寸判定
            Shape::Text(ref s) => s.trim().is_empty(),
            Shape::Arrow => (self.b - self.a).length() < MIN_SIZE,
            _ => {
                let b = self.bounds();
                b.size.width < MIN_SIZE || b.size.height < MIN_SIZE
            }
        }
    }
}

/// 点到线段的距离。
fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len_sq = ab.x * ab.x + ab.y * ab.y;
    if len_sq <= f64::EPSILON {
        return (p - a).length();
    }
    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / len_sq).clamp(0.0, 1.0);
    let proj = Point::new(a.x + ab.x * t, a.y + ab.y * t);
    (p - proj).length()
}

/// 拖拽中的操作。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    None,
    /// 正在拉出一个新图形。
    Creating,
    /// 正在移动选中对象，`grab` 为按下点相对其 `a` 的偏移。
    Moving { grab: Point },
    /// 正在缩放，`corner` 为 0 表示握住 `a`，1 表示握住 `b`。
    Resizing { corner: u8 },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnnotationDoc {
    pub objects: Vec<Annotation>,
    pub selected: Option<usize>,
}

/// 快照式撤销栈。
///
/// 标注对象数量通常在十几个量级，整份快照的成本可以忽略；
/// 若将来支持自由笔迹（点数成千上万），应改为命令式 diff。
struct History {
    stack: Vec<AnnotationDoc>,
    index: usize,
}

impl History {
    fn new(doc: &AnnotationDoc) -> Self {
        Self { stack: vec![doc.clone()], index: 0 }
    }

    fn commit(&mut self, doc: &AnnotationDoc) {
        // 新操作会截断重做分支，这是撤销栈的通行语义
        self.stack.truncate(self.index + 1);
        self.stack.push(doc.clone());
        self.index = self.stack.len() - 1;
    }

    fn undo(&mut self) -> Option<AnnotationDoc> {
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        Some(self.stack[self.index].clone())
    }

    fn redo(&mut self) -> Option<AnnotationDoc> {
        if self.index + 1 >= self.stack.len() {
            return None;
        }
        self.index += 1;
        Some(self.stack[self.index].clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditEvent {
    Down(Point),
    Move(Point),
    Up(Point),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditOutcome {
    /// 无变化。
    Idle,
    /// 需要重绘。
    Redraw,
    /// 新建了一个文字对象，渲染层应就地弹出输入框。
    ///
    /// 文字输入交给平台原生控件处理，从而免费获得输入法支持 ——
    /// 自行实现候选词与预编辑状态的代价远高于其价值。
    BeginTextInput(usize),
}

pub struct AnnotationEditor {
    doc: AnnotationDoc,
    history: History,
    tool: Tool,
    drag: Drag,
    pub color: Color,
    pub width: f64,
}

impl Default for AnnotationEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl AnnotationEditor {
    pub fn new() -> Self {
        let doc = AnnotationDoc::default();
        Self {
            history: History::new(&doc),
            doc,
            tool: Tool::Select,
            drag: Drag::None,
            color: Color::RED,
            width: 3.0,
        }
    }

    pub fn objects(&self) -> &[Annotation] {
        &self.doc.objects
    }

    pub fn selected(&self) -> Option<usize> {
        self.doc.selected
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        // 切换工具时取消选中，否则用户以为还在编辑上一个对象
        self.doc.selected = None;
        self.drag = Drag::None;
    }

    pub fn is_empty(&self) -> bool {
        self.doc.objects.is_empty()
    }

    /// 修改指定文字对象的内容。由平台的文本输入控件回调。
    pub fn set_text(&mut self, index: usize, text: String) {
        if let Some(o) = self.doc.objects.get_mut(index) {
            if matches!(o.shape, Shape::Text(_)) {
                o.shape = Shape::Text(text);
            }
        }
    }

    /// 结束文字输入。内容为空的文字对象会被丢弃 ——
    /// 否则误点会在画面上留下看不见却能被选中的空对象。
    pub fn finish_text(&mut self, index: usize) {
        let empty = self
            .doc
            .objects
            .get(index)
            .is_some_and(|o| o.is_degenerate());
        if empty {
            self.doc.objects.remove(index);
            if self.doc.selected == Some(index) {
                self.doc.selected = None;
            }
        }
        self.history.commit(&self.doc);
    }

    pub fn delete_selected(&mut self) -> bool {
        let Some(i) = self.doc.selected.take() else {
            return false;
        };
        if i >= self.doc.objects.len() {
            return false;
        }
        self.doc.objects.remove(i);
        self.history.commit(&self.doc);
        true
    }

    pub fn undo(&mut self) -> bool {
        match self.history.undo() {
            Some(d) => {
                self.doc = d;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.history.redo() {
            Some(d) => {
                self.doc = d;
                true
            }
            None => false,
        }
    }

    pub fn handle(&mut self, event: EditEvent) -> EditOutcome {
        match event {
            EditEvent::Down(p) => self.on_down(p),
            EditEvent::Move(p) => self.on_move(p),
            EditEvent::Up(p) => self.on_up(p),
        }
    }

    fn on_down(&mut self, p: Point) -> EditOutcome {
        match self.tool {
            Tool::Select => {
                // 先判手柄再判对象：手柄在对象边角上，两者重叠时应优先缩放
                if let Some(i) = self.doc.selected {
                    if let Some(o) = self.doc.objects.get(i) {
                        if (p - o.a).length() <= HANDLE_RADIUS {
                            self.drag = Drag::Resizing { corner: 0 };
                            return EditOutcome::Redraw;
                        }
                        if (p - o.b).length() <= HANDLE_RADIUS {
                            self.drag = Drag::Resizing { corner: 1 };
                            return EditOutcome::Redraw;
                        }
                    }
                }
                // 自上而下命中：后添加的对象画在上层，理应优先被选中
                let hit = self.doc.objects.iter().rposition(|o| o.hit(p));
                self.doc.selected = hit;
                self.drag = match hit {
                    Some(i) => Drag::Moving { grab: p - self.doc.objects[i].a },
                    None => Drag::None,
                };
                EditOutcome::Redraw
            }
            Tool::Text => {
                self.doc.objects.push(Annotation {
                    shape: Shape::Text(String::new()),
                    a: p,
                    b: Point::new(p.x + 160.0, p.y + 24.0),
                    color: self.color,
                    width: self.width,
                });
                let i = self.doc.objects.len() - 1;
                self.doc.selected = Some(i);
                EditOutcome::BeginTextInput(i)
            }
            tool => {
                let shape = match tool {
                    Tool::Rect => Shape::Rect,
                    Tool::Arrow => Shape::Arrow,
                    _ => Shape::Redact,
                };
                self.doc.objects.push(Annotation {
                    shape,
                    a: p,
                    b: p,
                    color: self.color,
                    width: self.width,
                });
                self.doc.selected = Some(self.doc.objects.len() - 1);
                self.drag = Drag::Creating;
                EditOutcome::Redraw
            }
        }
    }

    fn on_move(&mut self, p: Point) -> EditOutcome {
        let Some(i) = self.doc.selected else {
            return EditOutcome::Idle;
        };
        let Some(o) = self.doc.objects.get_mut(i) else {
            return EditOutcome::Idle;
        };
        match self.drag {
            Drag::Creating => o.b = p,
            Drag::Moving { grab } => {
                // 整体平移：保持 a 与 b 的相对关系不变
                let d = o.b - o.a;
                o.a = p - grab;
                o.b = Point::new(o.a.x + d.x, o.a.y + d.y);
            }
            Drag::Resizing { corner } => {
                if corner == 0 {
                    o.a = p;
                } else {
                    o.b = p;
                }
            }
            Drag::None => return EditOutcome::Idle,
        }
        EditOutcome::Redraw
    }

    fn on_up(&mut self, _p: Point) -> EditOutcome {
        if self.drag == Drag::None {
            return EditOutcome::Idle;
        }
        let creating = self.drag == Drag::Creating;
        self.drag = Drag::None;

        // 新建时若只是点了一下没拖开，丢弃这个退化图形
        if creating {
            if let Some(i) = self.doc.selected {
                if self.doc.objects.get(i).is_some_and(|o| o.is_degenerate()) {
                    self.doc.objects.remove(i);
                    self.doc.selected = None;
                    return EditOutcome::Redraw;
                }
            }
        }
        self.history.commit(&self.doc);
        EditOutcome::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn drag(e: &mut AnnotationEditor, from: Point, to: Point) {
        e.handle(EditEvent::Down(from));
        e.handle(EditEvent::Move(to));
        e.handle(EditEvent::Up(to));
    }

    #[test]
    fn draws_rect() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(10.0, 10.0), p(110.0, 60.0));
        assert_eq!(e.objects().len(), 1);
        assert_eq!(e.objects()[0].bounds(), Rect::from_xywh(10.0, 10.0, 100.0, 50.0));
    }

    /// 反向拖拽的包围盒必须归一化，否则后续命中与渲染都会出错。
    #[test]
    fn reverse_drag_normalizes_bounds() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(110.0, 60.0), p(10.0, 10.0));
        assert_eq!(e.objects()[0].bounds(), Rect::from_xywh(10.0, 10.0, 100.0, 50.0));
    }

    #[test]
    fn click_without_drag_creates_nothing() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(10.0, 10.0), p(11.0, 11.0));
        assert!(e.is_empty(), "退化图形应被丢弃");
    }

    /// 箭头按线段距离命中，而非包围盒 ——
    /// 否则斜箭头旁边一大片空白都会被判为命中。
    #[test]
    fn arrow_hit_uses_segment_distance_not_bounds() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Arrow);
        drag(&mut e, p(0.0, 0.0), p(100.0, 100.0));
        let arrow = &e.objects()[0];
        assert!(arrow.hit(p(50.0, 50.0)), "线段上应命中");
        assert!(!arrow.hit(p(95.0, 5.0)), "包围盒内但远离线段，不应命中");
    }

    #[test]
    fn select_move_and_resize() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(10.0, 10.0), p(110.0, 60.0));

        e.set_tool(Tool::Select);
        // 选中并整体移动
        drag(&mut e, p(50.0, 30.0), p(70.0, 50.0));
        assert_eq!(
            e.objects()[0].bounds(),
            Rect::from_xywh(30.0, 30.0, 100.0, 50.0),
            "移动后尺寸不应变化"
        );

        // 抓住 b 角缩放
        let b = e.objects()[0].b;
        drag(&mut e, b, p(b.x + 40.0, b.y + 20.0));
        assert_eq!(e.objects()[0].bounds().size.width, 140.0);
    }

    /// 手柄与对象重叠时应优先缩放，而不是把对象整体拖走。
    #[test]
    fn handle_takes_priority_over_move() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(10.0, 10.0), p(110.0, 60.0));
        e.set_tool(Tool::Select);
        e.handle(EditEvent::Down(p(60.0, 35.0))); // 先选中
        e.handle(EditEvent::Up(p(60.0, 35.0)));

        let a = e.objects()[0].a;
        drag(&mut e, a, p(a.x - 10.0, a.y - 10.0));
        assert_eq!(
            e.objects()[0].bounds(),
            Rect::from_xywh(0.0, 0.0, 110.0, 60.0),
            "应缩放而非整体移动"
        );
    }

    /// 后添加的对象画在上层，重叠时应优先被选中。
    #[test]
    fn topmost_object_wins_hit_test() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(0.0, 0.0), p(100.0, 100.0));
        drag(&mut e, p(20.0, 20.0), p(80.0, 80.0));
        e.set_tool(Tool::Select);
        e.handle(EditEvent::Down(p(50.0, 50.0)));
        assert_eq!(e.selected(), Some(1), "应选中后添加的那个");
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(0.0, 0.0), p(50.0, 50.0));
        drag(&mut e, p(60.0, 60.0), p(90.0, 90.0));
        assert_eq!(e.objects().len(), 2);

        assert!(e.undo());
        assert_eq!(e.objects().len(), 1);
        assert!(e.undo());
        assert!(e.is_empty());
        assert!(!e.undo(), "已到栈底应返回 false");

        assert!(e.redo());
        assert_eq!(e.objects().len(), 1);
        assert!(e.redo());
        assert_eq!(e.objects().len(), 2);
        assert!(!e.redo(), "已到栈顶应返回 false");
    }

    /// 撤销后再做新操作，应截断重做分支。
    #[test]
    fn new_action_truncates_redo_branch() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(0.0, 0.0), p(50.0, 50.0));
        drag(&mut e, p(60.0, 60.0), p(90.0, 90.0));
        e.undo();
        drag(&mut e, p(100.0, 100.0), p(150.0, 150.0));
        assert!(!e.redo(), "新操作后不应还能重做旧分支");
        assert_eq!(e.objects().len(), 2);
    }

    #[test]
    fn text_tool_requests_input_and_drops_empty() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Text);
        let out = e.handle(EditEvent::Down(p(20.0, 20.0)));
        let EditOutcome::BeginTextInput(i) = out else {
            panic!("应请求文本输入，实得 {out:?}");
        };
        // 用户没输入任何内容就失焦
        e.finish_text(i);
        assert!(e.is_empty(), "空文字对象应被丢弃");
    }

    #[test]
    fn text_with_content_is_kept() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Text);
        let EditOutcome::BeginTextInput(i) = e.handle(EditEvent::Down(p(20.0, 20.0))) else {
            panic!("应请求文本输入");
        };
        e.set_text(i, "重点在这里".into());
        e.finish_text(i);
        assert_eq!(e.objects().len(), 1);
        assert_eq!(e.objects()[0].shape, Shape::Text("重点在这里".into()));
    }

    #[test]
    fn delete_selected() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(0.0, 0.0), p(50.0, 50.0));
        e.set_tool(Tool::Select);
        e.handle(EditEvent::Down(p(25.0, 25.0)));
        assert!(e.delete_selected());
        assert!(e.is_empty());
        assert!(!e.delete_selected(), "无选中时应返回 false");
    }

    /// 切换工具须清空选中，否则用户会以为还在编辑上一个对象。
    #[test]
    fn switching_tool_clears_selection() {
        let mut e = AnnotationEditor::new();
        e.set_tool(Tool::Rect);
        drag(&mut e, p(0.0, 0.0), p(50.0, 50.0));
        assert!(e.selected().is_some());
        e.set_tool(Tool::Arrow);
        assert_eq!(e.selected(), None);
    }
}
