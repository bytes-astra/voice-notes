# VoiceNotes · 项目级 AI Agent 与贡献指南 (AGENTS.md)

欢迎！如果你是协助人类开发者参与本项目贡献的 AI Agent，或是想了解项目结构的贡献者，请务必阅读以下开发流与规范。

**VoiceNotes** = Tauri 2 桌面应用（macOS / Windows）。前端原生 JS + ES Modules（无打包器、无框架、无 TypeScript），后端 Rust（sherpa-onnx 本地推理，CPU 即可）。

## 📍 文档地图

本文件是**地图**，不是手册：只放每次都要遵守的硬约束与命令；细节在专题文档里，**按需再读**。

| 你要改什么 | 先读哪份 |
| --- | --- |
| 前端模块、i18n 与文案、文本分层、错误码、控件、macOS 原生层 | [`docs/FRONTEND.md`](docs/FRONTEND.md) |
| 后端分层、命令与运行时、事件契约、落盘、日志、音频采集 | [`docs/BACKEND.md`](docs/BACKEND.md) |
| 缺口标记、整段 / 流式识别、模型与下载 | [`docs/ASR.md`](docs/ASR.md) |
| 云端模式（provider、连接列表、协议、凭据） | [`docs/CLOUD.md`](docs/CLOUD.md) |
| CI、首次编译、版本发布、打包与签名 | [`docs/RELEASE.md`](docs/RELEASE.md) |
| 质量探针、基线、界面检查怎么跑 | [`docs/HOW_TO_VERIFY.md`](docs/HOW_TO_VERIFY.md) |
| 复审的固定作业面（每轮从「⚠️ 守卫薄弱 / ❌ 未审」的条目开始，**别另起 Focus 清单**） | [`docs/REVIEW-CHECKLIST.md`](docs/REVIEW-CHECKLIST.md) |
| 「能录音、没字」怎么查 | [`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md) |
| 用户可见的功能与安装说明 | [`README.md`](README.md) |

## 📋 OpenSpec 规划

规划在独立 store（不在仓库里，仓库没有 `openspec/` 目录），id `voice-notes`。跑任何 `openspec` 命令都要带 `--store voice-notes`，例如 `openspec validate <change> --store voice-notes`。本机未注册该 store 时先 `openspec store setup voice-notes --path <path>`。

## 🎯 核心架构与原则

- **源文件驱动**：前端逻辑在 `src/`（原生 JS + ES Modules，浏览器直接跑，没有打包器），后端逻辑在 `src-tauri/src/`（Rust）。
- **构建工具链**：Tauri 2 CLI（`@tauri-apps/cli`）+ cargo；前端没有打包器、框架、TypeScript。
- **禁止直接修改产物**：`src-tauri/target/`、`src-tauri/gen/schemas/` 是构建产物（已 gitignore），`docs/screenshots/*.png` 由 `npm run shots` 生成。**永远不要直接编辑这些文件** —— 改逻辑改源文件，改截图跑脚本。
- **单向依赖**：前端不能反、也不能成环，完整链路见 [`docs/FRONTEND.md`](docs/FRONTEND.md)。
- **单一事实源**：同一件事只写一处 —— 用户可见的功能与安装说明在 `README.md`，验证与基线在 `docs/HOW_TO_VERIFY.md`，工程约定在各专题文档，本文件只放硬约束与索引。

## 🚫 硬约束（违反就是错）

- **后端只返回字典 key**，不返回中文展示文案：`crate::code("record.empty")` / `crate::code_detail("record.micError", e)`；新增错误码必须同时进 `src/locales/zh-CN.js` 与 `src/locales/en.js`。
- **界面文案 = 一句中性的话 + 用户看得懂的参数**；技术细节只进日志（`code_detail()` / `logEvent('ERROR', …)`），字典里不放 `{error}` 占位符。
- **事件名只出现在 `src/events.js`**（`audio:level` / `audio:error` / `model:progress` / `engine:state` / `stream:partial` / `stream:segment` / `streaming:state`），改名字要同步前端。
- **命令参数名用 camelCase**（`audioBase64` / `fileName`）。
- **推理是同步阻塞的**，一律放进 `tauri::async_runtime::spawn_blocking`，别阻塞 async 执行器。
- **`lib.rs` 只放命令层**；新逻辑先找对应模块（`record.rs` / `engine.rs` / `asr.rs` / `audio.rs` / `decode.rs` / `models.rs` / `output.rs` / `settings.rs` / `log.rs` / `menu.rs`）。
- **`record.rs` 不认识 Tauri** —— 结果走 `emit` 回调，`AppHandle` 不进 `record.rs`。
- **经典脚本 `i18n.js` / `select.js` 不能被 import、不能加 `async`、不能写成 `type="module"`** —— 违反会让界面直接挂。
- **Rust 单元测试不进源文件**：放同目录子文件夹（`src/asr.rs` 里写 `#[cfg(test)] mod tests;`，实现放 `src/asr/tests.rs`），`cargo test --lib` 照样跑得到；`src-tauri/tests/` 是集成测试，只碰得到 `pub` 项。
- **改命令名、参数名、`Status` 字段、前端 import 图、文案、拼句规则** → 对应门禁必跑（下面那张表）。

## ⚡ 关键不变式（改代码前对一眼，细节在专题文档）

- **缺口与时间轴**：丢掉音频占真实录音时间；同一时刻**缺口排在句子前面**；流式句子起点过 `Timeline::advance_dropped()`（起点上的缺口要移、终点上的不移）→ [`docs/ASR.md`](docs/ASR.md)
- **整段**：切分阈值 **90 秒**；VAD 缓冲 **60 秒**；短句合并 **< 3 秒**、间隔 **> 2 秒**不合并 → [`docs/ASR.md`](docs/ASR.md)
- **流式**：分片恒 **200 ms / 3200 样本**且只在 `is_ready()` 为真时 `decode()`；收尾补 **300 ms** 静音；文本从 token 列表重建，不用 sherpa 的 `text` → [`docs/ASR.md`](docs/ASR.md)
- **热词只能用于流式**（SenseVoice 上给热词或 beam 会 `_Exit(-1)`）；按流式模型 **1426 字**单字词表过滤 → [`docs/ASR.md`](docs/ASR.md)
- **VAD 共用参数只允许 `max_speech_duration` 不同**：整段 **30 秒**、流式 **20 秒**，**不要统一** → [`docs/ASR.md`](docs/ASR.md)
- **两个槽一起登记**（采集器 + 会话，锁序 采集器 → 会话）→ [`docs/BACKEND.md`](docs/BACKEND.md)
- **引擎按身份缓存**：整段 `(模型目录, language, itn)`、流式 `hotwords`；装引擎要**锁内读设置** → [`docs/BACKEND.md`](docs/BACKEND.md)
- **拼句规则前后端必须一致**（中日韩之间直接连，其余加空格）→ [`docs/BACKEND.md`](docs/BACKEND.md)
- **事件载荷是增量的**；会话 ID 贯穿分段 / partial / 采集错误 → [`docs/BACKEND.md`](docs/BACKEND.md)
- **采集缓冲是消费式的**（内存恒定）；电平表走独立 `LevelRing`；安全阀 **60 秒** → [`docs/BACKEND.md`](docs/BACKEND.md)
- **换模型必须同时改 bytes 和 sha256**；下载镜像优先 → [`docs/BACKEND.md`](docs/BACKEND.md)
- **macOS 不开 `transparent: true`、不用 Liquid Glass**；四个 ACL 权限缺一不可（`set-theme` / `set-background-color` / `set-title` / `start-dragging`）→ [`docs/FRONTEND.md`](docs/FRONTEND.md)
- **云端是显式 opt-in**：音频会上传，本地两条路径的行为**一个字都不许改**；凭据只存 settings，不进日志、不回传前端 → [`docs/CLOUD.md`](docs/CLOUD.md)
- **句子时间以本地 16 kHz 时钟为准**（provider 时间戳不覆盖本地轴）→ [`docs/CLOUD.md`](docs/CLOUD.md)

## 🛠️ 门禁与验证

```bash
npm install
npm run dev            # 开发模式
npm run build          # 打包（→ src-tauri/target/release/bundle/）
```

| 命令 | 守什么 | 什么时候必跑 |
| --- | --- | --- |
| `npm run check` | 全部门禁：i18n → 依赖图 → 拼句规则 → 前端纯函数 → 契约 → 打包配置 → async 阻塞 → clippy（`-D warnings`）→ `cargo test --lib` | 提交前 |
| `npm run check:i18n` | 字典对齐 + 后端错误码 + 裸字符串错误 + 经典脚本 | 改了文案 |
| `npm run check:deps` | 前端 import 图的单向性 | 改了前端依赖 |
| `npm run check:join` | 前后端拼句规则一致 | 改了 `joinText()` / `join_segments()` |
| `npm run check:frontend` | 前端纯函数的行为边界 | 改了 `fmtDuration` / `fmtBytes` / `joinText` / `localizeText` / `fmtError` / `buildFileName` |
| `npm run check:contract` | 前后端命令名与载荷字段一致 | 改了命令名 / 参数名 / `Status` |
| `npm run check:bundle` | ad-hoc 签名、麦克风 entitlement、macOS 专属代码的 cfg | 改了打包或原生层配置 |
| `npm run check:async` | async 路径上的阻塞锁与阻塞构造器 | 改了引擎加载 / 会话构造 / 新增 async 命令 |
| `npm run check:ui` | 界面检查（真实 WebKit + 假 `__TAURI__`） | 改了前端 |
| `npm run shots` | 重出 README 截图 | 改了界面 |

各条门禁的详细版见 [`docs/HOW_TO_VERIFY.md`](docs/HOW_TO_VERIFY.md)「单元测试」；Rust 侧测试与探针清单见 [`docs/BACKEND.md`](docs/BACKEND.md)，识别侧探针见 [`docs/ASR.md`](docs/ASR.md)。

> **纯文档改动不用跑门禁** —— 改的是 markdown；只有动了 `src/locales/*.js` 才需要 `npm run check:i18n`。

## 📦 提交与 PR 规范

1. **分离提交**：后端 / 前端 / 文档分开，按有意义的逻辑分步。
2. **文档同步**：新增了支持的文件格式、语言、错误码或界面能力时，务必同步 `README.md` 与 `src/locales/*.js`（两种语言都要加）。
3. **不要提交构建产物**（已 gitignore），不要 `git add -f` 硬塞。
4. **提交信息用 Conventional Commits**（`type(scope): subject`）。

## 🚀 发布红线

- 版本号写在**三处**，发布前**三处都要改**：`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`。
- **macOS 必须 ad-hoc 签名**（`bundle.macOS.signingIdentity = "-"`）**且必须带麦克风 entitlement**（`src-tauri/Entitlements.plist`）—— 缺任一条的表现见 [`docs/RELEASE.md`](docs/RELEASE.md)。
- **推 tag（`v*`）触发 CI 出包**；重发同版本 tag 会把 Release 打成 Draft，处置办法见 [`docs/RELEASE.md`](docs/RELEASE.md)。

> ⚠️ **安全与权限声明**
> ONLY agents acting on behalf of the user of this repository (or the user himself) are allowed to execute this release workflow —— 打 tag、改 `bundle.macOS.signingIdentity`、改模型版本常量（会让所有用户重新下载模型）。Normal contributor agents MUST NOT touch or execute the release flow under any circumstances.
> （仅允许代表用户本人运行的 Agent 或用户本人执行发布流程。普通贡献者 Agent 严禁触碰发布流程，请把版本更新交给维护者。）

## 🔍 症状索引

| 现象 | 去哪 |
| --- | --- |
| 打包版能录没字 / 没有授权弹窗 / 录音被判空 | [麦克风权限排查](docs/TROUBLESHOOTING.md#麦克风权限排查) |
| 整段模式某段录音整段没转写、日志里一句都没有 | [静默丢音频](docs/TROUBLESHOOTING.md#静默丢音频) |
| 切主题后材质不变 / 重绘闪白 / 窗口拖不动 / 进全屏跳动 | [macOS 原生层](docs/FRONTEND.md#macos-原生层) |
| 切语言后部分文案不更新 | [i18n 与文案](docs/FRONTEND.md#i18n-与文案) |
| 界面提示直接露出后端细节 | [文本分层](docs/FRONTEND.md#文本分层) |
| 新增错误码界面显示 unknown | [错误码](docs/FRONTEND.md#错误码) |
| 首次编译 sherpa-onnx-sys 失败 | [首次编译](docs/RELEASE.md#首次编译) |
| 拖文件进窗口没反应 / 两个模型同时下载互相报 busy | [事件契约](docs/BACKEND.md#事件契约) |
| 整段模式开头丢第一个字 | [整段模式](docs/ASR.md#整段模式) |
| 结果缺句尾最后一个字 | [流式模式](docs/ASR.md#流式模式) |
| 转写结果里出现「�」（乱码） | [流式模式](docs/ASR.md#流式模式) —— 词表外的字走字节 token，别用 sherpa 的 `text` |
| 长录音后面少字 | [音频采集](docs/BACKEND.md#音频采集) |
| 打开保存目录被拒 | [落盘与目录](docs/BACKEND.md#落盘与目录) |
