//! 图像编码。

use crate::CapturedImage;

/// 把捕获结果编码为 PNG 字节。
///
/// 捕获层内部统一使用 BGRA8（与 CoreGraphics 及 wgpu 的原生格式一致），
/// PNG 要求 RGBA8，故此处做一次通道重排。
pub fn encode_png(img: &CapturedImage) -> Result<Vec<u8>, png::EncodingError> {
    let mut rgba = Vec::with_capacity(img.bgra.len());
    for p in img.bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }

    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        // 记录物理像素密度，使图片在其他应用中按正确的显示尺寸打开，
        // 而不是把 Retina 截图当成两倍大的图
        let ppm = (img.scale * 39.3701 * 72.0).round() as u32; // 每米像素数
        enc.set_pixel_dims(Some(png::PixelDimensions {
            xppu: ppm,
            yppu: ppm,
            unit: png::Unit::Meter,
        }));
        enc.write_header()?.write_image_data(&rgba)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_valid_png_with_correct_channel_order() {
        // 单像素纯红：BGRA = [0, 0, 255, 255]
        let img = CapturedImage { width: 1, height: 1, scale: 2.0, bgra: vec![0, 0, 255, 255] };
        let bytes = encode_png(&img).unwrap();
        assert_eq!(&bytes[1..4], b"PNG", "应为合法 PNG");

        let decoded = png::Decoder::new(std::io::Cursor::new(&bytes)).read_info().unwrap();
        let mut reader = decoded;
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..info.buffer_size()], &[255, 0, 0, 255], "BGRA 应重排为 RGBA");
    }
}
