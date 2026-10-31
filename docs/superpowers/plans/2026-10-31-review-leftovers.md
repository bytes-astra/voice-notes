# 2026-10-30 审查遗留项修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **复选框已回填（2026-11-01）**：Task 1–6 都已实现，之前的复选框只是没回填。
> 实现与计划正文有出入的地方（照计划写会写错）：
> - **Task 5 的时间轴换算比计划更复杂**：计划只说「`duration_seconds` 改成喂入 + 丢掉」。实际还要把**句子的时间**换算到真实录音时间轴 —— 整段模式的 VAD 只见过喂进去的音频，不换算的话缺口会排到它本该在的位置之后。落地是 `record.rs` 的 `to_real_time()` / `gap_at()`，且换算在**秒域**比较（绕道「秒 → 采样下标」在长录音上会因 f32 往返差 ±1 个采样）。实时事件流也统一成真实时间轴。
> - **多了一处计划没写的修复**：`asr.rs` 的 `insert_gaps()` 要让同一时刻的缺口标记排在句子**前面** —— 缺口终点与它后面第一句的起点相同，稳定排序会把后 extend 的缺口排到句子后面。
> - **Task 6 的脚本比计划更严**：计划只说「把拼句常量钉在一起」。实际把两边的**判定结构**归一化后比对（CJK 区间带端点开闭、占位符完整形状、三条规则的表达式），否则 `..=` 改 `..`、`||` 改 `&&` 这类最自然的漂移都会漏检。
> 现行口径见 [AGENTS.md](../../../AGENTS.md) 与 [HOW_TO_VERIFY.md](../../HOW_TO_VERIFY.md)。

**Goal:** 把 2026-10-30 全库 `/code-review` 的遗留项逐条修掉 —— 文档描述与实现对齐、两个魔法串收掉、徽标三态收敛到一处、整段模式的时间轴补上被丢掉的音频，并给跨语言拼句规则加一道守门。

**Architecture:** 后端改动集中在 `record.rs`（缺口标记与录音时钟）和两个小工具函数（`models.rs` 的取消哨兵、`record.rs` 的停止失败判定）；前端只动 `status.js`（徽标三态的唯一出处）与 `events.js`（调用它）。新增一条静态检查脚本把 `core.js` 与 `asr.rs` 的拼句常量钉在一起。

**Tech Stack:** Rust 1.78（Tauri 2，sherpa-onnx 1.13.8）、原生 JS ES Modules（无打包器）、Playwright + 真实 WebKit（`scripts/ui/check.mjs`）、Node 脚本（`scripts/check-*.mjs`）

**Spec:** 本文档的「背景与已核实的结论」一节（= 2026-10-30 全库审查的遗留项 + 2026-10-31 对当前 `main` b764dbf 的逐条复核）。没有独立的设计稿：本次是一次性小改，范围与方案已在对话里定过。

## Global Constraints

- 前端依赖单向且不成环：`icons.js` → `core.js`/`tauri.js` → `theme.js`/`export.js` → `history.js` → `result.js` → `status.js` → `recorder.js` → `files.js`/`events.js` → `app.js`；`status.js` **不得** import `recorder.js`
- Rust 单元测试不进源文件：源文件里只写 `#[cfg(test)] mod tests;`，实现放兄弟文件；crate root 例外 → `src-tauri/src/command_tests.rs`
- 门禁：`npm run check` = `check:i18n` → `check:deps` → `check:rust`（`clippy -D warnings` → `cargo test --lib`）；前端改动**另跑** `npm run check:ui`
- 真模型用例需要环境变量：`VOICE_NOTES_MODEL_DIR`（整段）、`VOICE_NOTES_STREAMING_MODEL_DIR`（流式）；没设时这些用例会**静默跳过并返回通过** —— 门禁绿 ≠ 真模型路径验证过
- 后端只返回字典 key：`crate::code("x.y")` / `crate::code_detail("x.y", detail)`；前端 `core.js` 的 `fmtError()` 翻译
- 新增错误码必须同时进 `src/locales/zh-CN.js` 与 `src/locales/en.js`（本次不新增错误码，不需要动字典）
- 事件名只出现在 `src/events.js`；事件载荷是增量的
- 提交分离：后端 / 前端 / 文档分开提交；不提交构建产物（`src-tauri/target/`、`src-tauri/gen/schemas/`、`docs/screenshots/*.png`）
- MSRV 1.78；symphonia 锁 0.5（0.6 要 Rust 1.85）
- 界面上不要出现内部黑话（线程 / 收尾 / 引擎状态）；技术细节只进日志

## Review Focus

每条都是规格暗示、但没有哪条任务自己的测试覆盖的输入或故障模式；对应的测试挂在下面标明的任务里。

1. **识别跟不上时整段模式的录音时长** —— 丢掉的音频占真实录音时间，时长不能少算（→ Task 5）
2. **流式引擎加载失败时的徽标底色** —— 两种模式必须一致，不能一个红一个灰（→ Task 4）
3. **停止录音超时** —— 错误串带细节（`record.stopFailed:细节`）时也要走超时文案，不能把 key 甩给界面（→ Task 3）
4. **模型下载被取消** —— 界面显示「已取消」，不能显示成下载失败，也不能露出内部哨兵串（→ Task 2）
5. **拼句规则** —— 缺口标记前后不加空格、中日韩之间直连；前后端必须逐条一致（→ Task 6）

---

## 背景与已核实的结论

2026-10-30 对全库（固定点 `1aca40d`）跑过 `/code-review` 双轴审查，Standards 轴 10 项 + Spec 轴 3 项。已修：界面文案黑话、`Cargo.toml` 版本漂移、`opener`/`base64`/`dirs` 无用依赖、hybrid 文档漂移，另加 i18n 占位符门禁。**本次做剩下的**（行号基于 `main` b764dbf）：

| # | 项 | 位置 | 本次处置 |
|---|---|---|---|
| 1 | README 门禁描述与实现不符 | `README.md:111` | Task 1 |
| 2 | `"cancelled"` 魔法串散在两处 | `src-tauri/src/models.rs:403`、`:526` | Task 2 |
| 3 | `error == STOP_FAILED` 整串比较 | `src-tauri/src/lib.rs:715`、`:747` | Task 3 |
| 4 | 徽标三态写了两套且已漂移 | `src/events.js:113-126`、`src/status.js:44`、`:58` | Task 4 |
| 5 | 缺口处理 4 处重复 + 两模式时间轴不一致 | `src-tauri/src/record.rs` 四处 | Task 5 |
| 6 | 拼句规则跨语言写两份且无守门 | `src/core.js:322` ↔ `src-tauri/src/asr.rs:300` | Task 6 |

**本次不做**（用户已定）：

- 第 2 项原审查项「`lib.rs` 两个 start 命令同形」—— 抽样板。AGENTS.md 明确要求「录音按模式拆成 `start_streaming_recording` / `start_offline_recording` 两个独立命令」，拆开本身是有意决定；且它动的恰好是「并发 start 只允许一个成功」那段最敏感的代码，背后没有 bug 撑着 → 单独留待以后评估
- 原审查项「4 处注释漂移」—— 2026-10-31 复核复现不出：注释里的反引号标识符全部能在代码里找到，AGENTS.md 的 11 项数字断言逐条对代码一致。等拿到原文再改
- 「拼句规则收进后端」（事件契约带 `space: bool`）—— 收益不抵改事件契约的代价，只做 Task 6 的静态守门

**Task 5 顺带修掉的相邻缺陷**（同一个根因，必须一起做，否则缺口位置无法对齐）：整段模式的 `duration_seconds` 来自 `OfflineSession::recorded_samples()`，那只算**喂进识别器**的采样，不含被丢掉的音频 —— 识别跟不上时录音时长会少算一截。流式路径一直是对的（`StreamingSession::advance_samples()` 把丢掉的也算进时钟，回归测试 `streaming_dropped_audio_advances_recording_clock` 守着）。统一到流式的语义。

---

### Task 1: README 的门禁描述对齐实现

**Files:**
- Modify: `README.md:111`（门禁那行）、`README.md:113` 附近（命令清单）
- 不新增测试（纯 markdown；AGENTS.md：纯文档改动不用跑门禁）

**Interfaces:**
- Consumes: 无
- Produces: 无（只改文档）

- [x] **Step 1: 改门禁描述那一行**

把 `README.md:111` 的

```
npm run check        # 门禁：i18n → clippy → 单元测试
```

改成

```
npm run check        # 门禁：i18n → 前端依赖图 → clippy → 单元测试
```

- [x] **Step 2: 命令清单补一条 `check:deps`**

在 `npm run check:i18n` 那行后面加一行：

```
npm run check:deps   # 只改前端 import 图时跑这个
```

- [x] **Step 3: 验证描述与实现一致**

Run: `node -e "console.log(require('./package.json').scripts.check)"` → 期望 `npm run check:i18n && npm run check:deps && npm run check:rust`
再 Run: `npm run check:deps` → 期望退出码 0、无环、无反向依赖

- [x] **Step 4: 提交**

```bash
git add README.md
git commit -m "docs: 修正门禁描述，补上前端依赖图检查"
```

---

### Task 2: 模型下载的「取消」哨兵改用常量

**Files:**
- Modify: `src-tauri/src/models.rs`（新增常量与判定函数；改 `:403` 与 `:526` 两处字面量）
- Test: `src-tauri/src/models/tests.rs`（该文件已存在，`models.rs` 末尾已有 `#[cfg(test)] mod tests;`）

**Interfaces:**
- Consumes: `crate::code("setup.cancelled")`（`src-tauri/src/lib.rs:43`）
- Produces:
  ```rust
  // src-tauri/src/models.rs
  pub const DOWNLOAD_CANCELLED: &str = "download.cancelled";
  pub fn is_cancelled(error: &str) -> bool;
  ```
  `is_cancelled` 只认 `DOWNLOAD_CANCELLED` 这一个哨兵；**下载被取消时 `download_asset` 仍然返回 `code("setup.cancelled")`**（字典 key，界面文案不变）

- [x] **Step 1: 写失败测试**

在 `src-tauri/src/models/tests.rs` 追加：

```rust
#[test]
fn cancelled_sentinel_is_only_the_download_cancelled_marker() {
    assert!(super::is_cancelled(super::DOWNLOAD_CANCELLED));
    assert!(!super::is_cancelled("setup.cancelled"));
    assert!(!super::is_cancelled("error.downloadInterrupted"));
}

#[test]
fn cancelled_sentinel_never_reaches_the_ui() {
    // 哨兵是内部串，不能是字典 key —— 界面只该看到 setup.cancelled
    assert_ne!(super::DOWNLOAD_CANCELLED, "setup.cancelled");
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cancelled_sentinel`
Expected: FAIL —— `cannot find function is_cancelled in module crate::models`

- [x] **Step 3: 实现**

在 `src-tauri/src/models.rs` 顶部常量区加：

```rust
/// 下载被取消的内部哨兵。**不是字典 key** —— 界面看到的永远是
/// `setup.cancelled`（见 `download_asset` 的映射）。
pub const DOWNLOAD_CANCELLED: &str = "download.cancelled";

/// 这个错误是不是「下载被取消」。
pub fn is_cancelled(error: &str) -> bool {
    error == DOWNLOAD_CANCELLED
}
```

把 `fetch_one` 里 `return Err("cancelled".into());` 改成 `return Err(DOWNLOAD_CANCELLED.into());`，
把 `download_asset` 里 `if err == "cancelled" {` 改成 `if is_cancelled(&err) {`。

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cancelled_sentinel`
Expected: PASS（2 个用例）

- [x] **Step 5: 跑门禁的 Rust 部分**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: 零警告、单测全过

- [x] **Step 6: 提交**

```bash
git add src-tauri/src/models.rs src-tauri/src/models/tests.rs
git commit -m "refactor(models): 下载取消的哨兵改用常量"
```

---

### Task 3: 停止失败按错误码判定，不比整串

**Files:**
- Modify: `src-tauri/src/record.rs`（新增 `is_stop_failed`）、`src-tauri/src/lib.rs:715`、`:747`
- Test: `src-tauri/src/record/tests.rs`

**Interfaces:**
- Consumes: `record::STOP_FAILED`（`src-tauri/src/record.rs:37`，值 `"record.stopFailed"`）
- Produces:
  ```rust
  // src-tauri/src/record.rs
  pub fn is_stop_failed(error: &str) -> bool; // 比较 ':' 前的错误码部分
  ```

- [x] **Step 1: 写失败测试**

在 `src-tauri/src/record/tests.rs` 追加：

```rust
#[test]
fn stop_failed_is_recognised_with_and_without_detail() {
    assert!(super::is_stop_failed("record.stopFailed"));
    assert!(super::is_stop_failed("record.stopFailed:通道关闭"));
    assert!(!super::is_stop_failed("record.empty"));
    assert!(!super::is_stop_failed("record.stopFailedOther"));
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib stop_failed_is_recognised`
Expected: FAIL —— `cannot find function is_stop_failed in module crate::record`

- [x] **Step 3: 实现**

在 `src-tauri/src/record.rs` 的 `STOP_FAILED` 常量下面加：

```rust
/// 这个错误是不是「停止录音时识别线程没交回结果」。
///
/// 比的是**错误码**部分（':' 之前），不是整个字符串 ——
/// `code_detail(STOP_FAILED, …)` 出来的是 `record.stopFailed:细节`，
/// 整串比较会失配，超时文案就换不上了。
pub fn is_stop_failed(error: &str) -> bool {
    error.split(':').next() == Some(STOP_FAILED)
}
```

把 `src-tauri/src/lib.rs` 里两处 `if error == STOP_FAILED {` 改成 `if record::is_stop_failed(&error) {`
（两处的分支内容不变：流式 → `code("record.streamTimeout")`，整段 → `code("record.offlineTimeout")`）

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib stop_failed_is_recognised`
Expected: PASS

- [x] **Step 5: 跑门禁的 Rust 部分**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: 零警告、单测全过

- [x] **Step 6: 提交**

```bash
git add src-tauri/src/record.rs src-tauri/src/lib.rs src-tauri/src/record/tests.rs
git commit -m "fix(record): 停止失败按错误码判定，不比整串"
```

---

### Task 4: 徽标三态收敛到 status.js

**Files:**
- Modify: `src/status.js`（新增 `applyEngineState`；`renderStatus()` 里两处三元链改用它）、`src/events.js:110-127`（`engine:state` 与 `streaming:state` 两个处理器改调它）
- Test: `scripts/ui/check.mjs`（新增界面检查用例）

**Interfaces:**
- Consumes: `state.engine`（`src/core.js`）、`modelBadgeText(phase)`（`src/status.js:16`）
- Produces:
  ```js
  // src/status.js
  export function applyEngineState(phase) // phase: 'error' | 'loading' | 'ready'
  ```
  唯一决定「dot 的 data-state + 徽标底色 + 徽标文案」的地方。`error` → dot `error` + `badge danger`；`loading` → dot `loading` + `badge`；`ready` → dot `ready` + `badge`。

**注意（既有行为，别改）**：`renderStatus()` 里「模型没就绪」的两个分支（`model.badge.streamingMissing` / `model.badge.offlineMissing`）不是引擎三态，保持独立 —— 它们用 `badge warn`，文案是「模型没装好」，不是「引擎在报错」。

- [x] **Step 1: 写失败测试（界面检查）**

在 `scripts/ui/check.mjs` 里，紧跟现有的「模型加载失败显示字典文案」用例之后插入：

```js
// 流式引擎报错：徽标底色必须跟整段一致（红底），不能一个红一个灰
//
// 必须点模式芯片切到流式：前端只在 setMode / refreshStatus 时读 status，
// 直接改 window.__mockStatus 不会让前端状态跟着变。
await page.locator('.mode-chip[data-mode="streaming"]').click()
await page.waitForTimeout(200)
await page.evaluate(() =>
  window.__emit('streaming:state', {
    phase: 'error',
    message: 'model.streamLoadFailed:/Users/someone/Library/Application Support/models/streaming/model.int8.onnx',
  }),
)
await page.waitForTimeout(200)
const streamingBadge = await page.locator('#model-badge').getAttribute('class')
check('流式引擎报错时徽标是红底', streamingBadge.includes('danger'), streamingBadge)
check(
  '流式引擎报错时状态点标红',
  (await page.locator('#engine-dot').getAttribute('data-state')) === 'error',
)
// 切回整段，后面的用例还按整段模式走
await page.locator('.mode-chip[data-mode="offline"]').click()
await page.waitForTimeout(200)
```

- [x] **Step 2: 跑界面检查确认失败**

Run: `npm run check:ui`
Expected: FAIL —— 「流式引擎报错时徽标是红底」不成立（当前 `status.js:40` 只按 `ready` 判底色，流式分支给的是 `badge`）

- [x] **Step 3: 实现 `applyEngineState`**

在 `src/status.js` 的 `modelBadgeText` 下面加：

```js
/// 引擎三态 → 状态点 + 徽标底色 + 徽标文案。**唯一一处**。
///
/// 整段（engine:state）和流式（streaming:state）都走这里 —— 两套写法
/// 曾经各写一遍，结果流式报错时徽标是灰底、整段是红底。
export function applyEngineState(phase) {
  if (phase === 'error') {
    els.dot.dataset.state = 'error'
    els.badge.className = 'badge danger'
    els.badge.textContent = modelBadgeText('error')
  } else if (phase === 'loading') {
    els.dot.dataset.state = 'loading'
    els.badge.className = 'badge'
    els.badge.textContent = modelBadgeText('loading')
  } else {
    els.dot.dataset.state = 'ready'
    els.badge.className = 'badge'
    els.badge.textContent = modelBadgeText('ready')
  }
}
```

把 `src/events.js` 的 `engine:state` 处理器（`:110-127`）整个换成：

```js
await listen('engine:state', (event) => {
  const phase = event.payload?.phase
  state.engine = phase
  applyEngineState(phase)
  if (phase === 'error') {
    toast(errorMessage(event.payload?.message), 'error')
  }
})
```

把 `src/events.js` 的 `streaming:state` 处理器（`:94-108`）改成：

```js
await listen('streaming:state', (event) => {
  const payload = event.payload || {}
  state.engine = payload.phase
  if (payload.phase === 'loading') {
    if (state.phase === 'requesting') els.hint.textContent = t('record.loadingStreaming')
  }
  // 徽标交给 renderStatus（它内部按引擎三态渲染）
  renderStatus()
  if (payload.phase === 'error') {
    toast(errorMessage(payload.message), 'error')
  }
})
```

（流式分支里 `renderStatus()` 已经在「模型就绪」时调用 `applyEngineState(state.engine)`，所以这里**不要**再单独调一次 —— 否则「模型没就绪」分支的 warn 底色会被冲掉。）

把 `src/status.js` 的 `renderStatus()` 里两处三元链换成调用：
- 流式分支（`:44`）：`applyEngineState(state.engine)` —— 但**底色要保留 `ready ? 'badge' : 'badge warn'` 的模型缺失判断**：

```js
if (streamingMode) {
  if (!ready) {
    els.badge.className = 'badge warn'
    els.badge.textContent = t('model.badge.streamingMissing')
    els.dot.dataset.state = 'idle'
  } else {
    applyEngineState(state.engine)
  }
}
```

- 整段已就绪分支（`:55-58`）：`applyEngineState(state.engine)`

`src/events.js` 的 import 行补上 `applyEngineState`：

```js
import { applyEngineState, modelBadgeText, renderStatus, setProgress, setStreamProgress } from './status.js'
```

- [x] **Step 4: 跑界面检查确认通过**

Run: `npm run check:ui`
Expected: 全部通过（含新增的 2 条），既有 95 项一条都不能红

- [x] **Step 5: 跑门禁**

Run: `npm run check`
Expected: i18n → 依赖图 → clippy → 单测全过（依赖图检查确认没引入反向依赖）

- [x] **Step 6: 提交**

```bash
git add src/status.js src/events.js scripts/ui/check.mjs
git commit -m "fix(ui): 徽标三态收敛到一处，流式报错也变红底"
```

---

### Task 5: 整段模式的时间轴补上被丢掉的音频

**Files:**
- Modify: `src-tauri/src/record.rs`（新增两个辅助函数；整段主循环 `:167-186`、整段收尾 `:215-231`、流式主循环 `:317-334`、流式收尾 `:413-428` 四处统一；整段 `duration_seconds` 的算法）
- Test: `src-tauri/src/record/tests.rs`

**Interfaces:**
- Consumes: `audio::gap_seconds`、`audio::gap_marker`、`audio::resampled_len`、`asr::SAMPLE_RATE`（都已存在）
- Produces:
  ```rust
  // src-tauri/src/record.rs
  fn gap_segment(dropped: u32, rate: u32, at: f32, what: &str) -> asr::Segment;
  fn recorded_seconds(fed_16k: usize, dropped_16k: usize) -> f32;
  ```
  `gap_segment` 记一条 WARN 日志并返回 `start == end == at` 的缺口段；`what` 是日志前缀（如 `"离线识别跟不上，"`），日志全文是 `{what}丢掉 {seconds} 秒音频`。
  `recorded_seconds` = `(fed_16k + dropped_16k) / 16000` —— **丢掉的音频占真实录音时间**。

**语义决定（已定，别改）**：缺口标记落在**缺口终点**（丢掉的音频之后才开始识别）。流式路径本来就是这样（`advance_samples()` 先把丢掉的算进时钟），整段路径原来落在缺口起点 —— 本次统一到终点。

- [x] **Step 1: 写失败测试**

在 `src-tauri/src/record/tests.rs` 追加（**不需要模型**）：

```rust
#[test]
fn gap_segment_marks_the_end_of_the_dropped_window() {
    let segment = super::gap_segment(16_000, 16_000, 1.5, "离线识别跟不上，");
    assert!(segment.text.starts_with("[[gap:"), "缺口标记格式不对: {}", segment.text);
    assert_eq!(segment.start, 1.5);
    assert_eq!(segment.end, 1.5);
}

#[test]
fn dropped_audio_counts_towards_recorded_time() {
    // 丢掉 16000 个采样（1 秒）也算在录音时长里
    assert!((super::recorded_seconds(32_000, 16_000) - 3.0).abs() < 1e-6);
    assert!((super::recorded_seconds(32_000, 0) - 2.0).abs() < 1e-6);
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib gap_segment_marks_the_end`
Expected: FAIL —— `cannot find function gap_segment in module crate::record`

- [x] **Step 3: 实现两个辅助函数**

在 `src-tauri/src/record.rs` 里（`log_segment` 附近）加：

```rust
/// 识别跟不上、丢掉一段音频时：造一个缺口标记段。
///
/// 缺口标记落在**缺口终点**（`at` 由调用方给）—— 丢掉的音频在这之后
/// 才开始被识别。流式与整段必须同一条规则，否则同一段录音在两个模式里
/// 缺口的位置不一样。
fn gap_segment(dropped: u32, rate: u32, at: f32, what: &str) -> asr::Segment {
    let seconds = audio::gap_seconds(dropped, rate);
    log::warn(format!("{what}丢掉 {seconds} 秒音频"));
    let marker = audio::gap_marker(dropped, rate);
    asr::Segment {
        text: marker,
        start: at,
        end: at,
    }
}

/// 真实录音时间（秒）：喂给识别器的采样 + 被丢掉的采样。
///
/// 丢掉的音频占真实录音时间（流式路径一直这么算）—— 不算进去，
/// 识别跟不上时录音时长会短一截。
fn recorded_seconds(fed_16k: usize, dropped_16k: usize) -> f32 {
    (fed_16k + dropped_16k) as f32 / asr::SAMPLE_RATE as f32
}
```

- [x] **Step 4: 四处调用点统一改用 `gap_segment`**

**整段主循环**（`src-tauri/src/record.rs:167-186`）—— 改成：

```rust
if new_audio.dropped > 0 {
    // 识别跟不上：丢掉了一段音频，要在结果里留下可见的缺口。
    // 标记既进最终文本，也作为一句实时发出去（前端追加后才看得到）。
    let dropped_16k = audio::resampled_len(new_audio.dropped, rate);
    let segment = gap_segment(
        new_audio.dropped,
        rate,
        recorded_seconds(fed_16k, dropped_16k),
        "离线识别跟不上，",
    );
    dropped_16k_total += dropped_16k;
    gaps.push(segment.clone());
    if !discarded() {
        emit(OfflineEvent::Segment(segment));
    }
}
```

（`fed_16k` 含义不变：已喂给识别器的 16 kHz 采样数；新增 `let mut dropped_16k_total = 0usize;` 与它并列声明）

**整段收尾**（`:215-231`）—— 同样的形状，`at` 用 `recorded_seconds(fed_16k, dropped_16k)`，日志前缀 `"离线收尾"`，`dropped_16k_total` 同样累加（这一处没有 `discarded()` 判断，保持原样）。

**流式主循环**（`:317-334`）—— `at` 保持 `session.elapsed_seconds()`（那个时钟已经把丢掉的算进去了），日志前缀 `"流式识别跟不上，"`，其余形状不变。

**流式收尾**（`:413-428`）—— `at` 保持 `session.elapsed_seconds()`，日志前缀 `"流式收尾"`，其余形状不变。

**整段时长**：`run_offline` 结尾构造 Transcript 的地方（`src-tauri/src/record.rs:256`）把

```rust
duration_seconds: session.recorded_samples() as f32 / asr::SAMPLE_RATE as f32,
```

改成

```rust
duration_seconds: recorded_seconds(session.recorded_samples(), dropped_16k_total),
```

- [x] **Step 5: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib gap_segment_marks_the_end dropped_audio_counts`
Expected: PASS

- [x] **Step 6: 补一条带模型的端到端断言**

在 `src-tauri/src/record/tests.rs` 现有的 `offline_gap_marker_reaches_transcript`（`:289`）末尾追加：

```rust
    // 缺口标记落在缺口终点：丢掉 N 个采样 → 标记在 N/16000 秒处
    let gap = transcript
        .segments
        .iter()
        .find(|s| s.text.contains("[[gap:"))
        .expect("结果里应该有缺口标记");
    let dropped = (wave.samples().len() as u32).saturating_sub(1_000);
    let expected = dropped as f32 / 16_000.0;
    assert!(
        (gap.start - expected).abs() < 0.05,
        "缺口标记应落在缺口终点附近：期望 {expected}，实际 {}",
        gap.start
    );
    // 录音时长要把丢掉的音频算进去
    assert!(
        transcript.duration_seconds >= expected - 0.05,
        "录音时长应包含被丢掉的音频：{} < {expected}",
        transcript.duration_seconds
    );
```

Run: `VOICE_NOTES_MODEL_DIR=$HOME/Library/Application\ Support/com.github.bytes-astra.voicenotes/models cargo test --manifest-path src-tauri/Cargo.toml --lib offline_gap_marker_reaches_transcript -- --nocapture`
Expected: PASS（**没设 `VOICE_NOTES_MODEL_DIR` 时这个用例会静默跳过** —— 那是「没验」不是「过了」，必须带环境变量跑）

- [x] **Step 7: 跑门禁的 Rust 部分**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: 零警告、单测全过

- [x] **Step 8: 提交**

```bash
git add src-tauri/src/record.rs src-tauri/src/record/tests.rs
git commit -m "fix(record): 整段模式时间轴补上被丢掉的音频"
```

---

### Task 6: 跨语言拼句规则的静态一致性检查

**Files:**
- Create: `scripts/check-join-rules.mjs`
- Modify: `package.json`（加 `check:join` 脚本，并挂进 `check` 链）
- 不改 `src/core.js` 与 `src-tauri/src/asr.rs`（这两个文件是**被检查方**，本任务不改它们的逻辑）

**Interfaces:**
- Consumes: `src/core.js` 的 `isCjk()` 码位区间与 `[[gap:` 前缀、`src-tauri/src/asr.rs` 的 `is_cjk()` 码位区间、`src-tauri/src/audio.rs` 的 `GAP_MARKER_PREFIX`
- Produces: `npm run check:join` —— 不一致时打印**文件 + 具体是哪个码位区间/前缀**并以退出码 1 结束

**检查什么（三条，各一条断言）**：

1. CJK 码位区间集合相同 —— `core.js` 的 `isCjk` 里 `(0x…. && code <= 0x…)` 对，与 `asr.rs` 的 `is_cjk` 里 `0x…. ..= 0x…` 区间，解析后按集合比较（顺序可以不同，集合必须一样）
2. 缺口前缀相同 —— `core.js` 的 `GAP_RE` / `isGapMarker` 用的 `[[gap:`，与 `audio.rs` 的 `GAP_MARKER_PREFIX` 必须是同一个串
3. 缺口前后的空格规则两边都在 —— `core.js` 的 `joinText` 里必须同时出现「右边是占位符」和「左边以占位符结尾」两个判定；`asr.rs` 的 `join_segments` 里必须同时出现 `is_gap_marker(part)` 与 `ends_with_gap_marker(&out)`

**明确不覆盖**：行为级等价（同一对文本在两边是否真的拼出同样的结果）。那需要跨语言跑同一批用例，代价远大于收益；本任务只钉住常量与规则的存在性 —— 真正会漂的是码位区间。

- [x] **Step 1: 写脚本**

创建 `scripts/check-join-rules.mjs`：

```js
#!/usr/bin/env node
// 拼句规则的前后端一致性检查。
//
// core.js 的 joinText() 与 asr.rs 的 join_segments() 必须拼出一样的结果 ——
// 规则在两种语言里各写一份，没有这道检查的话，改了一边没人知道。
// 只钉常量与规则的存在性；行为级等价需要跨语言跑同一批用例，代价太大。

import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = (p) => readFileSync(join(root, p), 'utf8')

const errors = []

// 1. CJK 码位区间
const js = read('src/core.js')
const rust = read('src-tauri/src/asr.rs')

const jsRanges = [...js.matchAll(/\(code >= (0x[0-9a-f]+) && code <= (0x[0-9a-f]+)\)/gi)].map(
  (m) => `${m[1].toLowerCase()}-${m[2].toLowerCase()}`,
)
const rustRanges = [...rust.matchAll(/(0x[0-9a-f]+)\s*\.\.=\s*(0x[0-9a-f]+)/gi)].map(
  (m) => `${m[1].toLowerCase()}-${m[2].toLowerCase()}`,
)
const jsSet = new Set(jsRanges)
const rustSet = new Set(rustRanges)
for (const range of rustSet) {
  if (!jsSet.has(range)) errors.push(`src/core.js 缺 CJK 区间 ${range}（后端 asr.rs 有）`)
}
for (const range of jsSet) {
  if (!rustSet.has(range)) errors.push(`src-tauri/src/asr.rs 缺 CJK 区间 ${range}（前端 core.js 有）`)
}

// 2. 缺口前缀
const audio = read('src-tauri/src/audio.rs')
const rustPrefix = audio.match(/GAP_MARKER_PREFIX:\s*&str\s*=\s*"([^"]+)"/)?.[1]
// core.js 里前缀是引号里的字面量（isGapMarker 的 startsWith('[[gap:')）。
// 注意别用 `'([^']*\[\[gap:[^']*)'` 这种写法 —— [^']* 会跨行吞掉半个文件。
const jsPrefix = js.match(/'\[\[gap:'/)?.[0]?.slice(1, -1)
if (!rustPrefix) errors.push('src-tauri/src/audio.rs 里找不到 GAP_MARKER_PREFIX')
if (!jsPrefix) errors.push('src/core.js 里找不到 [[gap: 前缀')
if (rustPrefix && jsPrefix && rustPrefix !== jsPrefix) {
  errors.push(`缺口前缀不一致：core.js "${jsPrefix}" vs audio.rs "${rustPrefix}"`)
}

// 3. 缺口前后的空格规则两边都在
if (!/isGapMarker\(right\)/.test(js) || !/GAP_END_RE\.test\(left\)/.test(js)) {
  errors.push('src/core.js 的 joinText 缺少缺口的空格规则（isGapMarker(right) / GAP_END_RE.test(left)）')
}
if (!/is_gap_marker\(part\)/.test(rust) || !/ends_with_gap_marker\(&out\)/.test(rust)) {
  errors.push('src-tauri/src/asr.rs 的 join_segments 缺少缺口的空格规则（is_gap_marker(part) / ends_with_gap_marker(&out)）')
}

if (errors.length) {
  console.error('拼句规则不一致：')
  for (const e of errors) console.error(`  - ${e}`)
  process.exit(1)
}
console.log('拼句规则一致 ✓')
```

- [x] **Step 2: 跑脚本确认当前代码通过**

Run: `node scripts/check-join-rules.mjs`
Expected: 退出码 0，输出 `拼句规则一致 ✓`

> 这一步在写计划时已经在临时目录里验过：当前代码通过；四种人为漂移（改前端码位 / 改后端码位 / 删掉前端缺口空格规则 / 改后端缺口前缀）全部被抓住并精确报到文件与区间。

- [x] **Step 3: 反向验证 —— 故意改坏一边，脚本必须报错**

把 `src/core.js` 的 `isCjk` 里第一段 `0x3000` 改成 `0x2fff`（只做这一处、只为了验证脚本有效）：

Run: `node scripts/check-join-rules.mjs`
Expected: 退出码 1，并打印出**具体是哪个区间**在 `src/core.js` 或 `src-tauri/src/asr.rs` 缺失

然后把 `src/core.js` 改回原样：

Run: `node scripts/check-join-rules.mjs`
Expected: 退出码 0

- [x] **Step 4: 挂进门禁**

`package.json` 的 `scripts` 加一条：

```json
"check:join": "node scripts/check-join-rules.mjs"
```

并把 `check` 改成：

```json
"check": "npm run check:i18n && npm run check:deps && npm run check:join && npm run check:rust"
```

- [x] **Step 5: 跑门禁确认通过**

Run: `npm run check`
Expected: 全过（i18n → 依赖图 → 拼句规则 → clippy → 单测）

- [x] **Step 6: 提交**

```bash
git add scripts/check-join-rules.mjs package.json
git commit -m "test(ui): 加跨语言拼句规则一致性检查"
```

---

## 自查

- **规格覆盖**：背景表里 6 项 → Task 1-6 一一对应；本次不做的两项写明了理由与出处
- **步骤粒度**：每个 Step 都能落成「一个动作 + 一个可检查的结果」；没有 TBD、没有「处理边界情况」这类空步骤
- **类型一致性**：`gap_segment` / `recorded_seconds` / `is_stop_failed` / `is_cancelled` / `applyEngineState` 的签名在 Task 里定义、在用到它的 Step 里按同一签名调用
- **Review Focus 覆盖**：4 → Task 5、2 → Task 4、3 → Task 3、1 → Task 5、5 → Task 6，各自有测试步骤
- **比例**：6 个任务、约 40 个步骤，对应 6 处代码/文档改动 —— 没有把实现抄进计划
