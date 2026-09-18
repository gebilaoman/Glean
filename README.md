# Glean

系统级划词工具：在任意应用里选中文字，光标附近弹出一条不抢焦点的悬浮工具栏，
就地翻译 / 解释 / AI 搜索 / 复制 / 保存。多个模型可并排对比。

技术栈：Tauri 2 + Rust + React/TypeScript。目前主攻 macOS。

## 四层结构

| 层 | 位置 | 做什么 |
| --- | --- | --- |
| 触发 | `gui/src-tauri/src/selection.rs` | 自建 CGEventTap（只订阅左键按下/松开），把 mousedown/mouseup 合成「拖选」「双击选词」手势 |
| 取词 | 同上 | `get-selected-text`：macOS AX / Windows UIA，取不到时回落模拟 `Cmd+C` |
| 悬浮窗 | `gui/src-tauri/src/panel.rs` | `tauri-nspanel` 把窗口换成非激活 `NSPanel`，光标处定位、边缘翻转 |
| 动作 | `gui/src-tauri/src/actions.rs` | 多模型并发流式调用（OpenAI 兼容），复制与保存走本地 |

前端两个页面共用一份产物，用 hash 区分：`index.html` 是悬浮工具栏，
`index.html#/settings` 是设置窗。

## 跑起来

```bash
cd gui
pnpm install
pnpm tauri:dev
```

首次运行要到「系统设置 → 隐私与安全性 → 辅助功能」里勾上 Glean（开发模式勾的是终端
或 `glean-gui` 可执行文件），否则鼠标钩子起不来、AX 也取不到词。授权后重启应用。

应用没有 Dock 图标，入口在菜单栏托盘图标里（设置 / 退出）。

其它常用命令：

```bash
cargo check -p glean-gui   # 只编 Rust
cd gui && pnpm build       # 只编前端（含 tsc 类型检查）
cd gui && pnpm tauri:build # 出包
```

> 仓库根的 `rust-toolchain.toml` 把工具链钉在原生 `aarch64` 宿主上。依赖链里的
> bindgen 需要与 Xcode 同架构的 libclang，用 Rosetta 下的 x86_64 工具链会在
> `appkit-nsworkspace-bindings` 上直接编译失败。

## 配置

落盘在 `~/Library/Application Support/Glean/config.json`，也可以在设置窗里改。

设置页的「从 cc-switch 导入」会读 `~/.cc-switch/cc-switch.db`（只读），把里面的供应商
转成模型候选：Anthropic 协议的端点会换算成 OpenAI 协议的（智谱 → `/api/paas/v4`，
其余按「去掉 `/anthropic`、补 `/v1`」处理），同一把 key 两边通用。换算是启发式的，
加进来之后核对一下端点和模型名。

模型清单是一个数组，端点填到 `/v1` 为止，兼容 llama.cpp / Ollama / vLLM /
智谱 / DeepSeek / OpenRouter：

```json
{
  "models": [
    {
      "id": "local",
      "name": "本地 Qwen3",
      "endpoint": "http://localhost:11434/v1",
      "model": "qwen3:8b",
      "api_key": "",
      "enabled": true,
      "primary": true
    }
  ],
  "target_lang": "中文",
  "drag_threshold": 5.0,
  "settle_ms": 120,
  "double_click_trigger": true,
  "save_dir": ""
}
```

- `drag_threshold`：位移小于它的鼠标动作当普通点击丢弃。
- `settle_ms`：松开鼠标到取词之间的等待。太短会读到上一次的选区，觉得取词「慢半拍」就调大它。
- 保存动作按天追加到 `save_dir/YYYY-MM-DD.md`，留空则用 `~/Documents/Glean`。

## 发版

1. 三处版本号同步：`gui/package.json`、`gui/src-tauri/tauri.conf.json`、
   `gui/src-tauri/Cargo.toml`（改完跑一次 `cargo check` 让根 `Cargo.lock` 跟上）。
2. `git tag -a vX.Y.Z -m "..." && git push origin vX.Y.Z`
3. GitHub Actions 出 macOS universal 包，挂 **draft** release，要去 GitHub 手动 Publish，
   否则 updater 拿不到 `latest.json`。

**首次发版前必须做**：生成 updater 签名密钥对。

```bash
cd gui && pnpm tauri signer generate -w ~/.tauri/glean.key
```

- 公钥填进 `gui/src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`
  （现在是占位符 `PLACEHOLDER_REPLACE_WITH_TAURI_SIGNER_PUBKEY`）。
- 私钥内容与密码分别存进仓库 Secrets：`TAURI_SIGNING_PRIVATE_KEY`、
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。
- `plugins.updater.endpoints` 里的仓库地址按实际仓库改。

## 已知限制

- **没有 Apple Developer ID 签名与公证**，CI 包只有 updater 签名。签名哈希每次变 →
  TCC 授权不持久 → 每次更新后要重新授予辅助功能权限。根治要配 `bundle.macOS`
  的签名身份 + 公证。
- `gui/src-tauri/icons/` 目前是占位图标，需要替换。
- Windows / Linux 的悬浮窗只是普通置顶无边框窗，没有「不抢焦点」这层保证；
  Wayland 下全局输入监听受限，暂不支持。
- 取词对自绘 UI（部分 Electron 应用、游戏）可能失败，这时会回落到模拟复制；
  再失败就静默放弃，不弹窗。
- 朗读（TTS）尚未实现。

## 为什么没用 rdev

设计文档里原本选的是 `rdev` 做全局鼠标钩子，实际用下来有两个硬伤，所以换成了自建
CGEventTap（`selection.rs`）：

1. **敲键盘会让进程崩溃。** rdev 0.5.3 的事件掩码写死了，把键盘事件也一并订阅，
   而它的 `convert()` 会调 HIToolbox 的 `TSMGetInputSourceProperty` 查键盘布局——
   那个 API 断言必须跑在主队列上，事件 tap 却在自己的线程上，于是只要工具开着时
   敲一下键盘就 SIGTRAP。掩码不可配，绕不过去。
2. **拖拽期间拿不到坐标。** rdev 的 macOS `convert()` 只处理 `kCGEventMouseMoved`，
   把 `kCGEventLeftMouseDragged` 丢掉了，按住左键拖动的整个过程一个坐标都不给，
   拿它算拖选位移永远是 0——划词根本不会触发。

自建 tap 只订阅 `LeftMouseDown` / `LeftMouseUp`，坐标直接取自事件本身，两个问题一起没了；
另外还处理了 `TapDisabledByTimeout`（系统停掉 tap 后自动重新启用）。
