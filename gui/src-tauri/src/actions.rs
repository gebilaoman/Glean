//! 动作层：工具栏按钮按下后干的事。
//!
//! 翻译 / 解释 / AI 搜索都走同一条通道：把缓存的划词文本发给配置里所有启用的模型，
//! 并发流式返回，前端按 `model_id` 把分片归到对应的列做对比。
//! 复制和保存是纯本地操作，不联网。

use glean_core::{ActionKind, ModelConfig, StreamEvent, config::system_prompt, stream_chat};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use crate::config;
use crate::panel;
use crate::state::SpeechTick;
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

    for model in models {
        // 每个模型一个独立任务：一路超时或报错不拖累其它列。
        spawn_model_stream(&app, &state.http, model, system.clone(), text.clone(), request_id.clone(), false);
    }

    Ok(briefs)
}

/// 把一个模型的流式任务丢到后台，分片走 `llm` 事件推送。
/// `run_action` 对全部启用的模型各来一次（retry = false），
/// `retry_model` 对单个模型来一次（retry = true，温度抬高重新抽）。
fn spawn_model_stream(
    app: &AppHandle,
    client: &reqwest::Client,
    model: glean_core::ModelConfig,
    system: String,
    text: String,
    request_id: String,
    retry: bool,
) {
    let app = app.clone();
    let client = client.clone();
    tauri::async_runtime::spawn(async move {
        let model_id = model.id.clone();
        stream_chat(&client, &model, &system, &text, retry, |ev| {
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

/// 单个模型重试。只重发被点的那一列，其它列的结果不动。
#[tauri::command]
pub async fn retry_model(
    app: AppHandle,
    state: State<'_, AppState>,
    action: ActionKind,
    model_id: String,
    request_id: String,
) -> Result<(), String> {
    let text = state.selection();
    if text.is_empty() {
        return Err("没有可用的划词文本".into());
    }
    let (model, target_lang) = {
        let cfg = state.config.read();
        let model = cfg
            .models
            .iter()
            .find(|m| m.id == model_id)
            .cloned()
            .ok_or_else(|| format!("配置里没有模型 {model_id}"))?;
        (model, cfg.target_lang.clone())
    };
    let system = system_prompt(action, &target_lang);
    spawn_model_stream(&app, &state.http, model, system, text, request_id, true);
    Ok(())
}

/// 当前缓存的划词文本（前端刷新/重挂载时用）。
#[tauri::command]
pub fn get_selection(state: State<'_, AppState>) -> String {
    state.selection()
}

/// 系统里装的一个朗读音色。
#[derive(Debug, Clone, Serialize)]
pub struct VoiceInfo {
    pub name: String,
    pub locale: String,
}

/// 解析 `say -v '?'` 的输出。名字可以含空格（"Bad News"、"Eddy (Alto)"），
/// 靠「形如 zh_CN / en_US 的那个词」定位切分点，注释丢弃。
fn parse_voices(out: &str) -> Vec<VoiceInfo> {
    let mut voices = Vec::new();
    for line in out.lines() {
        let mut name = String::new();
        let mut locale = String::new();
        for tok in line.split_whitespace() {
            if looks_like_locale(tok) {
                locale = tok.to_string();
                break;
            }
            if !name.is_empty() {
                name.push(' ');
            }
            name.push_str(tok);
        }
        if name.is_empty() || locale.is_empty() {
            continue;
        }
        voices.push(VoiceInfo { name, locale });
    }
    voices
}

fn looks_like_locale(tok: &str) -> bool {
    let Some((lang, region)) = tok.split_once(['_', '-']) else {
        return false;
    };
    (2..=3).contains(&lang.len())
        && lang.chars().all(|c| c.is_ascii_lowercase())
        && (2..=8).contains(&region.len())
        && region.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 列出系统已装的朗读音色，设置页的音色候选用。
#[tauri::command]
pub fn list_voices() -> Result<Vec<VoiceInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("say")
            .arg("-v")
            .arg("?")
            .output()
            .map_err(|e| format!("查询音色失败：{e}"))?;
        Ok(parse_voices(&String::from_utf8_lossy(&out.stdout)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("当前平台暂不支持朗读".into())
    }
}

/// `say` 的公共管道：停掉在念的、按参数起新的、派看护线程收尾。
/// 工具栏的「朗读」和设置页的「试听」都走这里。
///
/// macOS 直接调系统的 `say`（AVSpeechSynthesizer 的命令行前端），零依赖；
/// 「停」就是杀子进程。看护线程每 200ms `try_wait` 一次，念完自然收尾并通知前端。
fn spawn_say(
    app: &AppHandle,
    state: &AppState,
    text: &str,
    voice: &str,
    rate: u32,
) -> Result<bool, String> {
    // 已经在念 → 先停（toggle / 换音色重放都依赖这个语义）
    if state.stop_speech() {
        let _ = app.emit("tts-stopped", ());
    }
    if text.trim().is_empty() {
        return Err("没有可朗读的文本".into());
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};

        let mut cmd = Command::new("say");
        cmd.arg(text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if !voice.trim().is_empty() {
            cmd.arg("-v").arg(voice.trim());
        }
        if rate > 0 {
            cmd.arg("-r").arg(rate.to_string());
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("启动朗读失败：{e}"))?;
        let round = state.start_speech(child);
        let _ = app.emit("tts-started", ());

        let handle = app.clone();
        std::thread::spawn(move || {
            // State<'_> 的借用带不进线程，用 AppHandle 重新拿一份
            let state = handle.state::<AppState>();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                match state.speech_tick(round) {
                    SpeechTick::Running => continue,
                    SpeechTick::Done => {
                        let _ = handle.emit("tts-stopped", ());
                        return;
                    }
                    // 被停/被换：停的那条路径自己会发事件
                    SpeechTick::Gone => return,
                }
            }
        });
        Ok(true)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("当前平台暂不支持朗读".into())
    }
}

/// 朗读划词文本；再点一次停止（toggle）。正在朗读时按钮亮着。
#[tauri::command]
pub fn speak_selection(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let text = state.selection();
    let (voice, rate) = {
        let cfg = state.config.read();
        (cfg.tts_voice.clone(), cfg.tts_rate)
    };
    spawn_say(&app, &state, &text, &voice, rate)
}

/// 设置页「试听」：用表单里还没保存的音色/语速念一句样例，先听再存。
#[tauri::command]
pub fn preview_voice(
    app: AppHandle,
    state: State<'_, AppState>,
    voice: String,
    rate: u32,
) -> Result<bool, String> {
    spawn_say(
        &app,
        &state,
        "你好，这是当前音色和语速的试听。Hello, this is the voice.",
        &voice,
        rate,
    )
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
    app: AppHandle,
    state: State<'_, AppState>,
    config: glean_core::AppConfig,
) -> Result<(), String> {
    crate::config::save(&config)?;
    *state.config.write() = config;
    // 悬浮工具栏要按新配置重排按钮（动作开关），广播一下。
    let _ = app.emit("config-updated", ());
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

#[cfg(test)]
mod tests {
    use super::parse_voices;

    /// 名字可含空格与括号，切分点靠 locale 记号；乱行直接跳过。
    #[test]
    fn parses_say_voice_list() {
        let sample = "Albert                en_US    # Hello! My name is Albert.\nBad News              en_US    # The light you see...\nEddy (Alto)           en_US    # Hello, my name is Eddy.\nTingting              zh_CN    # 你好，我叫婷婷。\nYu-shu                zh_CN    # 你好，我叫Yu-shu。\n";
        let v = parse_voices(sample);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0].name, "Albert");
        assert_eq!(v[1].name, "Bad News");
        assert_eq!(v[2].name, "Eddy (Alto)");
        assert_eq!(v[3].name, "Tingting");
        assert_eq!(v[3].locale, "zh_CN");
    }

    #[test]
    fn skips_lines_without_locale() {
        assert!(parse_voices("没有 locale 的行\n\n").is_empty());
    }
}
