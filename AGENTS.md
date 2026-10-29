# VoiceNotes · 项目级 AI Agent 与贡献指南 (AGENTS.md)

欢迎！如果你是协助人类开发者参与本项目贡献的 AI Agent，或是想了解项目结构的贡献者，请务必阅读以下开发流与规范。

规范之外还有两份文档：质量调研与实测数据在 [`docs/research/`](docs/research/)，验证手段与基线全在 [`docs/HOW_TO_VERIFY.md`](docs/HOW_TO_VERIFY.md) —— **改任何模型 / 参数 / 热词之前，先跑探针做前后对比，别凭感觉。**

## 🎯 核心架构与原则

- **源文件驱动**：前端逻辑在 `src/`（原生 JS + ES Modules，浏览器直接跑，没有打包器），后端逻辑在 `src-tauri/src/`（Rust）。
  - `src/app.js`：应用入口 —— init / bindUI / switchView / applyLocale。
  - `src/core.js`：全局状态、DOM 引用、格式化、错误码翻译 —— 其他模块都从它取。
- **禁止直接修改产物**：`src-tauri/target/`、`src-tauri/gen/schemas/` 是构建产物（已 gitignore），`docs/screenshots/*.png` 由 `npm run shots` 生成。**永远不要直接编辑这些文件** —— 改逻辑改源文件，改截图跑脚本。
- **构建工具链**：Tauri 2 CLI（`@tauri-apps/cli`）+ cargo；前端没有打包器、框架、TypeScript。
- **单向依赖**：`icons.js`（零依赖叶子）→ `core.js` / `tauri.js` → `theme.js` / `export.js` → `history.js` → `result.js` → `status.js` → `recorder.js` → `files.js` / `events.js` → `app.js`。不能反，也不能成环。
- **单一事实源**：同一件事只写一处。用户可见的功能与安装说明在 `README.md`，验证与基线在 `docs/HOW_TO_VERIFY.md`，本文件只放工程约定与踩过的坑。

## 🛠️ 开发与构建工作流

在准备提交你的代码之前，按以下步骤本地验证：

1. 装依赖、起开发模式、打包：

   ```bash
   npm install
   npm run dev          # 开发模式
   npm run build        # → src-tauri/target/release/bundle/
   ```

2. 门禁与检查（逐条说明）：

   - `npm run check`：门禁 —— i18n → 前端依赖图 → clippy（`-D warnings`）→ `cargo test --lib`。
   - `npm run check:i18n`：各语言 key 对齐 + HTML/JS 引用的 key 存在（改文案后必跑）。
   - `npm run check:deps`：前端 import 图 —— 单向依赖不能反、也不能成环（改前端依赖后必跑）。
   - `npm run check:ui`：界面检查（真实 WebKit + 假 `__TAURI__`），前端改动后跑。
   - `npm run shots`：重出 README 截图，改了界面后跑。

3. Rust 侧测试：`cargo test --lib`（纯逻辑），以及 `src-tauri/tests/` 下的 `asr_smoke`（整段真模型，需 `VOICE_NOTES_MODEL_DIR`）、`streaming_smoke`（流式，需 `VOICE_NOTES_STREAMING_MODEL_DIR`）、`decode_smoke`（后端解码全链路）、`quality_probe`（质量探针，只打印不断言）。

> **建议**：提交前在终端跑 `npm run check`，前端改动再跑 `npm run check:ui`。**纯文档改动不用跑门禁** —— 改的是 markdown；只有动了 `src/locales/*.js` 才需要 `npm run check:i18n`。

### 首次编译

首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 拉预编译静态库（onnxruntime 等），之后走缓存。想复用本地已有的库，`SHERPA_ONNX_ARCHIVE_DIR=/path/to/dir` 指向那个目录（里面放 `sherpa-onnx-v1.13.8-<os>-<arch>-static-lib.tar.bz2`），这条路径不经过 GitHub。

### CI 在哪跑

`.github/workflows/check.yml`（推 main / 提 PR 都跑）：Rust job 用 macOS —— 这个项目的 Rust 端在 Linux 上没验证过（cpal 要 ALSA 头文件），别把没支持的平台塞进 CI。前端检查（i18n + 界面检查）只在本地跑，不进 CI。

## 🧹 代码规范

### 前端模块边界

- **原生 JS + ES Modules**，没有打包器、没有框架、没有 TypeScript。不要为 UI 引入构建步骤。
- **依赖方向不能反、也不能成环**：完整链路见「核心架构与原则」的「单向依赖」。`core.js` / `tauri.js` 不 import 任何业务模块；`status.js` 不 import `recorder.js`（所以 `idleHint()` 放 core）。
- **`state` / `els` 只在 `core.js` 创建一次**，其他模块 `import` 引用（live binding 是同一个对象）。
- **Tauri 插件调用面集中在 `src/tauri.js` 解构**，其他模块从那里 import。新增插件命令要同步在 `src-tauri/capabilities/default.json` 加权限，否则运行时直接拒绝。
- **外壳是「侧边栏 + 主区」**：`.app[data-view]` 控制 `[data-view-panel]` 显隐，`switchView()` 统一处理导航高亮/标题，录音中不允许切走。新增页面 = 一个 `[data-view-panel]` + 一个 `.nav-item`。
- **默认窗口尺寸要容得下整张转写卡片**：窗口默认宽高写在 `src-tauri/tauri.conf.json`，它是唯一的尺寸来源 —— `scripts/ui/check.mjs` 和 `scripts/ui/shots.mjs` 都从那里读 viewport（界面检查和 README 截图永远等于用户启动后看到的样子）。改窄了或改矮了，转写结果卡片会被裁掉、`check:ui` 里「默认窗口尺寸下没有纵向滚动条」那条断言会红。

### i18n 与文案

文案统一放字典，机制单独成文件：`src/locales/*.js` 放文案，`src/i18n.js` 只放机制（`t()` / `apply()` / `setLocale()` / `resolveLocale()`）。**`i18n.js` 里不放任何文案。**

- JS 用 `t('key', {变量})`，HTML 用 `data-i18n` / `-title` / `-placeholder` / `-aria`，启动时 `I18N.apply()` 填充。
- **加语言 = 加一个字典文件 + 在 `index.html` 里加一行 `<script src="locales/xx.js"></script>`**，且必须排在 `i18n.js` 之前（`i18n.js` 启动时读 `window.I18N_LOCALES`，晚加载就没了）。
- **`i18n.js` / `select.js` 保持经典脚本**（挂 `window.I18N` / `window.UISelect`），在 `index.html` 里必须排在 `<script type="module" src="app.js">` 之前。用 `import` 引入它们会被重新执行一遍、覆盖 `window.I18N`，表现为「切语言后部分文案不跟着变」。
- **字典文案在渲染时取**：存 key，渲染时 `t(key)`。模块顶层取 `t()` 会在切语言时不更新（语言芯片、阶段提示、视图标题都踩过）。视图标题是 `VIEW_KEYS` + `viewTitle()`。
- **随状态变的标签**（麦克风「开始录音 / 结束录音」）用单独的渲染函数（如 `renderMicLabel()`），并在 `applyLocale()` 里重调一次 —— `I18N.apply()` 只认 HTML 上的静态 `data-i18n-aria`。
- **同一个控件的文案不按模式变**：麦克风下方的空闲提示两种模式统一写「点击麦克风开始录音」，模式差异由「整段 / 流式」芯片的副标题说明。
- 后端不返回中文展示文案（模型来源返回 `app_data` / `custom` 这种 token）。

### 文本分层

界面文案与技术细节是两条线，分开写：

- **界面文案 = 一句中性的话 + 用户看得懂的参数**（字典里只留 `{value}` / `{seconds}` 这类占位符）。措辞中性（不用「识别跟不上，丢掉了…」这种口语），也不出现内部黑话（线程、预热、收尾、引擎状态、断句）—— 用「麦克风发生异常」「识别已停止」这种说法。
- **技术细节（系统错误文本、路径、采样格式名）只进日志**：`code_detail()` 会把细节写进日志文件。**字典里不放 `{error}` 占位符** —— `{error}` 只用来装「嵌套的干净文案」，比如 `result.fileFailed` 里那句已经翻译过的原因。
- 字典里查不到的错误码 → 界面显示 `error.unknown`，错误码和细节都不原样甩给用户。前端把错误抛给界面时同理：toast 里放干净文案，细节用 `logEvent('ERROR', …)` 记进日志。

### 错误码

后端错误只返回字典 key：用 `crate::code("record.empty")` / `crate::code_detail("record.micError", e)`（lib.rs 里定义），得到 `"record.empty"` / `"record.micError:<细节>"`。前端 `core.js` 的 `fmtError()` 查字典翻译。

**新增错误码必须同时加进 `src/locales/zh-CN.js` 和 `src/locales/en.js`**，`npm run check:i18n` 会校验后端引用的码在字典里。

### 缺口标记

识别跟不上丢掉音频时，后端在文本里插 `[[gap:秒数]]`（`audio::gap_marker`），前端渲染时用 `core.js` 的 `localizeText()` 换成 `t('record.gap', {seconds})`。后端不写中文标记。

### 控件

下拉框用自绘组件 `src/select.js`（`UISelect.create(el, {onChange})`）：原生 `<select>` 在 macOS/Windows 样式不可控。弹层挂 body 上用 fixed 定位，避免被滚动容器裁掉。`setItems` 在 `create()` 之后调用，反了拿到 null。

### macOS 原生层

**侧栏材质**：`lib.rs` 的 setup 里 `set_effects(Effect::Sidebar)`（NSVisualEffectView），配合 `style.css` 里 `body.macos` 的透明规则 —— 侧栏不画底色，系统材质从后面透出来，主区自己刷不透明底色。

- **不开 `transparent: true`**：Tauri 内置的 `set_effects` 自己就把 webview 处理成透明了。开了反而更糟 —— 那是全局窗口属性，Windows 也会变 layered 窗口（DWM 投影会没），而 Tauri 没有平台专属的窗口配置（`tauri.windows.conf.json` 的合并是 RFC 7386，数组整体替换，改一个字段得抄整份 window 配置）。
- **材质的明暗跟的是窗口外观**（NSWindow.appearance），不是网页的 `data-theme` —— 切主题时必须同步 `setTheme('Dark'|'Light')`（`theme.js` 的 `syncWindowTheme()`），否则系统浅色 + 应用暗色 = 浅色材质配深色字。
- **`setTheme` 需要 `core:window:allow-set-theme` 权限**，漏了会被 ACL 直接拒绝，而前端那行 `.catch` 会把错误吞掉 —— 表现是「暗色下材质不变」且毫无提示。失败要 `logEvent` 记进日志。
- **`html` 在 macOS 不刷 `var(--bg)`**：body 和侧栏透明但根元素不透明，同样会把材质盖住。用 `html:has(body.macos) { background-color: transparent }` 给 macOS 留透明，其他平台仍保留防闪白底色。
- 材质用经典 NSVisualEffectView，**不要用 Liquid Glass**（macOS 26+ 的 NSGlassEffectView，效果不如经典材质）。

**非 macOS 的窗口底色**：`html` 铺上 `var(--bg)`，窗口底色跟主题走（`syncWindowBackground()` → `setBackgroundColor`，需要 `core:window:allow-set-background-color` 权限）。只给 `body` 上色不够 —— 重绘时会闪白。

**窗口标题跟界面语言走**：`tauri.conf.json` 的 `title` 只是启动时的默认值（写 `VoiceNotes`），运行时由 `applyLocale()` 调 `getCurrentWindow().setTitle(t('app.title'))` 设置（需要 `core:window:allow-set-title` 权限）。

**原生菜单**：`menu.rs` 创建，界面切语言时 `applyLocale()` 把 `t('menu.edit')` / `t('menu.view')` 传给 `set_menu_texts` 更新子菜单；预定义编辑项交给系统本地化，**Rust 里不存任何菜单文案**（加语言只改字典）。

**拖动窗口**：`data-tauri-drag-region="deep"`（`.sidebar-top` 与 `.main-head`，按钮自动排除）+ **`core:window:allow-start-dragging` 权限** —— 这权限不在 `core:default` 里，删了窗口就拖不动。

**侧栏顶部留位**：`body.macos .sidebar-top { height: 78px }` 给红绿灯留位，全屏时红绿灯会自动隐藏但占位**始终保留 78px**（否则进出全屏内容会跳）。Windows 没有这个占位 —— `body.macos` 不生效时侧栏顶部高度跟着内容走，写死 12px 会切掉 30px 高的品牌行。

### 后端分层

- **lib.rs 只放命令层**：新逻辑先找对应模块放 —— `record.rs`（录音编排）、`engine.rs`（引擎与状态）、`asr.rs`（识别会话）、`audio.rs`（采集与重采样）、`decode.rs`（解码）、`models.rs`（模型下载）、`output.rs`（落盘）、`settings.rs`（设置）、`log.rs`（日志）、`menu.rs`（macOS 菜单）。命令本身保持薄：一个命令别同时干两件事（录音按模式拆成 `start_streaming_recording` / `start_offline_recording` 两个独立命令）。
- **`record.rs` 不认识 Tauri**：结果通过 `emit` 回调交出去，所以它能被单测直接调用。`AppHandle` 不进 `record.rs` —— 实在要发事件就在 lib.rs 那层包一下。
- **名字别撞**：`record.rs` 是「录音怎么跑」，`asr.rs` 里的 `OfflineSession` / `StreamingSession` 是「识别会话」，两回事 —— 所以前者不叫 session.rs。两种录音共用 `RecordingRun { stop, discard, rx }`，停止统一走 `finish_recording`：麦克风故障也要先等 ASR 收尾，不能当取消丢文本。
- **单元测试不进源文件**：测试放同目录子文件夹，`src/asr.rs` 里写 `#[cfg(test)] mod tests;`，实现放 `src/asr/tests.rs`（一个模块有多个测试组就是 `src/asr/gap_tests.rs` 这样另起文件）。crate root 例外 —— `lib.rs` 的子模块落在 `src/` 根下（`src/command_tests.rs`）。**拆出去仍是单元测试**：`use super::*` 照样能摸到父模块的私有项，`cargo test --lib` 照样跑得到，门禁不用改。别为了「看起来整齐」搬到 `src-tauri/tests/` —— 那是集成测试，只碰得到 `pub` 项，而且 `cargo test --lib` 不再覆盖它。

### 命令与运行时

- **命令参数名用 camelCase**，如 `audioBase64`、`fileName`。
- **推理是同步阻塞的**，一律放进 `tauri::async_runtime::spawn_blocking`，别阻塞 async 执行器。
- **引擎按 `(language, itn)` 缓存**，语言或 ITN 变化时重建。想「切语言不重载」得走 C API 的 `setConfig`，Rust 绑定没暴露。录音中语言和 ITN 前后端都锁定；自定义模型目录变更由 `set_model_dir` 清缓存。
- **文件解码在后端**：`decode::decode_to_16k()` 逐块解码，内存恒定。symphonia 锁 **0.5**（0.6 要 Rust 1.85，会破本项目声明的 1.78）。

### 事件契约

- **事件名只出现在 `src/events.js`**：`audio:level`、`audio:error`、`model:progress`、`engine:state`、`stream:partial`、`stream:segment`、`streaming:state`。改名字要同步前端。
- **会话 ID**：录音/文件都先 `reserve_session`，启动/文件命令携带 `sessionId`，分段、partial、采集错误带同一个 ID；前端只接收当前会话，超时同时置 discard 阻止旧线程继续发射。
- **`audio:error` 的载荷是对象**：`{ code, detail?, seconds?, fatal, sessionId? }`。`code` 是字典 key，`fatal: false` 表示「提示但不中断」（比如丢掉音频的缺口提示 —— 录音要继续）。致命 `record.micError` 由前端停止录音并保存非空部分结果；用户主动 Esc 才走取消并清空，其他致命错误仍取消。
- **停止时的采集故障走 `Transcript.warning`**：`finish_recording` 先让识别线程收尾、保住非空转写，再由 `stop_recording` 把 `record.micError` 写进 `Transcript.warning`；正常转写不带这个字段（`skip_serializing_if` 保证载荷不变）。前端据此提示「麦克风发生异常」而不是「完成」，也不丢结果。
- **事件载荷是增量的**：`stream:segment` 只带新句子，`stream:partial` 只带中间结果。累计全文在长时间会话里是 O(n²) 的传输与重绘。前端 `core.js` 的 `joinText()` 与后端 `asr::join_segments()` 规则必须一致（中日韩之间直接连，其余加空格）。
- **拖拽用 Tauri 的 `onDragDropEvent`**：Tauri v2 默认拦截系统拖放，HTML5 `drop` 事件在真应用里不触发。`tauri.conf.json` 的 `dragDropEnabled` 必须保持 `true` —— 关掉它 wry 就不装拖放 handler，`onDragDropEvent` 永远不触发，拖拽转写直接变成死代码（而且没有 HTML5 兜底）。
- **模型下载是两条独立的线**（可以同时下）：各自有 `downloading` / `cancel_download` 标志（`engine.rs` 的 `AppState` 里 `downloading_streaming` / `cancel_download_streaming`）。共用一个标志的话，整段在下载时点流式下载会直接报 busy 失败。
  - 进度事件是同一个名字（`model:progress`），载荷里带 `kind`（`"offline"` / `"streaming"`，由 `lib.rs` 的 `EventSink` 塞进去）—— **前端必须按 kind 分流**，别看「当前是不是在下载」那种状态，否则两个面板会互相显示对方的进度（看起来就像流式在重复下载整段模型）。
  - `cancel_download` 命令带 `kind` 参数，指明取消哪一个。

### 落盘与目录

- **转写结果的默认输出目录用 ASCII**（`settings::default_output_dir` → `~/Documents/VoiceNotes`）：中文目录名在终端、脚本、跨平台工具里处处要转义。旧版的中文目录「语音转文字」由 `settings::migrate_output_dir()` 在启动时一次性改名（只在新目录不存在时才搬，不覆盖已有数据）。
- **打开文件夹用自己的 `open_output_dir` 命令，不用 opener 插件**：插件的 `opener:allow-open-path` 权限是「without any pre-configured scope」—— 不配 scope 的话任何路径都被拒（报 "Not allowed to open path"）；而保存目录是用户可改的（`settings.output_dir`），静态 scope 盖不全。后端自己解析出真实路径再 `open`/`explorer`/`xdg-open` 更省事（应用自己的命令不需要 ACL 权限，`__allow_command` 直接放行）。

### 日志

**日志一直开着，不需要用户配置**：`log.rs`（`log::info` / `warn` / `error`）写到系统标准日志目录 —— macOS `~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，Windows `%LOCALAPPDATA%\com.github.bytes-astra.voicenotes\logs\voice-notes.log`；超 1 MB 轮转一次保留 `.1`，同时镜像 stderr。没有开关、没有设置页 —— 用户反馈问题直接把这个文件发过来。前端那边用 `client_log` 命令写进同一个文件（`src/tauri.js` 的 `logEvent()`），这样一份文件就是完整的一份记录。

### 整段模式

- `OfflineSession` 用 VAD 切句、逐句 `Engine::recognize()`，边录边出字。
- **VAD 缓冲给 60 秒**（`VoiceActivityDetector::create(&vad, 60.0)`，在 `asr.rs` 的 `OfflineSession::new`）。sherpa 的 `max_speech_duration` 不是硬切，它会把 VAD 参数调严（min_silence 0.5→0.1、threshold 0.5→0.9 逼它找静音），连续说话时找不到，句子能到 1 分钟以上。给 32 秒会触发 CircularBuffer 扩容（Overflow 警告 + 整段拷贝）。
- **默认整句一次识别**（最准，`asr.rs` 的 `MAX_SINGLE_SHOT_SECONDS` / `merge_transcripts`）。SenseVoice 的「30 秒上限」实测不是硬约束。**切分阈值是 90 秒**（`MAX_SINGLE_SHOT_SECONDS`），不是官方的 30 秒 —— 只有超过 90 秒才切 30 秒窗口 + 2 秒重叠（`merge_transcripts` 合并），防编码器注意力平方级吃显存。提前切反而更差，而且会把边界上的词劈开（实测见 `docs/research/`）。
- **VAD 会切掉开头第一个字**：音频开头直接是语音（没前导静音，比如 TTS、剪过的文件）时 sherpa VAD 的内部 padding 只有 ≈314 ms（「今天下午」→「天下午」）。`asr.rs` 的 `should_pre_roll` 只在「第一段 + 起点 < 0.6 s + 段前 RMS > 段 RMS × 0.3」时把起点回扩 0.4 s。回扩保持**带门控** —— 无门控回扩会把开头噪声当语音（真人样本多出「您好」）。
- **ITN 自带标点**（ITN 开关在 `engine.rs` 的 `Engine::new`）：开 ITN 的 SenseVoice 自己就输出标点（关 ITN 才是纯文本）。标点模型只用于流式，叠在整段上会变成「。。」。
- **短句合并**（`asr.rs` 的 `SegmentMerger`）：VAD 切出来的句子短于 3 秒（`MERGE_MIN_SECONDS`）不立刻定稿，等下一句来了**在音频层**合成一句再识别 —— 短句单独送 SenseVoice 时上下文太少，容易把词认错。两段之间隔超过 2 秒（`MERGE_MAX_GAP_SECONDS`）就不合并，否则「说完停顿十秒再开口」会被连成一句。**录音和文件转写两条生产路径都走 `OfflineSession`**，所以行为一致；`Engine::transcribe`（只给测试/探针用）也接了同一个合并器，否则两边结果对不上。
- **VAD 参数整段和流式必须一致**（`engine.rs` 的 `VAD_MIN_SILENCE`）：整段 `Engine::new` 和流式 `StreamingEngine::new` 用同一个常量（min_silence 0.3 秒），否则同一段音频在两个模式里切出的句子数量不同。

### 流式模式

- **分片恒为 200 ms（3200 样本），且只在 `is_ready()` 为真时 `decode()`**（`asr.rs` 的 `StreamingSession`）。改小分片或无条件 decode 都会让 sherpa-onnx 特征缓冲越界直接 abort（`features.cc GetFrames`）。`start_streaming_recording` 先 `ensure_streaming_engine` 再起采集（`lib.rs`）。
- **收尾补 300 ms 静音**（`TAIL_SILENCE_SAMPLES`，在 `asr.rs`）：解码器需要尾部上下文才吐出最后一个字，不补就丢句尾（「产品评审会」→「产品评审」）。补的静音不计入时长。
- **VAD 门控**（在 `record.rs` 的 `run_streaming_core`）：Silero 判到静音就 `force_end()` 提交并 reset，静音段不喂模型（否则 transducer 会在噪声上编出重复字）；起音前留 0.4 s 预录避免切字头。
- 流式模型是独立的一组 assets（`models.rs` 的 `STREAM_*`：`streaming-zipformer-zh-int8-2025-06-30` + `silero_vad.onnx`），只覆盖中文；选粤/日/韩时前端自动切回整段。**流式的 `silero_vad.onnx` 与整段的是同一个 v5 文件**（`STREAM_VAD` 和 `VAD` 指向同一 commit 下的 `silero_vad_v5.onnx`）—— 两种模式的切句边界要一致。
- **流式结果就是最终结果**（不要用 SenseVoice 覆盖它，这个功能已删）：实测流式明显更准，而且覆盖要保留全部音频，和「不限时长、内存恒定」直接冲突。
- **术语表（热词）**（`settings.rs` 清洗，`engine.rs` 的 `StreamingEngine::new` 用它）：`settings.hotwords`（≤ 100 条、单条 ≤ 32 字，`settings::clean_hotwords` 清洗）。非空时 `StreamingEngine` 用 `modified_beam_search` + `config.hotwords_buf`（短音频约慢 6%），为空保持 greedy。**只能用在流式** —— SenseVoice 上给热词或 beam 会 `_Exit(-1)` 杀进程（已复现）。改词表会清空流式引擎，下次录音重载 161 MB。设置页「填入示例」用的是 `src-tauri/assets/hotwords.txt`。
- 标点模型（`PUNCT`，CT-Transformer zh-en，281 MB，`engine.rs` 的 `PunctuationEngine`）独立放在 `models/punct/`，默认关闭；开启后 `StreamEmitter` 在每句定稿时调 `PunctuationEngine::apply`，实时中间结果不加标点。模型缺失/加载失败时静默降级，不阻断录音。

### 音频采集

- **采集缓冲是消费式的**（`audio.rs`）：`audio::SampleQueue` 的 `take_new()` 取走即释放，内存恒定（录 1 小时和 1 分钟占一样多）。**电平表走独立的 `LevelRing`**（2048 样本）—— 采集缓冲没有历史了，音量表去读它会恒读 0。
- **安全阀**（`audio.rs`）：未消费采样超过 `MAX_PENDING_SECONDS × 设备实际采样率`（60 秒，采样率不能写死 48k）时丢最旧的，并用 `audio::gap_marker()` 在结果里留可见缺口，不能静默丢。
- **停止录音时 `Recorder::stop()` 只关流、不取采样**（`audio.rs` 的 `stop`；收尾取样在 `record.rs`）：句尾最后一段要留给识别线程收尾时 `take_new()`。主线程先取走会切掉最后一个字（离线/流式都中招）。
- **麦克风权限在界面渲染完之后申请**：macOS 是第一次碰输入设备才弹权限框，而「列出输入设备」就会触发。流程：启动 → 界面画完（约 0.9 s）→ `list_input_devices` → 弹框（这一步同时把设备列表填好）。
- **启动时不列设备**：弹框会打断用户，实测弹框期间窗口是空白的。点麦克风时仍先 `warm_up_mic` 探权限，用户拒绝过要能给出明确报错。
- **输入设备可选**：`list_input_devices` 列出 cpal 输入设备（默认设备带标记），`set_input_device` 存 settings（None = 系统默认），`audio::start` 按名字挑，挑不到退回默认。下拉框每次展开重新列一次（`select.js` 的 `onBeforeOpen`），中途插拔也能看到。**没有名字的设备直接不列**（`audio::device_entry`）：按名字挑不中它，列出来只会让界面出现一条选不中的项 —— 设备名是身份，不能拿展示文案（本地化回退标签）当身份传给后端。
- **输入电平自动增益**（`audio::Agc`）：在**重采样之后、VAD 之前**提电平（目标 RMS 0.02 / 增益上限 32× / 软限幅 ±0.99）。电平够就一个样本不动；电平低时能把整段「VAD 判非语音丢掉」的音频救回来（实测 -35 dB：0 字 → 717 字）。三条路径都要接：麦克风整段、麦克风流式（在**门控 VAD** 之前）、文件转写。详见「静默丢音频」。
- 录音**没有时长上限**。

### 模型与下载

- 模型版本固定：HF commit + sha256 + bytes 写在 `models.rs` 的 `MODEL` / `TOKENS` / `VAD` / `STREAM_*` / `PUNCT` 常量里。**换模型必须同时改 bytes 和 sha256**，否则下载校验失败。
- **下载走镜像优先**（`[MIRROR, HF]`）：国内直连 huggingface.co 经常连不上，镜像失败再回退官方源。顺序保持镜像优先。
- **模型目录必须有 `silero_vad.onnx`**：整段分句、文件转写都靠 VAD，`models::resolve()` 会直接跳过没有 VAD 的目录（否则录完只得空结果）。
- 已存在且大小正确的文件直接跳过下载；`.part` + Range 支持断点续传；下完做 sha256 校验。应用数据目录里的文件按**字节数**判定是否就绪（`app_data_file_ok`，含流式 VAD），所以换 VAD 版本后会自动重下；截断文件不能显示「已就绪」。下载忙碌标志用 `DownloadFlagGuard` 在所有退出路径复位，别在目录创建前裸 `?` 留下永久 busy。

## 📦 提交与 PR 规范

1. **分离提交**：核心功能改动与文档改动尽量按有意义的逻辑分步提交（后端 / 前端 / 文档分开）。
2. **文档同步**：新增了支持的文件格式、语言、错误码或界面能力时，务必同步 `README.md` 与 `src/locales/*.js`（两种语言都要加）。
3. **不要提交构建产物**：构建产物（见「核心架构与原则」）已被 gitignore，不要用 `git add -f` 硬塞。

## 🚀 版本发布流程与权限声明

- 版本号写在两处：`package.json` 的 `version` 与 `src-tauri/tauri.conf.json` 的 `version`，发布前两处都要改。
- 推 tag（`v*`）触发 CI，同时产出 macOS（dmg）和 Windows（msi/exe），在 Artifacts 下载。
- **重发同版本 tag 会把 Release 打成 Draft**：删远端 tag（`git push origin :refs/tags/v1.0.0`）之后，GitHub 会把该 Release 退回 draft，重推 tag 也**不会**自动发布 —— 仓库侧栏只剩「Create a new release」，产物其实都在。构建跑完要 `gh release edit v1.0.0 --draft=false` 才真正公开；想连正文一起刷新，就先 `gh release delete v1.0.0 --yes` 再重推 tag，让 workflow 走 `gh release create --generate-notes`。
- 发布前先用本地 `npm run build` 出的 `.app` 验一遍麦克风权限。

### 打包与发布

- **macOS 必须 ad-hoc 签名**：`tauri.conf.json` 的 `bundle.macOS.signingIdentity` 必须是 `"-"`。不设时 Tauri 不做 codesign，链接器留下的是 `linker-signed`（`Sealed Resources=none`、Info.plist 未绑定），`codesign --verify` 失败；别的 Mac 从 dmg 装完 Gatekeeper 报「已损坏」（不是「未验证开发者」）。签名后仍提示「无法验证开发者」，可右键打开或 `xattr -dr com.apple.quarantine`；要免提示只能 Developer ID + 公证。
- **打包版必须带麦克风 entitlement**：`src-tauri/Entitlements.plist` 里的 `com.apple.security.device.audio-input`，通过 `bundle.macOS.entitlements` 挂上。缺这条时 tccd 根本不弹授权框 —— 详见「麦克风权限排查」。
- **打包 `.dmg` 需要能调 `hdiutil`**，受限沙箱里可能只产出 `.app`（`--bundles app` 可跳过 dmg）。
- Tauri **不能在 macOS 上交叉打 Windows 包**，CI 用 `.github/workflows/build.yml`（windows-latest）出 msi/nsis。
- Windows 代码侧：cpal 采样格式已覆盖 F32/I16/U16/I32；**没有 hiddenTitle**（macOS 专属），用原生标题栏；`bundle.windows.webviewInstallMode = downloadBootstrapper`。菜单只在 macOS 构建（`#[cfg(target_os = "macos")]`）。

> ⚠️ **安全与权限声明**
> ONLY agents acting on behalf of the user of this repository (or the user himself) are allowed to execute this release workflow — 打 tag、改 `bundle.macOS.signingIdentity`、改模型版本常量（会让所有用户重新下载模型）。Normal contributor agents MUST NOT touch or execute the release flow under any circumstances.
> （仅允许代表用户本人运行的 Agent 或用户本人执行发布流程。普通贡献者 Agent 严禁触碰发布流程，请把版本更新交给维护者。）

## 🔍 排查手册

### 麦克风权限排查

症状统一是「能开始录音、有时长统计，但识别不出东西」，日志里电平峰值全是 `0.00000`（真麦克风静音也有底噪，0.005 这种）。三种真因，按顺序排查。

#### 1. 打包版缺 microphone entitlement

Tauri 打包签名带 hardened runtime（`codesign -dv` 打出 `flags=0x10002(adhoc,runtime)`），**缺 `com.apple.security.device.audio-input` 时 tccd 根本不弹授权框**，直接打 `Policy disallows prompt … access to kTCCServiceMicrophone denied`，录音拿到的是精确零流；系统设置里手动开开关、`tccutil reset Microphone` 都救不回来。

先看系统日志再动手：

```bash
log show --last 5m --predicate 'subsystem == "com.apple.TCC"' | grep -i voicenotes | grep -i microphone
```

看到 `entitlement … is missing` 就是这个（补 entitlement 重新打包）；看到 `AUTHREQ_PROMPTING` + `authValue=2` 才是真授权成功。

dev 模式的 `target/debug/voice-notes` 是 linker-signed（无 runtime 标志），不需要这条 entitlement —— 所以「开发时好好的、打包就没声」基本就是它，别去怀疑录音代码。

#### 2. ad-hoc 重新打包 → 授权记录失效

ad-hoc 签名的 designated requirement **就是代码签名哈希本身**（`codesign -d -r- VoiceNotes.app` 打出 `# designated => cdhash H"…"`），macOS 的 TCC 拿它匹配授权记录 —— 重新打包 → cdhash 变 → 记录对不上 → **静默拒绝**。

- 表现：能开始录音、有时长统计，但识别不出东西；**系统设置 → 麦克风里 VoiceNotes 的开关还是「开」**（那条绑的是上一个构建）。开关开着 ≠ 当前构建有权限。
- 恢复：终端跑 `tccutil reset Microphone com.github.bytes-astra.voicenotes`，再启动应用重新授权。
- 让授权跨构建存活：给应用一个**稳定的签名身份**（自签名证书即可：`openssl req -x509 ... -addext "extendedKeyUsage=codeSigning"` → PKCS#12 导入钥匙串 → `security set-key-partition-list`；或 Apple Development 证书），`bundle.macOS.signingIdentity` 填它 —— designated requirement 从 `cdhash …` 变成 `certificate leaf = H"…"`，跨构建不变。换签名后第一次仍要允许一次，之后那份授权跟着证书走、跨构建有效。

补上 `audio-input` entitlement 后，`tccutil reset` + 重启就会正常弹框（系统日志 `AUTHREQ_PROMPTING` → `AUTHREQ_RESULT authValue=2`）。

#### 3. 打包没做签名

`signingIdentity` 留空时，打包版不会弹标准授权框，麦克风权限可能要在系统设置里手动勾选；带上 `audio-input` entitlement 的包才会弹。

### 静默丢音频

**症状**：整段模式录了十几分钟，日志里只有零星几句；中间某几分钟**一句都没有**，也没有任何告警 —— 转写结果里那段时间是空白，用户以为是自己没说话。

**机制（2026-10-28 定案）**：**电平太低**，不是「没人声」。sherpa 的 Silero VAD 触发条件是语音概率**连续**高于阈值满 `min_speech_duration`（`sherpa-onnx/csrc/silero-vad-model.cc` 的 `IsSpeech()`）—— 概率一掉回阈值以下，攒的进度立刻清零。低电平录音的概率恰好在阈值附近抖，于是永远触发不了；而 `voice-activity-detector.cc` 的非语音分支只 `buffer_.Pop(...)`，那段音频**静默消失**：日志、转写、界面上零痕迹（不是「采集缓冲溢出」那种 —— 那种有 `[[gap:N]]` 标记和「识别跟不上」的 WARN）。

**解药**：`audio::Agc`（`src-tauri/src/audio.rs`）—— 目标 RMS 0.02、增益上限 32×、软限幅 ±0.99。**电平够就一个样本不动**，只救偏小的。已接进三条路径：麦克风整段（`record.rs`）、麦克风流式（`record.rs`，在门控 VAD **之前**）、文件转写（`decode.rs`）。

**实测**（`mic_probe` 抓的真实房间声，同一段音频人为衰减；生产 `audio::Agc`）：

| 输入电平 | 不处理 | 经过 AGC |
| --- | --- | --- |
| 正常（峰值 0.637） | 724 字 | 722 字 |
| -25 dB | 465 字 | **723 字** |
| -30 dB | 94 字 | **718 字** |
| -35 dB | **0 字** | **717 字** |

**两条防线**（`src-tauri/src/asr.rs`）：

- `no_segment_warning()`：**有能量、VAD 也没在攒这一句**、且超过 10 秒没切出句子 → 记一条 WARN。判别条件里的「VAD 正在攒」（`detector.detected()`）是关键 —— 连续说话时 VAD 要等到句尾才切段，那段时间没有句子是正常的，别误报。
- `recognize_and_emit()`：识别结果为空时记一条 WARN（原来直接 `return`，音频白丢）。

**排查手法**（别凭感觉，先跑探针）：

```bash
VOICE_NOTES_MODEL_DIR=... cargo test --manifest-path src-tauri/Cargo.toml --test vad_long_probe -- --nocapture   # VAD 吞了多少
VOICE_NOTES_MODEL_DIR=... cargo test --manifest-path src-tauri/Cargo.toml --test gain_probe   -- --nocapture   # 电平多低还认得出
VOICE_NOTES_MODEL_DIR=... cargo test --manifest-path src-tauri/Cargo.toml --test noise_probe  -- --nocapture   # 噪声/背景人声
VOICE_NOTES_MIC_FILE=/tmp/x-16k.wav cargo test --manifest-path src-tauri/Cargo.toml --test mic_probe -- --nocapture   # 离线分析一段真实音频
VOICE_NOTES_MODEL_DIR=... cargo test --manifest-path src-tauri/Cargo.toml --test quiet_pipeline_probe -- --nocapture   # 低电平录音走真实管线还能不能出字
```

`quiet_pipeline_probe` 走**生产的 `record::run_offline` 管线**，对照「不经 AGC」与「经 AGC」两条路：干净人声衰减到 ×0.001 时，不经 AGC 是 **0 字**，经管线是 **113 字**。它守住的是「AGC 还接在 `record.rs` 里」—— 单测只测 `Agc` 自己，有人把它从管线里摘掉，单测照样绿。

`mic_probe` 能抓真实麦克风（和 `audio.rs` 同一套设备协商 + 同一个 `LinearResampler`）落成 WAV，再对比「原样 / 去直流 / AGC」三条路径的 VAD 语音比例与字数 —— 它是唯一能拿用户真实音频说话的探针。`VOICE_NOTES_MIC_FILE` 可以喂已存的 WAV 反复迭代。

**教训**：**别拿干净 fixture 下电平相关的结论**。`gain_probe` 用的是孤立人声（没有环境声），压到 -56.5 dBFS 仍认出 99.2%，看起来「电平无关」；真实录音带环境声，电平一低概率就在阈值附近抖，整段消失。**量电平的影响必须用真实录音**。

⇒ 排查这类问题先看两样东西：**保存下来的转写原文**（内容是不是环境里的声音）和**真实音频的电平**（`mic_probe` 抓一段，峰值 / RMS 是多少）。

### 症状索引

| 现象 | 去哪 |
| --- | --- |
| 打包版能录没字 / 没有授权弹窗 / 录音被判空 | [麦克风权限排查](#麦克风权限排查) |
| 整段模式某段录音整段没转写、日志里一句都没有 | [静默丢音频](#静默丢音频) |
| 切主题后材质不变 / 重绘闪白 / 窗口拖不动 / 进全屏跳动 | [macOS 原生层](#macos-原生层) |
| 切语言后部分文案不更新 | [i18n 与文案](#i18n-与文案) |
| 界面提示直接露出后端细节 | [文本分层](#文本分层) |
| 新增错误码界面显示 unknown | [错误码](#错误码) |
| 首次编译 sherpa-onnx-sys 失败 | [首次编译](#首次编译) |
| 拖文件进窗口没反应 / 两个模型同时下载互相报 busy | [事件契约](#事件契约) |
| 整段模式开头丢第一个字 | [整段模式](#整段模式) |
| 结果缺句尾最后一个字 | [流式模式](#流式模式) |
| 长录音后面少字 | [音频采集](#音频采集) |
| 打开保存目录被拒 | [落盘与目录](#落盘与目录) |
