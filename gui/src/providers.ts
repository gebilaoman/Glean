/**
 * 内置模型提供商预设：选它 → 只贴个 Key，端点和模型都给好。
 * 模型清单是「拉不到时」的兜底；有 Key 时设置页会从服务的 /models 实时拉。
 */
import type { Thinking } from './api';

export interface ProviderPreset {
  id: string;
  label: string;
  /** OpenAI 兼容端点，填到 /v1（或等价路径）为止 */
  endpoint: string;
  /** 兜底模型清单（「自定义」为空，靠手填或实时拉取） */
  models: string[];
  /** 是否需要 API Key（本地推理服务不需要） */
  needsKey: boolean;
  /** 建议的思考档位；不填则不动用户的设置 */
  thinking?: Thinking;
}

export const PROVIDERS: ProviderPreset[] = [
  {
    id: 'zhipu',
    label: '智谱 GLM',
    endpoint: 'https://open.bigmodel.cn/api/paas/v4',
    models: ['glm-5.3', 'glm-5.1', 'glm-4.7'],
    needsKey: true,
    // GLM-5.3 起始终思考且默认最高档（最慢），低档快很多
    thinking: 'low',
  },
  {
    id: 'deepseek',
    label: 'DeepSeek',
    endpoint: 'https://api.deepseek.com/v1',
    models: ['deepseek-v4-flash', 'deepseek-chat', 'deepseek-reasoner'],
    needsKey: true,
  },
  {
    id: 'minimax',
    label: 'MiniMax',
    endpoint: 'https://api.minimaxi.com/v1',
    models: ['MiniMax-M2.7'],
    needsKey: true,
  },
  {
    id: 'ollama',
    label: 'Ollama（本地）',
    endpoint: 'http://localhost:11434/v1',
    models: [],
    needsKey: false,
  },
  {
    id: 'llamacpp',
    label: 'llama.cpp（本地）',
    endpoint: 'http://localhost:8080/v1',
    models: [],
    needsKey: false,
  },
  { id: 'custom', label: '自定义', endpoint: '', models: [], needsKey: true },
];

/** 按端点反推提供商（去掉尾部斜杠后精确匹配）；匹配不上 = 自定义。 */
export function inferProvider(endpoint: string): ProviderPreset {
  const ep = endpoint.trim().replace(/\/+$/, '');
  const hit = PROVIDERS.find(
    (p) => p.id !== 'custom' && ep === p.endpoint.replace(/\/+$/, ''),
  );
  // cc-switch 导入等路径来的端点可能带尾斜杠或大小写差异，这里给回一个可写的副本
  return hit ?? PROVIDERS[PROVIDERS.length - 1];
}
