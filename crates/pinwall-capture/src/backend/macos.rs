//! macOS 捕获后端，基于 ScreenCaptureKit。
//!
//! 使用 `SCScreenshotManager::captureImageInRect_completionHandler` ——
//! 相比 `SCStream`，它不需要构造 content filter，适合单帧截图。
//!
//! 该 API 是异步回调式的，本模块在内部桥接为同步调用：
//! 回调中就地把 `CGImage` 转成 BGRA 字节（`CGImage` 不是 `Send`，
//! 不能跨线程传递），再经 channel 送回，等待期间泵主线程 run loop
//! 以免回调需要主线程时死锁。

use std::ffi::c_void;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::SCScreenshotManager;
use pinwall_platform::geom::Rect;

use crate::{CapturedImage, Capturer, Error, Permission, Result};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn permission_status() -> Permission {
    if CGPreflightScreenCaptureAccess() {
        Permission::Granted
    } else {
        Permission::Denied
    }
}

pub fn request_permission() -> bool {
    CGRequestScreenCaptureAccess()
}

pub struct MacCapturer;

impl Capturer for MacCapturer {
    fn capture_rect(&self, rect: Rect, scale: f64) -> Result<CapturedImage> {
        if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
            return Err(Error::EmptyRect(rect));
        }
        if permission_status() != Permission::Granted {
            return Err(Error::PermissionDenied);
        }

        // CoreGraphics 全局显示坐标与本项目约定一致（主屏左上角原点、y 向下），
        // 故此处无需坐标换算。
        let cg_rect = objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(rect.origin.x, rect.origin.y),
            objc2_core_foundation::CGSize::new(rect.size.width, rect.size.height),
        );

        let (tx, rx) = mpsc::channel::<std::result::Result<(u32, u32, Vec<u8>), String>>();

        let handler = RcBlock::new(move |image: *mut CGImage, err: *mut NSError| {
            if !err.is_null() {
                let msg = unsafe { &*err }.localizedDescription().to_string();
                let _ = tx.send(Err(msg));
                return;
            }
            if image.is_null() {
                let _ = tx.send(Err("回调返回空图像且无错误信息".into()));
                return;
            }
            // CGImage 非 Send，必须在回调内就地转成字节
            let result = unsafe { cgimage_to_bgra(&*image) };
            let _ = tx.send(result);
        });

        unsafe {
            SCScreenshotManager::captureImageInRect_completionHandler(cg_rect, Some(&handler));
        }

        let (width, height, bgra) = wait_pumping(&rx, CAPTURE_TIMEOUT)?
            .map_err(Error::CaptureFailed)?;

        Ok(CapturedImage { width, height, scale, bgra })
    }
}

/// 等待回调结果，同时泵 run loop。
///
/// 直接 `recv()` 阻塞会在「回调需要主线程」时死锁，故边等边泵。
fn wait_pumping<T>(rx: &mpsc::Receiver<T>, timeout: Duration) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(v) => return Ok(v),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(Error::CaptureFailed("回调未产生结果即被释放".into()));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout(timeout));
        }
        // SAFETY: kCFRunLoopDefaultMode 是 CoreFoundation 提供的不可变全局常量
        let mode = unsafe { kCFRunLoopDefaultMode };
        CFRunLoop::run_in_mode(mode, 0.005, true);
    }
}

// CGBitmapContextCreate 未被 objc2-core-graphics 0.3 导出，此处自行声明。
unsafe extern "C-unwind" {
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *const CGColorSpace,
        bitmap_info: u32,
    ) -> *mut CGContext;
    fn CGColorSpaceCreateDeviceRGB() -> *mut CGColorSpace;
    fn CGColorSpaceRelease(space: *mut CGColorSpace);
    fn CGContextRelease(ctx: *mut CGContext);
}

/// 把 `CGImage` 归一化为 BGRA8。
///
/// 不直接读 `CGImageGetDataProvider` 的原始字节，是因为源图的像素格式、
/// 行距、色彩空间都不保证 —— 绘制到自建的位图上下文可一次性归一化。
unsafe fn cgimage_to_bgra(image: &CGImage) -> std::result::Result<(u32, u32, Vec<u8>), String> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    if width == 0 || height == 0 {
        return Err(format!("图像尺寸非法: {width}x{height}"));
    }

    let bytes_per_row = width * 4;
    let mut buf = vec![0u8; bytes_per_row * height];

    // BGRA8 预乘 alpha、小端序 —— 与 wgpu 的 Bgra8Unorm 直接对应
    let bitmap_info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;

    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        if space.is_null() {
            return Err("创建色彩空间失败".into());
        }
        let ctx = CGBitmapContextCreate(
            buf.as_mut_ptr() as *mut c_void,
            width,
            height,
            8,
            bytes_per_row,
            space,
            bitmap_info,
        );
        CGColorSpaceRelease(space);
        if ctx.is_null() {
            return Err("创建位图上下文失败".into());
        }
        let rect = objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(0.0, 0.0),
            objc2_core_foundation::CGSize::new(width as f64, height as f64),
        );
        CGContext::draw_image(Some(&*ctx), rect, Some(image));
        CGContextRelease(ctx);
    }

    Ok((width as u32, height as u32, buf))
}
