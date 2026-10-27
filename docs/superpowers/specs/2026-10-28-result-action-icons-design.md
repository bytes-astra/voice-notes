# 结果区动作按钮改成纯图标

日期：2026-10-28

## 意图与背景

转写结果卡片右上角的三个动作按钮（复制 / 保存 / 清空）现在是 `.btn small` 文字按钮，
在卡片标题栏里占了三段横向空间（合计约 190px），而这三个操作在别的应用里基本都是纯图标。

项目里已经有一套现成的图标零件：历史记录每行右侧的复制 / 保存 / 删除按钮就是内联 SVG +
`.icon-btn`，侧栏主题按钮也是同一套画法。所以这不是新造轮子，是把结果区对齐到已有样式。

## 目标

- 结果区三个按钮改成纯图标，视觉与历史记录、侧栏主题按钮一致。
- 按钮仍有可访问名称（`aria-label`）和悬停提示（`title`），切界面语言时跟着变。
- 同一段 SVG 只有一份，不在 HTML 和 JS 里各写一遍。

## 非目标

- 不动历史记录页（已经是图标按钮）。
- 不动历史卡片头部的「清空历史」按钮 —— 那是另一个控件，语义不同（清掉整个列表）。
- 不新增 i18n key，不改现有文案。
- 不改 `.icon-btn` / `.btn` 的样式定义，不引入图标库或字体图标。

## 反目标

- 不出现「只有图形、没有可访问名称」的按钮。
- 不让同一段 SVG 在两个文件里各存一份。

## 约束

- 前端是原生 JS + ES Modules，没有打包器、没有框架、没有 TypeScript。
- 依赖方向单向且不成环：`core.js` / `tauri.js` → `theme.js` / `export.js` → `history.js` →
  `result.js` → `status.js` → `recorder.js` → `files.js` / `events.js` → `app.js`。
- 文案统一在 `src/locales/*.js`，机制在 `src/i18n.js`；HTML 用 `data-i18n` / `-title` /
  `-placeholder` / `-aria`。
- `result.copy` / `result.save` / `result.clear` 在 `zh-CN.js` 和 `en.js` 里都已存在。

## 方案

### A. SVG 直接内联进 `index.html`

跟 `src/index.html:51` 的主题按钮先例一致，只动 HTML 一个文件，改动最小。

代价：复制 / 保存 / 垃圾桶这三段 SVG 会在 `index.html` 和 `src/history.js` 里各存一份，
违反项目「单一事实源」的约定。

### B. 抽出 `src/icons.js` 共享模块（选定）

新增零依赖的叶子模块 `src/icons.js`，导出 `copyIcon()` / `saveIcon()` / `trashIcon()`。
`src/history.js` 删掉本地定义、改为 import；结果区三个按钮由 JS 注入同一份 SVG。

代价：多一个模块；HTML 里按钮是空的，图标靠 JS 填（`.icon-btn` 有固定 32×32 尺寸，
不会引起布局跳动）。

### C. 用 `history.js` 的 `iconAction()` 在 JS 里重建按钮

不清空 HTML，直接把 `.card-actions` 的内容换成三个 `iconAction()` 按钮。

代价：HTML 不再是结构的事实源，`els.copyBtn` 这类 DOM 引用要改成动态查询，
偏离项目「DOM 引用集中在 `core.js`」的做法。

**选定 B。** 它同时满足「只写一处」和「HTML 仍是结构的事实源」，代价（多一个叶子模块）
在这个规模下可接受。

## 已定的实现细节

- **图标来源**：新文件 `src/icons.js`，零 import，导出三个返回 SVG 字符串的函数。
  SVG 内容沿用 `src/history.js` 现有写法（`viewBox="0 0 24 24"`、`currentColor` 描边、
  `stroke-width="1.7"`），尺寸统一成 `16×16`（历史记录原来是 15×15）。
- **依赖方向**：`icons.js` 不 import 任何东西，是链底的新叶子；`history.js` 和
  `result.js` 都可以 import 它，不成环、不反向。
- **HTML**：`src/index.html:146-150` 的三个按钮改成
  `<button type="button" class="icon-btn" id="…" data-i18n-title="result.…"
  data-i18n-aria="result.…"></button>`。id 不变（`check:ui` 按 `#clear-btn` 点）。
- **注入**：由 `result.js` 新增 `renderResultActions()` 写三个按钮的 `innerHTML`，
  `app.js` 的 `init()` 里调一次。图标是静态的，切语言不用重绘；
  `title` / `aria-label` 由 `i18n.js` 的 `apply()` 按 `data-i18n-title` / `-aria` 填。
  **SVG 不内联进 HTML** —— 图标只有 `src/icons.js` 一处来源。
- **尺寸**：结果区用 `.icon-btn`（32×32，跟现在的 `.btn small` 30px 接近）；
  历史记录继续用 `.icon-btn sm`（28×28）。
- **主次层级**：复制 / 保存是主操作，保留描边（`.icon-btn.outlined`，新增的 CSS 变体，
  不改 `.icon-btn` 本身）；清空是次要操作，不描边。改之前是「复制/保存有边框、清空是
  ghost」的层级，图标化之后三个不能长得一模一样。
- **「清空」用橡皮擦**（`clearIcon()`），不用垃圾桶 —— 垃圾桶已经表示「删掉这条历史」
  （不可恢复），清空只是抹掉文本框里的字，两个强度不同的操作不该用同一个图形。
- **无障碍**：纯图标按钮必须带 `aria-label`（`data-i18n-aria`）和 `title`
  （`data-i18n-title`），两者共用现有 key，不新增字典条目。
- **`AGENTS.md`**：单向依赖链那一行补上 `icons.js`（零依赖叶子）。

## 验证

- `npm run check:i18n` —— `data-i18n-title` / `-aria` 用的 key 已存在，不应新增报错。
- `npm run check:ui` —— 补断言：三个按钮是图标按钮（内含 `svg`）、有 `aria-label`、
  没有可见文字；复制/保存带 `outlined`、清空不带；清空的图标与历史删除按钮不同。
  已有的「清空后切中文不复原」用例继续按 `#clear-btn` 点，不受影响。
- `npm run shots` —— 改了界面，重出 README 截图并提交更新后的 PNG。

## 交给实现的部分

- 写 `src/icons.js`，搬 `history.js` 的三个图标函数并统一 16×16；再加 `clearIcon()`。
- `history.js` 改为从 `icons.js` import，删掉本地定义。
- `index.html` 三个按钮换成 `.icon-btn` + `data-i18n-title` / `data-i18n-aria`
  （复制/保存再加 `outlined`）。
- `result.js` 加 `renderResultActions()`，`app.js` 的 `init()` 调一次。
- `style.css` 加 `.icon-btn.outlined` 变体。
- `check:ui` 补断言；跑 `check:i18n` / `check:ui` / `shots`。
- `AGENTS.md` 依赖链补一行。
