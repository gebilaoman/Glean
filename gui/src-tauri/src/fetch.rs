//! 取词层：跨应用拿选中文本。
//!
//! 两条路线，自己实现（替代 get-selected-text crate）：
//! 1. **AX 直查**（毫秒级）：systemWide → 焦点元素 → kAXSelectedTextAttribute。
//!    TextEdit/Xcode/Safari 等原生或规矩的应用都给。
//! 2. **模拟 Cmd+C 兜底**：应用不暴露 AX（Chrome/Electron 常见）时，备份剪贴板、
//!    合成 Cmd+C、**轮询剪贴板最长 600ms**、恢复剪贴板。
//!    替代原因是原 crate 的兜底起 osascript（300ms+ 固定开销）且只等 100ms——
//!    慢一拍的应用（浏览器/Electron）100ms 内没完成复制，稳定返回空。

use std::thread;
use std::time::{Duration, Instant};

/// 取选中文本。`Ok("")` = 两条路线都没拿到（当前确实没有选区）。
pub fn fetch_selected_text() -> Result<String, String> {
    let ax = fetch_by_ax();
    crate::diag::log(format!(
        "fetch: AX={:?}",
        ax.as_ref().map(|t| t.chars().count())
    ));
    if let Some(text) = ax
        && !text.is_empty()
    {
        return Ok(text);
    }
    // AX 拿不到或返回空串都走兜底：Safari 的网页区会「成功返回空串」但
    // 实际有选区（截图实锤），空串不可信。
    fetch_by_copy_simulation()
}

/// 当前焦点应用名（诊断日志用，拿不到就空串）。
pub fn frontmost_app() -> String {
    active_win_pos_rs::get_active_window()
        .map(|w| w.app_name)
        .unwrap_or_default()
}

/// AX 直查。`None` = 该应用不暴露选区（走兜底）；`Some` = 查到了。
#[cfg(target_os = "macos")]
fn fetch_by_ax() -> Option<String> {
    use accessibility_ng::{AXAttribute, AXUIElement};

    let system = AXUIElement::system_wide();
    // 返回类型直接由 AXAttribute 的泛型参数定，干净利落
    let Ok(focused) = system.attribute(&AXAttribute::focused_uielement()) else {
        return None;
    };
    let Ok(text) = focused.attribute(&AXAttribute::selected_text()) else {
        return None;
    };
    Some(text.to_string())
}

#[cfg(not(target_os = "macos"))]
fn fetch_by_ax() -> Option<String> {
    None
}

/// 模拟 Cmd+C 兜底。剪贴板必须原样归还（用户可能存着重要内容）。
fn fetch_by_copy_simulation() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let backup = clipboard.get_text().ok();

    post_cmd_c();

    // 轮询等复制真正落进剪贴板：慢应用（Electron）要几百毫秒。
    let deadline = Instant::now() + Duration::from_millis(600);
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(40));
        match clipboard.get_text() {
            Ok(cur) if Some(&cur) != backup.as_ref() => {
                // 拿到了：先归还用户剪贴板再返回
                if let Some(b) = backup {
                    let _ = clipboard.set_text(b);
                }
                return Ok(cur);
            }
            _ => continue,
        }
    }
    // 超时：没等到新内容，剪贴板没动过，无需恢复
    crate::diag::log("fetch: 兜底超时（600ms 无变化）");
    Ok(String::new())
}

/// 合成一次 Cmd+C（HID 级注入，走当前焦点应用）。
#[cfg(target_os = "macos")]
fn post_cmd_c() {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    const KEY_C: u16 = 8; // kVK_ANSI_C
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else {
        return;
    };
    let Ok(down) = CGEvent::new_keyboard_event(source.clone(), KEY_C, true) else {
        return;
    };
    let Ok(up) = CGEvent::new_keyboard_event(source, KEY_C, false) else {
        return;
    };
    for ev in [&down, &up] {
        ev.set_flags(CGEventFlags::CGEventFlagCommand);
    }
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
}

#[cfg(not(target_os = "macos"))]
fn post_cmd_c() {}
