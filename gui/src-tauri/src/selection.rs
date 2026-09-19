//! 触发层 + 取词层。
//!
//! 系统里没有「划词事件」，得自己合成：鼠标按下记起点，松开时比位移，超阈值判为拖选；
//! 另外识别双击选词。判定成立才去取词。
//!
//! **为什么不用 rdev**：rdev 0.5.3 的事件掩码写死了，把键盘事件也一并订阅，
//! 而它的 `convert()` 会调 HIToolbox 的 `TSMGetInputSourceProperty` 去查键盘布局——
//! 那个 API 断言必须跑在主队列上，而事件 tap 在自己的线程上，于是**只要在划词工具
//! 开着的时候敲一下键盘，进程就 SIGTRAP 崩溃**。掩码不可配，只能自己建 tap：
//! 这里直接用 CoreGraphics 开一个只收左键按下/松开的 ListenOnly tap。
//!
//! 线程结构——**tap 回调必须保持零工作量**。回调跑在系统的 run loop 上，稍慢就会被
//! 以超时为由停用 tap，停用窗口内的鼠标事件全部丢失，并且手势状态会留下过期的按下，
//! 与后续无关的松开配成「幽灵拖选」（曾导致划词时灵时不灵、日志里出现位移上千像素
//! 的假触发）：
//! - tap 线程：回调只把 (事件类型, 坐标) 塞进 channel 立即返回；
//! - 手势 worker：消费原始事件，配对、判定、收起面板等全部逻辑都在这条线程；
//! - 取词 worker：消费判定出的触发点，带 3s 超时地取词、弹窗；
//! - 监护线程：权限就绪时拉起 tap，权限被收回则自动重启。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::actions;
use crate::diag;
use crate::panel;
use crate::state::AppState;

/// 事件 tap 是否在运行。监护线程据此决定要不要把它拉起来。
static HOOK_UP: AtomicBool = AtomicBool::new(false);

/// 双击判定的时间窗。
const DOUBLE_CLICK_MS: u128 = 400;
/// 双击两次落点允许的最大偏移（逻辑像素）。
const DOUBLE_CLICK_SLOP: f64 = 6.0;
/// 按下超过这么久的配对作废：中间大概率发生过事件丢失（tap 被系统暂停等），
/// 此时的位移没有意义，不能当成拖选。
const STALE_PRESS: Duration = Duration::from_secs(2);

/// 推给前端的划词结果。
#[derive(Debug, Clone, Serialize)]
pub struct SelectionPayload {
    pub text: String,
}

/// tap 回调转发的最小事件。
#[derive(Debug)]
enum RawEvent {
    Down { x: f64, y: f64 },
    Up { x: f64, y: f64 },
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

/// 启动各 worker 与 tap 监护。调用一次。
///
/// 权限经常是「后到」的：用户装完应用才去系统设置里授权，而 tap 只能在有权限时
/// 创建。监护线程每 2 秒看一眼，权限一到位（或中途被收回又恢复）就自动拉起 tap，
/// 无需重启应用。
pub fn spawn(app: AppHandle) {
    let (raw_tx, raw_rx) = channel::<RawEvent>();
    let (trig_tx, trig_rx) = channel::<Trigger>();

    // 手势 worker：事件配对与判定
    {
        let app = app.clone();
        thread::spawn(move || {
            let mut g = Gesture::default();
            for ev in raw_rx {
                match ev {
                    RawEvent::Down { x, y } => on_press(&app, &mut g, x, y),
                    RawEvent::Up { x, y } => on_release(&app, &mut g, &trig_tx, x, y),
                }
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

    #[cfg(target_os = "macos")]
    {
        let tx = raw_tx;
        thread::spawn(move || loop {
            let trusted = actions::accessibility_trusted();
            if !HOOK_UP.load(Ordering::SeqCst) && trusted {
                diag::log("权限就绪，拉起鼠标监听");
                HOOK_UP.store(true, Ordering::SeqCst);
                let thread_app = app.clone();
                let tx = tx.clone();
                let emit_app = app.clone();
                thread::spawn(move || match run_hook(thread_app, tx) {
                    Ok(()) => diag::log("run_hook 线程退出（run loop 结束）"),
                    Err(e) => {
                        // 权限被收回或系统拒绝：放回"未运行"，监护线程稍后重试
                        HOOK_UP.store(false, Ordering::SeqCst);
                        diag::log(format!("全局鼠标监听启动失败：{e}"));
                        let _ = emit_app.emit("hook-error", e);
                    }
                });
            }
            thread::sleep(Duration::from_secs(2));
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        let tx = raw_tx;
        thread::spawn(move || {
            if let Err(e) = run_hook(app, tx) {
                diag::log(format!("全局鼠标监听启动失败：{e}"));
                let _ = app.emit("hook-error", e);
            }
        });
    }
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

    // 过期配对：按下与松开隔了太久，说明中间丢过事件（tap 被暂停、系统休眠等）。
    // 这时的位移是两个不相干动作的距离，绝不能当成拖选。
    if pt.elapsed() > STALE_PRESS {
        diag::log(format!("mouse-up 与按下间隔 {pt:?}，配对作废，按普通点击处理"));
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

#[cfg(target_os = "macos")]
fn run_hook(app: AppHandle, raw_tx: Sender<RawEvent>) -> Result<(), String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use core_foundation::base::TCFType;
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    };

    // tap 被系统停掉时要重新打开，但回调是在 tap 建好之前就写好的，
    // 所以先留个空位，建好之后再填进去。
    let port: Rc<RefCell<Option<core_foundation::mach_port::CFMachPort>>> =
        Rc::new(RefCell::new(None));
    let port_for_cb = port.clone();

    let tap = CGEventTap::new(
        // Session 级：拿得到本用户会话里所有应用的事件。
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        // ListenOnly：只旁听，不改也不吞事件。
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::LeftMouseDown, CGEventType::LeftMouseUp],
        move |_proxy, kind, event| {
            match kind {
                CGEventType::LeftMouseDown => {
                    let p = event.location();
                    let _ = raw_tx.send(RawEvent::Down { x: p.x, y: p.y });
                }
                CGEventType::LeftMouseUp => {
                    let p = event.location();
                    let _ = raw_tx.send(RawEvent::Up { x: p.x, y: p.y });
                }
                // 回调超时或用户输入过快时系统会停掉 tap，必须自己重新打开，
                // 否则划词会毫无征兆地彻底失灵。
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                    if let Some(port) = port_for_cb.borrow().as_ref() {
                        unsafe { CGEventTapEnable(port.as_concrete_TypeRef(), true) };
                    }
                    diag::log("事件监听被系统暂停，已重新启用");
                }
                _ => {}
            }
            // ListenOnly 下返回值会被忽略，照约定返回 None。
            None
        },
    )
    .map_err(|_| "创建事件监听失败，通常是没给辅助功能权限".to_string())?;

    *port.borrow_mut() = Some(tap.mach_port.clone());

    let source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|_| "创建 run loop source 失败".to_string())?;
    // tap 线程有自己的 run loop，这里把 source 挂上去然后阻塞在 run 里。
    unsafe { CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes) };
    tap.enable();
    CFRunLoop::run_current();
    Ok(())
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: core_foundation::mach_port::CFMachPortRef, enable: bool);
}

/// 其它平台暂未实现全局监听（Windows 要 UIA + 低级鼠标钩子，
/// Wayland 干脆禁止全局输入监听）。
#[cfg(not(target_os = "macos"))]
fn run_hook(_app: AppHandle, _raw_tx: Sender<RawEvent>) -> Result<(), String> {
    Err("当前平台尚不支持自动划词".to_string())
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
