//! 触发层 + 取词层。
//!
//! 系统里没有「划词事件」，得自己合成：按下记起点，松开时比位移，超阈值判为拖选；
//! 另外识别双击选词。判定成立才去取词。
//!
//! **为什么是轮询，而不是事件回调**——前两版踩遍了 macOS 事件机制：
//! 1. `rdev`（CGEventTap 封装）：事件掩码写死连键盘一起订阅，其内部调用的
//!    HIToolbox API 断言必须在主队列上跑，工具开着时敲一下键盘就 SIGTRAP；
//!    且 macOS 下它不转发拖拽坐标，拖选位移恒为 0。
//! 2. 自建 CGEventTap（只订阅鼠标）：macOS 会以「回调超时」为由频繁停用 tap
//!    （实测 9 分钟 13 次，回调已归零也照停），停用窗口内的鼠标事件全部丢失，
//!    划词时灵时不灵。
//! 3. `NSEvent` 全局监听：实测这台系统上只送 mouse-down、不送 mouse-up，
//!    拖选永远配不上对。
//!
//! 轮询（`CGEventSourceButtonState` + 光标位置）是无状态查询：系统没有机制
//! 停用它，也不存在「丢事件」——按键状态翻转必然被某次采样看到。25ms 的
//! 采样间隔就是检测延迟上限，人手操作完全无感。副作用：轮询能看到自己面板上
//! 的点击，所以按下时要做面板内命中测试（事件回调时代的 press_inside_panel 回来了）。
//!
//! ⚠️ 测试注意：`CGEventSourceButtonState` **不反映合成事件**（cliclick /
//! CGEventPost 注入的点击看不见），但它对真实硬件鼠标完全可靠（用户实测）。
//! 自动化测试只能覆盖「事件转发 → AX 取词 → 面板弹出」的后半段，手势判定
//! 这一段必须用真实鼠标验证。

use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::diag;
use crate::panel;
use crate::state::AppState;

/// 鼠标采样间隔，也是手势检测延迟的上限。
const POLL_MS: u64 = 25;
/// 双击判定的时间窗。
const DOUBLE_CLICK_MS: u128 = 400;
/// 双击两次落点允许的最大偏移（逻辑像素）。
const DOUBLE_CLICK_SLOP: f64 = 6.0;
/// 按下超过这么久的配对作废（休眠恢复等场景下时钟跳跃的保险丝）。
const STALE_PRESS: Duration = Duration::from_secs(60);

/// 推给前端的划词结果。
#[derive(Debug, Clone, Serialize)]
pub struct SelectionPayload {
    pub text: String,
}

/// 手势 worker 丢给取词 worker 的触发点（全局逻辑坐标）。
struct Trigger {
    x: f64,
    y: f64,
}

/// 手势判定的可变状态。只在手势 worker 线程上被访问，不存在并发。
#[derive(Default)]
struct Gesture {
    /// 本次按下的起点与时刻。
    press: Option<(f64, f64, Instant)>,
    /// 这一次按下是不是落在自己的面板上（是的话既不收起也不触发）。
    press_inside_panel: bool,
    /// 上一次「单击」的落点与时刻，用来判双击。
    last_click: Option<(f64, f64, Instant)>,
}

/// 启动手势 worker 与取词 worker。调用一次。
pub fn spawn(app: AppHandle) {
    let (trig_tx, trig_rx) = channel::<Trigger>();

    // 手势 worker：轮询鼠标状态，配对按下/松开。
    //
    // 权限未就绪时也照常轮询——手势判定本身不需要辅助功能权限，只是取词会
    // 失败；权限一到位取词立刻就能工作，无需重启。
    {
        let app = app.clone();
        thread::spawn(move || {
            let mut g = Gesture::default();
            let mut was_down = false;
            // 事件源只建一次：CGEventSource::new 每次都要连 WindowServer，
            // 放进循环里会把采样周期拖长到几百毫秒（实测教训）。
            let source = mouse_source();
            loop {
                let iter_start = Instant::now();
                thread::sleep(Duration::from_millis(POLL_MS));
                let down = mouse_left_down();
                let Some((x, y)) = cursor_position(&source) else { continue };
                let cost = iter_start.elapsed();
                if cost > Duration::from_millis(100) {
                    diag::log(format!("采样周期异常：{cost:?}"));
                }
                // 探针：每秒打一次按钮状态与光标
                match (was_down, down) {
                    (false, true) => on_press(&app, &mut g, x, y),
                    (true, false) => on_release(&app, &mut g, &trig_tx, x, y),
                    _ => continue,
                }
                was_down = down;
            }
        });
    }

    // 取词 worker：一次触发一个线程地跑，限时放弃（取词可能长时间卡在
    // 无响应的应用上，不能堵死后续触发）。
    {
        let app = app.clone();
        thread::spawn(move || {
            for trigger in trig_rx {
                let app = app.clone();
                let (done_tx, done_rx) = channel::<()>();
                thread::spawn(move || {
                    handle_trigger(&app, trigger);
                    let _ = done_tx.send(());
                });
                if done_rx.recv_timeout(Duration::from_secs(3)).is_err() {
                    diag::log("取词超时（3s），放弃本次触发");
                }
            }
        });
    }
}

fn mouse_source() -> core_graphics::event_source::CGEventSource {
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .expect("创建 CGEventSource 失败")
}

/// 左键当前是否按下（CombinedSession 状态，含合成事件）。
#[cfg(target_os = "macos")]
fn mouse_left_down() -> bool {
    // kCGEventSourceStateCombinedSessionState = 1，kCGMouseButtonLeft = 0
    unsafe { CGEventSourceButtonState(1, 0) }
}

/// 当前光标位置（全局逻辑坐标，左上原点）。造一个空事件问一次位置。
#[cfg(target_os = "macos")]
fn cursor_position(source: &core_graphics::event_source::CGEventSource) -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;

    let event = CGEvent::new(source.clone()).ok()?;
    let p = event.location();
    Some((p.x, p.y))
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceButtonState(state_id: u32, button: u32) -> bool;
}

/// 按下：记起点；如果面板开着而且没点在面板上，就先收起它。
fn on_press(app: &AppHandle, g: &mut Gesture, x: f64, y: f64) {
    diag::log(format!("mouse-down ({x:.0},{y:.0})"));
    let state = app.state::<AppState>();
    let rect = state.geometry.rect();
    g.press_inside_panel = rect.map(|r| r.contains(x, y)).unwrap_or(false);
    if !g.press_inside_panel && rect.is_some() {
        let _ = app.emit("panel-dismiss", ());
        panel::hide(app);
    }
    g.press = Some((x, y, Instant::now()));
}

/// 松开：位移够大判拖选；不够大就看是不是双击。
fn on_release(app: &AppHandle, g: &mut Gesture, tx: &Sender<Trigger>, x: f64, y: f64) {
    let Some((px, py, pt)) = g.press.take() else {
        return;
    };
    if g.press_inside_panel {
        g.press_inside_panel = false;
        return;
    }

    // 保险丝：正常手势绝不可能超过这么久（休眠恢复等时钟异常时兜底）。
    if pt.elapsed() > STALE_PRESS {
        diag::log("mouse-up 与按下间隔异常，配对作废，按普通点击处理");
        g.last_click = Some((x, y, Instant::now()));
        return;
    }

    let state = app.state::<AppState>();
    let moved = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
    if moved >= state.config.read().drag_threshold {
        g.last_click = None;
        diag::log(format!("拖选成立（位移 {moved:.0}px），发送触发"));
        let _ = tx.send(Trigger { x, y });
        return;
    }
    diag::log(format!("位移不足（{moved:.0}px）"));

    let now = Instant::now();
    let is_double = state.config.read().double_click_trigger
        && g.last_click
            .map(|(lx, ly, t)| {
                now.duration_since(t).as_millis() <= DOUBLE_CLICK_MS
                    && ((x - lx).powi(2) + (y - ly).powi(2)).sqrt() <= DOUBLE_CLICK_SLOP
            })
            .unwrap_or(false);
    if is_double {
        g.last_click = None;
        let _ = tx.send(Trigger { x, y });
    } else {
        g.last_click = Some((x, y, now));
    }
}

fn handle_trigger(app: &AppHandle, trigger: Trigger) {
    let state = app.state::<AppState>();
    let settle = state.config.read().settle_ms;
    // 等一下再取词：选区在 mouseup 后才落定，取早了会读到上一次的内容。
    thread::sleep(Duration::from_millis(settle));

    let started = std::time::Instant::now();
    let frontmost = crate::fetch::frontmost_app();
    let text = match crate::fetch::fetch_selected_text() {
        Ok(t) => t,
        Err(e) => {
            // 取不到很常见（自绘 UI、没选中、权限不足），不打扰用户，只记日志。
            diag::log(format!("取词失败({:?})：{e}", started.elapsed()));
            return;
        }
    };
    let text = text.trim().to_string();
    diag::log(format!(
        "触发({:.0},{:.0}) [{}] 取词耗时 {:?}，{} 字符",
        trigger.x,
        trigger.y,
        if frontmost.is_empty() { "?" } else { &frontmost },
        started.elapsed(),
        text.chars().count()
    ));
    // 空选区必须丢弃，否则普通点击也会让工具栏乱闪。
    if text.is_empty() {
        diag::log("空文本，丢弃");
        return;
    }

    state.set_selection(text.clone());
    // 划了新词就别继续念旧的了
    if state.stop_speech() {
        let _ = app.emit("tts-stopped", ());
    }

    let _ = app.emit("selection", SelectionPayload { text });
    panel::show_at(app, trigger.x, trigger.y);
}
