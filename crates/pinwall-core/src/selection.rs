//! 框选状态机。
//!
//! # 为什么需要状态机
//!
//! 捕获遮罩是**每屏一个窗口**（macOS 不允许单窗口跨屏），因此一次框选的
//! 鼠标事件会来自不同的窗口：用户可能在屏 A 按下、拖到屏 B 松开。
//!
//! 本状态机只认**全局逻辑坐标**，由事件源负责先把各窗口的局部坐标换算成
//! 全局坐标再喂进来。这样跨屏与单屏在逻辑上没有区别，也让整套行为可以
//! 脱离 UI 完整单测。

use pinwall_platform::geom::{Point, Rect};
use pinwall_platform::{ScreenId, ScreenInfo};

/// 选区小于该尺寸（逻辑点）视为误触，不予提交。
pub const MIN_SELECTION: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    /// 等待按下。
    Idle,
    /// 拖拽中。`anchor` 为按下点，`current` 为当前指针位置。
    /// 两者顺序任意 —— 用户可从任一方向拖拽。
    Dragging { anchor: Point, current: Point },
    /// 已提交，等待宿主取走结果。
    Done,
    /// 已取消。
    Cancelled,
}

#[derive(Debug, Clone, Copy)]
pub enum Event {
    /// 指针按下（全局逻辑坐标）。
    Down(Point),
    Move(Point),
    Up(Point),
    /// 取消，通常来自 Esc 或右键。
    Cancel,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// 无变化，无需重绘。
    Idle,
    /// 选区有变化，需重绘全部遮罩（选区可能跨屏，不能只重绘一块）。
    Redraw,
    Committed(Selection),
    Cancelled,
}

/// 选区落在某一块屏上的部分。
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionPart {
    pub screen_id: ScreenId,
    /// 该部分在**全局逻辑坐标**下的矩形。
    pub rect: Rect,
    /// 该屏的逻辑点→像素倍率。捕获时须按此取像素。
    pub scale: f64,
}

impl SelectionPart {
    /// 该部分的物理像素尺寸。
    pub fn pixel_size(&self) -> (u32, u32) {
        (
            (self.rect.size.width * self.scale).round() as u32,
            (self.rect.size.height * self.scale).round() as u32,
        )
    }
}

/// 一次完成的框选。
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// 全局逻辑坐标下的完整选区。
    pub rect: Rect,
    /// 按屏切分后的各部分。**捕获时必须逐部分进行** ——
    /// 各屏倍率可能不同，整块传给捕获层会得到错误的像素尺寸。
    pub parts: Vec<SelectionPart>,
}

impl Selection {
    /// 是否跨越多块显示器。
    pub fn is_cross_screen(&self) -> bool {
        self.parts.len() > 1
    }

    /// 合成输出所应采用的倍率。
    ///
    /// 跨屏且各屏倍率不同（混合 DPI）时取**最大值**：宁可把低分屏的部分
    /// 放大，也不要把高分屏的部分降采样丢失细节。
    pub fn output_scale(&self) -> f64 {
        self.parts.iter().map(|p| p.scale).fold(0.0, f64::max)
    }

    /// 合成后的输出像素尺寸。
    pub fn output_pixel_size(&self) -> (u32, u32) {
        let s = self.output_scale();
        (
            (self.rect.size.width * s).round() as u32,
            (self.rect.size.height * s).round() as u32,
        )
    }
}

pub struct SelectionMachine {
    state: State,
    screens: Vec<ScreenInfo>,
}

impl SelectionMachine {
    /// `screens` 应为进入捕获流程时枚举到的显示器快照。
    /// 捕获过程中若发生热插拔，应重建本机而非原地修改。
    pub fn new(screens: Vec<ScreenInfo>) -> Self {
        Self { state: State::Idle, screens }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// 当前正在拖拽出的矩形，供遮罩绘制高亮框。
    pub fn current_rect(&self) -> Option<Rect> {
        match self.state {
            State::Dragging { anchor, current } => Some(Rect::from_points(anchor, current)),
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
    }

    pub fn handle(&mut self, event: Event) -> Outcome {
        match (self.state, event) {
            (_, Event::Cancel) => {
                self.state = State::Cancelled;
                Outcome::Cancelled
            }

            (State::Idle, Event::Down(p)) => {
                self.state = State::Dragging { anchor: p, current: p };
                Outcome::Redraw
            }

            (State::Dragging { anchor, current }, Event::Move(p)) => {
                if p == current {
                    return Outcome::Idle;
                }
                self.state = State::Dragging { anchor, current: p };
                Outcome::Redraw
            }

            (State::Dragging { anchor, .. }, Event::Up(p)) => {
                let rect = Rect::from_points(anchor, p);
                // 误触保护：把「点一下」和「拖出一个极小的框」都视为取消
                if rect.size.width < MIN_SELECTION || rect.size.height < MIN_SELECTION {
                    self.state = State::Cancelled;
                    return Outcome::Cancelled;
                }
                match self.split_by_screen(rect) {
                    Some(sel) => {
                        self.state = State::Done;
                        Outcome::Committed(sel)
                    }
                    None => {
                        // 选区完全落在显示器之间的空隙里
                        self.state = State::Cancelled;
                        Outcome::Cancelled
                    }
                }
            }

            // Idle 下的 Move/Up、Done/Cancelled 下的任何输入，均忽略
            _ => Outcome::Idle,
        }
    }

    /// 把选区按屏切分。与任何一块屏都无交集时返回 `None`。
    fn split_by_screen(&self, rect: Rect) -> Option<Selection> {
        let parts: Vec<SelectionPart> = self
            .screens
            .iter()
            .filter_map(|s| {
                rect.intersection(&s.frame).map(|r| SelectionPart {
                    screen_id: s.id,
                    rect: r,
                    scale: s.scale,
                })
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(Selection { rect, parts })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinwall_platform::geom::Size;

    /// 实测环境的真实布局：
    /// 内置屏 (0,0) 1470x956 @2x，Dell (-1920,0) 1920x1080 @2x。
    /// 两屏在 x=0 处相邻，但 Dell 更高 —— y>956 且 x>0 的区域没有任何屏幕。
    fn screens() -> Vec<ScreenInfo> {
        vec![
            ScreenInfo {
                id: ScreenId(0),
                name: "Built-in".into(),
                frame: Rect::from_xywh(0.0, 0.0, 1470.0, 956.0),
                scale: 2.0,
                is_primary: true,
            },
            ScreenInfo {
                id: ScreenId(1),
                name: "DELL".into(),
                frame: Rect::from_xywh(-1920.0, 0.0, 1920.0, 1080.0),
                scale: 2.0,
                is_primary: false,
            },
        ]
    }

    fn machine() -> SelectionMachine {
        SelectionMachine::new(screens())
    }

    fn drag(m: &mut SelectionMachine, from: Point, to: Point) -> Outcome {
        m.handle(Event::Down(from));
        m.handle(Event::Move(to));
        m.handle(Event::Up(to))
    }

    #[test]
    fn single_screen_selection() {
        let mut m = machine();
        let out = drag(&mut m, Point::new(100.0, 100.0), Point::new(300.0, 250.0));
        let Outcome::Committed(sel) = out else { panic!("应提交，实得 {out:?}") };
        assert_eq!(sel.rect, Rect::from_xywh(100.0, 100.0, 200.0, 150.0));
        assert_eq!(sel.parts.len(), 1);
        assert_eq!(sel.parts[0].screen_id, ScreenId(0));
        assert_eq!(sel.parts[0].pixel_size(), (400, 300));
        assert!(!sel.is_cross_screen());
    }

    /// 从右下往左上拖，矩形必须被归一化，不能出现负的宽高。
    #[test]
    fn reverse_drag_is_normalized() {
        let mut m = machine();
        let out = drag(&mut m, Point::new(300.0, 250.0), Point::new(100.0, 100.0));
        let Outcome::Committed(sel) = out else { panic!("应提交") };
        assert_eq!(sel.rect, Rect::from_xywh(100.0, 100.0, 200.0, 150.0));
        assert!(sel.rect.size.width > 0.0 && sel.rect.size.height > 0.0);
    }

    /// 核心场景：在 Dell 上按下，拖到内置屏松开。
    #[test]
    fn cross_screen_selection_splits_by_screen() {
        let mut m = machine();
        let out = drag(&mut m, Point::new(-200.0, 100.0), Point::new(400.0, 500.0));
        let Outcome::Committed(sel) = out else { panic!("应提交") };
        assert!(sel.is_cross_screen());
        assert_eq!(sel.parts.len(), 2);

        let dell = sel.parts.iter().find(|p| p.screen_id == ScreenId(1)).unwrap();
        let builtin = sel.parts.iter().find(|p| p.screen_id == ScreenId(0)).unwrap();
        // Dell 侧：x 从 -200 到 0
        assert_eq!(dell.rect, Rect::from_xywh(-200.0, 100.0, 200.0, 400.0));
        // 内置屏侧：x 从 0 到 400
        assert_eq!(builtin.rect, Rect::from_xywh(0.0, 100.0, 400.0, 400.0));
        // 两部分面积之和应等于总面积，不重不漏
        assert_eq!(dell.rect.area() + builtin.rect.area(), sel.rect.area());
    }

    /// 选区完全落在显示器之间的空隙（y>956 且 x>0）——应取消而非产出空图。
    #[test]
    fn selection_entirely_in_dead_zone_is_cancelled() {
        let mut m = machine();
        let out = drag(&mut m, Point::new(200.0, 1000.0), Point::new(600.0, 1060.0));
        assert_eq!(out, Outcome::Cancelled, "空隙区域不应提交");
    }

    /// 选区部分落在空隙：只应产出与真实屏幕相交的部分。
    #[test]
    fn selection_partially_in_dead_zone_keeps_only_real_parts() {
        let mut m = machine();
        // x 从 -100 到 500，y 从 900 到 1050：
        // Dell 侧 (x<0) 完整覆盖；内置屏侧 (x>0) 只到 y=956
        let out = drag(&mut m, Point::new(-100.0, 900.0), Point::new(500.0, 1050.0));
        let Outcome::Committed(sel) = out else { panic!("应提交") };
        assert_eq!(sel.parts.len(), 2);
        let builtin = sel.parts.iter().find(|p| p.screen_id == ScreenId(0)).unwrap();
        assert_eq!(builtin.rect.max_y(), 956.0, "内置屏部分应被裁到屏幕下边界");
        let dell = sel.parts.iter().find(|p| p.screen_id == ScreenId(1)).unwrap();
        assert_eq!(dell.rect.max_y(), 1050.0, "Dell 部分应保留完整高度");
        // 有空隙时，各部分面积之和小于选区总面积
        assert!(dell.rect.area() + builtin.rect.area() < sel.rect.area());
    }

    #[test]
    fn click_without_drag_is_cancelled() {
        let mut m = machine();
        let p = Point::new(100.0, 100.0);
        m.handle(Event::Down(p));
        assert_eq!(m.handle(Event::Up(p)), Outcome::Cancelled, "单击不应提交");
    }

    #[test]
    fn tiny_drag_is_cancelled() {
        let mut m = machine();
        let out = drag(&mut m, Point::new(100.0, 100.0), Point::new(102.0, 102.0));
        assert_eq!(out, Outcome::Cancelled, "低于最小尺寸不应提交");
    }

    #[test]
    fn esc_cancels_mid_drag() {
        let mut m = machine();
        m.handle(Event::Down(Point::new(100.0, 100.0)));
        m.handle(Event::Move(Point::new(300.0, 300.0)));
        assert_eq!(m.handle(Event::Cancel), Outcome::Cancelled);
        assert_eq!(m.state(), State::Cancelled);
    }

    #[test]
    fn events_after_commit_are_ignored() {
        let mut m = machine();
        drag(&mut m, Point::new(100.0, 100.0), Point::new(300.0, 300.0));
        assert_eq!(m.state(), State::Done);
        assert_eq!(m.handle(Event::Down(Point::new(0.0, 0.0))), Outcome::Idle);
        assert_eq!(m.handle(Event::Move(Point::new(50.0, 50.0))), Outcome::Idle);
        assert_eq!(m.state(), State::Done, "提交后状态不应被后续事件改变");
    }

    #[test]
    fn move_without_down_is_ignored() {
        let mut m = machine();
        assert_eq!(m.handle(Event::Move(Point::new(10.0, 10.0))), Outcome::Idle);
        assert_eq!(m.state(), State::Idle);
    }

    /// 指针未实际移动时不应触发重绘，否则拖拽期间会空转。
    #[test]
    fn identical_move_does_not_request_redraw() {
        let mut m = machine();
        m.handle(Event::Down(Point::new(100.0, 100.0)));
        assert_eq!(m.handle(Event::Move(Point::new(150.0, 150.0))), Outcome::Redraw);
        assert_eq!(m.handle(Event::Move(Point::new(150.0, 150.0))), Outcome::Idle);
    }

    #[test]
    fn current_rect_tracks_drag() {
        let mut m = machine();
        assert!(m.current_rect().is_none());
        m.handle(Event::Down(Point::new(100.0, 100.0)));
        m.handle(Event::Move(Point::new(50.0, 300.0)));
        assert_eq!(
            m.current_rect(),
            Some(Rect::new(Point::new(50.0, 100.0), Size::new(50.0, 200.0)))
        );
    }

    /// 混合 DPI：跨屏时输出倍率取各部分最大值，避免高分屏部分被降采样。
    #[test]
    fn mixed_dpi_output_uses_max_scale() {
        let mut mixed = screens();
        mixed[1].scale = 1.0; // 把 Dell 当作 1x 屏
        let mut m = SelectionMachine::new(mixed);
        let out = drag(&mut m, Point::new(-200.0, 100.0), Point::new(400.0, 500.0));
        let Outcome::Committed(sel) = out else { panic!("应提交") };
        assert_eq!(sel.output_scale(), 2.0, "应取最大倍率");
        // 选区 600x400 逻辑点，按 2x 输出
        assert_eq!(sel.output_pixel_size(), (1200, 800));
        // 各部分仍按各自倍率捕获
        let dell = sel.parts.iter().find(|p| p.screen_id == ScreenId(1)).unwrap();
        assert_eq!(dell.pixel_size(), (200, 400), "1x 屏按 1x 取像素");
    }
}
