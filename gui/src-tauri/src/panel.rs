//! 悬浮窗层：把 spotlight 窗口变成不抢焦点的 overlay，并负责定位 / 显示 / 收起。
//!
//! macOS 走 `tauri-nspanel`，把 NSWindow 换成带 `NSWindowStyleMaskNonActivatingPanel`
//! 的 NSPanel —— 这是「点工具栏时底层 App 不失焦、选区不丢」的唯一正解。
//! 其它平台回落到普通的无边框置顶窗（体验略差，但能用）。

use parking_lot::Mutex;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::state::AppState;

pub const SPOTLIGHT: &str = "spotlight";

/// 工具栏逻辑宽度，和前端 CSS 里的 `--panel-width` 必须一致。
pub const WIDTH: f64 = 560.0;
/// 只有工具栏时的逻辑高度。
pub const COLLAPSED_HEIGHT: f64 = 72.0;
/// 工具栏与光标的垂直间距。
const GAP: f64 = 18.0;
/// 距屏幕边缘的最小留白。
const MARGIN: f64 = 8.0;

/// 逻辑坐标下的矩形，用于判断「鼠标事件是不是发生在面板内」。
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// 面板的几何状态。锚点是触发划词时的光标位置，展开/收起时按它重新定位。
#[derive(Default)]
pub struct PanelGeometry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    anchor: (f64, f64),
    height: f64,
    rect: Option<Rect>,
}

impl PanelGeometry {
    pub fn rect(&self) -> Option<Rect> {
        self.inner.lock().rect
    }

    fn set_anchor(&self, x: f64, y: f64) {
        let mut inner = self.inner.lock();
        inner.anchor = (x, y);
        inner.height = COLLAPSED_HEIGHT;
    }

    fn anchor(&self) -> (f64, f64) {
        self.inner.lock().anchor
    }

    fn height(&self) -> f64 {
        let h = self.inner.lock().height;
        if h <= 0.0 { COLLAPSED_HEIGHT } else { h }
    }

    fn set_height(&self, h: f64) {
        self.inner.lock().height = h;
    }

    fn set_rect(&self, rect: Rect) {
        self.inner.lock().rect = Some(rect);
    }

    fn clear_rect(&self) {
        self.inner.lock().rect = None;
    }
}

fn spotlight(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(SPOTLIGHT)
}

/// 启动时把窗口改造成 overlay。只调用一次。
pub fn init(app: &AppHandle) -> Result<(), String> {
    let window = spotlight(app).ok_or("找不到 spotlight 窗口")?;

    #[cfg(target_os = "macos")]
    #[allow(deprecated)] // nspanel 转发的是 cocoa crate 的旧常量，暂无 objc2 版替代
    {
        use tauri_nspanel::{WebviewWindowExt, cocoa::appkit::NSWindowCollectionBehavior};

        let panel = window.to_panel().map_err(|e| format!("转 NSPanel 失败：{e:?}"))?;

        // NSStatusWindowLevel = 25：盖在普通窗口和多数浮层之上。
        panel.set_level(25);

        // 关键：非激活面板。点它时本 App 不会被激活，底层 App 不失焦、选区不丢。
        #[allow(non_upper_case_globals)]
        const NSWindowStyleMaskNonActivatingPanel: i32 = 1 << 7;
        panel.set_style_mask(NSWindowStyleMaskNonActivatingPanel);

        // 跟随所有 Space，并且能浮在别人的全屏窗口上。
        panel.set_collection_behaviour(
            NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary
                | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary,
        );

        // App 切到后台时别自动藏——我们自己控制显隐。
        panel.set_hides_on_deactivate(false);
        panel.set_floating_panel(true);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_always_on_top(true);
        let _ = window.set_skip_taskbar(true);
    }

    Ok(())
}

/// AppKit 的窗口操作**必须在主线程**上做：rdev 钩子线程 / 命令线程上直接调
/// `orderOut:` 会让 AppKit 断言失败，进程当场 SIGTRAP 挂掉。
/// 所有对外接口都先跳回主线程，调用方不必自己操心。
fn on_main(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || f(&handle)) {
        eprintln!("[glean] 切主线程失败：{e}");
    }
}

/// 在光标处显示工具栏（收起态）。`x`/`y` 是全局逻辑坐标。
pub fn show_at(app: &AppHandle, x: f64, y: f64) {
    on_main(app, move |app| {
        let geo = &app.state::<AppState>().geometry;
        geo.set_anchor(x, y);
        if let Err(e) = reposition(app, geo) {
            eprintln!("[glean] 摆放工具栏失败：{e}");
            return;
        }
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            match app.get_webview_panel(SPOTLIGHT) {
                // order_front_regardless：显示但不抢 key，也不激活本 App。
                Ok(panel) => panel.order_front_regardless(),
                Err(e) => eprintln!("[glean] 取面板失败：{e:?}"),
            }
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(w) = spotlight(app) {
            let _ = w.show();
        }
    });
}

pub fn hide(app: &AppHandle) {
    on_main(app, |app| {
        app.state::<AppState>().geometry.clear_rect();
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            if let Ok(panel) = app.get_webview_panel(SPOTLIGHT) {
                panel.order_out(None);
            }
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(w) = spotlight(app) {
            let _ = w.hide();
        }
    });
}

/// 前端内容高度变了（展开结果区 / 收回）时调用，保持锚点不动重新摆放。
pub fn set_height(app: &AppHandle, height: f64) {
    on_main(app, move |app| {
        let geo = &app.state::<AppState>().geometry;
        geo.set_height(height.clamp(COLLAPSED_HEIGHT, 720.0));
        if let Err(e) = reposition(app, geo) {
            eprintln!("[glean] 调整工具栏高度失败：{e}");
        }
    });
}

/// 按锚点 + 当前高度算位置，处理边缘夹取与上下翻转，然后 set_position/set_size。
fn reposition(app: &AppHandle, geo: &PanelGeometry) -> Result<(), String> {
    let window = spotlight(app).ok_or("找不到 spotlight 窗口")?;
    let (cx, cy) = geo.anchor();
    let height = geo.height();

    // 取光标所在那块屏，多屏 / 高 DPI 下才不会弹错地方。
    let monitor = window
        .monitor_from_point(cx, cy)
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or("拿不到显示器信息")?;
    let scale = monitor.scale_factor();
    let mx = monitor.position().x as f64 / scale;
    let my = monitor.position().y as f64 / scale;
    let mw = monitor.size().width as f64 / scale;
    let mh = monitor.size().height as f64 / scale;

    // 水平：以光标为中心，夹取到屏内。
    let x = (cx - WIDTH / 2.0).clamp(mx + MARGIN, (mx + mw - WIDTH - MARGIN).max(mx + MARGIN));

    // 垂直：默认弹在光标下方；下方放不下就翻到上方；再放不下就贴顶。
    let below = cy + GAP;
    let y = if below + height <= my + mh - MARGIN {
        below
    } else {
        let above = cy - GAP - height;
        if above >= my + MARGIN {
            above
        } else {
            (my + mh - height - MARGIN).max(my + MARGIN)
        }
    };

    window
        .set_size(PhysicalSize::new(
            (WIDTH * scale).round() as u32,
            (height * scale).round() as u32,
        ))
        .map_err(|e| e.to_string())?;
    window
        .set_position(PhysicalPosition::new(
            (x * scale).round() as i32,
            (y * scale).round() as i32,
        ))
        .map_err(|e| e.to_string())?;

    geo.set_rect(Rect {
        x,
        y,
        w: WIDTH,
        h: height,
    });
    Ok(())
}
