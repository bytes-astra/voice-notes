# 前端模块化拆分 + 长时间内容支持 —— 完整实施方案

日期：2026-10-22
状态：待批准

> 这份方案把两件事整合成一条线：
> **Phase A** 把 1275 行的 `src/app.js` 拆成 ES Modules（纯重构，行为不变）；
> **Phase B** 在拆好的模块上实现「不限时长、内存恒定」的长内容支持。
> 顺序不能反 —— 见文末「为什么先拆后做」。

**关联文档**
- 设计：`docs/superpowers/specs/2026-10-22-long-content-design.md`
- Phase B 的详细任务分解：`docs/superpowers/plans/2026-10-22-long-content.md`（本方案在其基础上补上「改动落在哪个模块」）

---

## 一、为什么要拆

`src/app.js` 现在 1275 行，混着六件事：全局状态、录音流程、结果渲染、历史记录、模型/设置、事件绑定。问题：

1. **改一处要看全文**：加一个设置项要翻 1275 行找相关函数
2. **重复的「找函数」成本**：Phase B 要改的代码（录音流程、文件转写、事件处理、设置项）**分散在 app.js 的五个角落**，先做功能再拆，diff 会叠成两倍
3. **两个隐性 bug 的藏身处**：`VIEW_TITLES` 在模块顶层取 `t()`（切语言后标题不更新）、`Transcript` 缺 `serde(rename_all)`（时长恒为 0）—— 都是「一处定义、多处使用」的结构性问题

拆分后每个模块只做一件事，改录音不会碰到历史记录。

---

## 二、Phase A：模块化拆分

### 2.1 关键决策：ES Modules（`type="module"`）

**已验证可行**：Tauri 2.12.5 的资源协议用 `MimeType::parse` 分发 Content-Type（`tauri/src/protocol/asset.rs:90`），`.js` → `text/javascript`（`tauri-utils/src/mime_type.rs:35`），满足 ES Module 的 MIME 要求。

**为什么不用「window 命名空间」那套**：`i18n.js` / `select.js` 已经是 `window.I18N` / `window.UISelect` 的经典脚本风格，但 app.js 用命名空间会得到 8 个全局对象、依赖关系只能靠约定维持。ES Modules 有静态依赖图、能发现循环依赖、能被 linter 检查。**保留 `i18n.js` / `select.js` 不动**（它们是经典脚本，加载顺序简单），app.js 及其拆分出来的模块用 ES Modules。

**关键约束**：`type="module"` 的脚本是 defer 的 —— 经典脚本（`locales/en.js` → `select.js` → `i18n.js`）先执行，模块后执行。所以 `window.I18N` / `window.UISelect` / `window.__TAURI__` 在模块求值时一定已就绪 ✅

### 2.2 最终文件布局

```
src/
  index.html          入口页面（script 标签顺序：locales → select → i18n → module:app.js）
  style.css
  i18n.js             既有（经典脚本，window.I18N）
  select.js           既有（经典脚本，window.UISelect）
  locales/en.js       既有
  core.js             【新】状态、DOM 引用、常量、格式化、toast、t
  tauri.js            【新】Tauri 插件的唯一解构处
  theme.js            【新】主题（浅色/深色、窗口底色、图标）
  export.js           【新】复制 / 保存文本
  history.js          【新】历史记录读写与渲染
  result.js           【新】结果区渲染（转写结果、实时行）
  status.js           【新】模型状态、模式、语言、设备、下载、设置项
  recorder.js         【新】录音流程（波形、计时、阶段、开始/停止/取消）
  files.js            【新】文件转写
  events.js           【新】后端事件 → 前端动作
  app.js              【新，入口】init / bindUI / switchView / applyLocale
```

### 2.3 模块职责与公开接口

每个模块的导出清单是**固定契约** —— 后续 Phase B 直接往对应模块加东西。

#### `core.js` —— 状态与工具（无内部依赖）
```js
export const state        // 全局状态对象（唯一实例，各模块 import 同一个引用）
export const els          // DOM 元素引用表
export const t            // = window.I18N.t（解构安全：t 闭包读 i18n.js 内部的 current）
export const I18N         // = window.I18N（applyLocale 要用 setLocale）
export const LANGS, HISTORY_KEY, THEME_KEY, LOCALE_KEY, BARS, IDLE_WAVE, MAX_SECONDS, MIB
export const STREAM_ASSETS, STREAM_LANGS
export function $(id)
export function fmtError(error), fmtBytes(bytes), fmtClock(seconds), fmtDuration(seconds)
export function toast(message, type = 'info')
export function idleHint()          // = t('record.idle')；recorder 和 status 都要用，放这里避免互相依赖
export function historyStorageGet() / historyStorageSet(items)   // localStorage 读写（history.js 用）
```
> `state` / `els` 在 `core.js` 里**只创建一次**，其他模块 `import` 引用（ES Module 的 live binding 保证是同一个对象，不会拷贝）。

#### `tauri.js` —— 插件解构（无内部依赖）
```js
export const { invoke } = window.__TAURI__.core
export const { listen } = window.__TAURI__.event
export const { open: openDialog } = window.__TAURI__.dialog
export const { writeText } = window.__TAURI__.clipboardManager
export const { openPath } = window.__TAURI__.opener
```

#### `theme.js`（依赖 core）
```js
export function currentTheme(), syncWindowBackground(), renderThemeIcon(), toggleTheme()
```

#### `export.js`（依赖 core, tauri）
```js
export async function copyText(text), saveText(text)
export function buildFileName(text)
```
> 单独成模块是为了打断循环：`history.js` 需要 copy/save，`files.js` 需要 showTranscript，而 `result.js` 需要 history 的 addHistory —— 复制/保存放最底层，环就断了。

#### `history.js`（依赖 core, export）
```js
export function loadHistory(), saveHistory(), addHistory(transcript, text), renderHistory()
export function iconAction(label, icon, onClick)
export function copyIcon(), saveIcon(), trashIcon()
```

#### `result.js`（依赖 core, history）
```js
export function showTranscript(transcript)   // 结果区 + meta + 写历史
export function showPartial(partial)         // 实时行（流式中间结果）
```

#### `status.js`（依赖 core, tauri）
```js
export function modelBadgeText(phase)
export async function refreshStatus()
export function renderStatus(), renderMode()
export async function setMode(mode), setLanguage(language)
export function renderLang()
export function deviceItems(), localeItems()
export async function loadDevices()
export function setProgress(done, total), setStreamProgress(done, total)
export async function downloadModel(), downloadStreamingModel(), downloadPunctuationModel(), pickModelDir()
export async function warmUp()
export async function pickOutputDir(), openOutputDir()
export function initSelects()               // 创建 deviceSelect / localeSelect（供 app.js 调用）
export function refreshSelectItems(localeValue)
```
> `deviceSelect` / `localeSelect` 这两个实例**归 status.js 所有**（core 不需要知道），app.js 通过 `initSelects()` / `refreshSelectItems()` 与它交互。

#### `recorder.js`（依赖 core, tauri, status, result）
```js
export function setPhase(phase), renderMicLabel()
export function buildWave(), renderWave(), resetWave(), pushLevel(level)
export function startTimer(), stopTimer()
export async function startRecording(), stopRecording(), cancelRecording()
```

#### `files.js`（依赖 core, tauri, status, result）
```js
export async function transcribeFile(file)
export function encodeWave(samples), audioBase64(bytes)   // Phase B 会删掉
export async function pickOutputDir(), openOutputDir()     // Phase B 会移到 status.js
```

#### `events.js`（依赖 core, status, recorder, result）
```js
export async function listenEvents()
```
> 单独成模块：它把「后端事件名」和「前端动作」对应起来，是 Phase B 改事件载荷（增量）时唯一要动的地方。

#### `app.js` —— 入口（依赖全部）
```js
export function switchView(view)
export function applyLocale(preference)
export function bindUI()
async function init()
```
> `applyLocale` 是协调者（要重画语言芯片、历史、状态、主题、麦克风标签、下拉框、结果区），放入口最自然 —— 它是唯一需要「同时认识所有模块」的角色。

### 2.4 依赖图（无环）

```
core.js ──────────────┬──────────────┬─────────────┐
  │                   │              │             │
tauri.js           theme.js      export.js    (无依赖)
  │                   │              │
  │                   │         history.js
  │                   │              │
  │                   │          result.js
  │                   │              │
  │              status.js ──────────┤
  │                   │              │
  │              recorder.js          │
  │                   │              │
  │                 files.js          │
  │                   │              │
  │                 events.js ────────┤
  │                                  │
  └────────────── app.js ────────────┘
```

**四条防环规则**（实现时必须遵守）：
1. `core.js` 和 `tauri.js` 不 import 任何业务模块
2. `status.js` 不 import `recorder.js`（所以 `renderMode` 里那句 `idleHint()` 要用 `core.idleHint()`；`setPhase` 调 `renderMode` 是 recorder → status 单向）
3. `result.js` 不 import `files.js`（所以 `files.transcribeFile` 调 `showTranscript` 是 files → result 单向）
4. `history.js` 不 import `result.js`（`addHistory` 由 result 调用，是 result → history 单向）

### 2.5 Phase A 的任务清单

每个任务 = **只搬函数 + 改 import，不改任何逻辑**。搬完必须页面行为完全一致。

| # | 任务 | 内容 | 验证 |
|---|---|---|---|
| A0 | 准备 | harness 的 `sync.sh` 改成拷贝 `src/*.js`（现在写死了 5 个文件名）；确认 `mock-tauri.js` / `drag.js` 仍插在 `i18n.js` 之前 | `bash /tmp/uitest/sync.sh` 打印出的 script 顺序正确 |
| A1 | core + tauri | 建 `core.js` / `tauri.js`；app.js 改成 `import { state, els, ... } from './core.js'`，其余逻辑原地不动 | Playwright `check_i18n_fix.js` 全过 |
| A2 | theme + export | 搬 `currentTheme` / `syncWindowBackground` / `renderThemeIcon` / `toggleTheme` → theme.js；`copyText` / `saveText` / `buildFileName` → export.js | 同上 + 主题切换检查 |
| A3 | history + result | 搬历史记录与结果渲染；`addHistory` 里对 `copyText`/`saveText` 的引用改从 export.js import | 同上 + 历史记录检查 |
| A4 | status | 搬模型状态/模式/语言/设备/下载/设置项（约 350 行，最大的一块）；`deviceSelect`/`localeSelect` 收归 status.js | 同上 + 状态渲染检查 |
| A5 | recorder + files | 搬录音流程与文件转写 | 同上 + 录音流程检查 |
| A6 | events + 入口 | 搬 `listenEvents`；app.js 只剩 `init` / `bindUI` / `switchView` / `applyLocale`；**index.html 把 `<script src="app.js">` 改成 `<script type="module" src="app.js">`** | 全量 Playwright + 打包实机 |
| A7 | 修两个 bug | ① `VIEW_TITELS` 改成渲染时取文案（`switchView` 里 `t('view.transcribe')`）—— 现在切语言后左上角标题不更新；② 后端 `Transcript` / `Segment` 加 `#[serde(rename_all = "camelCase")]` —— 现在时长/耗时/RTF 和历史记录时长恒为 0 | 加测试：切语言后标题变；序列化后有 `durationSeconds` |
| A8 | 验证与收尾 | Playwright 全量 + `npm run check:i18n` + 重新出图 + 打包实机确认 | 全部通过 |

### 2.6 Phase A 的测试

Phase A 是纯重构，测试策略是「**行为不变**」：

- 现有的 `/tmp/uitest/check_i18n_fix.js`（15 项）必须**一项不少地继续通过** —— 它覆盖了两种模式提示语、aria-label、中英切换、无未翻译 key、控制台无报错
- 新增 `/tmp/uitest/check_modules.js`（A6 之后写）：
  - 页面加载后 `window.__TAURI__` 调用正常、无未捕获异常
  - 切换视图（转写/历史/设置）标题正确
  - **切语言后视图标题跟着变**（A7 的回归测试）
  - 录音 → 波形有反应 → 停止 → 结果区出现文本
  - 历史记录里点一条能载入到结果区
- 后端 A7 的 camelCase 修复：Rust 单元测试断言 `serde_json::to_value(&transcript)` 里有 `durationSeconds` 键

---

## 三、Phase B：长时间内容支持（8 个任务）

沿用 `docs/superpowers/plans/2026-10-22-long-content.md` 的任务分解，**补上每一步落在哪个模块**：

> 那份计划已按代码评审意见修订过（2026-10-22）：安全阀上限改为按设备实际采样率算（`MAX_PENDING_SECONDS * sample_rate`）；`recorded_samples` / `recognized_samples` 两个计数器口径分开；symphonia 测试补齐 AAC/损坏文件/不支持格式三类；「文本等价性测试失败时停下来分析，不要放宽归一化规则」写成硬要求；两种模式的 VAD 参数差异（流式 20s / 整段 30s）写进 AGENTS 已知坑。

| Phase B 任务 | 后端改动 | 前端落点（拆分后的模块） |
|---|---|---|
| B1 采集缓冲消费化 + 电平环 | `audio.rs`：`SampleQueue` / `NewAudio` / `gap_marker` | — |
| B2 `Engine::recognize()` | `asr.rs` | — |
| B3 `OfflineSession` | `asr.rs` | — |
| B4 录音命令改走 ASR 线程 | `lib.rs`：`run_offline` / `AppState.offline_session` / 引擎取走还回 | — |
| B5 事件载荷改增量 + hybrid 缩减 | `lib.rs`：`SegmentPayload` / `PartialPayload`；删 `correct_streaming`；`settings.warn_on_bad` | `events.js`：`stream:segment` 改成追加；`result.js` 加 `appendSegment(text)` |
| B6 symphonia 后端解码 | 新建 `decode.rs`；`transcribe_file` 命令 | — |
| B7 前端改造 | — | `recorder.js`（去掉 10 分钟自动停、加「正在听」指示）、`files.js`（改调 `transcribe_file`、加进度、删 `encodeWave`/`audioBase64`）、`status.js`（hybrid 开关改成「异常时提醒」）、`events.js`（增量）、`index.html`（`#listening`）、`i18n.js` + `locales/en.js` |
| B8 文档 + 端到端 | — | README / AGENTS.md / 截图 |

**Phase B 带来的新依赖图变化**（拆分后不会产生环）：
- `result.js` 新增 `appendSegment()` → 仍只依赖 core ✅
- `files.js` 依赖 `status.js`（转写进度要 `renderStatus`？）—— 不需要，仍只依赖 result ✅
- `recorder.js` 的「正在听」指示 → 只依赖 core ✅

---

## 四、为什么先拆后做

| | 先做功能再拆 | 先拆再���功能（推荐） |
|---|---|---|
| diff 体积 | 两份改动叠在同一批行上，review 难 | 拆分是纯移动（易 review），功能改动落在小文件里 |
| 出问题定位 | 「是功能写错了还是搬错了」分不清 | 拆分后行为可逐项对比；功能改动独立 |
| 模块边界 | 边界是照着「旧代码长什么样」定的，不是照着「新代码需要什么」定的 —— 比如新加的增量事件处理会不知道放哪 | 先拆完，边界清晰，再把新功能放进去 |
| 返工 | 拆完之后往往还要再调边界，等于拆两次 | 一次到位 |

**代价**：Phase A 多花一轮「搬代码 + 验证」的时间（约半天），换来 Phase B 每个任务只改 1~2 个文件。

---

## 五、总体验证清单

Phase A 结束后：
- [ ] `npm run check:i18n` 通过（2 个语言 key 对齐）
- [ ] Playwright：`check_i18n_fix.js` 15 项 + `check_modules.js` 全过
- [ ] `cargo test --lib` 全过
- [ ] 打包（`npx tauri build --bundles app`）后实机启动：三个视图、主题切换、中英切换、录音→结果、历史载入，全对
- [ ] 9 张截图重出，与拆分前视觉一致

Phase B 结束后（沿用原计划的验证）：
- [ ] 录 15 分钟不自动停、内存不涨
- [ ] 拖 30 分钟音频 / 一个 mp4 能完整转写
- [ ] 两种模式都边录边出字
- [ ] 流式结果异常时弹「结果可能有问题」

---

## 六、风险与回滚

| 风险 | 应对 |
|---|---|
| ES Modules 在 Tauri 里加载失败（白屏） | MIME 已验证（`text/javascript`）；A6 后必须打包实机验证，不只看 harness |
| 循环依赖导致模块初始化顺序异常 | 2.4 的四条规则 + A1~A6 逐步搬，每步跑测试 |
| `state` 被误当成各模块各存一份 | 只在 core.js 创建一次，其他模块一律 import |
| harness 脚本没同步新模块 → 页面半渲染 | A0 先改 `sync.sh`；截图脚本里加 `requestfailed` 监听 |
| 拆分后行为悄悄变了 | 每个任务只做「移动 + 改 import」；`check_i18n_fix.js` 作为回归网 |

**回滚**：Phase A 和 Phase B 各自独立提交，出问题可以单独 revert 某一段。
