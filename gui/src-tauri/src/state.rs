//! 全局状态。核心是「划词文本一拿到就缓存」——后续所有动作都读缓存，
//! 不再依赖实时选区，这样点工具栏导致选区失焦也不影响。

use glean_core::AppConfig;
use parking_lot::{Mutex, RwLock};

use crate::panel::PanelGeometry;

pub struct AppState {
    pub config: RwLock<AppConfig>,
    /// 最近一次成功取到的划词文本。
    selection: Mutex<String>,
    pub geometry: PanelGeometry,
    /// 复用连接，避免每次动作都重新握手。
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: RwLock::new(config),
            selection: Mutex::new(String::new()),
            geometry: PanelGeometry::default(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("构建 HTTP client 失败"),
        }
    }

    pub fn set_selection(&self, text: String) {
        *self.selection.lock() = text;
    }

    pub fn selection(&self) -> String {
        self.selection.lock().clone()
    }
}
