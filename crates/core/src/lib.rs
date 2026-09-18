//! Glean 核心：与 UI / Tauri 无关的纯逻辑。
//!
//! 目前只有两块：配置数据模型（`config`）和 OpenAI 兼容的流式对话客户端（`llm`）。
//! 放在独立 crate 里是为了能脱离 Tauri 单测，也方便以后接 CLI。

pub mod config;
pub mod llm;

pub use config::{ActionKind, AppConfig, ModelConfig};
pub use llm::{StreamEvent, stream_chat};
