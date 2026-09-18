import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { Settings } from './settings/Settings';
import { Spotlight } from './spotlight/Spotlight';
import './styles.css';

// 两个窗口共用一份产物，用 hash 区分：设置窗的 url 是 index.html#/settings。
const isSettings = window.location.hash.startsWith('#/settings');
document.body.dataset.window = isSettings ? 'settings' : 'spotlight';

createRoot(document.getElementById('root')!).render(
  <StrictMode>{isSettings ? <Settings /> : <Spotlight />}</StrictMode>,
);
