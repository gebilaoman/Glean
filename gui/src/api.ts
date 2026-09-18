/** Rust 侧命令与事件的类型化包装。前端只通过这里跟后端打交道。 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type ActionKind = 'translate' | 'explain' | 'search' | 'speak';

/** 思考强度。auto = 不发任何思考参数（兼容性最好）。 */
export type Thinking = 'auto' | 'off' | 'low' | 'high' | 'max';

export interface ModelConfig {
  id: string;
  name: string;
  endpoint: string;
  model: string;
  api_key: string;
  enabled: boolean;
  primary: boolean;
  thinking: Thinking;
}

export interface AppConfig {
  models: ModelConfig[];
  target_lang: string;
  drag_threshold: number;
  settle_ms: number;
  double_click_trigger: boolean;
  /** 工具栏上显示哪些动作（开关集合，渲染顺序固定）。 */
  actions: ActionKind[];
  /** 朗读音色，空 = 跟随系统默认。 */
  tts_voice: string;
  /** 朗读语速（每分钟字数），0 = 系统默认（约 175）。 */
  tts_rate: number;
}

/** 系统里装的一个朗读音色。 */
export interface VoiceInfo {
  name: string;
  locale: string;
}

export interface ModelBrief {
  id: string;
  name: string;
  primary: boolean;
}

/** `llm` 事件：多模型并发时按 model_id 归列，request_id 用来作废过期请求。 */
export interface LlmEvent {
  request_id: string;
  model_id: string;
  kind: 'delta' | 'done' | 'error';
  data: string;
}

export interface SystemInfo {
  build_type: string;
  platform: string;
  arch: string;
  os_version: string;
}

export const api = {
  runAction: (action: ActionKind, requestId: string) =>
    invoke<ModelBrief[]>('run_action', { action, requestId }),
  /**
   * 单列请求：只发那一列，其它列不动。
   * retry=true 重译（高温重抽）；retry=false 惰性展开的首次请求（常温）。
   */
  retryModel: (action: ActionKind, modelId: string, requestId: string, retry: boolean) =>
    invoke<void>('retry_model', { action, modelId, requestId, retry }),
  getSelection: () => invoke<string>('get_selection'),
  /** 朗读划词文本；再点一次停止。返回是否开始朗读。 */
  speakSelection: () => invoke<boolean>('speak_selection'),
  /** 从端点拉模型清单（GET /models），失败时前端回落预设。 */
  fetchModels: (endpoint: string, apiKey: string) =>
    invoke<string[]>('fetch_models', { endpoint, apiKey }),
  /** 系统已装的音色列表，设置页候选用。 */
  listVoices: () => invoke<VoiceInfo[]>('list_voices'),
  /** 用表单里未保存的音色/语速念一句样例。 */
  previewVoice: (voice: string, rate: number) =>
    invoke<boolean>('preview_voice', { voice, rate }),
  hidePanel: () => invoke<void>('hide_panel'),
  setPanelHeight: (height: number) => invoke<void>('set_panel_height', { height }),
  openSettings: () => invoke<void>('open_settings'),
  getConfig: () => invoke<AppConfig>('get_config'),
  saveConfig: (config: AppConfig) => invoke<void>('save_config', { config }),
  accessibilityTrusted: () => invoke<boolean>('accessibility_trusted'),
  openAccessibilitySettings: () => invoke<void>('open_accessibility_settings'),
  openConfigDir: () => invoke<void>('open_config_dir'),
  /** 读 cc-switch 的配置库，返回可导入的模型候选（未启用，由用户挑）。 */
  importCcSwitch: () => invoke<ModelConfig[]>('import_cc_switch'),
  getAppVersion: () => invoke<string>('get_app_version'),
  getSystemInfo: () => invoke<SystemInfo>('get_system_info'),
};

export const events = {
  onSelection: (cb: (text: string) => void): Promise<UnlistenFn> =>
    listen<{ text: string }>('selection', (e) => cb(e.payload.text)),
  onLlm: (cb: (e: LlmEvent) => void): Promise<UnlistenFn> =>
    listen<LlmEvent>('llm', (e) => cb(e.payload)),
  onPanelDismiss: (cb: () => void): Promise<UnlistenFn> =>
    listen('panel-dismiss', () => cb()),
  onHookError: (cb: (msg: string) => void): Promise<UnlistenFn> =>
    listen<string>('hook-error', (e) => cb(e.payload)),
  onTtsStarted: (cb: () => void): Promise<UnlistenFn> =>
    listen('tts-started', () => cb()),
  onTtsStopped: (cb: () => void): Promise<UnlistenFn> =>
    listen('tts-stopped', () => cb()),
  onConfigUpdated: (cb: () => void): Promise<UnlistenFn> =>
    listen('config-updated', () => cb()),
};
