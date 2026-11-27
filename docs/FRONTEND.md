# 前端约定（VoiceNotes）

前端是**原生 JS + ES Modules**，没有打包器、没有框架、没有 TypeScript。模块边界、i18n 与文案口径、错误码、控件、macOS 原生层 —— 改前端前先读这里。

工程约定的总索引在 [`../AGENTS.md`](../AGENTS.md)；验证手段与基线在 [`HOW_TO_VERIFY.md`](HOW_TO_VERIFY.md)。

## 前端模块边界

- `src/app.js`：应用入口 —— init / bindUI / switchView / applyLocale。
- `src/core.js`：全局状态、DOM 引用、格式化、错误码翻译 —— 其他模块都从它取。
- **不要为 UI 引入构建步骤**（前端没有打包器、没有框架、没有 TypeScript）。
- **单向依赖**：`icons.js`（零依赖叶子）→ `core.js` / `tauri.js` → `theme.js` / `export.js` → `history.js` → `result.js` → `status.js` → `recorder.js` → `files.js` / `events.js` → `app.js`。不能反，也不能成环：`core.js` / `tauri.js` 不 import 任何业务模块；`status.js` 不 import `recorder.js`（所以 `idleHint()` 放 core）。
- **`state` / `els` 只在 `core.js` 创建一次**，其他模块 `import` 引用（live binding 是同一个对象）。
- **Tauri 插件调用面集中在 `src/tauri.js` 解构**，其他模块从那里 import。新增插件命令要同步在 `src-tauri/capabilities/default.json` 加权限，否则运行时直接拒绝。
- **外壳是「侧边栏 + 主区」**：`.app[data-view]` 控制 `[data-view-panel]` 显隐，`switchView()` 统一处理导航高亮/标题，录音中不允许切走。新增页面 = 一个 `[data-view-panel]` + 一个 `.nav-item`。
- **默认窗口尺寸要容得下整张转写卡片**：窗口默认宽高写在 `src-tauri/tauri.conf.json`，它是唯一的尺寸来源 —— `scripts/ui/check.mjs` 和 `scripts/ui/shots.mjs` 都从那里读 viewport（界面检查和 README 截图永远等于用户启动后看到的样子）。改窄了或改矮了，转写结果卡片会被裁掉、`check:ui` 里「默认窗口尺寸下没有纵向滚动条」那条断言会红。

## 纯函数该往哪加断言

前端有两层测试，**分工不重叠**：

| 层 | 跑在哪 | 守什么 | 怎么跑 |
| --- | --- | --- | --- |
| **纯函数单测** | Node（`scripts/frontend/`） | **纯函数的行为边界** —— 进位、跨字符、全角半角、错误码切分 | `npm run check:frontend` |
| **界面检查** | 真实 WebKit + 假 `__TAURI__`（`scripts/ui/check.mjs`） | **交互结果** —— 点下去界面上是什么 | `npm run check:ui` |

**纯函数**（不碰 DOM、不碰后端）该在单元测试层加断言：格式化、拼句、缺口占位符、错误码翻译、
文件名生成。碰 DOM 的（`toast` / `renderStatus` / `renderHistory`）归界面检查层。

**为什么边界只在单元测试层能测**：`fmtDuration` 的「秒 60 → 分」进位、`buildFileName` 的
「不劈开 emoji」，在界面上要摆一整套状态才碰得到一条 —— 而这类函数恰恰是最容易在重构中被改坏的。

**沙箱**（`scripts/frontend/sandbox.mjs`）让 `src/` 的浏览器模块能在 Node 里跑：补上
`window` / `document` / `navigator` / `localStorage`，并按浏览器的顺序加载**真的字典**
（`src/locales/*.js` 再 `src/i18n.js`，用 `new Function` —— 它们是浏览器脚本不是 ES module）。
**字典必须是真的**：`gapText` 的「中文全角不加空格、英文半角加空格」依赖文案内容，
假字典测不出这条规则。

**测试文件不放 `src/` 下**：`src/` 是 Tauri 的 `frontendDist`（会被打进 app bundle），
且 `check:deps` 规则四要求 `src/` 下每个 `.js` 都登记在层表里。

## 样式组织

样式入口是 `src/style.css`，**只含浏览器原生 `@import`**；规则本身按原文顺序切成 `src/styles/` 下的分片。没有打包器 / PostCSS / Sass，也不许引入 —— 浏览器直接加载这些文件。

| 分片 | 职责 |
| --- | --- |
| `styles/tokens.css` | 设计令牌：`:root` 变量、`prefers-color-scheme` 深色、`:root[data-theme="dark"]` |
| `styles/base.css` | 全局重置、`html` / `body`、`.hidden` / `.muted` 等工具类、macOS 透明底、`.app` 基础容器 |
| `styles/components.css` | 主界面与卡片：顶栏、卡片、首次下载、录音、结果 |
| `styles/lists.css` | 历史列表与云端连接列表 |
| `styles/controls.css` | 通用按钮、拖放遮罩、设置表单与对话框、Toast |
| `styles/motion.css` | `@keyframes`（pulse / spin / ring）与 `prefers-reduced-motion` |
| `styles/shell.css` | 窄屏 `@media`、滚动条、应用外壳、侧栏、导航、主区、宽屏适配 |
| `styles/select.css` | 自绘下拉框（`select.js`） |

- **`@import` 的顺序就是层叠顺序**：同特异性规则后写者赢，`:root` 变量后定义覆盖先定义。原文件里存在前后两段重复定义（`.app`、`html, body`、`.brand`、`.history-list`、`.settings-foot`），后一段靠书写顺序覆盖前一段。**调整分片顺序或插入新分片都会改变渲染结果。**
- **判据是顺序等价**：按 `@import` 顺序拼接所有分片，必须与拆分前的 `src/style.css` 逐字节一致。新增 / 调整分片后按这条自检。
- **不清理重复或看似无用的规则**：原文有 HTML 中已无对应元素的选择器（如 `.topbar`），也可能被 JS 动态加类命中；删除 / 合并属于行为改动，另立 change。
- **`@import` 是本地文件读取**：Tauri 把 `src/` 打进应用资源，分片走本地 asset 协议而不是网络，串行发现的代价是几次磁盘读取；入口与分片仍都在首次绘制前加载完，不会闪烁。若将来界面改为远程加载，这条判断失效。

## i18n 与文案

文案统一放字典，机制单独成文件：`src/locales/*.js` 放文案，`src/i18n.js` 只放机制（`t()` / `apply()` / `setLocale()` / `resolveLocale()`）。**`i18n.js` 里不放任何文案。**

- JS 用 `t('key', {变量})`，HTML 用 `data-i18n` / `-title` / `-placeholder` / `-aria`，启动时 `I18N.apply()` 填充。
- **加语言 = 加一个字典文件 + 在 `index.html` 里加一行 `<script src="locales/xx.js"></script>`**，且必须排在 `i18n.js` 之前（`i18n.js` 启动时读 `window.I18N_LOCALES`，晚加载就没了）。
- **`i18n.js` / `select.js` 保持经典脚本**（挂 `window.I18N` / `window.UISelect`），在 `index.html` 里排在 `<script type="module" src="app.js">` 之前。三条硬约束（`npm run check:i18n` 守着，每条都实测过）：
  - **不能加 `async`** —— async 脚本脱离文档顺序，可能晚于 `app.js` 跑，那时 `window.I18N` 还不存在 → 界面直接挂（等 `.chip` 超时）。
  - **不能写成 `type="module"`** —— 改成 module 后「位置」变成决定性的：挪到 `app.js` 之后界面就挂（实测过）。
  - **不能被 `import`** —— 被 import 会执行两遍（实测 `i18n.js` 执行 2 次、`window.I18N` 被换成第二个实例）。当前代码恰好没因此坏掉（哪个实例生效取决于模块求值顺序），但这种依赖求值顺序的隐患不值得留。
  - 顺带说明：经典脚本排在 `app.js` 之前本身不会坏（实测挪后仍 124/124 通过）；位置这条约束只是「不能是 module」的配套 —— 一旦变成 module，位置才决定成败。
- **字典文案在渲染时取**：存 key，渲染时 `t(key)`。模块顶层取 `t()` 会在切语言时不更新（语言芯片、阶段提示、视图标题都踩过）。视图标题是 `VIEW_KEYS` + `viewTitle()`。
- **随状态变的标签**（麦克风「开始录音 / 结束录音」）用单独的渲染函数（如 `renderMicLabel()`），并在 `applyLocale()` 里重调一次 —— `I18N.apply()` 只认 HTML 上的静态 `data-i18n-aria`。
- **同一个控件的文案不按模式变**：麦克风下方的空闲提示两种模式统一写「点击麦克风开始录音」，模式差异由「整段 / 流式」芯片的副标题说明。
- 后端不返回中文展示文案（模型来源返回 `app_data` / `custom` 这种 token）。

## 文本分层

界面文案与技术细节是两条线，分开写：

- **界面文案 = 一句中性的话 + 用户看得懂的参数**（字典里只留 `{value}` / `{seconds}` 这类占位符）。措辞中性（不用「识别跟不上，丢掉了…」这种口语），也不出现内部黑话（线程、预热、收尾、引擎状态、断句）—— 用「麦克风发生异常」「识别已停止」这种说法。**黑话由 `npm run check:i18n` 查**（只查文案的值，不查 key）。
- **技术细节（系统错误文本、路径、采样格式名）只进日志**：`code_detail()` 会把细节写进日志文件。**字典里不放 `{error}` 占位符** —— `{error}` 只用来装「嵌套的干净文案」，比如 `result.fileFailed` 里那句已经翻译过的原因。
- 字典里查不到的错误码 → 界面显示 `error.unknown`，错误码和细节都不原样甩给用户。前端把错误抛给界面时同理：toast 里放干净文案，细节用 `logEvent('ERROR', …)` 记进日志。

## 错误码

后端错误只返回字典 key：用 `crate::error_code::code(ErrorCode::RecordEmpty)` /
`crate::error_code::code_detail(ErrorCode::RecordMicError, e)`（`error_code.rs` 里定义），
得到 `"record.empty"` / `"record.micError:<细节>"`。前端 `core.js` 的 `fmtError()` 查字典翻译。

**错误码是类型，不是字符串** —— `code()` / `code_detail()` 只收 `ErrorCode` 枚举，
拼错 key **编译不过**。新增错误码 = 在 `error_code.rs` 的枚举里加一个变体、
在 `ErrorCode::key()` 的 match 里写上它的 key。

**新增错误码必须同时加进 `src/locales/zh-CN.js` 和 `src/locales/en.js`**，
`npm run check:i18n` 会校验**枚举里的每个 key** 都在字典里（内部哨兵
`download.cancelled` 显式豁免 —— 它不进字典，界面上看到的是 `setup.cancelled`）。

## 控件

下拉框用自绘组件 `src/select.js`（`UISelect.create(el, {onChange})`）：原生 `<select>` 在 macOS/Windows 样式不可控。弹层挂 body 上用 fixed 定位，避免被滚动容器裁掉。`setItems` 在 `create()` 之后调用，反了拿到 null。

## macOS 原生层

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
