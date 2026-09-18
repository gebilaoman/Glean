//! 配置读写。落盘在 `dirs::config_dir()/Glean/config.json`。

use std::path::PathBuf;

use glean_core::AppConfig;

pub fn config_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Glean");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// 读配置；文件不存在或解析失败都回落到默认配置（不让坏配置把应用卡死在启动）。
pub fn load() -> AppConfig {
    let path = config_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return AppConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|e| {
        eprintln!("[glean] 配置解析失败，回落默认值：{e}");
        AppConfig::default()
    })
}

pub fn save(config: &AppConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(config_path(), json).map_err(|e| e.to_string())
}

/// 保存动作落盘的目录：配置里指定了就用它，否则 `~/Documents/Glean`。
pub fn save_dir(config: &AppConfig) -> PathBuf {
    let dir = if config.save_dir.trim().is_empty() {
        dirs::document_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Glean")
    } else {
        PathBuf::from(config.save_dir.trim())
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}
