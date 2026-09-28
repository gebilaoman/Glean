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

    // 顺序即优先级：第一个标成主模型，结果区默认展开它（见 active_models）。
    let briefs: Vec<ModelBrief> = models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut b = ModelBrief::from(m);
            b.primary = i == 0;
            b
        })
        .collect();
    let system = system_prompt(action, &target_lang);

    // 惰性请求：只立刻发第一个（主模型）。
    // 其余列等用户在结果区展开时由前端经 retry_model(retry=false) 补发，省 token。
    if let Some(first) = models.into_iter().next() {
        spawn_model_stream(&app, &state.http, first, system, text, request_id, false);
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

/// 单列请求。只发被点的那一列，其它列不动。两个用途：
/// - 结果区的「重译」按钮：`retry = true`，温度抬高重新抽一次；
/// - 惰性展开的首次请求：`retry = false`（run_action 只发了主模型，
///   其余列等用户展开才经这里补发，省 token）。
#[tauri::command]
pub async fn retry_model(
    app: AppHandle,
    state: State<'_, AppState>,
    action: ActionKind,
    model_id: String,
    request_id: String,
    retry: bool,
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
    spawn_model_stream(&app, &state.http, model, system, text, request_id, retry);
    Ok(())
}

/// 从 OpenAI 兼容端点拉模型清单（GET /models）。
/// 设置页的模型下拉用；拉不到（厂商没实现/网络/没填 key）前端回落预设清单。
#[tauri::command]
pub async fn fetch_models(
    state: State<'_, AppState>,
    endpoint: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    let url = format!("{}/models", endpoint.trim_end_matches('/'));
    let mut req = state.http.get(&url);
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        let detail: String = detail.chars().take(200).collect();
        return Err(format!("{status}：{detail}"));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("解析失败：{e}"))?;
    let mut ids: Vec<String> = v["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    if ids.is_empty() {
        return Err("返回里没有模型".into());
    }
    Ok(ids)
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

/// `say` 的「永远新起」管道：停掉在念的、按参数起新的、派看护线程收尾。
/// 只有设置页「试听」直接走这里（试听永远是重新放一遍）。
/// 工具栏「朗读」的 toggle 语义（点第二次是停、**不重放**）在 speak_selection 里，
/// 别把这层挪进 spawn_say——上次重构就是把它俩混了，停止变成从头重念。
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
    // 已经在念 → 这次点的是「停」，直接返回，绝不再起新的
    if state.stop_speech() {
        let _ = app.emit("tts-stopped", ());
        return Ok(false);
    }
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
    // 让原生窗口外观（红绿灯、滚动条）跟随主题；auto 时交还给系统。
    let native = match config.theme.as_str() {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    };
    for w in app.webview_windows().values() {
        let _ = w.set_theme(native);
    }
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
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

/// 弹一次系统的辅助功能授权对话框，让 macOS 把**当前**二进制自动登记进
/// 辅助功能列表（无签名应用每个构建的指纹都不同，靠用户手动「+添加」
/// 经常加成旧指纹的死条目——开关看着开着、实际不放行）。
#[cfg(target_os = "macos")]
pub fn prompt_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const _) }
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

// ─────────────────────── 联动 Attune(听读) ───────────────────────

/// 「收藏到Attune」动作:把划词文本**切成句块直接追加**到指定收藏文档,再唤起 Attune。
///
/// **收藏即成品**:收藏的内容本来就是一段/一句句,不需要 AI 转化——本地启发式切句
/// 后直接落为可听读的 Paragraph/Sentence 结构(converted=true),进 Attune 打开就能
/// 逐句播放。句 id 沿用 Attune 的位置性方案并**接着现有最大序号继续发号**
/// (绝不重排,否则会打乱老句与音频缓存的映射);zh 留空,想要译文可后续在
/// Attune 里对该句「AI优化」。重复句(按英文原文比对)自动跳过。
///
/// 文档级收藏夹(B站式):所有收藏住在 `docs/{收藏文件夹}/`(默认 Glean),
/// 「夹」= 里面的文档——选择器列出已有文档 + 默认文档置顶 + 可新建。
/// doc 参数 = 目标文档名;None 则用配置里的默认收藏文档。
#[tauri::command]
pub fn send_to_attune(
    state: State<'_, AppState>,
    doc: Option<String>,
) -> Result<String, String> {
    let text = state.selection().trim().to_string();
    if text.is_empty() {
        return Err("没有选中文本".into());
    }
    let (vault_cfg, folder_cfg, doc_cfg) = {
        let cfg = state.config.read();
        (cfg.attune_vault.clone(), cfg.attune_folder.clone(), cfg.attune_doc.clone())
    };
    let vault = resolve_attune_vault(&vault_cfg)?;
    let folder = sanitize_folder_name({
        let f = folder_cfg.trim();
        if f.is_empty() { "Glean" } else { f }
    });
    let doc_name = doc
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| {
            let d = doc_cfg.trim();
            if d.is_empty() { "收集箱".to_string() } else { d.to_string() }
        });

    let dir = attune_docs_root(&vault).join(&folder);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败:{e}"))?;

    // 按 JSON 里的 title 找现有文档(文件名可能带 -id 后缀,不能拿文件名当标题)
    let existing = find_note_by_title(&dir, &doc_name);
    let (added, total, all_dup) = match existing {
        Some(path) => {
            let mut note: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&path)
                    .map_err(|e| format!("读取收藏文档失败:{e}"))?,
            )
            .map_err(|e| format!("收藏文档解析失败({}):{e}", path.display()))?;
            let (added, all_dup) = append_collected_block(&mut note, &text);
            if added > 0 {
                std::fs::write(&path, serde_json::to_string_pretty(&note).unwrap_or_default())
                    .map_err(|e| format!("写入收藏文档失败:{e}"))?;
            }
            (added, count_note_sentences(&note), all_dup)
        }
        None => {
            // 新建收藏文档:直接是成品句块(免转化)
            let mut path = dir.join(format!("{}.json", sanitize_file_name(&doc_name)));
            if path.exists() {
                path = dir.join(format!(
                    "{}-{}.json",
                    sanitize_file_name(&doc_name),
                    &attune_note_id()[..12]
                ));
            }
            let note_id = attune_note_id();
            let mut note = serde_json::json!({
                "id": note_id,
                "title": doc_name,
                "folder": folder,
                "tags": [],
                "created_at": chrono::Utc::now().format("%Y-%m-%d").to_string(),
                "source": "Glean 划词",
                "raw": text,
                "converted": true,
                "blocks": [],
                "audio_ready": false,
            });
            let (added, _) = append_collected_block(&mut note, &text);
            std::fs::write(&path, serde_json::to_string_pretty(&note).unwrap_or_default())
                .map_err(|e| format!("写入收藏文档失败:{e}"))?;
            (added, count_note_sentences(&note), false)
        }
    };

    // 唤起 Attune;失败忽略(文档已落库,不算发送失败)
    let _ = std::process::Command::new("open").args(["-a", "Attune"]).spawn();
    Ok(if added > 0 {
        format!("已收藏到「{folder}/{doc_name}」(+{added} 句,共 {total} 句)")
    } else if all_dup {
        format!("这批内容已在「{folder}/{doc_name}」里,未重复收藏")
    } else {
        format!("没有可收藏的新句子(「{folder}/{doc_name}」共 {total} 句)")
    })
}

/// 去重比对用的归一化:去首尾空白、转小写、剥句末标点/收尾引号括号。
/// ("Old one." 与 "old one" 算同一句,收藏箱宁可少收不重复。)
fn norm_en(s: &str) -> String {
    let t = s.trim().to_lowercase();
    let t = t.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | '…' | '"' | '\'' | ')' | ']' | '”' | '’'));
    t.to_string()
}

/// 把一次收藏切成句块追加进 note(本次收集 = 一个 Paragraph 块,按句切 Sentence)。
/// 与现有句按归一化英文去重;句 id 从现有最大序号+1 续发。返回 (新增句数, 是否全部重复)。
fn append_collected_block(note: &mut serde_json::Value, text: &str) -> (usize, bool) {
    let note_id = note["id"].as_str().unwrap_or("note_glean").to_string();
    let existing: Vec<String> = note_sentence_ens(note).iter().map(|e| norm_en(e)).collect();
    let candidates: Vec<String> = split_sentences_en(text)
        .into_iter()
        .filter(|s| !existing.contains(&norm_en(s)))
        .collect();
    let raw_count = split_sentences_en(text).len();
    let all_dup = raw_count > 0 && candidates.is_empty();

    if candidates.is_empty() {
        return (0, all_dup);
    }
    let mut seq = next_sentence_seq(note);
    let sentences: Vec<serde_json::Value> = candidates
        .iter()
        .map(|en| {
            seq += 1;
            serde_json::json!({
                "id": format!("{note_id}_{seq}"),
                "speaker": "",
                "en": en,
                "zh": "",
                "audio": format!("{note_id}/{seq}.mp3"),
                "read_aloud": true,
                "mastered": false,
            })
        })
        .collect();
    note["blocks"]
        .as_array_mut()
        .unwrap_or(&mut vec![])
        .push(serde_json::json!({ "type": "paragraph", "sentences": sentences }));
    note["converted"] = serde_json::Value::Bool(true); // 收藏即成品
    note["audio_ready"] = serde_json::Value::Bool(false); // 新句还没音频
    (candidates.len(), all_dup)
}

/// note 里现有全部句子的英文(去重比对用)。
fn note_sentence_ens(note: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(blocks) = note["blocks"].as_array() {
        for b in blocks {
            let key = if b["type"] == "list" { "items" } else { "sentences" };
            if key == "sentences" {
                if let Some(ss) = b["sentences"].as_array() {
                    for s in ss {
                        if let Some(en) = s["en"].as_str() {
                            out.push(en.to_string());
                        }
                    }
                }
            } else if let Some(items) = b["items"].as_array() {
                for it in items {
                    if let Some(ss) = it["sentences"].as_array() {
                        for s in ss {
                            if let Some(en) = s["en"].as_str() {
                                out.push(en.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// 现有句 id({note.id}_{序号})的最大序号;没有句则 0。新句从 +1 续发,绝不重排。
fn next_sentence_seq(note: &serde_json::Value) -> usize {
    note_sentence_ids(note)
        .iter()
        .filter_map(|id| id.rsplit_once('_').and_then(|(_, s)| s.parse::<usize>().ok()))
        .max()
        .unwrap_or(0)
}

fn note_sentence_ids(note: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(blocks) = note["blocks"].as_array() {
        for b in blocks {
            let lists: Vec<&serde_json::Value> = if b["type"] == "list" {
                b["items"].as_array().map(|a| a.iter().collect()).unwrap_or_default()
            } else {
                vec![b]
            };
            for holder in lists {
                if let Some(ss) = holder["sentences"].as_array() {
                    for s in ss {
                        if let Some(id) = s["id"].as_str() {
                            out.push(id.to_string());
                        }
                    }
                }
            }
        }
    }
    out
}

/// note 里句子总数(进度提示用)。
fn count_note_sentences(note: &serde_json::Value) -> usize {
    note_sentence_ids(note).len()
}

/// 英文启发式切句:按句末标点(. ! ? …)分句,标点后可跟收尾引号/括号;
/// 常见缩写(Mr. / e.g. / U.S. 等)与数字小数(3.14)不切。收藏内容本身
/// 已是一段/一句句,本地切句就够,不需要 AI。
fn split_sentences_en(text: &str) -> Vec<String> {
    // 带句点的缩写(比对含句点的末 token,如 "e.g")
    const ABBREV_DOTTED: [&str; 8] = ["e.g", "i.e", "u.s", "u.k", "a.m", "p.m", "u.s.a", "etc."];
    // 纯词缩写(末 token 不含句点时比对,如 "Mr")
    const ABBREV_PLAIN: [&str; 13] = [
        "mr", "mrs", "ms", "dr", "st", "jr", "sr", "vs", "inc", "ltd", "co", "fig", "approx",
    ];

    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        if !matches!(c, '.' | '!' | '?' | '…') {
            i += 1;
            continue;
        }
        // 吸收句末收尾引号/括号:"”’)] 等
        let mut j = i + 1;
        while j < n && matches!(chars[j], '"' | '\'' | ')' | ']' | '”' | '’') {
            j += 1;
        }
        // 必须后面是空白或结尾才算句末(3.14 的小数点后面是数字,不切)
        if j < n && !chars[j].is_whitespace() {
            i = j;
            continue;
        }
        // 缩写守卫:回看末 token(字母数字与句点组成的连续段)
        let seg: String = chars[start..i].iter().collect();
        let token: String = seg
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '.')
            .collect::<String>()
            .to_lowercase()
            .chars()
            .rev()
            .collect();
        let is_abbrev = if token.contains('.') {
            ABBREV_DOTTED.contains(&token.as_str())
        } else {
            ABBREV_PLAIN.contains(&token.as_str())
        };
        if !is_abbrev {
            let sentence: String = chars[start..j].iter().collect();
            let trimmed = sentence.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            start = j;
        }
        i = j;
    }
    let tail: String = chars[start..].iter().collect();
    let tail = tail.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// 在目录里按 JSON 内的 title 找文档(文件名可能带 -id 后缀,以内容为准)。
fn find_note_by_title(dir: &std::path::Path, title: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else { continue };
        if v.get("title").and_then(|t| t.as_str()) == Some(title) {
            return Some(path);
        }
    }
    None
}

/// 与 Attune 的 gen_note_id 同格式:note_ + 毫秒时间戳hex + 纳秒低16位hex。
fn attune_note_id() -> String {
    let ms = chrono::Utc::now().timestamp_millis() as u64;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("note_{:x}{:x}", ms, nanos & 0xffff)
}

/// 文件名清洗:镜像 Attune 的 sanitize_filename 规则,两边落盘命名保持一致。
fn sanitize_file_name(name: &str) -> String {
    let trimmed = name.trim();
    let fallback = "未命名";
    let name = if trimmed.is_empty() { fallback } else { trimmed };
    let mut out = String::new();
    for ch in name.chars() {
        if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let out = out.trim_matches('.').trim();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out.to_string()
    }
}

/// 文件夹名清洗:按段清洗(支持嵌套如 工作/Glean)。
fn sanitize_folder_name(folder: &str) -> String {
    folder
        .split('/')
        .map(sanitize_file_name)
        .collect::<Vec<_>>()
        .join("/")
}

/// 解析 Attune 库目录:设置里填了就用;留空则读 Attune 自己的 config.json 自动发现。
fn resolve_attune_vault(configured: &str) -> Result<std::path::PathBuf, String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = std::path::PathBuf::from(configured);
        if p.is_dir() {
            return Ok(p);
        }
        return Err(format!(
            "Attune 库目录不存在:{configured}(在 Glean 设置里改,或先在 Attune 里选一次库)"
        ));
    }
    let cfg_path = dirs::config_dir()
        .map(|d| d.join("Attune").join("config.json"))
        .ok_or("找不到系统配置目录")?;
    let vault = vault_from_attune_config(&cfg_path).ok_or(
        "未能自动发现 Attune 库:请先在 Attune 里选择库目录,或在 Glean 设置里手动填写",
    )?;
    Ok(std::path::PathBuf::from(vault))
}

/// 从 Attune 的 config.json 读 vault_path(只取这一个字段;失败/为空都返回 None)。
fn vault_from_attune_config(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let vp = v.get("vault_path")?.as_str()?.trim().to_string();
    if vp.is_empty() {
        None
    } else {
        Some(vp)
    }
}

/// 设置页展示「当前生效的库目录」。candidate = 表单里还没保存的值:
/// 传空串表示按「自动发现」解析 —— 这样打字时能实时看到解析结果。
#[derive(Debug, Serialize)]
pub struct AttuneVaultStatus {
    pub path: String,
    /// "configured"(手填/选择) / "auto"(读 Attune 配置发现)
    pub source: &'static str,
}

#[tauri::command]
pub fn get_attune_vault(candidate: Option<String>) -> Result<AttuneVaultStatus, String> {
    let c = candidate.unwrap_or_default();
    let path = resolve_attune_vault(&c)?;
    let source = if c.trim().is_empty() { "auto" } else { "configured" };
    Ok(AttuneVaultStatus {
        path: path.to_string_lossy().to_string(),
        source,
    })
}

/// 系统目录选择器,返回用户选的目录(取消返回 None)。Rust 侧直接调 dialog 插件,
/// 免装前端 npm 包;blocking 版本必须离开主线程,async 命令跑在 tokio 工作线程上。
#[tauri::command]
pub async fn pick_attune_vault(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().to_string()))
}

/// 收藏文档清单:收藏文件夹(docs/{收藏文件夹}/)下的文档标题,按修改时间新→旧。
/// 供悬浮面板的收藏选择器用;基于已保存配置解析 vault。
#[tauri::command]
pub fn list_attune_docs(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let vault_cfg = state.config.read().attune_vault.clone();
    let folder_cfg = state.config.read().attune_folder.clone();
    let vault = resolve_attune_vault(&vault_cfg)?;
    let folder = {
        let f = folder_cfg.trim();
        if f.is_empty() { "Glean".to_string() } else { f.to_string() }
    };
    let dir = attune_docs_root(&vault).join(sanitize_folder_name(&folder));
    if !dir.is_dir() {
        return Ok(vec![]); // 还没收藏过 → 只有默认文档可选
    }
    let mut out: Vec<(std::time::SystemTime, String)> = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else { continue };
        if let Some(title) = v.get("title").and_then(|t| t.as_str()) {
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            out.push((mtime, title.to_string()));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0)); // 最近收藏的在上面
    Ok(out.into_iter().map(|(_, t)| t).collect())
}

/// Attune 的文档根:vault 下的 docs/(与 Attune 的 docs_root 同源)。
/// media/ 是音频区、与 docs 平级;note.folder 字段相对 docs。
fn attune_docs_root(vault: &std::path::Path) -> std::path::PathBuf {
    vault.join("docs")
}

#[cfg(test)]
mod tests {
    use super::{
        append_collected_block, find_note_by_title, next_sentence_seq, parse_voices,
        sanitize_file_name, sanitize_folder_name, split_sentences_en, vault_from_attune_config,
    };

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

    /// 收藏即成品:本地切句 + 句块追加(续号、去重、converted 直出);名称清洗与 Attune 同规则。
    #[test]
    fn attune_collect_splits_and_appends_as_blocks() {
        // 切句:多句、带引号问号、缩写与小数不误切
        let ss = split_sentences_en(
            "Hi Iris, version 3.14 is live (finally!). Can you check e.g. the login flow? \"Yes,\" she said.",
        );
        assert_eq!(
            ss,
            vec![
                "Hi Iris, version 3.14 is live (finally!).",
                "Can you check e.g. the login flow?",
                "\"Yes,\" she said.",
            ]
        );
        // 无句末标点的尾巴也保留
        assert_eq!(split_sentences_en("one. two three"), vec!["one.", "two three"]);

        // 追加进新文档:converted=true、句 id 从 1 起、audio 指针就位
        let mut note = serde_json::json!({
            "id": "note_abc", "title": "收集箱", "blocks": [], "converted": false
        });
        let (added, all_dup) = append_collected_block(&mut note, "first one. second one");
        assert_eq!((added, all_dup), (2, false));
        assert_eq!(note["converted"], true);
        assert_eq!(note["blocks"].as_array().unwrap().len(), 1);
        let ss = note["blocks"][0]["sentences"].as_array().unwrap();
        assert_eq!(ss[0]["id"], "note_abc_1");
        assert_eq!(ss[1]["id"], "note_abc_2");
        assert_eq!(ss[0]["audio"], "note_abc/1.mp3");

        // 再追加:序号接着现有最大续发(不重排),重复句跳过
        let (added, all_dup) = append_collected_block(&mut note, "third. second one");
        assert_eq!((added, all_dup), (1, false));
        let ss = note["blocks"][1]["sentences"].as_array().unwrap();
        assert_eq!(ss[0]["id"], "note_abc_3", "新句必须从最大序号+1 续发");

        // 全部重复 → (0, true),不落块
        let (added, all_dup) = append_collected_block(&mut note, "second one");
        assert_eq!((added, all_dup), (0, true));
        assert_eq!(note["blocks"].as_array().unwrap().len(), 2);

        // List 块里的句也计入去重与续号
        let mut note2 = serde_json::json!({
            "id": "note_lst",
            "blocks": [{ "type": "list", "items": [
                { "sentences": [{ "id": "note_lst_7", "en": "old one" }] }] }]
        });
        assert_eq!(next_sentence_seq(&note2), 7);
        let (added, _) = append_collected_block(&mut note2, "old one. brand new");
        assert_eq!(added, 1);
        let ss = note2["blocks"][1]["sentences"].as_array().unwrap();
        assert_eq!(ss[0]["id"], "note_lst_8");

        assert_eq!(sanitize_file_name("a/b:c*?\"<>|d"), "a_b_c______d"); // /:*?"<>| 七个坏字符逐个换 _
        assert_eq!(sanitize_file_name("  ..dot.. "), "dot");
        assert_eq!(sanitize_folder_name("工作/Glean"), "工作/Glean");
        assert_eq!(sanitize_folder_name("a//b"), "a/未命名/b");
    }

    /// 自动发现:能从 Attune config.json 里读到 vault_path;空值/坏文件返回 None。
    #[test]
    fn reads_vault_path_from_attune_config() {
        let tmp = std::env::temp_dir().join(format!("glean_attune_cfg_{}.json", std::process::id()));
        std::fs::write(&tmp, r#"{"vault_path":"/tmp/somevault","tts":{}}"#).unwrap();
        assert_eq!(
            vault_from_attune_config(&tmp).as_deref(),
            Some("/tmp/somevault")
        );
        std::fs::write(&tmp, r#"{"vault_path":"  "}"#).unwrap();
        assert_eq!(vault_from_attune_config(&tmp), None);
        std::fs::write(&tmp, "not json").unwrap();
        assert_eq!(vault_from_attune_config(&tmp), None);
        let _ = std::fs::remove_file(&tmp);
    }

    /// 按标题找文档:文件名带 -id 后缀也能找到(标题以 JSON 内容为准);找不到返回 None。
    #[test]
    fn finds_note_by_title_even_with_id_suffixed_filename() {
        let dir = std::env::temp_dir().join(format!("glean_docs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 文件名与标题不一致(Attune 重名时会写成 标题-id.json)
        std::fs::write(
            dir.join("收集箱-note_abc123.json"),
            r#"{"id":"note_abc123","title":"收集箱","raw":"x"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("other.json"), r#"{"id":"n2","title":"别的","raw":"y"}"#).unwrap();
        assert_eq!(
            find_note_by_title(&dir, "收集箱")
                .map(|p| p.file_name().unwrap().to_string_lossy().to_string()),
            Some("收集箱-note_abc123.json".to_string())
        );
        assert!(find_note_by_title(&dir, "不存在").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
