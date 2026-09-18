//! 版本与系统信息，供设置页的「关于/检查更新」使用。更新下载安装走前端的
//! `@tauri-apps/plugin-updater`，这里只提供展示信息。

use serde::{Deserialize, Serialize};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tauri::command]
pub fn get_app_version() -> String {
    CURRENT_VERSION.to_string()
}

#[tauri::command]
pub fn get_system_info() -> SystemInfo {
    SystemInfo {
        build_type: if cfg!(debug_assertions) {
            "Debug"
        } else {
            "Release"
        }
        .to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        os_version: os_version(),
    }
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(out) = Command::new("sw_vers").arg("-productVersion").output()
            && let Ok(v) = String::from_utf8(out.stdout)
        {
            return format!("macOS {}", v.trim());
        }
        "macOS".to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::consts::OS.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub build_type: String,
    pub platform: String,
    pub arch: String,
    pub os_version: String,
}
