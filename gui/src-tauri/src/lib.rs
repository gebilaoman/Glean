//! Glean —— 系统级划词工具。
//!
//! 四层结构，各占一个模块：
//! - `selection`：触发层（rdev 鼠标钩子合成划词手势）+ 取词层（get-selected-text）
//! - `panel`：悬浮窗层（macOS 非激活 NSPanel，光标处定位、边缘翻转）
//! - `actions`：动作层（多模型并发流式翻译/解释/搜索、复制、保存）
//! - `config` / `state`：配置读写与运行时状态

mod actions;
mod ccswitch;
mod config;
mod panel;
mod selection;
mod state;
mod update;

use tauri::{
    Manager, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

use state::AppState;

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    builder
        .setup(|app| {
            let handle = app.handle().clone();

            // 常驻后台工具，不占 Dock；界面入口走托盘。
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            app.manage(AppState::new(config::load()));

            if let Err(e) = panel::init(&handle) {
                eprintln!("[glean] 悬浮窗初始化失败：{e}");
            }
            setup_tray(&handle)?;

            // 未授权时弹一次系统授权窗：让 macOS 把当前二进制自动登记进辅助功能
            // 列表（授权到位后 selection 的监护线程会自动拉起监听，无需重启）。
            #[cfg(target_os = "macos")]
            if !actions::accessibility_trusted() {
                actions::prompt_accessibility();
            }

            selection::spawn(handle.clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            // 设置窗关掉时只隐藏，并把激活策略切回 Accessory，免得 Dock 图标赖着不走。
            if window.label() == "settings"
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                let _ = window
                    .app_handle()
                    .set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            // 用户拖动工具栏后同步几何缓存（命中测试 + 锚点），否则拖走后
            // 点自己面板上的按钮会被当成点在外面、面板当场消失。
            if window.label() == panel::SPOTLIGHT
                && let WindowEvent::Moved(pos) = event
            {
                panel::sync_moved(window.app_handle(), (pos.x, pos.y));
            }
        })
        .invoke_handler(tauri::generate_handler![
            actions::run_action,
            actions::retry_model,
            actions::fetch_models,
            actions::speak_selection,
            actions::list_voices,
            actions::preview_voice,
            actions::get_selection,
            actions::hide_panel,
            actions::set_panel_height,
            actions::open_settings,
            actions::get_config,
            actions::save_config,
            actions::accessibility_trusted,
            actions::open_accessibility_settings,
            actions::open_config_dir,
            ccswitch::import_cc_switch,
            update::get_app_version,
            update::get_system_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let settings = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 Glean", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings, &quit])?;

    // 托盘图标必须是单色模板图（macOS 只吃它的 alpha 通道），
    // 直接用彩色应用图标会被渲染成一团剪影。
    let tray_png = include_bytes!("../icons/tray-template.png");
    let tray_img = tauri::image::Image::from_bytes(tray_png)?;

    TrayIconBuilder::with_id("glean-tray")
        .icon(tray_img)
        .icon_as_template(true)
        .tooltip("Glean · 划词助手")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "settings" => {
                if let Err(e) = actions::open_settings(app.clone()) {
                    eprintln!("[glean] 打开设置失败：{e}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}
