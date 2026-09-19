//! 运行日志：内存环形缓冲 + stderr 双写。
//!
//! 设置页「诊断」卡片可查看/复制，免去「出问题要从终端启动看 stderr」的门槛
//! （终端启动的可见性保留，两条路都通）。

use std::collections::VecDeque;
use std::sync::Mutex;

/// 最近 CAP 行。够定位问题，也不至于无界增长。
static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const CAP: usize = 300;

/// 记一行日志（带本地时间戳），同时打到 stderr。
pub fn log(msg: impl AsRef<str>) {
    let line = format!("[{}] {}", chrono::Local::now().format("%H:%M:%S%.3f"), msg.as_ref());
    eprintln!("{line}");
    if let Ok(mut buf) = LINES.lock() {
        if buf.len() >= CAP {
            buf.pop_front();
        }
        buf.push_back(line);
    }
}

/// 设置页拉取日志（旧→新）。
#[tauri::command]
pub fn get_logs() -> Vec<String> {
    LINES.lock().map(|b| b.iter().cloned().collect()).unwrap_or_default()
}
