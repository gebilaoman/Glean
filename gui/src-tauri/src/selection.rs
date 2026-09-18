//! 触发层 + 取词层。
//!
//! 系统里没有「划词事件」，得自己合成：鼠标按下记起点，松开时比位移，超阈值判为拖选；
//! 另外识别双击选词。判定成立才去调 `get_selected_text()`。
//!
//! **为什么不用 rdev**：rdev 0.5.3 的事件掩码写死了，把键盘事件也一并订阅，
//! 而它的 `convert()` 会调 HIToolbox 的 `TSMGetInputSourceProperty` 去查键盘布局——
//! 那个 API 断言必须跑在主队列上，而事件 tap 在自己的线程上，于是**只要在划词工具
//! 开着的时候敲一下键盘，进程就 SIGTRAP 崩溃**。掩码不可配，只能自己建 tap：
//! 这里直接用 CoreGraphics 开一个只收左键按下/松开的 ListenOnly tap。
//!
//! 两个线程：
//! - tap 线程跑事件回调，回调里**只做轻量判定**，然后往 channel 里丢一个触发点。
//!   （回调跑在 tap 的 run loop 上，在里面做取词这种耗时活会被系统判超时、直接停掉 tap。）
//! - worker 线程收触发点，等选区稳定后取词、定位、弹窗。

use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

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

/// tap 线程丢给 worker 的触发点（全局逻辑坐标）。
struct Trigger {
    x: f64,
    y: f64,
}

/// 手势判定的可变状态。事件回调是 `Fn`，所以放在 `RefCell` 里；
/// 它只在 tap 线程上被访问，不存在并发。
#[derive(Default)]
struct Gesture {
    /// 本次按下的起点与时刻。
    press: Option<(f64, f64, Instant)>,
    /// 这一次按下是不是落在自己的面板上（是的话既不收起也不触发）。
    press_inside_panel: bool,
    /// 上一次「单击」的落点与时刻，用来判双击。
    last_click: Option<(f64, f64, Instant)>,
}

/// 启动 tap 线程与 worker 线程。调用一次。
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
            eprintln!("[glean] 全局鼠标监听启动失败：{e}");
            // 绝大多数情况是 macOS 没给辅助功能权限，让设置页能提示用户。
            let _ = app.emit("hook-error", e);
        }
    });
}

/// 按下：记起点；如果面板开着而且没点在面板上，就先收起它。
fn on_press(app: &AppHandle, g: &mut Gesture, x: f64, y: f64) {
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
    let Some((px, py, _)) = g.press.take() else {
        return;
    };
    if g.press_inside_panel {
        g.press_inside_panel = false;
        return;
    }

    let state = app.state::<AppState>();
    let moved = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
    if moved >= state.config.read().drag_threshold {
        g.last_click = None;
        let _ = tx.send(Trigger { x, y });
        return;
    }

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
fn run_hook(app: AppHandle, tx: Sender<Trigger>) -> Result<(), String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use core_foundation::base::TCFType;
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    };

    let gesture = RefCell::new(Gesture::default());
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
                    on_press(&app, &mut gesture.borrow_mut(), p.x, p.y);
                }
                CGEventType::LeftMouseUp => {
                    let p = event.location();
                    on_release(&app, &mut gesture.borrow_mut(), &tx, p.x, p.y);
                }
                // 回调超时或用户输入过快时系统会停掉 tap，必须自己重新打开，
                // 否则划词会毫无征兆地彻底失灵。
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                    if let Some(port) = port_for_cb.borrow().as_ref() {
                        unsafe { CGEventTapEnable(port.as_concrete_TypeRef(), true) };
                    }
                    eprintln!("[glean] 事件监听被系统暂停，已重新启用");
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
fn run_hook(_app: AppHandle, _tx: Sender<Trigger>) -> Result<(), String> {
    Err("当前平台尚不支持自动划词".to_string())
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
