//! 配置数据模型。落盘在 `dirs::config_dir()/Glean/config.json`，由 GUI 侧负责读写。

use serde::{Deserialize, Serialize};

/// 一个可调用的模型端点。多模型对比就是把同一段文本并发发给列表里的每一项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// 稳定标识，前端用它把流式分片归到对应的列。
    pub id: String,
    /// 展示名，如「本地 Qwen3」。
    pub name: String,
    /// OpenAI 兼容服务的根地址，不带 `/chat/completions`，如 `http://localhost:8080/v1`。
    pub endpoint: String,
    /// 模型名，如 `qwen3-32b`。
    pub model: String,
    /// 本地服务通常不需要；留空则不发 Authorization 头。
    #[serde(default)]
    pub api_key: String,
    /// 是否参与多模型对比。关掉的模型仍保留在配置里。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 主模型在结果区默认展开，其余折叠。列表里应当只有一个为 true。
    #[serde(default)]
    pub primary: bool,
}

fn default_true() -> bool {
    true
}

/// 工具栏上的动作。复制是纯本地操作，其余走 LLM。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Translate,
    Explain,
    Search,
    Copy,
    Save,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub models: Vec<ModelConfig>,
    /// 译入语，默认中文。空字符串表示让模型自动决定。
    #[serde(default = "default_target_lang")]
    pub target_lang: String,
    /// 拖选判定阈值（逻辑像素）。小于它的位移当作普通点击丢弃。
    #[serde(default = "default_drag_threshold")]
    pub drag_threshold: f64,
    /// 松开鼠标后等多久再取词（毫秒）。太短会读到上一次的选区。
    #[serde(default = "default_settle_ms")]
    pub settle_ms: u64,
    /// 是否响应双击选词。
    #[serde(default = "default_true")]
    pub double_click_trigger: bool,
    /// 保存动作写入的目录。空则用 `dirs::document_dir()/Glean`。
    #[serde(default)]
    pub save_dir: String,
}

fn default_target_lang() -> String {
    "中文".to_string()
}

fn default_drag_threshold() -> f64 {
    5.0
}

fn default_settle_ms() -> u64 {
    120
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            // 默认指向本地 llama.cpp / Ollama 的 OpenAI 兼容端口，开箱即用。
            models: vec![ModelConfig {
                id: "local".to_string(),
                name: "本地模型".to_string(),
                endpoint: "http://localhost:11434/v1".to_string(),
                model: "qwen3:8b".to_string(),
                api_key: String::new(),
                enabled: true,
                primary: true,
            }],
            target_lang: default_target_lang(),
            drag_threshold: default_drag_threshold(),
            settle_ms: default_settle_ms(),
            double_click_trigger: true,
            save_dir: String::new(),
        }
    }
}

impl AppConfig {
    /// 参与对比的模型，主模型排在最前（前端据此决定默认展开哪一列）。
    pub fn active_models(&self) -> Vec<&ModelConfig> {
        let mut list: Vec<&ModelConfig> = self.models.iter().filter(|m| m.enabled).collect();
        list.sort_by_key(|m| !m.primary);
        list
    }
}

/// 按动作拼系统提示词。译入语来自配置。
pub fn system_prompt(action: ActionKind, target_lang: &str) -> String {
    let lang = if target_lang.trim().is_empty() {
        "中文"
    } else {
        target_lang.trim()
    };
    match action {
        ActionKind::Translate => format!(
            "你是翻译引擎。自动识别输入语言：若输入是{lang}，译成英文；否则译成{lang}。\
             只输出译文本身，不要解释、不要加引号、不要复述原文。"
        ),
        ActionKind::Explain => format!(
            "你用{lang}解释用户给出的词或句子：先给一句话的核心意思，再补充语境、用法或背景。\
             控制在 150 字以内，不要复述原文。"
        ),
        ActionKind::Search => format!(
            "你是检索型助手。针对用户给出的片段，用{lang}给出与之相关的关键事实、背景和延伸信息。\
             控制在 200 字以内，不确定的地方要明确说不确定。"
        ),
        // 复制和保存不走模型，兜底给个中性提示词。
        ActionKind::Copy | ActionKind::Save => format!("用{lang}简要概括用户给出的文本。"),
    }
}
