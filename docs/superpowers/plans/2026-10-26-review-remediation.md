# 全库审查问题修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **复选框已回填（2026-10-30）**：本计划的 Task 1–6 都已实现，之前的复选框只是没回填。
> 实际接口名与计划正文有三处偏差，别照着计划写：
> - 部分结果的麦克风错误**没有**走 `AudioErrorPayload` 事件，而是写进 `Transcript.warning`（`record.rs` 的 `finish_recording`）—— 前端据此提示而不是当成致命错误。
> - `finish_recording_with`（计划里的测试专用注入点）**没有**建；测试直接用 `audio::start_with` + `inject_error` 走真实路径。
> - 菜单同步命令是 `set_menu_texts(edit, view)` 而不是 `set_menu_locale(locale)` —— 文案从字典传进 Rust，Rust 里不存菜单文案（`error.serializeSettings` 也没新增，序列化失败复用 `error.saveSettingsFailed`）。

**Goal:** 修复审查确认的行为问题、规范违规与两个结构性坏味，保持现有产品行为并补齐故障路径回归测试。

**Architecture:** 沿用现有 `record.rs` 录音编排、`engine.rs` 状态、`lib.rs` 薄命令层和前端 ES Module 边界。下载用 RAII 复位标志；录音用统一 `RecordingRun` 和共享收尾辅助；部分结果保留现有 Transcript 结构，麦克风错误通过已有 `audio:error` 事件告知。不引入依赖。

**Tech Stack:** Rust 1.78、Tauri 2、sherpa-onnx 1.13.8、原生 JavaScript ES Modules、Playwright WebKit。

**Spec:** `docs/superpowers/specs/2026-10-22-long-content-design.md`；后续明确决策与规范以 `AGENTS.md` 和 `README.md` 为准；问题清单来自 2026-10-26 全库两轴审查。

## Global Constraints

- 不新增依赖、前端构建步骤或 TypeScript；不改变事件名和正常 `Transcript` 载荷。
- 后端错误返回 `code` / `code_detail`，技术细节写日志；前端只显示字典文案，新增 key 同时加入 `zh-CN.js` 和 `en.js`。
- 推理与录音停止继续放在 `spawn_blocking`；`record.rs` 不依赖 Tauri。
- 取消录音仍丢弃结果；采集故障的停止不是取消，必须保留非空部分转写。
- 模型版本和参数不变；应用内流式模型按常量字节数判定就绪，自定义整段目录保持宽松策略。
- 保留 `set_model_dir` 的清缓存机制；不改 VAD、模型下载镜像、签名或平台 CI 配置。
- 每个任务先写并运行失败测试，再实现，再运行通过；每完成一个任务在提交前运行该任务的门禁。

## Review Focus

- 下载目录创建失败、下载失败和取消都必须复位忙碌标志：Task 1 的 RAII 单元测试。
- 截断的流式模型与 VAD 不得显示就绪：Task 1 的固定字节数测试。
- 麦克风故障不能阻断 ASR 收尾、也不能把空结果写历史：Task 2 的 Rust 和 UI 故障注入测试。
- 录音中和启动中的语言/ITN 不能改变：Task 3 的 UI 测试与后端守卫。
- 未知前端/后端错误、菜单语言切换不能暴露技术细节或残留中文：Task 4 的浏览器 mock 与 macOS 原生菜单测试。

---

### Task 1: 模型就绪与下载状态

**Files:**
- Modify: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/engine.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `models::resolve_streaming_in(dir: &Path) -> Option<StreamingPaths>`，供 `resolve_streaming` 与单元测试共用。
- Produces: `engine::DownloadFlagGuard`，在 `Drop` 中将持有的 `Arc<AtomicBool>` 置 `false`。
- Consumes: 现有 `STREAM_ASSETS`、`Asset.bytes`、`AppState.downloading{,_streaming}`。

- [x] **Step 1: 写模型与忙碌守卫失败测试。** 在 `models.rs` 的测试中用临时目录建立四个流式文件和 VAD：只有全部文件大小与常数相等时 `resolve_streaming_in` 返回 `Some`；任一截断或 VAD 截断均返回 `None`。在 `engine.rs` 测试中让 `AppState.downloading.swap(true)` 后创建守卫并 drop，断言标志恢复 false。
- [x] **Step 2: 运行测试确认失败。** `cargo test --manifest-path src-tauri/Cargo.toml --lib streaming -- --nocapture`；预期找不到 `resolve_streaming_in` / 守卫，或截断断言失败。
- [x] **Step 3: 实现最小修复。** 按 `Asset` 检查 `path.is_file()` 与 `metadata.len() == asset.bytes`，删除重复的 `exists` 特判；流式 VAD 必须有效。`download_model` / `download_streaming_model` 在 `swap(true)` 成功后立即创建守卫，再执行所有可能失败的目录初始化、下载和预热；不在成功路径手动复位。
- [x] **Step 4: 运行模型和下载测试。** `cargo test --manifest-path src-tauri/Cargo.toml --lib`；预期全部通过，既有两个下载仍相互独立。
- [x] **Step 5: 提交。** `git add src-tauri/src/models.rs src-tauri/src/engine.rs src-tauri/src/lib.rs && git commit -m "fix: recover download state and validate streaming models"`（仅在用户允许提交时执行）。

### Task 2: 录音故障保留部分结果

**Files:**
- Modify: `src-tauri/src/record.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/recorder.js`
- Modify: `src/events.js`
- Modify: `scripts/ui/check.mjs`

**Interfaces:**
- Produces: `record::finish_recording(recorder: Recorder, run: RecordingRun, timeout_secs: u64) -> Result<Transcript, String>`；它停止采集、置 stop、等待 `rx`，即使 `Recorder::stop` 返回错误也先等待并返回非空 transcript，之后返回麦克风错误。
- Consumes: `RecordingRun { stop, discard, rx }`（Task 5 若先落地，本任务直接采用；否则 Task 2 合并两种会话并同步调用点）。
- Preserves: `stop_recording() -> Result<Transcript, String>`；麦克风错误与部分文本通过已有 `audio:error` 分流，不修改 transcript 字段。
- Test-only: `record::finish_recording_with(recorder, run, timeout, wait: impl FnOnce() -> Result<Transcript, String>)`，将等待收尾抽象为可失败注入的闭包。

- [x] **Step 1: 写停止失败注入测试。** 用真实 `audio::start_with` 获取 Recorder（无可用麦克风则沿用现有跳过方式），在其 error 槽注入 `record.micError:...`；频道先送入非空 Transcript。断言停止错误不会先于 transcript 返回，且 stop 已置位；再注入空 transcript 时断言返回原麦克风错误。增加等待失败时不返回部分结果的测试。
- [x] **Step 2: 写前端失败测试。** mock `stop_recording` 抛出 `record.streamTimeout`，编辑区已有实时文本；预期先失败：目前历史不增加。断言修复后已有文本保留、只增加一条历史、提示为 `record.streamTimeout` 而非保存成功；空文本时不写历史。另注入 `audio:error {code:'record.micError', fatal:true}`：正常停止返回已积累文本时必须保存而不是清空；仅该采集错误码走停止收尾，Esc 仍调用取消并清空。
- [x] **Step 3: 运行确认失败。** 指定 Rust 测试和 `npm run check:ui`；记录原行为。
- [x] **Step 4: 实现共享停止流程。** 两条模式均调用 `finish_recording`；麦克风错误记录日志，并在有结果时发送 `AudioErrorPayload { code: record.micError, fatal: false }`，有 transcript 优先返回。超时仍走各自原错误码。整段仍保留太短和空文本判断；流式空文本不额外引入成功历史。前端只在停止彻底失败且编辑区有非空文本时，构造 `{ text, durationSeconds: elapsed, elapsedMs: 0, language: state.status?.language, segments: [] }` 调 `showTranscript`，再显示原错误。
- [x] **Step 5: 运行回归。** `cargo test --manifest-path src-tauri/Cargo.toml --lib && npm run check:i18n && npm run check:ui`；全部通过。
- [x] **Step 6: 提交。** `git add src-tauri/src/record.rs src-tauri/src/lib.rs src/recorder.js src/events.js scripts/ui/check.mjs && git commit -m "fix: preserve partial transcripts when recording stops"`（仅允许提交时执行）。

### Task 3: 锁定录音配置并拆分启动命令

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/recorder.js`
- Modify: `src/status.js`
- Modify: `src/app.js`
- Modify: `src/locales/zh-CN.js`
- Modify: `src/locales/en.js`
- Modify: `scripts/ui/check.mjs`

**Interfaces:**
- Produces: `#[tauri::command] async fn start_offline_recording(app: AppHandle, state: State<'_, AppState>)` 与 `start_streaming_recording(...)`；删除 `start_recording`。
- Produces: `ensure_config_unlocked(recording: bool) -> Result<(), String>`（`lib.rs` 私有）及实际读取 recorder 槽的调用守卫，仅供 `set_language` / `set_itn`；新增 `record.configLocked` 文案。
- Preserves: 前端按当前模式调用相应启动命令；内部设备/线程创建逻辑不复制。

- [x] **Step 1: 写配置锁定与命令失败测试。** UI 在 `recording` 和 `requesting` 阶段断言所有语言芯片及 ITN disabled，点击不发 `set_language` / `set_itn`；idle 恢复。断言整段调用 `start_offline_recording`、流式调用 `start_streaming_recording`，不再调用统一命令。后端用纯判断 `ensure_config_unlocked(recording: bool)` 测试占用/空闲两个分支；生产守卫读取实际 recorder 槽，且仅用于语言/ITN，其他设置不加锁。
- [x] **Step 2: 运行确认失败。** `npm run check:i18n && npm run check:ui`；原统一命令和可点击配置触发断言。
- [x] **Step 3: 实现。** 删除薄分派命令，为两个启动函数加 `#[tauri::command]` 和原有 recorder 冲突检查，注册新名称；在 settings 命令变更前加后端保护。`renderStatus` 统一以 `state.phase === 'idle'` 决定语言与 ITN 可用性，`setLanguage` 与 ITN handler 非 idle 直接返回；ITN 失败时回滚 checkbox。
- [x] **Step 4: 验证。** `npm run check && npm run check:ui`；全部通过，旧 `start_recording` 在前端与后端无引用。
- [x] **Step 5: 提交。** `git add src-tauri/src/lib.rs src-tauri/src/engine.rs src/recorder.js src/status.js src/app.js src/locales/*.js scripts/ui/check.mjs && git commit -m "fix: lock recognition settings during recording"`（仅允许提交时执行）。

### Task 4: 错误边界与原生菜单国际化

**Files:**
- Modify: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/menu.rs`
- Modify: `src/core.js`
- Modify: `src/app.js`
- Modify: `src/locales/zh-CN.js`
- Modify: `src/locales/en.js`
- Modify: `scripts/check-i18n.mjs`
- Modify: `scripts/ui/check.mjs`

**Interfaces:**
- Produces: `set_menu_locale(app: AppHandle, locale: String) -> Result<(), String>`；仅接受 `en`，其余按中文；调用 `menu::set_locale(app.handle(), &locale)`。
- Produces: `menu::set_locale(app: &AppHandle, locale: &str) -> tauri::Result<()>`，通过 `app.menu()` 的子菜单顺序更新 Edit/View；不存在菜单时 no-op。
- Produces: `menu.edit` / `menu.view` 中英字典；错误 key 使用现有 `error.saveSettingsFailed`、`error.readFileFailed`、`error.connectFailed`、`record.stopFailed`，必要时新增 `error.serializeSettings` 并同步字典。
- Preserves: 原生预定义编辑项和系统快捷键；不从前端直接调用 Tauri menu plugin。

- [x] **Step 1: 写错误与菜单失败测试。** UI mock 分别让命令 reject 字符串 `permission denied /tmp/private`、对象 `{ detail: '/tmp/private' }`、Error；`fmtError` 不得返回原文，统一 `error.unknown`。记录 `set_menu_locale` 参数，切英文断言 `{ locale: 'en' }`，切中文断言 `zh-CN`。检查 `menu.rs` 不再有写死中文；macOS 通过原生菜单单元/集成验证检查子菜单文本。
- [x] **Step 2: 运行确认失败。** `npm run check:i18n && npm run check:ui`；原文泄露和缺 key 触发失败。
- [x] **Step 3: 实现错误边界。** 将审查定位的四个裸错误映射到现有/新增 code_detail；`fmtError` 对未知对象返回 `error.unknown` 并 `logEvent` 技术细节，未知字符串同理。嵌套的 `{ error }` 仍只能传已经 `fmtError` 的干净文案。
- [x] **Step 4: 实现菜单同步。** 初始化用英文字典等价的 `Edit` / `View`（启动系统语言同步由首次 `applyLocale` 完成），预定义菜单项继续由系统本地化。`set_menu_locale` 更新两个子菜单，错误映射 `code_detail`；前端调用失败 `logEvent`，不显示原始异常。
- [x] **Step 5: 验证。** `npm run check && npm run check:ui`；无未翻译 key、无路径泄露；macOS 原生菜单切换测试通过。
- [x] **Step 6: 提交。** `git add src-tauri/src/settings.rs src-tauri/src/models.rs src-tauri/src/lib.rs src-tauri/src/menu.rs src/core.js src/app.js src/locales/*.js scripts/check-i18n.mjs scripts/ui/check.mjs && git commit -m "fix: sanitize errors and localize native menus"`（仅允许提交时执行）。

### Task 5: 消除会话重复与背景规范问题

**Files:**
- Modify: `src-tauri/src/record.rs`
- Modify: `src-tauri/src/engine.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/style.css`
- Modify: `AGENTS.md`

**Interfaces:**
- Produces: `pub struct RecordingRun { pub stop: Arc<AtomicBool>, pub discard: Arc<AtomicBool>, pub rx: Receiver<Transcript> }`；`AppState.stream` 和 `offline_session` 均使用 `Mutex<Option<RecordingRun>>`。
- Removes: `StreamSession`、`OfflineRun`、`record::take_new`。
- Preserves: 两条录音核心的参数、队列消费和 discard 行为。

- [x] **Step 1: 运行现有回归作为重构基线。** `cargo test --manifest-path src-tauri/Cargo.toml --lib`；尤其记录停止尾巴、缺口、流式重采样测试通过。
- [x] **Step 2: 做无行为变化的替换。** 删除两个重复结构与转发函数，两个消费点直接 `queue.take_new()`；更新构造、stop/cancel 与测试 import。`html { background-color: var(--bg); }` 独立设置，避免覆盖 `body` 的 glow；清除文件末尾多余空行。AGENTS 只更新结构名称和已修复约定，不写未实测结论。
- [x] **Step 3: 验证。** `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check && npm run check && npm run check:ui && git diff --check`；全部通过。
- [x] **Step 4: 提交。** `git add src-tauri/src/record.rs src-tauri/src/engine.rs src-tauri/src/lib.rs src/style.css AGENTS.md && git commit -m "refactor: unify recording run state"`（仅允许提交时执行）。

### Task 6: 全量验证与独立复审

**Files:**
- Modify only if verification exposes a defect in the scoped fix.
- Review: all changed files and `git diff main...HEAD` / uncommitted diff as appropriate.

- [x] **Step 1: 运行静态与 UI 门禁。** `npm run check && npm run check:ui`；Clippy `-D warnings`、Rust lib、i18n、所有 UI 项通过。
- [x] **Step 2: 运行可用模型测试。** `cargo test --manifest-path src-tauri/Cargo.toml --test asr_smoke && cargo test --manifest-path src-tauri/Cargo.toml --test streaming_smoke && cargo test --manifest-path src-tauri/Cargo.toml --test decode_smoke`；缺模型时明确记录跳过，不能声称覆盖。
- [x] **Step 3: 检查长时证据。** 若本机模型可用，运行已有 15 分钟整段真模型用例与长探针；仅报告实测覆盖，不将“消费后队列为空”当作进程内存恒定证明。无 30 分钟真实 MP4/内存采样则列为剩余验证，不造新模型或改变音频算法。
- [x] **Step 4: 独立复审。** 派新 subagent 审查完整改动：只报告确定回归、漏掉的原审查项、错误处理或测试盲点；主会话核实后修复。
- [x] **Step 5: 最终检查。** `git status --short`、`git diff --check`、完整 `npm run check && npm run check:ui`；报告实际结果、未覆盖平台及未运行的真模型场景。
