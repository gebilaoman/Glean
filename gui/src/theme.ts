/** 主题应用：把配置里的 theme/accent 落到 documentElement 的 data 属性上，
 *  CSS 侧按属性切换变量组。两个窗口（工具栏/设置）共用。 */

let current = { theme: 'auto', accent: 'blue' };

export function applyTheme(theme: string, accent: string) {
  current = { theme: theme || 'auto', accent: accent || 'blue' };
  const resolved =
    current.theme === 'light' || current.theme === 'dark'
      ? current.theme
      : window.matchMedia('(prefers-color-scheme: light)').matches
        ? 'light'
        : 'dark';
  document.documentElement.dataset.theme = resolved;
  document.documentElement.dataset.accent = current.accent;
}

export async function initTheme() {
  try {
    const c = await api.getConfig();
    applyTheme(c.theme, c.accent);
  } catch {
    // 拉不到配置就用默认深色
  }
  // 系统外观变化时，auto 模式跟随
  window
    .matchMedia('(prefers-color-scheme: light)')
    .addEventListener('change', () => applyTheme(current.theme, current.accent));
  // 设置页保存后即时生效
  events.onConfigUpdated(async () => {
    try {
      const c = await api.getConfig();
      applyTheme(c.theme, c.accent);
    } catch {}
  });
}

import { api, events } from './api';
