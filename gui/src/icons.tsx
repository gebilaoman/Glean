/** 工具栏图标。统一 18×18、1.6 描边、currentColor，和 UI 参考图对齐。 */

type P = { size?: number };

const base = (size = 18) => ({
  width: size,
  height: size,
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.6,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
});

/** 左侧拖拽手柄：两列三行的点阵。 */
export const DragHandle = ({ size = 14 }: P) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
    {[6, 12, 18].map((cy) =>
      [9, 15].map((cx) => <circle key={`${cx}-${cy}`} cx={cx} cy={cy} r="1.6" />),
    )}
  </svg>
);

/** 品牌标记：一枚圆形渐变徽标。 */
export const Logo = ({ size = 22 }: P) => (
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden>
    <defs>
      <linearGradient id="glean-logo" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0%" style={{ stopColor: 'var(--accent-1)' }} />
        <stop offset="100%" style={{ stopColor: 'var(--accent-2)' }} />
      </linearGradient>
    </defs>
    <circle cx="12" cy="12" r="11" fill="url(#glean-logo)" />
    <path
      d="M15.6 9.2a4.6 4.6 0 1 0 .7 4.2h-3.6"
      fill="none"
      stroke="#fff"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </svg>
);

/** AI 搜索：放大镜 + 一点星芒。 */
export const SearchAI = ({ size }: P) => (
  <svg {...base(size)} aria-hidden>
    <circle cx="10.5" cy="10.5" r="6" />
    <path d="M15 15l4.5 4.5" />
    <path d="M18.2 4.2l.7 1.9 1.9.7-1.9.7-.7 1.9-.7-1.9-1.9-.7 1.9-.7z" fill="currentColor" stroke="none" />
  </svg>
);

/** 翻译：圆圈里一个 A。 */
export const Translate = ({ size }: P) => (
  <svg {...base(size)} aria-hidden>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M9.2 15.2L12 8.4l2.8 6.8" />
    <path d="M10.1 13.2h3.8" />
  </svg>
);

/** 解释：圆圈里一个问号。 */
export const Explain = ({ size }: P) => (
  <svg {...base(size)} aria-hidden>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M9.9 9.7a2.1 2.1 0 1 1 2.6 2.3c-.6.2-.9.7-.9 1.3v.4" />
    <path d="M12 16.6h.01" strokeWidth="2" />
  </svg>
);

/** 保存：一张带横线的纸。 */
/** 朗读：喇叭。 */
export const Speaker = ({ size }: P) => (
  <svg {...base(size)} aria-hidden>
    <path d="M4 9.6v4.8h3l4 3.4V6.2l-4 3.4H4z" />
    <path d="M14.4 9.4a3.6 3.6 0 0 1 0 5.2" />
    <path d="M16.6 7.2a6.6 6.6 0 0 1 0 9.6" />
  </svg>
);

/** 重试：环形箭头。 */
export const Refresh = ({ size = 12 }: P) => (
  <svg {...base(size)} aria-hidden>
    <path d="M19.4 12a7.4 7.4 0 1 1-2.17-5.2" />
    <path d="M19.4 3.6v3.6h-3.6" />
  </svg>
);

export const Save = ({ size }: P) => (
  <svg {...base(size)} aria-hidden>
    <rect x="4.5" y="4.5" width="15" height="15" rx="2.5" />
    <path d="M8 9.5h8M8 13h8M8 16.2h4.5" />
  </svg>
);

/** 复制：两张叠起来的纸。 */
export const Copy = ({ size }: P) => (
  <svg {...base(size)} aria-hidden>
    <rect x="9" y="9" width="10.5" height="10.5" rx="2.2" />
    <path d="M15 6.5A2.5 2.5 0 0 0 12.5 4h-6A2.5 2.5 0 0 0 4 6.5v6A2.5 2.5 0 0 0 6.5 15" />
  </svg>
);

/** 关闭：圆圈里一个叉。 */
export const Close = ({ size }: P) => (
  <svg {...base(size)} aria-hidden>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M9.6 9.6l4.8 4.8M14.4 9.6l-4.8 4.8" />
  </svg>
);

/** 折叠箭头。用 SVG 而不是 ▸/▾ 字符——后者在系统字体下会渲染成一个小点。 */
export const Chevron = ({ size = 10, open = false }: P & { open?: boolean }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 12 12"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.8"
    strokeLinecap="round"
    strokeLinejoin="round"
    style={{ transform: open ? 'rotate(90deg)' : undefined, transition: 'transform 0.12s ease' }}
    aria-hidden
  >
    <path d="M4.5 2.5L8 6l-3.5 3.5" />
  </svg>
);

export const Spinner = ({ size = 14 }: P) => (
  <svg width={size} height={size} viewBox="0 0 24 24" className="spin" aria-hidden>
    <circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2.6" />
    <path d="M21 12a9 9 0 0 0-9-9" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" />
  </svg>
);
