# Review Findings 修复收尾实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 修掉 `fix/review-findings` 分支独立审核查出的 6 个真问题（错误码占位符、前端依赖成环、mock 载荷与真实后端相反、切语言覆盖用户编辑、设备标签无回退、回归测试缺口），并把约定写进文档。

**Architecture:** 后端抽一个共用的「转写能不能用」守卫，流式/整段两条路径共用；前端把 `renderResultMeta` 挪进 `core.js` 打断依赖环，缺口占位符改成按「上次渲染的占位符文本」做替换；加一个静态依赖图检查脚本守住单向依赖。

**Tech Stack:** Rust（serde / sherpa-onnx）、原生 ES Modules（无打包器）、Playwright WebKit、Node。

**Spec:** `docs/superpowers/specs/2026-10-28-review-fixes-design.md`（范围 1 / 4 / 5、关键决策、验收）+ 本计划记录的审核发现。

## Global Constraints

- 后端错误只返回字典 key：用户看得懂的参数走 `code_detail`，不把技术细节甩给界面。
- 前端单向依赖，**不能反、也不能成环**（链：icons → core/tauri → theme/export → history → result → status → recorder → files/events → app）。
- 字典改动必须两种语言同时改；`npm run check:i18n` 会校验 key 对齐。
- 每个缺陷先有失败回归测试，再最小实现；纯文档改动不用跑门禁。
- 前端测试写进 `scripts/ui/check.mjs`（假 `__TAURI__` + 真实 WebKit），Rust 测试写进 `src-tauri/src/<模块>/tests.rs`。
- 不改模型、热词、VAD 参数、事件名、公开命令名、文本导出格式；不重新生成截图。
- 提交分离：后端 / 前端 / 文档分开提交。

## Review Focus

- 切语言**第二次**切换后，用户手改的文本仍在（第一版修复只挡住了一次）。
- 用户编辑过的文本里，缺口占位符仍随语言更新，且**不覆盖**用户改的其他字符。
- 流式录音过短时，界面显示「至少说 0.2 秒」而不是「至少说 {seconds} 秒」。
- 设备下拉里系统默认设备的「· 系统默认」标记真的出现（不是靠首项同名蒙混过关）。
- 前端 import 图无环，且每条边都符合上面的单向链（防回退）。
- 没有 `VOICE_NOTES_STREAMING_MODEL_DIR` 时，门禁跑绿**不代表**流式时间轴验证过 —— 文档要说清这一点。

---

### Task 1: 流式过短错误带上秒数

**Files:**
- Modify: `src-tauri/src/record.rs`（新增守卫函数）
- Modify: `src-tauri/src/lib.rs:721-726, 752-757`（两条路径改调守卫）
- Test: `src-tauri/src/record/tests.rs`

**Interfaces:**
- Produces: `record::ensure_transcript_usable(text: &str, duration_seconds: f32) -> Result<(), String>`，错误串格式 `record.tooShort:0.2` / `record.empty`。
- Consumes: `crate::code` / `crate::code_detail`（lib.rs 里已有）。

- [x] **Step 1: 写失败测试**

在 `src-tauri/src/record/tests.rs` 加：

```rust
#[test]
fn too_short_transcript_reports_seconds() {
    let err = ensure_transcript_usable("你好", 0.1).unwrap_err();
    assert_eq!(err, "record.tooShort:0.2");
}

#[test]
fn empty_transcript_is_rejected() {
    let err = ensure_transcript_usable("   ", 3.0).unwrap_err();
    assert_eq!(err, "record.empty");
}

#[test]
fn usable_transcript_passes() {
    assert!(ensure_transcript_usable("你好", 3.0).is_ok());
}
```

- [x] **Step 2: 运行确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib transcript`
Expected: FAIL —— `cannot find function ensure_transcript_usable in this scope`。

- [x] **Step 3: 实现守卫**

在 `src-tauri/src/record.rs` 加：

```rust
/// 界面能接受的最低音频时长（秒）。
pub const MIN_TRANSCRIPT_SECONDS: f32 = 0.2;

/// 这段转写能不能交给界面：太短或空文本都返回字典错误码。
///
/// 流式和整段走同一套判定 —— 两条路径的文案必须一致，
/// 否则一边显示「至少说 0.2 秒」一边显示「至少说 {seconds} 秒」。
pub fn ensure_transcript_usable(text: &str, duration_seconds: f32) -> Result<(), String> {
    if duration_seconds < MIN_TRANSCRIPT_SECONDS {
        return Err(crate::code_detail("record.tooShort", "0.2"));
    }
    if text.trim().is_empty() {
        return Err(crate::code("record.empty"));
    }
    Ok(())
}
```

- [x] **Step 4: 让 lib.rs 两条路径都走它**

`src-tauri/src/lib.rs`：流式分支（721-726 行）改成

```rust
ensure_transcript_usable(&finished.transcript.text, finished.transcript.duration_seconds)?;
```

整段分支（752-757 行）删掉重复判定，只保留 `ensure_transcript_usable(...)?`；
两处 `use` 里补 `record::ensure_transcript_usable`。

- [x] **Step 5: 运行确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: 全部通过（65 个）。

- [x] **Step 6: 提交**

```bash
git add src-tauri/src/record.rs src-tauri/src/record/tests.rs src-tauri/src/lib.rs
git commit -m "fix(record): 过短提示带上秒数，流式与整段走同一套守卫"
```

---

### Task 2: 打断前端依赖环

**Files:**
- Create: `scripts/check-deps.mjs`（静态依赖图检查）
- Modify: `src/core.js`（收下 `renderResultMeta`）
- Modify: `src/result.js`（改从 core.js 引）
- Modify: `src/history.js`（删掉 `import { renderResultMeta } from './result.js'`）
- Modify: `package.json`（加 `check:deps`，并进 `check`）
- Test: `scripts/check-deps.mjs` 本身

**Interfaces:**
- Produces: `core.js` 导出 `renderResultMeta()`（签名不变，只是搬家）。
- Produces: `npm run check:deps` —— 无环 + 每条边符合单向链，不满足时非零退出并打印违规项。

- [x] **Step 1: 写失败测试**

新建 `scripts/check-deps.mjs`：读 `src/*.js` 的 `import ... from './x.js'`，
建图，判环 + 判每条边是否满足

```
icons → core/tauri → theme/export → history → result → status → recorder → files/events → app
```

（同一层内允许反向？不允许 —— 边必须从靠前的层指向靠后的层）。
脚本发现环或反向边时打印 `违规: <from> -> <to>` 并 `process.exit(1)`。

- [x] **Step 2: 运行确认失败**

Run: `npm run check:deps`
Expected: FAIL —— 打印 `违规: history -> result`。

- [x] **Step 3: 把 renderResultMeta 挪进 core.js**

- `src/core.js`：加 `export function renderResultMeta() { ... }`（实现从 `src/result.js:23-46` 原样搬过来，用到 `els` / `state` / `t` / `fmtDuration` / `I18N`，全在 core.js 内部）。
- `src/result.js`：删掉该函数，`import { ..., renderResultMeta } from './core.js'`。
- `src/history.js`：删掉 `import { renderResultMeta } from './result.js'`，改成从 `'./core.js'` 引。

- [x] **Step 4: 运行确认通过**

Run: `npm run check:deps` → Expected: PASS，无输出。
Run: `npm run check:ui` → Expected: 95/95 通过。

- [x] **Step 5: 提交**

```bash
git add scripts/check-deps.mjs package.json src/core.js src/result.js src/history.js
git commit -m "refactor(ui): renderResultMeta 挪进 core.js，打断 history/result 依赖环"
```

---

### Task 3: mock 载荷与真实后端一致

**Files:**
- Modify: `scripts/ui/check.mjs:122`（mock 字段）
- Modify: `scripts/ui/shots.mjs:133`（mock 字段）
- Modify: `scripts/ui/check.mjs:229`（断言改成真断言）
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- 后端 `InputDevice` 序列化后是 `{ name, isDefault }`（`#[serde(rename_all = "camelCase")]`），mock 必须一致。

- [x] **Step 1: 把断言改成真断言（先让它失败）**

`scripts/ui/check.mjs:229` 现在是

```js
check('默认设备标记使用真实后端字段', (await page.locator('.select-popup').innerText()).includes('系统默认'))
```

改成断言**设备项本身**带后缀（首项本来就叫「系统默认」，所以旧断言恒真）：

```js
check('默认设备标记使用真实后端字段', (await page.locator('.select-popup').innerText()).includes('Mock Mic · 系统默认'))
```

- [x] **Step 2: 运行确认失败**

Run: `npm run check:ui`
Expected: FAIL —— `默认设备标记使用真实后端字段` 报错（mock 用了 `is_default`，前端读 `device.isDefault` 拿不到）。

- [x] **Step 3: 修 mock**

`scripts/ui/check.mjs:122` 与 `scripts/ui/shots.mjs:133`：

```js
return [{ name: 'Mock Mic', isDefault: true }]
```

- [x] **Step 4: 运行确认通过**

Run: `npm run check:ui` → Expected: 95/95 通过。
Run: `npm run shots` → README 截图里设备项重新带上「· 系统默认」标记。

- [x] **Step 5: 提交**

```bash
git add scripts/ui/check.mjs scripts/ui/shots.mjs
git commit -m "fix(ui): mock 设备载荷跟真实后端对齐成 camelCase"
```

---

### Task 4: 切语言不覆盖用户编辑

**Files:**
- Modify: `src/core.js`（新增 `gapMarkerTexts(rawText)`）
- Modify: `src/result.js`（`relocalizeResult` 改按占位符替换）
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- Produces: `core.js` 导出 `gapMarkerTexts(rawText: string) -> string[]` —— 按出现顺序返回**已本地化**的缺口占位符文本（`localizeText('[[gap:3]]')` 的结果）。
- `state.resultView` 增加 `markers: string[]` 字段：当前显示文本里那些占位符的**已渲染**文本（用户编辑后也保留这份快照）。

- [x] **Step 1: 写失败测试**

`scripts/ui/check.mjs` 里加两段：

```js
// 连续切两次语言，用户编辑仍在
await page.locator('#text').fill('用户手改的内容')
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: '简体中文' }).first().click()
await page.waitForTimeout(200)
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
await page.waitForTimeout(200)
check('连续切两次语言不覆盖用户编辑', (await page.locator('#text').inputValue()) === '用户手改的内容')
```

外加缺口占位符随语言更新（需要一段带 `[[gap:3]]` 的文本）：

```js
window.__mockTranscript = '浏览器结果 [[gap:3]]'
// ...触发一次 stop_recording 让结果区拿到这段文本...
check('缺口占位符随语言更新', (await page.locator('#text').inputValue()).includes('(3s not recognised)'))
```

- [x] **Step 2: 运行确认失败**

Run: `npm run check:ui`
Expected: FAIL —— `连续切两次语言不覆盖用户编辑` 报错（第二次切换时 `view.rendered` 被写成用户文本，守卫失效 → 原文覆盖编辑）。

- [x] **Step 3: 实现**

`src/core.js`：

```js
/// 文本里的缺口占位符，按出现顺序返回已本地化的文本。
///
/// 切语言时要按「上次渲染出来的占位符」去定位再替换 ——
/// 用户编辑后文本里的是旧语言的占位符，用新语言的模板找不到位置。
export function gapMarkerTexts(rawText) {
  if (!rawText) return []
  return [...rawText.matchAll(/\[\[gap:\d+\]\]/g)].map((m) => localizeText(m[0]))
}
```

`src/result.js`：

- `setResultView` 里同时存 `markers: gapMarkerTexts(rawText)`。
- `relocalizeResult` 改成：

```js
export function relocalizeResult() {
  const view = state.resultView
  if (!view) return
  const rendered = localizeText(view.rawText)
  if (els.text.value === view.rendered) {
    els.text.value = rendered
  } else {
    // 用户编辑过：只换缺口占位符，其他字符一个不动。
    els.text.value = swapGapMarkers(els.text.value, view.markers || [], gapMarkerTexts(view.rawText))
  }
  view.rendered = rendered
  view.markers = gapMarkerTexts(view.rawText)
  renderResultMeta()
}
```

- 新增私有函数：

```js
/// 把编辑后文本里的旧占位符换成新语言的占位符。
/// 找不到就跳过 —— 用户把那段改了，就别动它。
function swapGapMarkers(text, oldMarkers, newMarkers) {
  let out = text
  let from = 0
  for (let i = 0; i < oldMarkers.length; i++) {
    const oldMark = oldMarkers[i]
    const newMark = newMarkers[i]
    if (!oldMark || !newMark || oldMark === newMark) continue
    const at = out.indexOf(oldMark, from)
    if (at < 0) continue
    out = out.slice(0, at) + newMark + out.slice(at + oldMark.length)
    from = at + newMark.length
  }
  return out
}
```

**要点**：`view.rendered` 永远存「原文的本地化结果」，**不再被写成用户文本** —— 这是第二次切换不再覆盖编辑的关键。

- [x] **Step 4: 运行确认通过**

Run: `npm run check:ui` → Expected: 全部通过（新增断言在内）。

- [x] **Step 5: 提交**

```bash
git add src/core.js src/result.js scripts/ui/check.mjs
git commit -m "fix(ui): 切语言按占位符替换，不再覆盖用户编辑"
```

---

### Task 5: 设备标签空时有回退

> **已撤销（2026-10-29）**：原方案用本地化回退标签「未命名设备」当设备身份，
> 但设备名就是后端的身份（`audio::pick_device_name` 只做精确匹配）——
> 把展示文案当身份传过去，用户选了设备却静默录到系统默认麦，且标签随界面语言变。
> 现行做法：无名设备**根本不列**（`audio::device_entry`），界面上不出现选不中的项。
> 本节保留原计划记录。

**Files:**
- Modify: `src/locales/zh-CN.js`、`src/locales/en.js`（新 key）
- Modify: `src/status.js:178`（标签回退）
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- 新字典 key：`record.deviceUnnamed` —— zh「未命名设备」/ en「Unnamed device」。

- [x] **Step 1: 写失败测试**

`scripts/ui/check.mjs` 的 mock 里加一个空名设备：

```js
case 'list_input_devices':
  return [{ name: 'Mock Mic', isDefault: true }, { name: '', isDefault: false }]
```

断言：

```js
check('空设备名有回退标签', (await page.locator('.select-popup').innerText()).includes('未命名设备'))
```

- [x] **Step 2: 运行确认失败**

Run: `npm run check:ui`
Expected: FAIL —— 空名设备项没有「未命名设备」。

- [x] **Step 3: 实现**

- 两个字典文件加 `'record.deviceUnnamed': '未命名设备'` / `'record.deviceUnnamed': 'Unnamed device'`。
- `src/status.js` 的 `deviceItems()`：

```js
label: device.isDefault ? `${device.name || t('record.deviceUnnamed')} · ${t('record.deviceSystemDefault')}` : (device.name || t('record.deviceUnnamed')),
```

- [x] **Step 4: 运行确认通过**

Run: `npm run check:i18n` → Expected: key 对齐通过。
Run: `npm run check:ui` → Expected: 全部通过。

- [x] **Step 5: 提交**

```bash
git add src/locales/zh-CN.js src/locales/en.js src/status.js scripts/ui/check.mjs
git commit -m "fix(ui): 设备名为空时回退到未命名设备"
```

---

### Task 6: 补上缺失的回归测试

**Files:**
- Modify: `src-tauri/src/record/tests.rs`（VAD 队列清理）
- Modify: `scripts/ui/check.mjs`（过期文件复位、文件进度重绘）

> 这三处行为**已经是对的**，本任务补的是「锁住行为」的回归测试 —— 预期直接通过；若失败说明行为其实没修好。

- [x] **Step 1: VAD 队列清理测试（Rust）**

`record.rs` 的流式门控里每次判完 VAD 都要 `clear()`，把它抽成可测的小函数：

```rust
/// 判一次 VAD 就清空队列 —— 不清的话静音段会一直攒着，下一段永远切不出来。
fn vad_is_speech(detector: &mut sherpa_onnx::VoiceActivityDetector, samples: &[f32]) -> bool {
    let speaking = detector.is_speech(samples);
    detector.clear();
    speaking
}
```

测试：喂一段静音 → 返回 false；再喂同一段 → 仍然是 false（队列没被攒住）。

- [x] **Step 2: 过期文件任务复位测试（前端）**

`scripts/ui/check.mjs`：让 `transcribe_file` 挂起 → 用户取消/退出 → 断言 `state.fileSessionId` 复位、界面回到 idle。

- [x] **Step 3: 文件进度按当前数据重绘测试（前端）**

`scripts/ui/check.mjs`：发一条 `model:progress` 后，断言进度条文字用的是**最新一条**载荷，而不是第一次那条。

- [x] **Step 4: 运行确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib` 与 `npm run check:ui`
Expected: 全部通过（用例数上升）。

- [x] **Step 5: 提交**

```bash
git add src-tauri/src/record.rs src-tauri/src/record/tests.rs scripts/ui/check.mjs
git commit -m "test: 补 VAD 清理、过期文件复位、文件进度重绘的回归测试"
```

---

### Task 7: 清掉死状态与串位注释

**Files:**
- Modify: `src/core.js`（删 `state.lastTranscript` 字段）
- Modify: `src/app.js`、`src/recorder.js`、`src/files.js`、`src/history.js`、`src/result.js`（删 6 处赋值）
- Modify: `src-tauri/src/audio.rs:136-139`（注释 + 常量）

**Interfaces:**
- 无新接口。`state.lastTranscript` 是**只写不读**的死状态 —— main 上 `app.js` 读它做语言切换重绘，本分支已换成 `relocalizeResult()`，读取方没了。

- [x] **Step 1: 删死状态**

`src/core.js` 删 `lastTranscript: null,`；五个 JS 文件里 6 处 `state.lastTranscript = ...` 全部删掉。

- [x] **Step 2: 修 audio.rs 的串位注释与硬编码常量**

```rust
/// 重采样到 16 kHz 之后有多少采样。
pub fn resampled_len(samples: u32, sample_rate: u32) -> usize {
    (samples as u64 * crate::asr::SAMPLE_RATE as u64).div_ceil(sample_rate as u64) as usize
}
```

（`asr.rs` 已经 `use crate::audio::...`，Rust 允许模块互相引用；这里是为了让 16 kHz 只有一个出处。）

- [x] **Step 3: 运行确认通过**

Run: `npm run check` && `npm run check:ui` → Expected: 全绿。

- [x] **Step 4: 提交**

```bash
git add src/core.js src/app.js src/recorder.js src/files.js src/history.js src/result.js src-tauri/src/audio.rs
git commit -m "refactor: 删掉只写不读的 lastTranscript，修正 resampled_len 注释"
```

---

### Task 8: 文档基线同步

**Files:**
- Modify: `docs/HOW_TO_VERIFY.md`（检查项数、模型依赖说明）

> 纯文档改动，不用跑门禁。

- [x] **Step 1: 更新检查项数**

`npm run check:ui` 的实际通过数（跑完取），替换 `docs/HOW_TO_VERIFY.md:68` 的「78 项检查」。

- [x] **Step 2: 写清模型依赖的测试会静默跳过**

在 `docs/HOW_TO_VERIFY.md` 的 Rust 测试一节加一句：

> `cargo test --lib` 里的真模型用例（流式时间轴、预录、缺口）没有
> `VOICE_NOTES_STREAMING_MODEL_DIR` / `VOICE_NOTES_MODEL_DIR` 时**静默跳过**，
> 退出码仍是 0 —— 门禁跑绿不代表流式时间轴验证过。本机验证要设上环境变量再跑。

- [x] **Step 3: 提交**

```bash
git add docs/HOW_TO_VERIFY.md
git commit -m "docs: 同步界面检查项数，注明真模型用例会静默跳过"
```
