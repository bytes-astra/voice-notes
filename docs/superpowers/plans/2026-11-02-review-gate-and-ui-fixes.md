# 门禁盲区与界面缺陷修复实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 补掉三个前端门禁「只比结构不比行为」的盲区、让 CI 真正执行不需要模型的集成测试、修掉三处用户可见的前端缺陷，并让流式模型加载失败不再显示假成功。

**Architecture:** 门禁脚本保持纯 Node（无新依赖），新增的校验都先用「变异测试」证明它会红再实现。前端改动遵守单向依赖：`history.js` 不能 import `app.js`，所以用回调注入让 `app.js` 决定导航。Rust 侧只改事件发射与日志，不动识别逻辑。

**Tech Stack:** Node 22（`node:test` / `node:assert`）、原生 JS ES Modules、Rust + Tauri 2、Playwright WebKit（`npm run check:ui`）。

**Spec:** 本计划直接实现 2026-11-02 全库审查（经 3 个独立子代理复核）确认的 7 条问题；复核记录见 Engram 项目记忆 `discovery/gate-blind-spots` 与 `discovery/ui-and-rust-defects`。

## Global Constraints

- 前端**没有打包器、没有框架、没有 TypeScript** —— 不引入新依赖、不加构建步骤。
- 依赖方向不能反、不能成环：`icons.js` → `core.js`/`tauri.js` → `theme.js`/`export.js` → `history.js` → `result.js` → `status.js` → `recorder.js` → `files.js`/`events.js` → `app.js`。
- 字典文案在渲染时取；新增 key 必须同时加进 `src/locales/zh-CN.js` 与 `src/locales/en.js`。
- 后端错误只返回字典 key（`crate::code` / `crate::code_detail`），不返回展示文案。
- 提交用 Conventional Commits，一个逻辑改动一个提交；不主动 push。
- 每个任务先写「去掉修复就会红」的测试，再实现。
- 改完必须跑 `npm run check`；前端改动再跑 `npm run check:ui`。

## Review Focus

1. **录音中编辑结果框** —— 用户期望自己改的字留下来，而不是被下一句识别结果覆盖。
2. **含缺口占位符的历史条目** —— 载入后切语言，占位符仍要跟着界面语言走。
3. **纯键盘操作下拉框** —— 选完不能丢焦点；必须能继续用 Tab/空格，且空格不能变成开始录音。
4. **流式模型下载后引擎加载失败** —— 不能显示「已就绪」的成功提示，徽标不能永远停在「加载中」。
5. **新增语言但忘记在 `index.html` 注册** —— 门禁必须红，而不是静默变成一个永远加载不到的字典。

---

## Task 1: `check:join` 校验返回语句（不只是判定式）

**Files:**
- Modify: `scripts/check-join.mjs`
- Test: `scripts/check-join.test.mjs`

**Interfaces:**
- Consumes: `collectProblems(core, asr, audio)` 现有返回 `{ problems, summary }`。
- Produces: 无新导出；`collectProblems` 在返回分支对调时把问题写进 `problems`。

- [ ] **Step 1: 写会红的测试**

在 `scripts/check-join.test.mjs` 末尾追加两个用例：

```js
test('前端返回语句的分支对调会被抓住', () => {
  const broken = core.replace(
    'return needSpace ? `${left} ${right}` : `${left}${right}`',
    'return needSpace ? `${left}${right}` : `${left} ${right}`',
  )
  assert.notEqual(broken, core, '替换没生效 —— core.js 的返回语句写法变了？')
  const problems = collectProblems(broken, asr, audio).problems
  assert.ok(problems.length > 0, '三元分支对调仍然通过 —— 门禁没抓住')
})

test('后端空间分支对调会被抓住', () => {
  const broken = asr.replace('if need_space {', 'if !need_space {')
  assert.notEqual(broken, asr, '替换没生效 —— asr.rs 的写法变了？')
  const problems = collectProblems(core, broken, audio).problems
  assert.ok(problems.length > 0, '`if need_space` 改成 `if !need_space` 仍然通过 —— 门禁没抓住')
})
```

- [ ] **Step 2: 跑测试确认是红的**

Run: `node --test scripts/check-join.test.mjs`
Expected: 两个新用例 FAIL（`assert.ok(problems.length > 0)` 失败）。

- [ ] **Step 3: 在 `scripts/check-join.mjs` 里加两个解析函数**

放在 `ruleThreeRust()` 后面：

```js
// 规则 3 的**用法**：判定式算出来之后，到底哪一支加空格。
// 只比判定式的话，把三元两个分支对调照样通过 —— 而那正好决定用户看到的文本。
function ruleThreeUsageJs(text) {
  const m = text.match(/return\s+needSpace\s*\?\s*`([^`]*)`\s*:\s*`([^`]*)`/)
  return m ? { spaced: m[1], direct: m[2] } : null
}

function ruleThreeUsageRust(text) {
  if (/if\s+need_space\s*\{\s*out\.push\('\s+'\);/.test(text)) return 'spaced-on-true'
  if (/if\s*!\s*need_space\s*\{\s*out\.push\('\s+'\);/.test(text)) return 'spaced-on-false'
  return null
}
```

- [ ] **Step 4: 在 `collectProblems` 里调用它们**

紧接在现有 `checkGapRule(jsJoin, rustJoin, problems)` 之后加：

```js
  const jsUsage = ruleThreeUsageJs(jsJoin)
  if (!jsUsage) {
    problems.push('前端 joinText 的返回语句解析不出来 —— 写法变了？')
  } else if (jsUsage.spaced !== '${left} ${right}' || jsUsage.direct !== '${left}${right}') {
    problems.push(
      `前端 joinText 的返回分支反了：needSpace 为真时拼成「${jsUsage.spaced}」`,
    )
  }

  const rustUsage = ruleThreeUsageRust(rustJoin)
  if (rustUsage !== 'spaced-on-true') {
    problems.push(`后端 join_segments 的空间分支不对：${rustUsage || '解析不出来'}`)
  }
```

- [ ] **Step 5: 跑测试确认全绿**

Run: `node --test scripts/check-join.test.mjs && node scripts/check-join.mjs`
Expected: 全部 PASS；命令行输出「3 条规则前后端一致」。

- [ ] **Step 6: 变异复核（手动）**

把 `src/core.js` 的三元分支对调，确认 `node scripts/check-join.mjs` 报错（exit 1），然后还原。

- [ ] **Step 7: 提交**

```bash
git add scripts/check-join.mjs scripts/check-join.test.mjs
git commit -m "test(scripts): check-join 校验返回语句而不只是判定式"
```

---

## Task 2: `check:i18n` 校验 locale 注册并放宽两种写法

**Files:**
- Modify: `scripts/check-i18n.mjs`
- Test: `scripts/check-i18n.test.mjs`

**Interfaces:**
- Consumes: `collectProblems(root)`、`makeFixture(files)`（测试内）。
- Produces: 无新导出；新增三类问题写进 `problems`。

- [ ] **Step 1: 先把 fixture 的 index.html 补成「已注册」**

在 `scripts/check-i18n.test.mjs` 的 `makeFixture` 默认值里把：

```js
'src/index.html': '<html></html>',
```

改成：

```js
'src/index.html': '<script src="locales/zh-CN.js"></script><script src="i18n.js"></script>',
```

- [ ] **Step 2: 写三个会红的测试**

追加到 `scripts/check-i18n.test.mjs`：

```js
test('字典文件没在 index.html 注册要报出来', () => {
  const dir = makeFixture({
    'src/locales/fr.js':
      "window.I18N_LOCALES = window.I18N_LOCALES || {}\n" +
      "window.I18N_LOCALES['fr'] = { 'app.title': 'V', 'record.empty': 'X' }\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('fr')),
    `没注册的 fr.js 被放过了：${JSON.stringify(problems)}`,
  )
})

test('双引号写的 t() 也要检查', () => {
  const dir = makeFixture({ 'src/app.js': 'const x = t("record.notInDict")' })
  const { problems } = collectProblems(dir)
  assert.ok(problems.some((problem) => problem.includes('record.notInDict')))
})

test('data-i18n 属性名拼错要报出来', () => {
  const dir = makeFixture({
    'src/index.html':
      '<script src="locales/zh-CN.js"></script><script src="i18n.js"></script>' +
      '<i data-i18n-titel="record.empty"></i>',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('data-i18n-titel')),
    `拼错的属性名被放过了：${JSON.stringify(problems)}`,
  )
})
```

- [ ] **Step 3: 跑测试确认是红的**

Run: `node --test scripts/check-i18n.test.mjs`
Expected: 三个新用例 FAIL。

- [ ] **Step 4: 改 `scripts/check-i18n.mjs` 的扫描**

4a. 把 `index.html` 的读取提前（它现在在 locale 加载之后），并加注册校验：

```js
  // 字典文件必须在 index.html 里注册，且排在 i18n.js 之前 ——
  // 漏了这行，语言文件永远加载不到，切语言时部分文案不跟着变。
  const html = readFileSync(join(root, 'src/index.html'), 'utf8')
  const scriptTags = [...html.matchAll(/<script[^>]+src="([^"]+)"/g)].map((m) => ({
    src: m[1],
    at: m.index,
  }))
  const registered = scriptTags
    .filter((tag) => tag.src.startsWith('locales/') && tag.src.endsWith('.js'))
    .map((tag) => tag.src.slice('locales/'.length, -'.js'.length))
  for (const name of Object.keys(locales)) {
    if (!registered.includes(name)) {
      problems.push(`src/locales/${name}.js 没有在 index.html 注册（切语言会加载不到）`)
    }
  }
  const i18nTag = scriptTags.find((tag) => tag.src === 'i18n.js')
  if (i18nTag) {
    for (const tag of scriptTags) {
      if (tag.src.startsWith('locales/') && tag.at > i18nTag.at) {
        problems.push(`${tag.src} 必须排在 i18n.js 之前，否则 window.I18N_LOCALES 还读不到`)
      }
    }
  }
```

4b. 属性正则改成「匹配任意 `data-i18n*` 并校验后缀」：

```js
  const ATTR_SUFFIXES = new Set(['', '-title', '-placeholder', '-aria'])
  for (const match of html.matchAll(/\bdata-i18n(-[a-z-]+)?="([^"]*)"/g)) {
    const suffix = match[1] || ''
    if (!ATTR_SUFFIXES.has(suffix)) {
      problems.push(`index.html 里的 data-i18n${suffix} 不是合法属性名（界面不会翻译它）`)
      continue
    }
    if (!baseKeys.includes(match[2])) {
      problems.push(`index.html 引用了不存在的 key：${match[2]}`)
    }
  }
```

4c. 三处 `t()` 正则都接受单/双引号：

```js
  const callRe = /(?<![\w$.])t\(\s*(['"])([^'"]*?)\1/g
  const staticTemplateRe = /(?<![\w$.])t\(\s*`([^`$]*)`/g
  const callWithParamsRe = /(?<![\w$.])t\(\s*(['"])([^'"]*?)\1\s*,\s*\{([^}]*)\}/g
```

匹配结果从 `match[1]` 变成 `match[2]`（第 3 个正则的参数体从 `match[2]` 变成 `match[3]`）—— 同步改三处取用。

- [ ] **Step 5: 跑测试与门禁确认全绿**

Run: `node --test scripts/check-i18n.test.mjs && node scripts/check-i18n.mjs`
Expected: 全部 PASS，输出「2 个语言，共 224 个 key」。

- [ ] **Step 6: 变异复核（手动）**

删掉 `src/index.html` 里 `<script src="locales/en.js">` 那一行，确认门禁红；还原。把某个 `t('...')` 改成双引号拼错 key，确认门禁红；还原。

- [ ] **Step 7: 提交**

```bash
git add scripts/check-i18n.mjs scripts/check-i18n.test.mjs
git commit -m "test(scripts): check-i18n 校验 locale 注册与双引号/属性名写法"
```

---

## Task 3: CI 执行不需要模型的集成测试

**Files:**
- Modify: `.github/workflows/check.yml`

**Interfaces:**
- Consumes: `src-tauri/tests/decode_smoke.rs`（4 个不需要模型的用例）、`src-tauri/tests/asr_smoke.rs::joins_sensevoice_segments`。
- Produces: 无。

- [ ] **Step 1: 先本地确认这些用例真的不需要模型且通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test decode_smoke --test asr_smoke`
Expected: PASS（需要模型的用例打印「跳过：…」后 return，不算失败）。

- [ ] **Step 2: 在 `.github/workflows/check.yml` 的「单元测试」之后加一步**

```yaml
      # 不需要模型的集成回归：decode_smoke 的解码用例 + 拼句纯逻辑用例。
      # 需要模型的用例在没设 *_MODEL_DIR 时会自己跳过。
      - name: 集成回归（不需要模型）
        run: cargo test --manifest-path src-tauri/Cargo.toml --test decode_smoke --test asr_smoke
```

- [ ] **Step 3: 校验 YAML 缩进**

Run: `node -e "const fs=require('fs');const s=fs.readFileSync('.github/workflows/check.yml','utf8');if(!s.includes('--test decode_smoke'))process.exit(1)"`
Expected: exit 0。

- [ ] **Step 4: 提交**

```bash
git add .github/workflows/check.yml
git commit -m "ci: 跑不需要模型的集成测试"
```

---

## Task 4: 历史条目载入后切回转写页

**Files:**
- Modify: `src/history.js`
- Modify: `src/app.js:1-20`（import）、`src/app.js` 的 `bindUI()` 内
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- Produces: `history.js` 导出 `setHistoryLoadHandler(fn: () => void): void`；`app.js` 在 `bindUI()` 里注册 `() => switchView('transcribe')`。

- [ ] **Step 1: 写会红的界面测试**

在 `scripts/ui/check.mjs` 的「历史里的缺口占位符跟着语言变」那条之后追加：

```js
// 历史页点条目：必须切回转写页，否则文字写进 display:none 的面板里，用户看不到。
await page.locator('.history-item .history-main').first().click()
await page.waitForTimeout(200)
check('历史载入切回转写页', (await page.locator('.app').getAttribute('data-view')) === 'transcribe')
check('历史载入内容可见', (await page.locator('#text').inputValue()).length > 0)

// 载入的历史要带上 markers：用户改过之后切语言，缺口占位符仍要换。
await page.locator('#text').fill((await page.locator('#text').inputValue()) + '（手改）')
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(150)
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: '简体中文' }).first().click()
await page.waitForTimeout(250)
check(
  '历史载入后切语言换掉缺口占位符',
  (await page.locator('#text').inputValue()).includes('（约 4 秒未识别）'),
)
check('历史载入后切语言保留用户编辑', (await page.locator('#text').inputValue()).includes('（手改）'))
```

- [ ] **Step 2: 跑界面检查确认是红的**

Run: `npm run check:ui`
Expected: 「历史载入切回转写页」FAIL。

- [ ] **Step 3: 在 `src/history.js` 里加回调注入**

在 `import` 之后、`saveHistory()` 之前：

```js
/// 载入历史后要切回结果页 —— 但 history.js 不能 import app.js（依赖方向）。
/// 由 app.js 在绑定界面时注入。
let loadHandler = null
export function setHistoryLoadHandler(fn) {
  loadHandler = fn
}
```

把 click 回调改成（注意先切视图再写文本，否则 `focus()` 对隐藏元素不生效）：

```js
    main.addEventListener('click', () => {
      // 载入的是一条历史，不是「上次转写」：切语言时不能再把它重绘回去。
      const rendered = localizeText(item.text)
      loadHandler?.()
      els.text.value = rendered
      // markers 交给 relocalizeResult 用：用户改过之后切语言，
      // 靠它把旧语言的缺口占位符换成新语言的。
      state.resultView = {
        rawText: item.text,
        rendered,
        markers: gapMarkerTexts(item.text),
        meta: { kind: 'label', key: 'history.from', at: item.at },
      }
      renderResultMeta()
      els.text.focus()
    })
```

同时把文件顶部的 import 改成：

```js
import {
  I18N, els, fmtDuration, gapMarkerTexts, historyStorageSet, localizeText,
  renderResultMeta, state, t,
} from './core.js'
```

- [ ] **Step 4: 在 `src/app.js` 里注册**

import 行改成：

```js
import { renderHistory, saveHistory, setHistoryLoadHandler } from './history.js'
```

在 `bindUI()` 里、`for (const item of els.navItems)` 之前加：

```js
  setHistoryLoadHandler(() => switchView('transcribe'))
```

- [ ] **Step 5: 跑界面检查确认全绿**

Run: `npm run check:ui`
Expected: 全 PASS，且「历史里的缺口占位符跟着语言变」仍然 PASS。

- [ ] **Step 6: 提交**

```bash
git add src/history.js src/app.js scripts/ui/check.mjs
git commit -m "fix(history): 载入历史后切回结果页"
```

---

## Task 5: 实时追加不覆盖用户的编辑

**Files:**
- Modify: `src/result.js:90-97`（`appendSegment`）
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- Consumes: `state.resultView`（`rawText` / `rendered` / `markers`）、`joinText`、`gapMarkerTexts`、`renderResultMeta`。
- Produces: `appendSegment(text)` 行为不变，仅在用户编辑过时改为追加到可见文本。

**规则：** 用户没动过（`els.text.value === view.rendered`）→ 照旧整段重画；用户动过 → 把新句接到**用户文本**后面，`rawText` 仍照常累加（最终结果以它为准）。

- [ ] **Step 1: 写会红的界面测试**

插在「说话→静音→再说话：三句都在」这条 check **之后**、「停止录音」那次点击**之前**（此时仍在录音中）：

```js
// 录音中手改的内容，不能被下一句识别结果覆盖。
await page.locator('#text').fill('我手改的一行')
await page.evaluate(() => {
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '下一句', start: 0, end: 1 })
})
await page.waitForTimeout(120)
check('实时追加不覆盖用户编辑', (await page.locator('#text').inputValue()).startsWith('我手改的一行'))
check('实时追加接在用户编辑之后', (await page.locator('#text').inputValue()).includes('下一句'))
```

> 后面的「停止录音」会用后端全文重画结果框 —— 那是预期行为（最终结果以它为准），上面的断言只守「录音过程中不覆盖」。

- [ ] **Step 2: 跑界面检查确认是红的**

Run: `npm run check:ui`
Expected: 「实时追加不覆盖用户编辑」FAIL。

- [ ] **Step 3: 改 `src/result.js` 的 `appendSegment`**

```js
export function appendSegment(text) {
  // 取消 / 收尾结束之后迟到的句子不再追加，否则结果框会冒出被取消的内容。
  if (state.phase !== 'recording' && state.phase !== 'transcribing') return
  const piece = (text || '').trim()
  if (!piece) return
  const view = state.resultView
  const rawText = joinText(view?.rawText || els.text.value, piece)
  // 用户动过结果框：把新句接到他们改过的文本后面，别整段重画覆盖掉。
  // `rendered` 仍保持「原文的本地化结果」—— relocalizeResult 靠它判断
  // 文本有没有被编辑过，写成用户文本会让切语言时再次覆盖。
  if (view && els.text.value !== view.rendered) {
    els.text.value = joinText(els.text.value, piece)
    state.resultView = { ...view, rawText, markers: gapMarkerTexts(rawText) }
    renderResultMeta()
    return
  }
  setResultView(rawText, { kind: 'label', key: 'record.live' })
}
```

- [ ] **Step 4: 跑界面检查确认全绿**

Run: `npm run check:ui`
Expected: 全 PASS，且「切语言不覆盖用户编辑」仍然 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/result.js scripts/ui/check.mjs
git commit -m "fix(result): 实时追加不覆盖用户编辑"
```

---

## Task 6: 下拉框选中后把焦点还给按钮

**Files:**
- Modify: `src/select.js:104-109`
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- Produces: 无导出变化；选中条目后 `document.activeElement` 回到 `.select-button`。

- [ ] **Step 1: 写会红的界面测试**

在 `scripts/ui/check.mjs` 的设备下拉用例（「下拉 Esc 关闭弹层」附近）追加：

```js
// 选中后焦点必须回到按钮：掉到 body 的话，紧接着按空格会触发全局的「开始录音」。
await page.locator('#device-select .select-button').click()
await page.locator('.select-popup .select-item').first().click()
await page.waitForTimeout(120)
check(
  '选中后焦点回到下拉按钮',
  await page.evaluate(() => document.activeElement?.closest('#device-select') != null),
)
```

- [ ] **Step 2: 跑界面检查确认是红的**

Run: `npm run check:ui`
Expected: 「选中后焦点回到下拉按钮」FAIL。

- [ ] **Step 3: 改 `src/select.js` 的条目 click 回调**

```js
          row.addEventListener('click', () => {
            value = item.value
            render()
            close()
            // 焦点还给按钮：条目随弹层一起被移除，不还焦就会掉到 body，
            // 全局 keydown 把下一次空格当成「开始录音」。
            button.focus()
            if (opts.onChange) opts.onChange(value)
          })
```

- [ ] **Step 4: 跑界面检查确认全绿**

Run: `npm run check:ui`
Expected: 全 PASS，尤其是「下拉 Esc 不取消录音」仍然 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/select.js scripts/ui/check.mjs
git commit -m "fix(select): 选中后把焦点还给按钮"
```

---

## Task 7: 流式引擎加载失败不能再假成功

**Files:**
- Modify: `src-tauri/src/engine.rs:282-325`（`ensure_streaming_engine`）
- Modify: `src-tauri/src/lib.rs:492`
- Modify: `src/status.js:277-282`、`src/status.js:312-315`
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- Consumes: `EngineState { phase, message }`、`streaming:state` 事件（`src/events.js:95-108` 已处理 `error`）。
- Produces: `ensure_streaming_engine` 在**加载失败**时 emit `streaming:state { phase: "error", message }`；下载命令在预热失败时写日志。

**边界：** 模型没下载（`model.needStreaming`）**不算加载失败**，不能发 `error` —— 那种情况界面该显示「未下载」。

- [ ] **Step 1: 扩展 mock 并写会红的界面测试**

`scripts/ui/check.mjs` 的 mock 里，`download_model` / `download_streaming_model` 目前**永远挂起**（用来保持 busy 态）。加一个开关让它在需要时正常返回，并补 `warm_up_streaming` 的失败分支。把这一组 case：

```js
          case 'set_model_dir':
          case 'set_output_dir':
          case 'download_model':
          case 'download_streaming_model':
            // 挂起：模拟「正在下载」，这样界面上的 busy 状态能保持住
            return new Promise(() => {})
```

改成：

```js
          case 'set_model_dir':
          case 'set_output_dir':
            return clone(window.__mockStatus)
          case 'download_model':
          case 'download_streaming_model':
            // 默认挂起：模拟「正在下载」，busy 状态能保持住。
            // __mockDownloadOk 打开时正常返回，用来测「下载完成后预热失败」。
            if (!window.__mockDownloadOk) return new Promise(() => {})
            return clone(window.__mockStatus)
          case 'warm_up_streaming':
            if (window.__mockWarmError) throw window.__mockWarmError
            return null
```

> `set_model_dir` / `set_output_dir` 原来和下载共用「挂起」，改后单独返回状态 —— 否则设置页的路径切换用例会改变行为；执行时先跑一遍 `npm run check:ui` 确认原有断言不受影响。

然后在文件末尾「── 结果 ──」之前加：

```js
// 流式模型下载完、预热失败：不能弹「已就绪」。
await page.evaluate(() => {
  window.__mockDownloadOk = true
  window.__mockWarmError = 'model.streamLoadFailed'
})
await page.evaluate(async () => {
  const { downloadStreamingModel } = await import('./status.js')
  await downloadStreamingModel()
})
await page.waitForTimeout(200)
const warmToast = await page.locator('#toast').innerText()
check('预热失败不弹「已就绪」', !warmToast.includes('已就绪'), warmToast)
check('预热失败给出错误提示', warmToast.length > 0, warmToast)
await page.evaluate(() => {
  window.__mockDownloadOk = false
  window.__mockWarmError = null
})
```

- [ ] **Step 2: 跑界面检查确认是红的**

Run: `npm run check:ui`
Expected: 「预热失败不弹「已就绪」」FAIL（当前 `toast` 在 `warm()` 之前无条件弹）。

- [ ] **Step 3: 改 `src-tauri/src/engine.rs` 的 `ensure_streaming_engine`**

把「解析路径」之后的加载段包进一个内部 async 块，只在**加载**失败时发 `error`：

```rust
    // 模型没下载（上面的 resolve 失败）不算「加载失败」—— 界面该显示「未下载」。
    let result = async {
        let engine =
            tauri::async_runtime::spawn_blocking(move || -> Result<Arc<StreamingEngine>, String> {
                let mut guard = slot.lock().map_err(|_| code("error.streamEngineState"))?;
                if let Some(engine) = guard.as_ref() {
                    return Ok(engine.clone());
                }
                let engine = Arc::new(StreamingEngine::new(&paths, &hotwords)?);
                log::info(format!(
                    "流式引擎加载完成：热词 {} 条，模型 {}",
                    hotwords.len(),
                    paths.dir
                ));
                *guard = Some(engine.clone());
                Ok(engine)
            })
            .await
            .map_err(|e| code_detail("model.streamLoadFailed", e))??;
        Ok::<_, String>(engine)
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "streaming:state",
                EngineState {
                    phase: "ready",
                    message: None,
                },
            );
        }
        Err(err) => {
            // 没有这个事件，前端 state.engine 会永远停在 loading（徽标卡「加载中」）。
            let _ = app.emit(
                "streaming:state",
                EngineState {
                    phase: "error",
                    message: Some(err.clone()),
                },
            );
        }
    }
    result
```

- [ ] **Step 4: 改 `src-tauri/src/lib.rs:492` 的吞错**

```rust
    // 加载失败不能静默：模型已经下好了，用户需要知道「下载成功但加载失败」。
    if let Err(err) = ensure_streaming_engine(&app, &state).await {
        log::error(format!("流式模型下载完成，但引擎加载失败：{err}"));
    }
```

- [ ] **Step 5: 改 `src/status.js` 的成功提示时机**

`downloadStreamingModel` 的 `warm` 改成返回 Promise：

```js
    warm: async () => {
      await invoke('warm_up_streaming')
    },
```

`runDownload` 里把「弹成功 + 预热」改成「预热成功才弹成功」。注意**单独一层 try**：加载失败不是「下载失败」，文案不能混：

```js
    progressText.textContent = t('setup.done')
    renderStatus()
    // 和整段模型一样：下载完先把引擎加载起来，第一次录音不用等。
    // 预热失败就别说「已就绪」—— 那会让用户以为可以直接录音。
    try {
      await warm()
      toast(t(readyToast))
    } catch (error) {
      logEvent('ERROR', `模型预热失败: ${errorText(error)}`)
      toast(fmtError(error), 'error')
    }
```

（`logEvent` / `errorText` 已在 `status.js` 顶部 import，无需新增。）

- [ ] **Step 6: 跑门禁与界面检查**

Run: `npm run check:rust && npm run check:ui`
Expected: clippy 零警告、单测通过；界面检查全 PASS。

- [ ] **Step 7: 提交**

```bash
git add src-tauri/src/engine.rs src-tauri/src/lib.rs src/status.js scripts/ui/check.mjs
git commit -m "fix(engine): 流式引擎加载失败要报错，不能假成功"
```

---

## 已知限制（本计划不修）

**实时与最终文本的缺口顺序**：缺口在丢弃当刻就 emit（`record.rs:273` / `:415`），此时当前句还没定稿 → 实时流是「[[gap]]句子」，最终文本按 `start` 排序是「句子[[gap]]」。

- 这是 `Timeline::advance_dropped`「洞在句内起点不动」的**有意设计**（`asr/timeline_tests.rs:41`），不是 bug。
- 两者只在「停止超时、前端拿实时文本兜底」时才会同时出现。
- 要统一需要延迟 emit 缺口，改动横跨两条录音路径、风险高于收益，**留到有真实用户反馈再评估**。

---

## 执行顺序与提交粒度

按 Task 1 → 7 顺序执行，每个任务一个提交。Task 1/2 是纯脚本改动、互不依赖；Task 4/5/6 都动 `scripts/ui/check.mjs`，按顺序做避免冲突；Task 7 依赖 `check:ui` 的 mock 扩展，放在最后。
