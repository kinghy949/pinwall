//! 把跨屏选区的各部分捕获并拼接为一张图。

use pinwall_core::Selection;

use crate::{CapturedImage, Capturer, Error, Result};

/// 捕获一次框选的结果。
///
/// 单屏选区直接捕获；跨屏选区**逐屏捕获后拼接** ——
/// 各屏倍率可能不同，整块传给捕获层会得到错误的像素尺寸。
///
/// 输出倍率取各部分的最大值（见 [`Selection::output_scale`]），
/// 低倍率的部分会被放大以对齐。放大目前使用最近邻，
/// 仅在混合 DPI 且选区跨屏时才会发生；若该场景变得常见，应改为双线性。
pub fn capture_selection(capturer: &dyn Capturer, sel: &Selection) -> Result<CapturedImage> {
    if sel.parts.is_empty() {
        return Err(Error::EmptyRect(sel.rect));
    }

    // 快路径：单屏选区无需拼接
    if sel.parts.len() == 1 {
        let p = &sel.parts[0];
        if p.rect == sel.rect {
            return capturer.capture_rect(p.rect, p.scale);
        }
    }

    let out_scale = sel.output_scale();
    let (out_w, out_h) = sel.output_pixel_size();
    if out_w == 0 || out_h == 0 {
        return Err(Error::EmptyRect(sel.rect));
    }
    // 未被任何屏覆盖的空隙保持为全透明，而非黑色 ——
    // 黑色会被误认为是真实内容
    let mut out = vec![0u8; out_w as usize * out_h as usize * 4];

    for part in &sel.parts {
        let img = capturer.capture_rect(part.rect, part.scale)?;
        // 该部分在输出图中的左上角（像素）
        let dx = ((part.rect.origin.x - sel.rect.origin.x) * out_scale).round() as i64;
        let dy = ((part.rect.origin.y - sel.rect.origin.y) * out_scale).round() as i64;
        blit(&img, part.scale, &mut out, out_w, out_h, out_scale, dx, dy);
    }

    Ok(CapturedImage { width: out_w, height: out_h, scale: out_scale, bgra: out })
}

/// 把 `src` 贴到 `dst` 的 `(dx, dy)` 处，必要时按倍率差做最近邻放大。
#[allow(clippy::too_many_arguments)]
fn blit(
    src: &CapturedImage,
    src_scale: f64,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    dst_scale: f64,
    dx: i64,
    dy: i64,
) {
    let ratio = dst_scale / src_scale;
    // 该部分在输出图中应占的像素尺寸
    let span_w = (src.width as f64 * ratio).round() as i64;
    let span_h = (src.height as f64 * ratio).round() as i64;

    for oy in 0..span_h {
        let ty = dy + oy;
        if ty < 0 || ty >= dst_h as i64 {
            continue;
        }
        // ratio == 1.0 时退化为直接逐行拷贝
        let sy = ((oy as f64 / ratio) as usize).min(src.height as usize - 1);
        for ox in 0..span_w {
            let tx = dx + ox;
            if tx < 0 || tx >= dst_w as i64 {
                continue;
            }
            let sx = ((ox as f64 / ratio) as usize).min(src.width as usize - 1);
            let s = (sy * src.width as usize + sx) * 4;
            let d = (ty as usize * dst_w as usize + tx as usize) * 4;
            dst[d..d + 4].copy_from_slice(&src.bgra[s..s + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinwall_core::SelectionPart;
    use pinwall_platform::geom::Rect;
    use pinwall_platform::ScreenId;

    /// 桩捕获器：按请求的区域生成纯色图，颜色由 origin.x 的正负决定，
    /// 便于验证拼接后各部分是否落在正确位置。
    struct StubCapturer;

    impl Capturer for StubCapturer {
        fn capture_rect(
            &self,
            rect: pinwall_platform::geom::Rect,
            scale: f64,
        ) -> Result<CapturedImage> {
            let w = (rect.size.width * scale).round() as u32;
            let h = (rect.size.height * scale).round() as u32;
            let color: [u8; 4] = if rect.origin.x < 0.0 {
                [0, 0, 255, 255] // BGRA 红
            } else {
                [255, 0, 0, 255] // BGRA 蓝
            };
            Ok(CapturedImage {
                width: w,
                height: h,
                scale,
                bgra: color.iter().cycle().take((w * h * 4) as usize).copied().collect(),
            })
        }
    }

    fn px(img: &CapturedImage, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * img.width as usize + x as usize) * 4;
        img.bgra[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn cross_screen_parts_land_in_correct_positions() {
        let sel = Selection {
            rect: Rect::from_xywh(-200.0, 100.0, 600.0, 400.0),
            parts: vec![
                SelectionPart { screen_id: ScreenId(1), rect: Rect::from_xywh(-200.0, 100.0, 200.0, 400.0), scale: 2.0 },
                SelectionPart { screen_id: ScreenId(0), rect: Rect::from_xywh(0.0, 100.0, 400.0, 400.0), scale: 2.0 },
            ],
        };
        let img = capture_selection(&StubCapturer, &sel).unwrap();
        assert_eq!((img.width, img.height), (1200, 800));
        // 左侧 400 像素来自负坐标屏（红）
        assert_eq!(px(&img, 10, 10), [0, 0, 255, 255]);
        assert_eq!(px(&img, 399, 400), [0, 0, 255, 255]);
        // 右侧来自主屏（蓝）
        assert_eq!(px(&img, 400, 10), [255, 0, 0, 255]);
        assert_eq!(px(&img, 1199, 799), [255, 0, 0, 255]);
    }

    #[test]
    fn dead_zone_stays_transparent_not_black() {
        // 选区高 200，但唯一的部分只覆盖上半部
        let sel = Selection {
            rect: Rect::from_xywh(0.0, 0.0, 100.0, 200.0),
            parts: vec![SelectionPart {
                screen_id: ScreenId(0),
                rect: Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
                scale: 2.0,
            }],
        };
        let img = capture_selection(&StubCapturer, &sel).unwrap();
        assert_eq!(px(&img, 50, 50)[3], 255, "有屏幕覆盖处应不透明");
        assert_eq!(px(&img, 50, 350), [0, 0, 0, 0], "空隙应为全透明而非黑色");
    }

    #[test]
    fn mixed_dpi_upscales_low_dpi_part() {
        let sel = Selection {
            rect: Rect::from_xywh(-100.0, 0.0, 200.0, 100.0),
            parts: vec![
                SelectionPart { screen_id: ScreenId(1), rect: Rect::from_xywh(-100.0, 0.0, 100.0, 100.0), scale: 1.0 },
                SelectionPart { screen_id: ScreenId(0), rect: Rect::from_xywh(0.0, 0.0, 100.0, 100.0), scale: 2.0 },
            ],
        };
        let img = capture_selection(&StubCapturer, &sel).unwrap();
        assert_eq!(img.scale, 2.0);
        assert_eq!((img.width, img.height), (400, 200));
        // 1x 的部分被放大到 200 像素宽，仍应铺满左半边
        assert_eq!(px(&img, 5, 100), [0, 0, 255, 255]);
        assert_eq!(px(&img, 199, 100), [0, 0, 255, 255]);
        assert_eq!(px(&img, 200, 100), [255, 0, 0, 255]);
    }

    #[test]
    fn single_screen_uses_fast_path() {
        let sel = Selection {
            rect: Rect::from_xywh(10.0, 20.0, 100.0, 50.0),
            parts: vec![SelectionPart {
                screen_id: ScreenId(0),
                rect: Rect::from_xywh(10.0, 20.0, 100.0, 50.0),
                scale: 2.0,
            }],
        };
        let img = capture_selection(&StubCapturer, &sel).unwrap();
        assert_eq!((img.width, img.height), (200, 100));
    }
}
