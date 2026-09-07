//! 边缘探针：读出剪贴板里的图，报告四边最外几行/列的实际颜色。
//!
//! 用于判定「红边」到底是**烧进了像素**，还是只出现在屏幕上。
//!
//! 用法：先在 PinWall 里 F1 框选并 ⌘C，再运行
//!   cargo run -p pinwall-platform --example edge_probe

use pinwall_platform::read_clipboard_image;

/// 取 (x, y) 处像素，返回 (r, g, b, a)。缓冲区是 BGRA、行距紧凑。
fn px(bgra: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let i = ((y * w + x) * 4) as usize;
    (bgra[i + 2], bgra[i + 1], bgra[i], bgra[i + 3])
}

/// 判为「偏红」：红通道明显压过另两个通道。选区描边是 (255, 59, 48)。
fn reddish(p: (u8, u8, u8, u8)) -> bool {
    p.0 as i32 - p.1 as i32 > 40 && p.0 as i32 - p.2 as i32 > 40
}

fn main() {
    let Some(img) = read_clipboard_image() else {
        println!("剪贴板里没有图像。先在 PinWall 里 F1 框选并 ⌘C。");
        return;
    };
    let (w, h) = (img.width, img.height);
    println!("图像 {w}x{h} 像素，倍率 {}\n", img.scale);

    // 每条边看最外 3 行/列，够覆盖 1pt 线在二倍屏上的 2 像素
    let depth = 3.min(w.min(h));
    for d in 0..depth {
        let top: Vec<_> = (0..w).map(|x| px(&img.bgra, w, x, d)).collect();
        let bottom: Vec<_> = (0..w).map(|x| px(&img.bgra, w, x, h - 1 - d)).collect();
        let left: Vec<_> = (0..h).map(|y| px(&img.bgra, w, d, y)).collect();
        let right: Vec<_> = (0..h).map(|y| px(&img.bgra, w, w - 1 - d, y)).collect();

        for (name, line) in [("上", top), ("下", bottom), ("左", left), ("右", right)] {
            let n = line.iter().filter(|p| reddish(**p)).count();
            let pct = n as f64 / line.len() as f64 * 100.0;
            let sample = line[line.len() / 2];
            println!(
                "第 {} 层 {}边: 偏红 {:>5.1}%（{}/{}）  中点像素 rgba{:?}",
                d, name, pct, n, line.len(), sample
            );
        }
        println!();
    }

    let any = (0..depth).any(|d| {
        (0..w).any(|x| reddish(px(&img.bgra, w, x, d)) || reddish(px(&img.bgra, w, x, h - 1 - d)))
            || (0..h).any(|y| reddish(px(&img.bgra, w, d, y)) || reddish(px(&img.bgra, w, w - 1 - d, y)))
    });
    println!(
        "{}",
        if any {
            "结论：红边**在像素里**，是捕获时把遮罩描边一并抓进去了。"
        } else {
            "结论：像素干净，红边只存在于屏幕上（是遮罩本身，不在导出图里）。"
        }
    );
}
