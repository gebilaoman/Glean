//! 触发层 + 取词层。
//!
//! 系统里没有「划词事件」，得自己合成：全局鼠标钩子记 mousedown 起点，mouseup 时
//! 比位移，超阈值判为拖选；另外识别双击选词。判定成立才去调 `get_selected_text()`。
//!
//! 两个线程：
//! - 钩子线程跑 `rdev::listen`，回调里**只做轻量判定**，然后往 channel 里丢一个触发点。
//!   （回调跑在 CGEventTap 的 run loop 上，在里面做取词这种耗时活会把 tap 拖到被系统禁用。）
//! - worker 线程收触发点，等选区稳定后取词、定位、弹窗。

use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use rdev::{Button, EventType};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::cursor;
use crate::panel;
use crate::state::AppState;

/// 双击判定的时间窗。
const DOUBLE_CLICK_MS: u128 = 400;
/// 双击两次落点允许的最大偏移（逻辑像素）。
const DOUBLE_CLICK_SLOP: f64 = 6.0;

/// 推给前端的划词结果。
#[derive(Debug, Clone, Serialize)]
pub struct SelectionPayload {
    pub text: String,
}

/// 钩子线程丢给 worker 的触发点（全局逻辑坐标）。
struct Trigger {
    x: f64,
    y: f64,
}

/// 启动钩子线程与 worker 线程。调用一次。
pub fn spawn(app: AppHandle) {
    let (tx, rx) = channel::<Trigger>();

    {
        let app = app.clone();
        thread::spawn(move || {
            for trigger in rx {
                handle_trigger(&app, trigger);
            }
        });
    }

    thread::spawn(move || {
        if let Err(e) = run_hook(app.clone(), tx) {
            eprintln!("[glean] 全局鼠标钩子启动失败：{e}");
            // 绝大多数情况是 macOS 没给辅助功能权限，让设置页能提示用户。
            let _ = app.emit("hook-error", format!("{e}"));
        }
    });
}

fn run_hook(app: AppHandle, tx: Sender<Trigger>) -> Result<(), String> {
    // rdev 的 ButtonPress/Release 不带坐标，而且拖拽期间它压根不发 MouseMove
    // （见 cursor.rs），所以按下/松开时都向系统现问一次位置，
    // `tracked` 只是拿不到时的兜底。
    let mut tracked = (0.0_f64, 0.0_f64);
    let mut press: Option<(f64, f64, Instant)> = None;
    let mut press_inside_panel = false;
    let mut last_click: Option<(f64, f64, Instant)> = None;

    rdev::listen(move |event| {
        let state = app.state::<AppState>();
        match event.event_type {
            EventType::MouseMove { x, y } => {
                tracked = (x, y);
            }
            EventType::ButtonPress(Button::Left) => {
                let (x, y) = cursor::position().unwrap_or(tracked);
                // 点在自己面板上：不当作划词起点，也不收起面板。
                press_inside_panel = state
                    .geometry
                    .rect()
                    .map(|r| r.contains(x, y))
                    .unwrap_or(false);
                if !press_inside_panel && state.geometry.rect().is_some() {
                    let _ = app.emit("panel-dismiss", ());
                    panel::hide(&app);
                }
                press = Some((x, y, Instant::now()));
            }
            EventType::ButtonRelease(Button::Left) => {
                let (x, y) = cursor::position().unwrap_or(tracked);
                let Some((px, py, _)) = press.take() else {
                    return;
                };
                if press_inside_panel {
                    press_inside_panel = false;
                    return;
                }

                let moved = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
                let threshold = state.config.read().drag_threshold;

                if moved >= threshold {
                    // 拖选
                    last_click = None;
                    let _ = tx.send(Trigger { x, y });
                    return;
                }

                // 位移不够 → 可能是双击选词
                let now = Instant::now();
                let is_double = state.config.read().double_click_trigger
                    && last_click
                        .map(|(lx, ly, t)| {
                            now.duration_since(t).as_millis() <= DOUBLE_CLICK_MS
                                && ((x - lx).powi(2) + (y - ly).powi(2)).sqrt() <= DOUBLE_CLICK_SLOP
                        })
                        .unwrap_or(false);
                if is_double {
                    last_click = None;
                    let _ = tx.send(Trigger { x, y });
                } else {
                    last_click = Some((x, y, now));
                }
            }
            _ => {}
        }
    })
    .map_err(|e| format!("{e:?}"))
}

fn handle_trigger(app: &AppHandle, trigger: Trigger) {
    let state = app.state::<AppState>();
    let settle = state.config.read().settle_ms;
    // 等一下再取词：选区在 mouseup 后才落定，取早了会读到上一次的内容。
    thread::sleep(Duration::from_millis(settle));

    let text = match get_selected_text::get_selected_text() {
        Ok(t) => t,
        Err(e) => {
            // 取不到很常见（自绘 UI、没选中、权限不足），不打扰用户，只记日志。
            eprintln!("[glean] 取词失败：{e}");
            return;
        }
    };
    let text = text.trim().to_string();
    // 空选区必须丢弃，否则普通点击也会让工具栏乱闪。
    if text.is_empty() {
        return;
    }

    state.set_selection(text.clone());

    let _ = app.emit("selection", SelectionPayload { text });
    panel::show_at(app, trigger.x, trigger.y);
}
