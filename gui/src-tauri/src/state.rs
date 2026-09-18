//! 全局状态。核心是「划词文本一拿到就缓存」——后续所有动作都读缓存，
//! 不再依赖实时选区，这样点工具栏导致选区失焦也不影响。

use std::process::Child;

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
    /// 正在进行的朗读（`say` 子进程）。None = 没在念。
    pub speech: Mutex<Speech>,
}

#[derive(Default)]
pub struct Speech {
    child: Option<Child>,
    /// 轮次号。「停」会杀进程并 bump 它，看护线程据此知道自己盯的那轮已经没了。
    round: u64,
}

/// 看护线程每 200ms 问一次进度。
pub enum SpeechTick {
    /// 还在念。
    Running,
    /// 念完了（进程退出），状态已清。
    Done,
    /// 这轮被停掉或被新的一轮顶掉了，别再管它。
    Gone,
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
            speech: Mutex::new(Speech::default()),
        }
    }

    pub fn set_selection(&self, text: String) {
        *self.selection.lock() = text;
    }

    pub fn selection(&self) -> String {
        self.selection.lock().clone()
    }

    /// 有朗读在跑就杀掉。返回是否真的停了一个（调用方据此发 tts-stopped）。
    pub fn stop_speech(&self) -> bool {
        let mut s = self.speech.lock();
        match s.child.take() {
            Some(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
                s.round += 1;
                true
            }
            None => false,
        }
    }

    /// 登记新一轮朗读，返回轮次号。
    pub fn start_speech(&self, child: Child) -> u64 {
        let mut s = self.speech.lock();
        s.round += 1;
        let round = s.round;
        s.child = Some(child);
        round
    }

    /// 看护线程用：非阻塞地看一眼 `round` 这轮到什么状态了。
    pub fn speech_tick(&self, round: u64) -> SpeechTick {
        let mut s = self.speech.lock();
        if s.round != round {
            return SpeechTick::Gone;
        }
        match s.child.as_mut() {
            Some(c) => match c.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    s.child = None;
                    SpeechTick::Done
                }
                Ok(None) => SpeechTick::Running,
            },
            // round 对得上却没有进程：不该发生，当作已结束处理。
            None => {
                s.child = None;
                SpeechTick::Done
            }
        }
    }
}
