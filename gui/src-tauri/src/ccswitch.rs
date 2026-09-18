//! 从 cc-switch 导入模型配置。
//!
//! cc-switch 把各家供应商存在 `~/.cc-switch/cc-switch.db` 的 `providers` 表里，
//! `settings_config` 是一段 Claude Code 风格的 JSON：
//! `{"env": {"ANTHROPIC_BASE_URL": ..., "ANTHROPIC_AUTH_TOKEN": ..., "ANTHROPIC_MODEL": ...}}`。
//!
//! 那是 **Anthropic 协议**的地址，Glean 走的是 OpenAI 协议，所以要换端点；
//! 好在同一把 key 在两边通用。端点换算是启发式的（见 `to_openai_endpoint`），
//! 导入后请在设置页里核对一眼再保存。

use glean_core::ModelConfig;
use serde_json::Value;

/// 只读打开 cc-switch 的库（它自己可能正开着，别去写它）。
fn open_db() -> Result<rusqlite::Connection, String> {
    let path = dirs::home_dir()
        .ok_or("找不到 home 目录")?
        .join(".cc-switch/cc-switch.db");
    if !path.exists() {
        return Err("没找到 ~/.cc-switch/cc-switch.db，确认装了 cc-switch".into());
    }
    rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开 cc-switch 配置库失败：{e}"))
}

/// Anthropic 协议端点 → OpenAI 兼容端点。
///
/// 各家的 OpenAI 端点路径不统一，这里只覆盖常见几家，其余按「去掉 /anthropic 后缀、
/// 补上 /v1」处理。拿不准的让用户在设置页里改。
fn to_openai_endpoint(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.contains("open.bigmodel.cn") {
        // 智谱：Anthropic 走 /api/anthropic，OpenAI 走 /api/paas/v4
        return "https://open.bigmodel.cn/api/paas/v4".to_string();
    }
    let stripped = base.strip_suffix("/anthropic").unwrap_or(base);
    if stripped.ends_with("/v1") {
        stripped.to_string()
    } else {
        format!("{stripped}/v1")
    }
}

/// 读出所有可用的供应商，转成 Glean 的模型配置候选。
///
/// 返回的条目 `enabled: false`、`primary: false`，由用户在设置页里挑了再启用。
#[tauri::command]
pub fn import_cc_switch() -> Result<Vec<ModelConfig>, String> {
    let conn = open_db()?;
    // 同一个供应商在 claude / claude-desktop 下各有一行，只取 claude 那份免得重复。
    let mut stmt = conn
        .prepare("SELECT id, name, settings_config FROM providers WHERE app_type = 'claude' ORDER BY sort_index")
        .map_err(|e| format!("查询失败：{e}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("查询失败：{e}"))?;

    let mut out = Vec::new();
    for row in rows {
        let (id, name, raw) = row.map_err(|e| format!("读取失败：{e}"))?;
        let Ok(cfg) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let env = &cfg["env"];

        let token = env["ANTHROPIC_AUTH_TOKEN"]
            .as_str()
            .or_else(|| env["ANTHROPIC_API_KEY"].as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let base = env["ANTHROPIC_BASE_URL"].as_str().unwrap_or("").trim();
        // 官方登录态那种没 token 也没自定义地址的条目跳过。
        if token.is_empty() && base.is_empty() {
            continue;
        }

        let model = env["ANTHROPIC_MODEL"]
            .as_str()
            .or_else(|| cfg["model"].as_str())
            .unwrap_or("")
            .to_string();

        out.push(ModelConfig {
            // 带上前缀，避免和手工加的模型撞 id。
            id: format!("cc-{}", id.chars().take(8).collect::<String>()),
            name,
            endpoint: to_openai_endpoint(base),
            model,
            api_key: token,
            enabled: false,
            primary: false,
        });
    }

    if out.is_empty() {
        return Err("cc-switch 里没有可导入的供应商".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::to_openai_endpoint;

    #[test]
    fn maps_known_and_generic_endpoints() {
        assert_eq!(
            to_openai_endpoint("https://open.bigmodel.cn/api/anthropic"),
            "https://open.bigmodel.cn/api/paas/v4"
        );
        assert_eq!(
            to_openai_endpoint("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/v1"
        );
        // 已经是 OpenAI 端点的原样保留
        assert_eq!(
            to_openai_endpoint("http://localhost:8080/v1/"),
            "http://localhost:8080/v1"
        );
        // 光秃秃的地址补 /v1
        assert_eq!(
            to_openai_endpoint("http://localhost:11434"),
            "http://localhost:11434/v1"
        );
    }
}
