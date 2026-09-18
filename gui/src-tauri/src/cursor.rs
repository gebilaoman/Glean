//! 取当前光标位置（全局逻辑坐标，左上角为原点）。
//!
//! 为什么不直接用 rdev 的 `MouseMove`：rdev 0.5.3 在 macOS 上只把 `kCGEventMouseMoved`
//! 转成 `MouseMove`，**`kCGEventLeftMouseDragged` 被丢掉了**。也就是说按住左键拖动的
//! 整个过程 rdev 一个坐标都不给，拿它算拖选位移永远是 0，划词根本不会触发。
//! 所以按下/松开时直接向系统要一次当前位置。

/// macOS：造一个空 CGEvent，它的 location 就是当前光标位置（points，左上原点）。
#[cfg(target_os = "macos")]
pub fn position() -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let event = CGEvent::new(source).ok()?;
    let p = event.location();
    Some((p.x, p.y))
}

/// 其它平台暂时拿不到，调用方回落到 rdev 的 `MouseMove` 跟踪值。
#[cfg(not(target_os = "macos"))]
pub fn position() -> Option<(f64, f64)> {
    None
}
