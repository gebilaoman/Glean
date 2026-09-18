//! 动作层：工具栏按钮按下后干的事。
//!
//! 翻译 / 解释 / AI 搜索都走同一条通道：把缓存的划词文本发给配置里所有启用的模型，
//! 并发流式返回，前端按 `model_id` 把分片归到对应的列做对比。
//! 复制和保存是纯本地操作，不联网。

use glean_core::{ActionKind, ModelConfig, StreamEvent, config::system_prompt, stream_chat};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::config;
use crate::panel;
use crate::state::AppState;

/// 给前端的模型简介，用来预先把结果区的列排好。
#[derive(Debug, Clone, Serialize)]
pub struct ModelBrief {
    pub id: String,
    pub name: String,
    pub primary: bool,
}

/// 流式事件，统一走 `llm` 这个 channel。
#[derive(Debug, Clone, Serialize)]
struct LlmEvent {
    request_id: String,
    model_id: String,
    /// `delta` / `done` / `error`
    kind: &'static str,
    data: String,
}

impl From<&ModelConfig> for ModelBrief {
    fn from(m: &ModelConfig) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            primary: m.primary,
        }
    }
}

/// 发起一次多模型动作。立即返回参与的模型列表，正文通过 `llm` 事件流式推送。
#[tauri::command]
pub async fn run_action(
    app: AppHandle,
    state: State<'_, AppState>,
    action: ActionKind,
    request_id: String,
) -> Result<Vec<ModelBrief>, String> {
    let text = state.selection();
    if text.is_empty() {
        return Err("没有可用的划词文本".into());
    }

    let (models, target_lang) = {
        let cfg = state.config.read();
        (
            cfg.active_models().into_iter().cloned().collect::<Vec<_>>(),
            cfg.target_lang.clone(),
        )
    };
    if models.is_empty() {
        return Err("没有启用任何模型，请先到设置里配置".into());
    }

    let briefs: Vec<ModelBrief> = models.iter().map(ModelBrief::from).collect();
    let system = system_prompt(action, &target_lang);
    let client = state.http.clone();

    for model in models {
        let app = app.clone();
        let client = client.clone();
        let system = system.clone();
        let text = text.clone();
        let request_id = request_id.clone();
        // 每个模型一个独立任务：一路超时或报错不拖累其它列。
        tauri::async_runtime::spawn(async move {
            let model_id = model.id.clone();
            stream_chat(&client, &model, &system, &text, |ev| {
                let (kind, data) = match ev {
                    StreamEvent::Delta(d) => ("delta", d),
                    StreamEvent::Done => ("done", String::new()),
                    StreamEvent::Error(e) => ("error", e),
                };
                let _ = app.emit(
                    "llm",
                    LlmEvent {
                        request_id: request_id.clone(),
                        model_id: model_id.clone(),
                        kind,
                        data,
                    },
                );
            })
            .await;
        });
    }

    Ok(briefs)
}

/// 当前缓存的划词文本（前端刷新/重挂载时用）。
#[tauri::command]
pub fn get_selection(state: State<'_, AppState>) -> String {
    state.selection()
}

#[tauri::command]
pub fn copy_selection(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let text = state.selection();
    if text.is_empty() {
        return Err("没有可复制的文本".into());
    }
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

/// 保存：按天追加到一个 Markdown 文件，返回落盘路径供前端提示。
#[tauri::command]
pub fn save_selection(state: State<'_, AppState>) -> Result<String, String> {
    let text = state.selection();
    if text.is_empty() {
        return Err("没有可保存的文本".into());
    }
    let dir = config::save_dir(&state.config.read());
    let now = chrono::Local::now();
    let path = dir.join(format!("{}.md", now.format("%Y-%m-%d")));

    let entry = format!("\n## {}\n\n{}\n", now.format("%H:%M:%S"), text);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    f.write_all(entry.as_bytes()).map_err(|e| e.to_string())?;

    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn hide_panel(app: AppHandle) {
    panel::hide(&app);
}

/// 前端内容高度变化时调用，Rust 侧重新算位置（含边缘翻转）。
#[tauri::command]
pub fn set_panel_height(app: AppHandle, height: f64) {
    panel::set_height(&app, height);
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or("找不到设置窗口")?;
    // Accessory 策略下窗口拿不到焦点，开设置时临时切回 Regular（关窗时切回去）。
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    // 只在它当前没显示时摆位，免得用户自己挪过位置又被拽回中间。
    if !window.is_visible().unwrap_or(false) {
        panel::center_settings(&app);
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> glean_core::AppConfig {
    state.config.read().clone()
}

#[tauri::command]
pub fn save_config(
    state: State<'_, AppState>,
    config: glean_core::AppConfig,
) -> Result<(), String> {
    crate::config::save(&config)?;
    *state.config.write() = config;
    Ok(())
}

/// macOS 辅助功能（Accessibility）是否已授权。没授权的话鼠标钩子和 AX 取词都不工作。
#[tauri::command]
pub fn accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        // 只查询、不弹窗；弹窗交给 open_accessibility_settings。
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// 打开「系统设置 → 隐私与安全性 → 辅助功能」。
#[tauri::command]
pub fn open_accessibility_settings(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                None::<&str>,
            )
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

/// 打开配置目录，方便手工改 config.json。
#[tauri::command]
pub fn open_config_dir(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(config::config_dir().to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}
