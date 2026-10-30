# 结果区动作按钮改成纯图标 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **复选框已回填（2026-10-30）**：三个 Task 都已实现，之前的复选框只是没回填。
> 实现形态是下面「后续调整」那一版（`clearIcon()` 橡皮擦 + 复制/保存描边），
> 计划正文里「三段 SVG」「无描边」的描述是执行前的原样。

**Goal:** 转写结果卡片右上角的「复制 / 保存 / 清空」三个按钮从文字按钮改成纯图标按钮，视觉与历史记录的行内按钮、侧栏主题按钮一致。

**Architecture:** 新增零依赖的叶子模块 `src/icons.js`，持有三段内联 SVG 字符串，作为图标唯一来源；`src/history.js` 改为 import 它，转写结果卡片右上角的三个按钮由 `src/result.js` 的 `renderResultActions()` 注入同一份 SVG。按钮文字改用 `data-i18n-title` + `data-i18n-aria`（复用现有 key），不新增字典条目。

**Tech Stack:** 原生 JS + ES Modules（无打包器、无框架、无 TypeScript）、HTML、CSS、Playwright WebKit（`scripts/ui/check.mjs` 做界面回归）

**Spec:** `docs/superpowers/specs/2026-10-28-result-action-icons-design.md`

> **后续调整（2026-10-28，整分支评审之后）**：评审提的 3 个 Minor 采纳了 2 个 ——
> 「清空」改用橡皮擦图标 `clearIcon()`（垃圾桶留给「删除历史条目」）、
> 复制/保存加 `.icon-btn.outlined` 描边恢复主次层级。Minor 3（SVG 内联进 HTML）
> 不采纳 —— 用户明确要求**不要内连**，图标只留在 `src/icons.js` 一处。
> 下面的计划正文保留执行时的原样，最终形态以 spec 为准。

## Global Constraints

- 前端是原生 JS + ES Modules：没有打包器、没有框架、没有 TypeScript，**不要引入构建步骤、图标库或字体图标**。
- 依赖单向、不成环。`src/icons.js` 不 import 任何东西，是链底的新叶子。
- 文案只放 `src/locales/*.js`，机制只放 `src/i18n.js`；HTML 侧用 `data-i18n` / `data-i18n-title` / `data-i18n-aria`。
- **不新增 i18n key**：`result.copy` / `result.save` / `result.clear` 在 `src/locales/zh-CN.js` 和 `src/locales/en.js` 里都已存在。
- 按钮 id 不变：`copy-btn` / `save-btn` / `clear-btn`（`src/core.js` 的 `els` 引用和 `scripts/ui/check.mjs` 里按 `#clear-btn` 的点击都靠它）。
- 图标统一规格：`viewBox="0 0 24 24"`、`width="16" height="16"`、描边 `currentColor`、`stroke-width="1.7"`，并带 `aria-hidden="true"`（按钮的可访问名称由按钮自己的 `aria-label` 提供）。
- 结果区按钮用 `.icon-btn`（32×32）；历史记录继续用 `.icon-btn sm`（28×28）。不要改 `.icon-btn` / `.btn` 的样式定义。
- 提交用 Conventional Commits，且后端 / 前端 / 文档分开提交。

## Review Focus

1. 纯图标按钮**没有可访问名称**（`aria-label` 缺失、或切界面语言后没更新）—— 屏幕阅读器只会念「按钮」。
2. `title` 悬停提示**没跟着切语言** —— `I18N.apply()` 只在 `I18N.setLocale()` 里跑一次，`data-i18n-title` 必须真的挂上。
3. 切到设置页时转写结果面板是 `display: none`（`.hidden`），**隐藏面板里的按钮 `aria-label` 也要更新** —— DOM 属性不随可见性变化。
4. 同一段 SVG **在 `src/index.html` 和 `src/icons.js` 各存一份** —— 改图标时漏改一处，两处就不一致。
5. `scripts/ui/check.mjs` 里「清空后切中文不复原」那条用例是**按 `#clear-btn` 点**的 —— 纯图标按钮必须有非空 bounding box，否则 Playwright 点不中。

---

### Task 1: 抽出 `src/icons.js`，历史记录改用共享图标

**Files:**
- Create: `src/icons.js`
- Modify: `src/history.js`（第 3-4 行的 import 区、第 94-104 行的三个图标函数）
- Modify: `AGENTS.md`（「单向依赖」那一行）
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- Consumes: 无（`src/icons.js` 零依赖）
- Produces: `src/icons.js` 导出 `copyIcon(): string` / `saveIcon(): string` / `trashIcon(): string`，每个返回一段完整的 `<svg …>…</svg>` 字符串（`viewBox="0 0 24 24"`、`width="16" height="16"`、`aria-hidden="true"`、`stroke="currentColor"`、`stroke-width="1.7"`）。

- [x] **Step 1: 加一条特征测试（重构保护 —— 这一步就该通过）**

在 `scripts/ui/check.mjs` 里 `check('历史写了一条', ...)` 那行之后、`await page.screenshot({ path: `${SHOTS}/02-zh-result.png` })` 之前插入：

```js
check(
  '历史行内三个图标按钮都在',
  await page.evaluate(() => {
    const actions = document.querySelector('.history-item .history-actions')
    if (!actions) return false
    const buttons = [...actions.querySelectorAll('button.icon-btn')]
    return buttons.length === 3 && buttons.every((b) => b.querySelector('svg') && b.getAttribute('aria-label'))
  }),
  await page.evaluate(() => document.querySelector('.history-item .history-actions')?.innerHTML.slice(0, 120) || '(没有历史行)'),
)
```

- [x] **Step 2: 跑测试确认基线是绿的**

Run: `npm run check:ui`
Expected: `历史行内三个图标按钮都在` PASS，整体 `79/79 通过`（当前 78 条 + 新增 1 条）。

- [x] **Step 3: 新建 `src/icons.js`**

内容（三个函数原样搬 `src/history.js` 里的版本，只把 `width`/`height` 从 15 改成 16，并加 `aria-hidden="true"`）：

```js
/* 图标：内联 SVG 字符串。
 *
 * 这里是图标的唯一来源：历史记录的行内按钮和转写结果卡片右上角的按钮
 * 都从这里取 —— 同一段 SVG 不在 HTML 和 JS 里各写一份。
 *
 * 约定：viewBox 一律 "0 0 24 24"，尺寸 16×16，描边用 currentColor
 * （跟着主题走）。图形本身 aria-hidden —— 按钮的可访问名称由按钮自己的
 * aria-label 提供。
 */

/// 复制：两张叠在一起的纸。
export function copyIcon() {
  return '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><rect x="9" y="9" width="11" height="11" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M15 5.5A2.5 2.5 0 0 0 12.5 3H6.5A2.5 2.5 0 0 0 4 5.5v6A2.5 2.5 0 0 0 6.5 14" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>'
}

/// 保存：向下箭头落进托盘。
export function saveIcon() {
  return '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M12 4v10m0 0 3.5-3.5M12 14l-3.5-3.5M5 16v3h14v-3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}

/// 删除：垃圾桶。
export function trashIcon() {
  return '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M5 7h14M10 7V5h4v2m-7 0 .8 12h8.4L17 7" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}
```

- [x] **Step 4: `src/history.js` 改为 import**

在 `import { copyText, saveText } from './export.js'` 之后加一行：

```js
import { copyIcon, saveIcon, trashIcon } from './icons.js'
```

然后删掉 `src/history.js` 末尾的 `copyIcon()` / `saveIcon()` / `trashIcon()` 三个函数定义（第 94-104 行）。文件里其它东西一行不动。

- [x] **Step 5: 跑测试确认重构没破坏历史记录**

Run: `npm run check:i18n && npm run check:ui`
Expected: i18n 检查通过；`79/79 通过`，`历史行内三个图标按钮都在` 仍 PASS。

- [x] **Step 6: 提交**

```bash
git add src/icons.js src/history.js scripts/ui/check.mjs
git commit -m "refactor(icons): 抽出共享图标模块"
```

- [x] **Step 7: `AGENTS.md` 登记新模块**

把「核心架构与原则」里「单向依赖」那一行改成（`icons.js` 补在链首）：

```
- **单向依赖**：`icons.js`（零依赖叶子）→ `core.js` / `tauri.js` → `theme.js` / `export.js` → `history.js` → `result.js` → `status.js` → `recorder.js` → `files.js` / `events.js` → `app.js`。不能反，也不能成环。
```

```bash
git add AGENTS.md
git commit -m "docs(agents): 依赖链补上 icons.js"
```

---

### Task 2: 转写结果卡片的三个动作按钮改成纯图标

**Files:**
- Modify: `src/index.html`（第 146-150 行 `.card-actions` 里的三个按钮）
- Modify: `src/result.js`（加 import + 加 `renderResultActions()`）
- Modify: `src/app.js`（第 9 行的 import、第 360 行附近加调用）
- Test: `scripts/ui/check.mjs`

**Interfaces:**
- Consumes: `copyIcon(): string` / `saveIcon(): string` / `trashIcon(): string`（Task 1 的 `src/icons.js`）
- Produces: `src/result.js` 导出 `renderResultActions(): void` —— 把三个图标写进 `els.copyBtn` / `els.saveBtn` / `els.clearBtn` 的 `innerHTML`。

- [x] **Step 1: 写失败测试（中文首屏）**

在 `scripts/ui/check.mjs` 第 165 行 `check('无未翻译 key', ...)` 之后、`await page.screenshot({ path: `${SHOTS}/01-zh-light.png` })` 之前插入：

```js
check(
  '结果区按钮是纯图标且有可访问名称',
  await page.evaluate(() => {
    const want = { 'copy-btn': '复制', 'save-btn': '保存', 'clear-btn': '清空' }
    return Object.entries(want).every(([id, label]) => {
      const btn = document.getElementById(id)
      if (!btn) return false
      return (
        Boolean(btn.querySelector('svg')) &&
        btn.getAttribute('aria-label') === label &&
        btn.title === label &&
        !btn.textContent.trim()
      )
    })
  }),
  await page.evaluate(() => document.getElementById('copy-btn')?.outerHTML.slice(0, 140) || '(没有 copy-btn)'),
)
```

- [x] **Step 2: 跑测试确认它失败**

Run: `npm run check:ui`
Expected: `结果区按钮是纯图标且有可访问名称` **FAIL**（现在的按钮是文字按钮，没有 `svg`、`aria-label` 也不是「复制」），其余 79 条仍 PASS。

- [x] **Step 3: 改 `src/index.html`**

把第 146-150 行的 `.card-actions` 整块替换成：

```html
              <div class="card-actions">
                <button type="button" class="icon-btn" id="copy-btn" data-i18n-title="result.copy" data-i18n-aria="result.copy"></button>
                <button type="button" class="icon-btn" id="save-btn" data-i18n-title="result.save" data-i18n-aria="result.save"></button>
                <button type="button" class="icon-btn" id="clear-btn" data-i18n-title="result.clear" data-i18n-aria="result.clear"></button>
              </div>
```

图标本体不写在这里 —— 由 `renderResultActions()` 注入（见 Review Focus 第 4 条）。

- [x] **Step 4: `src/result.js` 加 `renderResultActions()`**

在 `import { addHistory } from './history.js'` 之后加：

```js
import { copyIcon, saveIcon, trashIcon } from './icons.js'
```

在 `clearResult` 之前加：

```js
/// 结果卡片右上角的三个动作按钮：纯图标，文字放在 title / aria-label 里。
/// 图标是静态的，切语言不用重绘 —— title / aria-label 由 i18n.apply() 填。
export function renderResultActions() {
  els.copyBtn.innerHTML = copyIcon()
  els.saveBtn.innerHTML = saveIcon()
  els.clearBtn.innerHTML = trashIcon()
}
```

- [x] **Step 5: `src/app.js` 调用它**

第 9 行改成：

```js
import { clearResult, renderResultActions, showTranscript } from './result.js'
```

在 `init()` 里 `renderHistory()` 之后、`bindUI()` 之前加一行：

```js
  renderResultActions()
```

（`init()` 里 `applyLocale()` 排在更前面，所以 `title` / `aria-label` 会在按钮有图标之前就填好。）

- [x] **Step 6: 跑测试确认通过**

Run: `npm run check:i18n && npm run check:ui`
Expected: i18n 检查通过；`80/80 通过`，新断言 PASS，「清空后切中文不复原」仍 PASS。

- [x] **Step 7: 加英文界面的断言**

切语言不能漏 —— 在 `scripts/ui/check.mjs` 第 230 行 `check('切英文 meta 跟随', ...)` 之后插入：

```js
check(
  '切英文后结果区按钮跟随',
  await page.evaluate(() => {
    const want = { 'copy-btn': 'Copy', 'save-btn': 'Save', 'clear-btn': 'Clear' }
    return Object.entries(want).every(([id, label]) => {
      const btn = document.getElementById(id)
      return Boolean(btn) && btn.getAttribute('aria-label') === label && btn.title === label
    })
  }),
  await page.evaluate(() => document.getElementById('copy-btn')?.getAttribute('aria-label') || '(没有 copy-btn)'),
)
```

这一步跑的时候界面停在设置页，转写结果面板是 `display: none` —— 断言查的是 DOM 属性，不受可见性影响（Review Focus 第 3 条）。

- [x] **Step 8: 跑测试确认通过**

Run: `npm run check:ui`
Expected: `81/81 通过`，`切英文后结果区按钮跟随` PASS。

- [x] **Step 9: 提交**

```bash
git add src/index.html src/result.js src/app.js scripts/ui/check.mjs
git commit -m "feat(ui): 结果区动作按钮改成图标"
```

---

### Task 3: 重出 README 界面截图

**Files:**
- Modify: `docs/screenshots/*.png`（脚本生成，不手改）

**Interfaces:**
- Consumes: Task 2 的界面改动
- Produces: 更新后的 `docs/screenshots/` 九张 PNG

- [x] **Step 1: 跑截图脚本**

Run: `npm run shots`
Expected: 输出九行「写出 …/docs/screenshots/<name>.png」，无报错。

- [x] **Step 2: 确认结果卡片变成了图标按钮**

Run: `git status --short docs/screenshots/`
Expected: 至少 `docs/screenshots/ready.png` 等文件被修改（纯图标按钮出现在结果卡片右上角）。

- [x] **Step 3: 人工看一眼截图**

打开 `docs/screenshots/ready.png`：转写结果卡片右上角应该是三个纯图标按钮（复制 / 保存 / 清空），不再有文字。

- [x] **Step 4: 提交**

```bash
git add docs/screenshots
git commit -m "docs: 重出界面截图"
```

---

## 自检记录

- **规格覆盖**：spec 的「已定的实现细节」逐条落到 Task 1（图标来源、依赖方向、尺寸、`清空` 用垃圾桶、`aria-hidden`）和 Task 2（HTML 形态、注入时机、无障碍）。spec 的「验证」三条对应 Task 2 Step 6/8（`check:i18n` + `check:ui`）和 Task 3（`shots`）。spec 的「AGENTS.md 依赖链补一行」是 Task 1 Step 7。spec 的非目标（不动历史记录页、不动「清空历史」按钮、不新增 key、不改 `.icon-btn` 样式定义）已写进 Global Constraints。
- **步骤粒度**：每步都能照着写出一份唯一合理的实现；SVG 字符串和代码块是 spec 已经定死的内容，其余步骤只给签名和断言。
- **命名一致**：`copyIcon` / `saveIcon` / `trashIcon` / `renderResultActions` 在 Task 1 的 Interfaces、Task 2 的 Interfaces 和代码里完全一致。
- **Review Focus 覆盖**：第 1、2、3 条由 Task 2 Step 1 和 Step 7 的断言守住；第 4 条由 Global Constraints 和 Task 2 Step 3 的说明守住；第 5 条由 Task 2 Step 6 里既有的「清空后切中文不复原」用例守住。
