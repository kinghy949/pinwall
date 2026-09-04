//! 系统「存储为」对话框（macOS）。
//!
//! # 为什么不能只有「直接存到桌面」
//!
//! 无提示地把文件丢进固定目录，从用户视角看与「什么都没发生」无法区分 ——
//! 按下 ⌘S 没有任何反馈，第一反应是快捷键坏了，而不是去桌面找。

use std::path::PathBuf;

use objc2_app_kit::{NSApplication, NSModalResponse, NSSavePanel};
use objc2_foundation::{MainThreadMarker, NSString};

/// 保存面板的窗口层级。
///
/// 必须高过贴图：贴图挂在 `NSScreenSaverWindowLevel`（1000），普通对话框
/// 会被自己的贴图整个盖住 —— 用户只会看到界面卡住，找不到那个对话框。
const SAVE_PANEL_LEVEL: isize = 1002;

/// `NSModalResponseOK`。用户点了「存储」。
const RESPONSE_OK: NSModalResponse = 1;

/// 弹出系统保存对话框，返回用户选定的路径。取消则返回 `None`。
///
/// **阻塞直到用户关闭对话框。** 期间由 AppKit 跑自己的模态事件循环，
/// 调用方的主循环暂停，全局热键会积压在通道里，等返回后一并消费。
pub fn ask_save_path(suggested_name: &str) -> Option<PathBuf> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(suggested_name));
    panel.setLevel(SAVE_PANEL_LEVEL);

    // 对话框要接收键盘输入，本应用就必须是活跃应用。与 `pin_view` 中取焦点
    // 同理，用 activateIgnoringOtherApps 而非 macOS 14 才有的 activate()。
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);

    if panel.runModal() != RESPONSE_OK {
        return None;
    }
    let url = panel.URL()?;
    let path = url.path()?;
    Some(PathBuf::from(path.to_string()))
}
