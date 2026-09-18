//! OpenAI 兼容的流式对话客户端。
//!
//! 只依赖 `/chat/completions` + `stream: true`，因此 llama.cpp / Ollama / vLLM /
//! 智谱 / DeepSeek / OpenRouter 都能直接接。调用方拿到的是一串 `StreamEvent`，
//! 由 GUI 侧转成 Tauri event 推给前端逐字渲染。

use futures::StreamExt;
use serde::Serialize;

use crate::config::ModelConfig;

/// 流式回调收到的事件。
#[derive(Debug, Clone, Serialize)]
pub enum StreamEvent {
    /// 新的一段文本增量。
    Delta(String),
    /// 正常结束。
    Done,
    /// 出错（网络、鉴权、模型名不对等），附人类可读原因。
    Error(String),
}

/// 发一次流式请求，每拿到一段增量就回调一次 `on_event`。
///
/// 函数自身不返回错误：失败也走 `StreamEvent::Error` 回调，这样多模型并发时
/// 一路挂掉不会影响其它列，前端也能就地把错误显示在对应的列里。
pub async fn stream_chat<F>(
    client: &reqwest::Client,
    model: &ModelConfig,
    system: &str,
    user: &str,
    mut on_event: F,
) where
    F: FnMut(StreamEvent) + Send,
{
    let url = format!(
        "{}/chat/completions",
        model.endpoint.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "model": model.model,
        "stream": true,
        "temperature": 0.2,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });

    let mut req = client.post(&url).json(&body);
    if !model.api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", model.api_key.trim()));
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            on_event(StreamEvent::Error(format!("请求失败：{e}")));
            return;
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        let detail: String = detail.chars().take(300).collect();
        on_event(StreamEvent::Error(format!("{status}：{detail}")));
        return;
    }

    // SSE 分片不保证按行对齐，用 buf 攒着按 \n 切。
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                on_event(StreamEvent::Error(format!("读取流失败：{e}")));
                return;
            }
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(idx) = buf.find('\n') {
            let line = buf[..idx].trim().to_string();
            buf.drain(..=idx);
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() {
                continue;
            }
            if payload == "[DONE]" {
                on_event(StreamEvent::Done);
                return;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            // 有的服务（如带 thinking 的模型）会先吐 reasoning_content，这里只取正文。
            if let Some(delta) = v["choices"][0]["delta"]["content"].as_str()
                && !delta.is_empty()
            {
                on_event(StreamEvent::Delta(delta.to_string()));
            }
        }
    }

    // 有的服务流结束不发 [DONE]，直接断开，这里补一个正常结束。
    on_event(StreamEvent::Done);
}
