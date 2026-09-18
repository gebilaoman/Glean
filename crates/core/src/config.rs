//! 配置数据模型。落盘在 `dirs::config_dir()/Glean/config.json`，由 GUI 侧负责读写。

use serde::{Deserialize, Serialize};

/// 思考强度。各家推理模型的开关方式不统一，这里统一成一个枚举，由用户按模型选。
///
/// - `Auto`：什么都不发，用服务端默认。兼容性最好，但 GLM-5.3 的默认是 `max`，很慢。
/// - `Off`：发 `thinking:{type:"disabled"}`。老的 GLM 推理模型靠它关思考；
///   **GLM-5.3 起不再支持关闭**，发了会返回 400 / code 1210。
/// - `Low` / `High` / `Max`：发 `thinking:{type:"enabled"}` + `reasoning_effort`。
///   划词翻译这种小任务选 `Low` 就够，延迟差别很大。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Thinking {
    #[default]
    Auto,
    Off,
    Low,
    High,
    Max,
}

impl Thinking {
    /// 对应 `reasoning_effort` 的取值；`Auto` / `Off` 不走这条路，返回 None。
    pub fn effort(self) -> Option<&'static str> {
        match self {
            Thinking::Low => Some("low"),
            Thinking::High => Some("high"),
            Thinking::Max => Some("max"),
            Thinking::Auto | Thinking::Off => None,
        }
    }
}

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
    /// 思考强度。老配置里没有这个字段，缺省为 `Auto`（什么都不发）。
    #[serde(default)]
    pub thinking: Thinking,
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
    /// 朗读：不走模型，调系统 TTS。
    Speak,
    /// 已下线的动作。旧配置里残留的取值（如 "save"/"copy"）落到这里，
    /// 反序列化不失败——一旦失败 `load()` 会整份回落默认配置，把用户
    /// 配好的模型全冲掉。永不渲染、永不执行，下次保存自然消失。
    #[serde(other)]
    Removed,
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
    /// 工具栏上显示哪些动作。渲染按固定顺序来，这里只当开关集合用；
    /// 老配置没有该字段时缺省全开。
    #[serde(default = "default_actions")]
    pub actions: Vec<ActionKind>,
    /// 朗读音色。空 = 跟随系统默认（中文常听着不对，推荐选个 zh_CN 音色）。
    /// 取值是 `say -v '?'` 列出的名字，如 "Tingting"。
    #[serde(default)]
    pub tts_voice: String,
    /// 朗读语速（每分钟字数，`say -r`）。0 = 系统默认（约 175）。
    #[serde(default)]
    pub tts_rate: u32,
}

/// 工具栏的固定渲染顺序（前端也按这个顺序过滤）。
pub const CANONICAL_ACTIONS: [ActionKind; 4] = [
    ActionKind::Search,
    ActionKind::Translate,
    ActionKind::Explain,
    ActionKind::Speak,
];

fn default_actions() -> Vec<ActionKind> {
    CANONICAL_ACTIONS.to_vec()
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
                thinking: Thinking::Auto,
            }],
            target_lang: default_target_lang(),
            drag_threshold: default_drag_threshold(),
            settle_ms: default_settle_ms(),
            double_click_trigger: true,
            actions: default_actions(),
            tts_voice: String::new(),
            tts_rate: 0,
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
        // 朗读不走模型，这个提示词实际不会被用到，兜底而已。
        ActionKind::Speak | ActionKind::Removed => {
            format!("用{lang}简要概括用户给出的文本。")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧配置里可能残留已下线的动作（save/copy）和已删除的字段（save_dir）。
    /// 解析必须成功且模型完好——失败的话 load() 会整份回落默认配置，
    /// 把用户配好的模型与 key 全冲掉。
    #[test]
    fn parses_config_with_removed_actions_and_fields() {
        let raw = r#"{
            "models": [{
                "id": "m", "name": "n", "endpoint": "http://x/v1", "model": "g",
                "api_key": "sk-x", "enabled": true, "primary": true, "thinking": "auto"
            }],
            "target_lang": "中文",
            "drag_threshold": 5.0,
            "settle_ms": 120,
            "double_click_trigger": true,
            "save_dir": "/tmp/old",
            "actions": ["search", "translate", "save", "copy", "speak", "explain"]
        }"#;
        let cfg: AppConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.models.len(), 1);
        assert_eq!(cfg.models[0].api_key, "sk-x");
        // 残留动作落到 Removed：不参与渲染执行，但不会让解析失败
        assert_eq!(
            cfg.actions.iter().filter(|a| **a == ActionKind::Removed).count(),
            2
        );
        assert!(cfg.actions.contains(&ActionKind::Speak));
    }
}
