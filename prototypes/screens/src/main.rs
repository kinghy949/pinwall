//! 枚举所有显示器及其 backingScaleFactor，用于 R4（多显示器混合 DPI）验证。
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

fn main() {
    let mtm = MainThreadMarker::new().expect("须主线程");
    let screens = NSScreen::screens(mtm);
    println!("检测到 {} 块显示器\n", screens.len());
    for (i, s) in screens.iter().enumerate() {
        let f = s.frame();
        let vf = s.visibleFrame();
        let scale = s.backingScaleFactor();
        let name = unsafe { s.localizedName() };
        println!("[{i}] {name}");
        println!("    frame(点)      : x={:.0} y={:.0} {:.0}x{:.0}", f.origin.x, f.origin.y, f.size.width, f.size.height);
        println!("    实际像素        : {:.0}x{:.0}", f.size.width * scale, f.size.height * scale);
        println!("    backingScale    : {scale}");
        println!("    visibleFrame(点): x={:.0} y={:.0} {:.0}x{:.0}", vf.origin.x, vf.origin.y, vf.size.width, vf.size.height);
        println!();
    }
    let scales: Vec<f64> = screens.iter().map(|s| s.backingScaleFactor()).collect();
    let mixed = scales.windows(2).any(|w| w[0] != w[1]);
    println!("是否混合 DPI: {}", if mixed { "是 —— 可直接验证 R4" } else { "否 —— 各屏缩放相同，需先把某块屏改成不同缩放才能验证混合 DPI" });
}
