/**
 * 设置窗：权限引导、划词手势参数、模型清单、关于与更新。
 *
 * 配置全量读出来在本地改，点「保存」整体写回 Rust（再落盘 config.json）。
 */
import { useEffect, useState } from 'react';

import { api, type ActionKind, type AppConfig, type ModelConfig, type SystemInfo, type Thinking, type VoiceInfo } from '../api';
import { Chevron, Spinner } from '../icons';
import { PROVIDERS, inferProvider, type ProviderPreset } from '../providers';

/** 工具栏动作的固定渲染顺序与文案（与后端 CANONICAL_ACTIONS 对应）。 */
/** say 的默认语速（约 175 字/分）。滑杆 0 居中 = 跟随默认。 */
const DEFAULT_RATE = 175;

/** 绝对语速 → 滑杆偏移（0 在中间）。旧配置里超范围的值夹到边界。 */
function rateOffset(rate: number): number {
  if (rate === 0) return 0;
  return Math.max(-100, Math.min(100, rate - DEFAULT_RATE));
}

/** 候选排序：中文最前、英文次之、其余靠后。 */
function localeRank(locale: string): number {
  if (locale.startsWith('zh')) return 0;
  if (locale.startsWith('en')) return 1;
  return 2;
}

const TOOLBAR_ACTIONS: { kind: ActionKind; label: string }[] = [
  { kind: 'search', label: 'AI 搜索' },
  { kind: 'translate', label: '翻译' },
  { kind: 'explain', label: '解释' },
  { kind: 'speak', label: '朗读' },
];

/** 思考强度选项。文案里写清各档的代价，免得用户要去翻文档。 */
const THINKING_OPTIONS: { value: Thinking; label: string }[] = [
  { value: 'auto', label: '思考：默认' },
  { value: 'off', label: '思考：关闭' },
  { value: 'low', label: '思考：低' },
  { value: 'high', label: '思考：高' },
  { value: 'max', label: '思考：最高' },
];

const blankModel = (): ModelConfig => ({
  id: crypto.randomUUID().slice(0, 8),
  name: '新模型',
  endpoint: 'http://localhost:11434/v1',
  model: '',
  api_key: '',
  enabled: true,
  primary: false,
  thinking: 'auto',
});

export function Settings() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [trusted, setTrusted] = useState(true);
  const [version, setVersion] = useState('');
  const [sys, setSys] = useState<SystemInfo | null>(null);
  const [status, setStatus] = useState('');
  /** cc-switch 导入的候选，挑中才进模型列表 */
  const [candidates, setCandidates] = useState<ModelConfig[] | null>(null);
  /** 系统已装的音色，中文的排前面 */
  const [voices, setVoices] = useState<VoiceInfo[]>([]);
  /** 每个模型卡的模型候选（实时拉取结果），键是模型 id */
  const [modelOptions, setModelOptions] = useState<Record<string, string[]>>({});
  /** 各模型卡的「刷新」是否在拉取中 */
  const [refreshing, setRefreshing] = useState<Record<string, boolean>>({});

  useEffect(() => {
    api.getConfig().then(setConfig).catch((e) => setStatus(String(e)));
    api.getAppVersion().then(setVersion).catch(() => {});
    api.getSystemInfo().then(setSys).catch(() => {});
    // 音色列表只用于候选展示，拉不到也不挡设置。中文排最前，方便挑。
    api.listVoices()
      .then((vs) =>
        setVoices(
          [...vs].sort(
            (a, b) =>
              localeRank(a.locale) - localeRank(b.locale) ||
              a.name.localeCompare(b.name),
          ),
        ),
      )
      .catch(() => {});

    // 权限可能在 App 运行期间被授予，轮询刷新状态。
    const check = () => api.accessibilityTrusted().then(setTrusted).catch(() => {});
    check();
    const timer = window.setInterval(check, 2000);
    return () => window.clearInterval(timer);
  }, []);

  if (!config) return <div className="settings loading">载入中…</div>;

  const patch = (p: Partial<AppConfig>) => setConfig({ ...config, ...p });

  const patchModel = (idx: number, p: Partial<ModelConfig>) => {
    const models = config.models.map((m, i) => (i === idx ? { ...m, ...p } : m));
    patch({ models });
  };

  /** 从服务拉真实模型清单，填进对应模型卡的候选。拉不到就静默保持预设。 */
  const refreshModels = async (endpoint: string, apiKey: string, modelId: string) => {
    if (!endpoint.trim()) return;
    setRefreshing((prev) => ({ ...prev, [modelId]: true }));
    try {
      const list = await api.fetchModels(endpoint, apiKey);
      setModelOptions((prev) => ({ ...prev, [modelId]: list }));
    } catch {
      // 留着预设清单
    } finally {
      setRefreshing((prev) => ({ ...prev, [modelId]: false }));
    }
  };

  /** 选定提供商：端点 / 默认模型 / 思考建议一次填好；有 Key（或本地服务）就直接拉真实清单。 */
  const applyProvider = (idx: number, preset: ProviderPreset) => {
    const m = config.models[idx];
    const p: Partial<ModelConfig> = {
      endpoint: preset.endpoint,
      model: preset.models[0] ?? '',
    };
    if (preset.thinking) p.thinking = preset.thinking;
    patchModel(idx, p);
    setModelOptions((prev) => ({ ...prev, [m.id]: preset.models }));
    if (m.api_key.trim() || !preset.needsKey) {
      refreshModels(preset.endpoint, m.api_key, m.id);
    }
  };

  /** 上/下移模型：顺序即优先级，第一个启用的模型默认展开。 */
  const moveModel = (idx: number, dir: -1 | 1) => {
    const to = idx + dir;
    if (to < 0 || to >= config.models.length) return;
    const models = [...config.models];
    [models[idx], models[to]] = [models[to], models[idx]];
    patch({ models });
  };

  const save = async () => {
    try {
      await api.saveConfig(config);
      setStatus('已保存');
    } catch (e) {
      setStatus(String(e));
    }
    window.setTimeout(() => setStatus(''), 1800);
  };

  const importFromCcSwitch = async () => {
    setStatus('读取 cc-switch…');
    try {
      const list = await api.importCcSwitch();
      setCandidates(list);
      setStatus(`找到 ${list.length} 个供应商`);
    } catch (e) {
      setCandidates(null);
      setStatus(String(e));
    }
    window.setTimeout(() => setStatus(''), 2500);
  };

  const adoptCandidate = (c: ModelConfig) => {
    // 同 id 已经加过就跳过，避免重复点
    if (config.models.some((m) => m.id === c.id)) return;
    patch({ models: [...config.models, { ...c, enabled: true }] });
  };

  const previewVoice = async () => {
    try {
      await api.previewVoice(config.tts_voice, config.tts_rate);
    } catch (e) {
      setStatus(String(e));
      window.setTimeout(() => setStatus(''), 2500);
    }
  };

  const checkUpdate = async () => {
    setStatus('检查更新中…');
    try {
      const { check } = await import('@tauri-apps/plugin-updater');
      const update = await check({ timeout: 30000 });
      if (!update) {
        setStatus('已是最新版本');
        return;
      }
      setStatus(`发现新版本 ${update.version}，下载中…`);
      await update.downloadAndInstall();
      setStatus('已安装，重启后生效');
    } catch (e) {
      setStatus(`检查更新失败：${e}`);
    }
  };

  return (
    <div className="settings">
      <header data-tauri-drag-region>
        <h1>Glean 设置</h1>
        <div className="actions">
          {status && <span className="status">{status}</span>}
          <button className="primary" onClick={save}>
            保存
          </button>
        </div>
      </header>

      {!trusted && (
        <section className="card warn">
          <h2>需要辅助功能权限</h2>
          <p>
            没有这个权限，全局鼠标钩子和跨应用取词都无法工作。到「系统设置 → 隐私与安全性 →
            辅助功能」里勾上 Glean，然后重启本应用。
          </p>
          <button onClick={() => api.openAccessibilitySettings()}>去授权</button>
        </section>
      )}

      <section className="card">
        <h2>划词</h2>
        <label className="row">
          <span>译入语</span>
          <input
            value={config.target_lang}
            onChange={(e) => patch({ target_lang: e.target.value })}
            placeholder="中文"
          />
        </label>
        <label className="row">
          <span>拖选阈值（像素）</span>
          <input
            type="number"
            min={1}
            value={config.drag_threshold}
            onChange={(e) => patch({ drag_threshold: Number(e.target.value) })}
          />
        </label>
        <label className="row">
          <span>取词延迟（毫秒）</span>
          <input
            type="number"
            min={0}
            step={10}
            value={config.settle_ms}
            onChange={(e) => patch({ settle_ms: Number(e.target.value) })}
          />
          <em>太短会读到上一次的选区</em>
        </label>
        <label className="row check">
          <input
            type="checkbox"
            checked={config.double_click_trigger}
            onChange={(e) => patch({ double_click_trigger: e.target.checked })}
          />
          <span>双击选词也触发</span>
        </label>
      </section>

      <section className="card">
        <h2>工具栏</h2>
        <p className="hint">勾选的动作才会出现在划词工具栏上，顺序固定。保存后立即生效。</p>
        <div className="action-toggles">
          {TOOLBAR_ACTIONS.map(({ kind, label }) => (
            <label className="check" key={kind}>
              <input
                type="checkbox"
                checked={config.actions.includes(kind)}
                onChange={(e) =>
                  patch({
                    actions: e.target.checked
                      ? [...config.actions, kind]
                      : config.actions.filter((k) => k !== kind),
                  })
                }
              />
              <span>{label}</span>
            </label>
          ))}
        </div>
      </section>

      <section className="card">
        <div className="card-head">
          <h2>模型</h2>
          <div className="btn-row">
            <button onClick={importFromCcSwitch}>从 cc-switch 导入</button>
            <button onClick={() => patch({ models: [...config.models, blankModel()] })}>
              添加模型
            </button>
          </div>
        </div>
        <p className="hint">
          选提供商 → 贴 API Key → 下拉选模型即可；「自定义」才需要手填端点。
          启用多个即可并排对比；列表顺序就是结果区顺序，第一个启用的默认展开，用卡片上的
          ↑↓ 调整。
          <br />
          「思考」默认不发任何参数、用服务端默认值。GLM-5.3 起始终思考且默认最高档，
          划词这种小任务选<strong>低</strong>会快很多；老的 GLM 推理模型才用得上
          <strong>关闭</strong>，对 5.3 发关闭会直接报 400。
        </p>
        {candidates && (
          <div className="candidates">
            <div className="candidates-head">
              <span>
                cc-switch 里的供应商（端点已按 OpenAI 协议换算过，加进来后核对一下模型名）
              </span>
              <button onClick={() => setCandidates(null)}>收起</button>
            </div>
            {candidates.map((c) => {
              const added = config.models.some((m) => m.id === c.id);
              return (
                <div className="candidate" key={c.id}>
                  <span className="cand-name">{c.name}</span>
                  <span className="cand-meta">{c.endpoint}</span>
                  <span className="cand-meta">{c.model || '（未填模型名）'}</span>
                  <button disabled={added} onClick={() => adoptCandidate(c)}>
                    {added ? '已添加' : '添加'}
                  </button>
                </div>
              );
            })}
          </div>
        )}
        {config.models.map((m, i) => (
          <div className="model-card" key={m.id}>
            <div className="model-top">
              <input
                className="name"
                value={m.name}
                onChange={(e) => patchModel(i, { name: e.target.value })}
              />
              <label className="check">
                <input
                  type="checkbox"
                  checked={m.enabled}
                  onChange={(e) => patchModel(i, { enabled: e.target.checked })}
                />
                <span>启用</span>
              </label>
              <div className="move">
                <button title="上移" disabled={i === 0} onClick={() => moveModel(i, -1)}>
                  <span className="rot up">
                    <Chevron size={11} />
                  </span>
                </button>
                <button
                  title="下移"
                  disabled={i === config.models.length - 1}
                  onClick={() => moveModel(i, 1)}
                >
                  <span className="rot down">
                    <Chevron size={11} />
                  </span>
                </button>
              </div>
              <button
                className="danger"
                onClick={() => patch({ models: config.models.filter((_, j) => j !== i) })}
              >
                删除
              </button>
            </div>
            <div className="model-grid">
              <select
                value={inferProvider(m.endpoint).id}
                onChange={(e) => {
                  const preset = PROVIDERS.find((p) => p.id === e.target.value);
                  if (preset) applyProvider(i, preset);
                }}
              >
                {PROVIDERS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </select>
              <input
                type="password"
                value={m.api_key}
                onChange={(e) => patchModel(i, { api_key: e.target.value })}
                placeholder={
                  inferProvider(m.endpoint).needsKey ? 'API Key' : 'API Key（本地可留空）'
                }
              />
              <div className="model-pick">
                <input
                  list={`model-opts-${m.id}`}
                  value={m.model}
                  onChange={(e) => patchModel(i, { model: e.target.value })}
                  placeholder="模型名（下拉选择或手填）"
                />
                <datalist id={`model-opts-${m.id}`}>
                  {(modelOptions[m.id] ?? inferProvider(m.endpoint).models).map((name) => (
                    <option key={name} value={name} />
                  ))}
                </datalist>
                <button
                  className="refresh"
                  title="从服务拉取模型列表"
                  disabled={refreshing[m.id]}
                  onClick={() => refreshModels(m.endpoint, m.api_key, m.id)}
                >
                  {refreshing[m.id] ? <Spinner size={12} /> : '刷新'}
                </button>
              </div>
              <select
                value={m.thinking ?? 'auto'}
                onChange={(e) => patchModel(i, { thinking: e.target.value as Thinking })}
              >
                {THINKING_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </div>
            {inferProvider(m.endpoint).id === 'custom' && (
              <input
                className="endpoint"
                value={m.endpoint}
                onChange={(e) => patchModel(i, { endpoint: e.target.value })}
                placeholder="端点，填到 /v1 为止，如 http://localhost:8080/v1"
              />
            )}
          </div>
        ))}
      </section>

      <section className="card">
        <h2>朗读</h2>
        <p className="hint">
          中文建议选 zh_CN 音色（如 Tingting / Yu-shu）。更多/更高质的音色在
          「系统设置 → 辅助功能 → 朗读内容 → 系统声音」里下载，装好后重开设置页。
        </p>
        <label className="row">
          <span>音色</span>
          <input
            list="tts-voices"
            value={config.tts_voice}
            onChange={(e) => patch({ tts_voice: e.target.value })}
            placeholder={`跟随系统${voices.length ? `（共 ${voices.length} 个可选）` : ''}`}
          />
          <datalist id="tts-voices">
            {voices.map((v) => (
              <option key={v.name + v.locale} value={v.name}>
                {v.locale}
              </option>
            ))}
          </datalist>
        </label>
        <div className="row">
          <span>语速</span>
          <div className="rate">
            <span className="rate-cap">慢</span>
            <input
              type="range"
              min={-100}
              max={100}
              step={5}
              value={rateOffset(config.tts_rate)}
              onChange={(e) => {
                const offset = Number(e.target.value);
                // 0（中间）存成 0 = 跟随系统默认；否则存绝对值 175±偏移
                patch({ tts_rate: offset === 0 ? 0 : DEFAULT_RATE + offset });
              }}
            />
            <span className="rate-cap">快</span>
            <span className="rate-value">
              {config.tts_rate === 0 ? '默认（约 175）' : `${config.tts_rate} 字/分`}
            </span>
          </div>
        </div>
        <div className="btn-row">
          <button onClick={previewVoice}>试听（用当前表单值）</button>
        </div>
      </section>

      <section className="card">
        <h2>关于</h2>
        <p className="mono">
          Glean {version}
          {sys && ` · ${sys.build_type} · ${sys.os_version} · ${sys.arch}`}
        </p>
        <div className="btn-row">
          <button onClick={checkUpdate}>检查更新</button>
          <button onClick={() => api.openConfigDir()}>打开配置目录</button>
        </div>
      </section>
    </div>
  );
}
