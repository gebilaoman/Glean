/** Rust 侧命令与事件的类型化包装。前端只通过这里跟后端打交道。 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type ActionKind = 'translate' | 'explain' | 'search' | 'copy' | 'save';

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
  save_dir: string;
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
  /** 单个模型重试：只重发那一列，其它列的结果不动。 */
  retryModel: (action: ActionKind, modelId: string, requestId: string) =>
    invoke<void>('retry_model', { action, modelId, requestId }),
  getSelection: () => invoke<string>('get_selection'),
  copySelection: () => invoke<void>('copy_selection'),
  saveSelection: () => invoke<string>('save_selection'),
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
};
