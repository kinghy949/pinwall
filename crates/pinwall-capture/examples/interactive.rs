//! 交互式框选实测：显示遮罩 → 鼠标框选 → 捕获 → 存 PNG。
//!
//! 这是第一次把真实鼠标事件接进选区状态机。
//! 左键拖拽框选，右键取消。
//!
//! 运行： cargo run -p pinwall-capture --example interactive -- <输出目录>

use std::cell::RefCell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

use pinwall_capture::{capture_selection, current_capturer, permission_status, CapturedImage, Permission};
use pinwall_core::{Event, Outcome, Selection, SelectionMachine};
use pinwall_platform::{current_platform, OverlaySet, PointerEvent};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    if permission_status() == Permission::Denied {
        eprintln!("未获屏幕录制权限");
        return Ok(());
    }

    let mtm = MainThreadMarker::new().expect("须在主线程");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let platform = current_platform()?;
    let capturer = current_capturer()?;
    let screens = platform.screens()?;
    println!("显示器 {} 块，遮罩每屏一个", screens.len());

    let overlays = Rc::new(OverlaySet::covering_all_screens(platform.as_ref())?);
    let machine = Rc::new(RefCell::new(SelectionMachine::new(screens.clone())));
    // 框选结果。N 个遮罩共享同一状态机，故结果也只有一份。
    let result: Rc<RefCell<Option<Selection>>> = Rc::new(RefCell::new(None));
    let finished = Rc::new(std::cell::Cell::new(false));

    {
        let (machine, overlays, result, finished) =
            (machine.clone(), overlays.clone(), result.clone(), finished.clone());
        overlays.clone().set_pointer_handler(Rc::new(move |ev: PointerEvent| {
            let core_event = match ev {
                PointerEvent::Down(p) => Event::Down(p),
                PointerEvent::Moved(p) => Event::Move(p),
                PointerEvent::Up(p) => Event::Up(p),
                PointerEvent::Cancel => Event::Cancel,
            };
            let outcome = machine.borrow_mut().handle(core_event);
            match outcome {
                Outcome::Redraw => {
                    // 选区可能跨屏，必须广播给所有遮罩
                    let r = machine.borrow().current_rect();
                    overlays.set_selection(r);
                }
                Outcome::Committed(sel) => {
                    *result.borrow_mut() = Some(sel);
                    finished.set(true);
                }
                Outcome::Cancelled => {
                    println!("已取消");
                    finished.set(true);
                }
                Outcome::Idle => {}
            }
        }));
    }

    overlays.show();
    println!("\n请在屏幕上拖拽框选（右键取消）…");

    // 主线程事件泵，直到框选结束
    while !finished.get() {
        let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
        if let Some(e) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any, Some(&until), NSDefaultRunLoopMode, true)
        } {
            app.sendEvent(&e);
        }
    }
    overlays.hide();

    let Some(sel) = result.borrow().clone() else {
        return Ok(());
    };
    println!(
        "\n选区 ({:.0},{:.0}) {:.0}x{:.0} 逻辑点，跨屏={}",
        sel.rect.origin.x, sel.rect.origin.y,
        sel.rect.size.width, sel.rect.size.height, sel.is_cross_screen()
    );
    for p in &sel.parts {
        let (pw, ph) = p.pixel_size();
        println!("  屏[{}] {:.0}x{:.0} 逻辑点 -> {pw}x{ph} 像素", p.screen_id.0, p.rect.size.width, p.rect.size.height);
    }

    let img = capture_selection(capturer.as_ref(), &sel)?;
    let path = format!("{out_dir}/selection.png");
    write_png(&img, &path)?;
    println!("\n已保存 {path}  ({}x{} 像素)", img.width, img.height);
    Ok(())
}

fn write_png(img: &CapturedImage, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut rgba = Vec::with_capacity(img.bgra.len());
    for p in img.bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    let f = std::fs::File::create(path)?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), img.width, img.height);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&rgba)?;
    Ok(())
}
