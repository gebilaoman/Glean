/**
 * 悬浮工具栏。
 *
 * 两个形态：收起时只有一条药丸工具栏；点翻译/解释/AI 搜索后在下方展开结果区，
 * 多个模型并列成若干段，主模型默认展开、其余折叠。
 *
 * 高度由前端量、Rust 摆位：内容一变就把实际高度报给 `set_panel_height`，
 * 由 Rust 按锚点重算位置（含贴边翻转），避免前端去猜屏幕边界。
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { api, events, type ActionKind, type ModelBrief } from '../api';
import {
  Chevron,
  Close,
  Copy,
  DragHandle,
  Explain,
  Logo,
  Refresh,
  Save,
  SearchAI,
  Spinner,
  Translate,
} from '../icons';

type Status = 'streaming' | 'done' | 'error';

interface Result {
  text: string;
  status: Status;
}

/** 走模型的三个动作；复制/保存是本地操作，不进结果区。 */
const LLM_ACTIONS: { kind: ActionKind; label: string; Icon: typeof Translate }[] = [
  { kind: 'search', label: 'AI 搜索', Icon: SearchAI },
  { kind: 'translate', label: '翻译', Icon: Translate },
  { kind: 'explain', label: '解释', Icon: Explain },
];

export function Spotlight() {
  const [selection, setSelection] = useState('');
  const [action, setAction] = useState<ActionKind | null>(null);
  const [models, setModels] = useState<ModelBrief[]>([]);
  const [results, setResults] = useState<Record<string, Result>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [toast, setToast] = useState('');

  // 每列只认自己当前那一路流：换动作时全部作废，重试时只换被点的那列，
  // 其它列照常收分片。凭动作 id 兜底放行「模型清单还没回来」的窗口期。
  const currentActionId = useRef('');
  const streamIds = useRef<Record<string, string>>({});
  const rootRef = useRef<HTMLDivElement>(null);

  const reset = useCallback(() => {
    currentActionId.current = '';
    streamIds.current = {};
    setAction(null);
    setModels([]);
    setResults({});
    setExpanded(new Set());
    setToast('');
  }, []);

  useEffect(() => {
    api.getSelection().then(setSelection).catch(() => {});

    const unlisten = Promise.all([
      events.onSelection((text) => {
        reset();
        setSelection(text);
      }),
      events.onPanelDismiss(reset),
      events.onLlm((e) => {
        const cur = streamIds.current[e.model_id];
        if (cur !== e.request_id && !(cur === undefined && e.request_id === currentActionId.current))
          return;
        setResults((prev) => {
          const cur = prev[e.model_id] ?? { text: '', status: 'streaming' as Status };
          if (e.kind === 'delta') {
            return { ...prev, [e.model_id]: { text: cur.text + e.data, status: 'streaming' } };
          }
          if (e.kind === 'done') {
            return { ...prev, [e.model_id]: { ...cur, status: 'done' } };
          }
          return { ...prev, [e.model_id]: { text: e.data, status: 'error' } };
        });
      }),
    ]);
    return () => {
      unlisten.then((fns) => fns.forEach((f) => f()));
    };
  }, [reset]);

  // 内容高度变了就报给 Rust，由它重新摆放窗口。
  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const report = () => api.setPanelHeight(Math.ceil(el.getBoundingClientRect().height)).catch(() => {});
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 面板是非激活 NSPanel，点过之后才拿得到键盘事件；Esc 收起。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') api.hidePanel().catch(() => {});
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const runLlm = async (kind: ActionKind) => {
    const id = crypto.randomUUID();
    currentActionId.current = id;
    streamIds.current = {};
    setAction(kind);
    setResults({});
    try {
      const briefs = await api.runAction(kind, id);
      if (currentActionId.current !== id) return;
      setModels(briefs);
      // 模型清单回来了，给每列登记本轮流 id；在那之前 onLlm 凭动作 id 放行。
      streamIds.current = Object.fromEntries(briefs.map((m) => [m.id, id]));
      // 主模型默认展开；没有标主模型时展开第一个。
      const first = briefs.find((m) => m.primary) ?? briefs[0];
      setExpanded(new Set(first ? [first.id] : []));
    } catch (e) {
      if (currentActionId.current !== id) return;
      setModels([{ id: '__error__', name: '出错了', primary: true }]);
      setResults({ __error__: { text: String(e), status: 'error' } });
      setExpanded(new Set(['__error__']));
    }
  };

  /** 单列重试：清掉该列结果重新流式；旧流的分片会被 streamIds 挡在外面。 */
  const retryModel = async (modelId: string) => {
    if (!action) return;
    const id = crypto.randomUUID();
    streamIds.current[modelId] = id;
    setResults((prev) => ({ ...prev, [modelId]: { text: '', status: 'streaming' } }));
    setExpanded((prev) => new Set(prev).add(modelId));
    try {
      await api.retryModel(action, modelId, id);
    } catch (e) {
      if (streamIds.current[modelId] !== id) return;
      setResults((prev) => ({ ...prev, [modelId]: { text: String(e), status: 'error' } }));
    }
  };

  const flash = (msg: string, thenHide = false) => {
    setToast(msg);
    window.setTimeout(() => {
      setToast('');
      if (thenHide) api.hidePanel().catch(() => {});
    }, 900);
  };

  const onCopy = async () => {
    try {
      await api.copySelection();
      flash('已复制', true);
    } catch (e) {
      flash(String(e));
    }
  };

  const onSave = async () => {
    try {
      const path = await api.saveSelection();
      flash(`已存到 ${path.split('/').pop()}`);
    } catch (e) {
      flash(String(e));
    }
  };

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className="spotlight" ref={rootRef}>
      <div className="toolbar">
        <button
          className="grip"
          title="拖动"
          onMouseDown={(e) => {
            e.preventDefault();
            getCurrentWindow().startDragging().catch(() => {});
          }}
        >
          <DragHandle />
        </button>

        <button className="brand" title="Glean 设置" onClick={() => api.openSettings().catch(() => {})}>
          <Logo />
        </button>

        <span className="sep" />

        {LLM_ACTIONS.map(({ kind, label, Icon }) => (
          <button
            key={kind}
            className={`item${action === kind ? ' active' : ''}`}
            onClick={() => runLlm(kind)}
          >
            <Icon />
            <span>{label}</span>
          </button>
        ))}

        <button className="item" onClick={onSave}>
          <Save />
          <span>保存</span>
        </button>

        <button className="item" onClick={onCopy}>
          <Copy />
          <span>复制</span>
        </button>

        <span className="sep" />

        <button className="item icon-only" title="关闭" onClick={() => api.hidePanel().catch(() => {})}>
          <Close />
        </button>
      </div>

      {toast && <div className="toast">{toast}</div>}

      {action && (
        <div className="results">
          <div className="quote" title={selection}>
            {selection}
          </div>
          {models.length === 0 && (
            <div className="pending">
              <Spinner /> 正在连接模型…
            </div>
          )}
          {models.map((m) => {
            const r = results[m.id];
            const open = expanded.has(m.id);
            return (
              <section key={m.id} className={`model${open ? ' open' : ''}`}>
                {/* div 而不是 button：里面还要嵌重试按钮，button 不允许嵌套 */}
                <div className="model-head" onClick={() => toggle(m.id)}>
                  <span className="chevron">
                    <Chevron open={open} />
                  </span>
                  <span className="model-name">{m.name}</span>
                  {m.primary && <span className="badge">主</span>}
                  {r?.status === 'streaming' && <Spinner size={12} />}
                  {r?.status === 'error' && <span className="badge err">失败</span>}
                  {m.id !== '__error__' && (
                    <button
                      className="retry"
                      title={
                        action === 'translate'
                          ? '重新翻译：换个译法再抽一次'
                          : '重新生成'
                      }
                      onClick={(e) => {
                        e.stopPropagation();
                        retryModel(m.id);
                      }}
                    >
                      <Refresh />
                      {action === 'translate' ? '重译' : '重试'}
                    </button>
                  )}
                </div>
                {open && (
                  <div className={`model-body${r?.status === 'error' ? ' err' : ''}`}>
                    {r?.text || (r?.status === 'error' ? '' : '…')}
                  </div>
                )}
              </section>
            );
          })}
        </div>
      )}
    </div>
  );
}
