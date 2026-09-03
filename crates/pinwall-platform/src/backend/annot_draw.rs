//! 标注图元的 CoreGraphics 绘制。
//!
//! # 为什么独立成模块
//!
//! 标注要画两次：一次画到屏幕上（贴图窗口），一次画到位图里
//! （存盘与复制时把标注烧进图像）。两处必须**逐像素一致** ——
//! 用户在屏幕上看到什么，导出的图就该是什么，否则复制出去发现
//! 线粗了、箭头歪了，比没有标注更让人困惑。
//!
//! 保证一致的办法不是"仔细写两遍"，而是只写一遍：本模块只认
//! [`CGContext`]，不关心它背后是屏幕还是位图。
//!
//! # 坐标约定
//!
//! 指令坐标为**贴图局部坐标**（左上角原点、y 向下、单位为逻辑点，
//! 与缩放无关）。CG 上下文则是左下角原点、y 向上，故需翻转 y。

use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSStringDrawing,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

use crate::{DrawCommand, Rgba};

/// 矩形四边同时内缩。负值即外扩。
pub fn inset(r: NSRect, d: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(r.origin.x + d, r.origin.y + d),
        NSSize::new(
            (r.size.width - d * 2.0).max(0.0),
            (r.size.height - d * 2.0).max(0.0),
        ),
    )
}

/// 把标注指令画进上下文。
///
/// - `logical_height`：贴图在**原始逻辑尺寸**下的高度，用于翻转 y。
///   注意不是上下文的高度 —— 上下文可能已被 `scale` 放大。
/// - `scale`：整体缩放系数。屏幕上传入当前缩放倍率，导出时传入像素倍率。
///   坐标与线宽、字号一同缩放 —— 若只缩放坐标，放大后线条会显得过细。
pub fn draw(ctx: &CGContext, cmds: &[DrawCommand], logical_height: f64, scale: f64) {
    if cmds.is_empty() {
        return;
    }
    let z = scale.max(f64::EPSILON);
    CGContext::save_g_state(Some(ctx));
    CGContext::scale_ctm(Some(ctx), z, z);
    let fy = |y: f64| logical_height - y;

    for c in cmds {
        match c {
            DrawCommand::Rect { rect, color, width } => {
                set_stroke(ctx, *color);
                CGContext::set_line_width(Some(ctx), *width);
                CGContext::stroke_rect(Some(ctx), flip(rect, &fy));
            }
            DrawCommand::Arrow { from, to, color, width } => {
                let (a, b) = (
                    NSPoint::new(from.x, fy(from.y)),
                    NSPoint::new(to.x, fy(to.y)),
                );
                set_stroke(ctx, *color);
                CGContext::set_line_width(Some(ctx), *width);
                CGContext::begin_path(Some(ctx));
                CGContext::move_to_point(Some(ctx), a.x, a.y);
                CGContext::add_line_to_point(Some(ctx), b.x, b.y);
                CGContext::stroke_path(Some(ctx));

                // 箭头头部：以线段方向为轴的等腰三角形
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let len = (dx * dx + dy * dy).sqrt();
                if len > f64::EPSILON {
                    let (ux, uy) = (dx / len, dy / len);
                    let (nx, ny) = (-uy, ux);
                    let back = 8.0 + width * 2.0;
                    let half = 4.0 + width;
                    set_fill(ctx, *color);
                    CGContext::begin_path(Some(ctx));
                    CGContext::move_to_point(Some(ctx), b.x, b.y);
                    CGContext::add_line_to_point(
                        Some(ctx),
                        b.x - ux * back + nx * half,
                        b.y - uy * back + ny * half,
                    );
                    CGContext::add_line_to_point(
                        Some(ctx),
                        b.x - ux * back - nx * half,
                        b.y - uy * back - ny * half,
                    );
                    CGContext::close_path(Some(ctx));
                    CGContext::fill_path(Some(ctx));
                }
            }
            DrawCommand::Redact { rect } => {
                // 以不透明纯色遮蔽。真正的马赛克需要读回底图像素，
                // 而纯色遮挡在防泄露上更彻底 —— 马赛克有被复原的先例。
                CGContext::set_rgb_fill_color(Some(ctx), 0.12, 0.12, 0.12, 1.0);
                CGContext::fill_rect(Some(ctx), flip(rect, &fy));
            }
            DrawCommand::SelectionBox { rect } => {
                let r = flip(rect, &fy);
                CGContext::set_rgb_stroke_color(Some(ctx), 0.0, 0.48, 1.0, 1.0);
                CGContext::set_line_width(Some(ctx), 1.0);
                CGContext::stroke_rect(Some(ctx), inset(r, -3.0));
                // 两个角手柄，与模型中的 a / b 对应
                for (hx, hy) in [
                    (r.origin.x, r.origin.y + r.size.height),
                    (r.origin.x + r.size.width, r.origin.y),
                ] {
                    let d = 3.5;
                    let hr =
                        NSRect::new(NSPoint::new(hx - d, hy - d), NSSize::new(d * 2.0, d * 2.0));
                    CGContext::set_rgb_fill_color(Some(ctx), 0.0, 0.48, 1.0, 1.0);
                    CGContext::fill_rect(Some(ctx), hr);
                    CGContext::set_rgb_stroke_color(Some(ctx), 1.0, 1.0, 1.0, 1.0);
                    CGContext::stroke_rect(Some(ctx), hr);
                }
            }
            DrawCommand::Text { origin, text, color, size } => {
                let s = NSString::from_str(text);
                let font = NSFont::systemFontOfSize(*size);
                let fg =
                    NSColor::colorWithSRGBRed_green_blue_alpha(color.r, color.g, color.b, color.a);
                let attrs = NSDictionary::from_slices(
                    &[unsafe { NSFontAttributeName }, unsafe {
                        NSForegroundColorAttributeName
                    }],
                    &[&*font as &objc2::runtime::AnyObject, &*fg],
                );
                // NSString 绘制以左下角为基准，故用文字高度回退
                let point = NSPoint::new(origin.x, fy(origin.y) - size * 1.2);
                unsafe { s.drawAtPoint_withAttributes(point, Some(&attrs)) };
            }
        }
    }
    CGContext::restore_g_state(Some(ctx));
}

/// 把左上原点的矩形翻成 CG 的左下原点。
fn flip(rect: &crate::geom::Rect, fy: &impl Fn(f64) -> f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.origin.x, fy(rect.origin.y + rect.size.height)),
        NSSize::new(rect.size.width, rect.size.height),
    )
}

fn set_stroke(ctx: &CGContext, c: Rgba) {
    CGContext::set_rgb_stroke_color(Some(ctx), c.r, c.g, c.b, c.a);
}

fn set_fill(ctx: &CGContext, c: Rgba) {
    CGContext::set_rgb_fill_color(Some(ctx), c.r, c.g, c.b, c.a);
}
