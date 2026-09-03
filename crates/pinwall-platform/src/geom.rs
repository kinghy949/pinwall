//! 几何类型与坐标系约定。
//!
//! # 坐标系
//!
//! PinWall 内部统一使用：**原点在主显示器左上角，y 轴向下，单位为逻辑点**。
//!
//! 这与 Windows 一致，但与 macOS 的 Cocoa 坐标系（原点在主屏**左下角**，y 轴向上）
//! 相反，故 macOS 后端负责在边界处做转换，转换只发生在后端内部。
//!
//! **副屏坐标可以为负。** 实测中外接屏的原点为 `(-1920, -124)`，
//! 即它位于主屏左侧且略高。任何「坐标从 (0,0) 起算」的假设都是错的。

/// 逻辑点坐标下的二维点。副屏方向上可为负值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// 矩形，`origin` 为**左上角**（遵循本 crate 的坐标系约定）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    pub const fn from_xywh(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self::new(Point::new(x, y), Size::new(width, height))
    }

    pub fn max_x(&self) -> f64 {
        self.origin.x + self.size.width
    }

    pub fn max_y(&self) -> f64 {
        self.origin.y + self.size.height
    }

    /// 求并集。
    ///
    /// 注意：并集**不可**用于构造覆盖全部显示器的单个遮罩窗口 ——
    /// macOS 默认「显示器各自拥有独立空间」，一个窗口无法跨屏。
    /// 遮罩必须每屏一个。此方法仅用于坐标换算与命中范围判断。
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.origin.x.min(other.origin.x);
        let y = self.origin.y.min(other.origin.y);
        let mx = self.max_x().max(other.max_x());
        let my = self.max_y().max(other.max_y());
        Rect::from_xywh(x, y, mx - x, my - y)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.origin.x && p.x < self.max_x() && p.y >= self.origin.y && p.y < self.max_y()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 副屏位于主屏左侧的真实布局（实测环境）：
    /// 主屏 (0,0) 1470x956，副屏 (-1920,0) 1920x1080。
    fn layout() -> (Rect, Rect) {
        (
            Rect::from_xywh(0.0, 0.0, 1470.0, 956.0),
            Rect::from_xywh(-1920.0, 0.0, 1920.0, 1080.0),
        )
    }

    #[test]
    fn union_handles_negative_origin() {
        let (primary, secondary) = layout();
        let u = primary.union(&secondary);
        assert_eq!(u.origin, Point::new(-1920.0, 0.0));
        assert_eq!(u.size, Size::new(3390.0, 1080.0));
    }

    #[test]
    fn union_is_commutative() {
        let (a, b) = layout();
        assert_eq!(a.union(&b), b.union(&a));
    }

    #[test]
    fn contains_respects_negative_coordinates() {
        let (primary, secondary) = layout();
        // 副屏中心
        assert!(secondary.contains(Point::new(-960.0, 540.0)));
        assert!(!primary.contains(Point::new(-960.0, 540.0)));
        // 主屏中心
        assert!(primary.contains(Point::new(735.0, 478.0)));
        assert!(!secondary.contains(Point::new(735.0, 478.0)));
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)), "左上角应包含");
        assert!(!r.contains(Point::new(10.0, 5.0)), "右边界应排除");
        assert!(!r.contains(Point::new(5.0, 10.0)), "下边界应排除");
    }

    /// 相邻两屏的交界处不应出现「两块屏都命中」或「都不命中」。
    /// 这是跨屏框选判断鼠标归属的正确性前提。
    #[test]
    fn adjacent_screens_do_not_overlap_or_gap() {
        let (primary, secondary) = layout();
        let boundary = Point::new(0.0, 300.0);
        assert!(primary.contains(boundary));
        assert!(!secondary.contains(boundary));
        let just_left = Point::new(-0.001, 300.0);
        assert!(!primary.contains(just_left));
        assert!(secondary.contains(just_left));
    }
}
