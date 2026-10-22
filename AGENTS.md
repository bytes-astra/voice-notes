# VoiceNotes

Tauri 2 桌面语音转文字应用，本地跑 SenseVoiceSmall INT8（sherpa-onnx）。

## 运行 / 构建

```bash
npm install
npm run dev            # 开发模式
npm run build          # 打包 → src-tauri/target/release/bundle/
```

- 前端是**原生 JS + ES Modules**：`src/index.html` + `app.js`（入口）+ `core.js` / `tauri.js` / `status.js` / `recorder.js` / `files.js` / `result.js` / `history.js` / `events.js` / `export.js` / `theme.js` + `style.css` + `i18n.js` + `select.js` + `locales/*.js`，没有打包器、没有框架、没有 TypeScript。不要为 UI 引入构建步骤。
- **模块依赖方向（不能反，也不能成环）**：`core.js` / `tauri.js` 在最底层 → `theme.js` / `export.js` → `history.js` → `result.js` → `status.js` → `recorder.js` → `files.js` / `events.js` → `app.js`（入口）。`core.js` 和 `tauri.js` 不 import 任何业务模块；`status.js` 不 import `recorder.js`（所以 `idleHint()` 放在 core）；`result.js` 不 import `files.js`；`history.js` 不 import `result.js`。
- **`i18n.js` / `select.js` 保持经典脚本**（挂 `window.I18N` / `window.UISelect`），`index.html` 里必须排在 `<script type="module" src="app.js">` 之前。**别把它们当模块 import** —— 会被重新执行一遍、覆盖掉 `window.I18N`，表现为「切语言后部分文案不跟着变」。
- **`state` / `els` 只在 `core.js` 创建一次**，其他模块一律 `import` 引用（ES Module 的 live binding 是同一个对象，不会拷贝）。
- **下拉框用自绘组件 `src/select.js`**（`UISelect.create(el, {onChange})`）：原生 `<select>` 在 macOS/Windows 样式不可控。弹层挂在 body 上用 fixed 定位，避免被滚动容器裁掉。
- **所有界面文案走 `src/i18n.js` 的字典**：JS 里用 `t('key', {变量})`，HTML 里给元素加 `data-i18n="key"`（还有 `data-i18n-title/-placeholder/-aria`），启动时 `I18N.apply()` 填充。加语言 = 加一个字典，不要在代码里写死中文。后端也别返回中文展示文案（模型来源返回 `app_data` / `custom` 这样的 token）。
- **字典文案必须在渲染时取，不能在模块顶层就取**：`const LANGS = [['auto', t('lang.auto')]]` 这种写法在切语言时不会跟着变（已经踩过三次：语言芯片、阶段提示 HINTS、视图标题）。正确写法是存 key，渲染时 `t(key)`（视图标题现在是 `VIEW_KEYS` + `viewTitle()`）。
- **`aria-label` 也要走 i18n**（`data-i18n-aria="key"`），别在 HTML 里写死中文：写死的那份在英文界面下会漏出来（麦克风按钮踩过）。**随状态变的中文标签**（麦克风「开始录音 / 结束录音」）要用单独的渲染函数（如 `renderMicLabel()`），并在 `applyLocale()` 里重新调一次——`I18N.apply()` 只认 HTML 上的静态 `data-i18n-aria`，不会跟着阶段变。
- **改文案后跑 `npm run check:i18n`**：检查各语言字典 key 是否对齐、`index.html` 引用的 key 是否存在（漏 key 会直接把 key 名显示在界面上）。
- **同一个控件的文案不要按模式变**：麦克风下方的空闲提示语两种模式统一写「点击麦克风开始录音」，模式差异由「整段 / 流式」芯片的副标题说明。按钮动作永远是录音，提示语却写成「实时转写」会让人以为点的是转写已有文件。

- 前端通过 `window.__TAURI__.*` 调插件（`withGlobalTauri: true`），调用面在 `src/tauri.js` 集中解构，其他模块从那里 import。
- 权限声明在 `src-tauri/capabilities/default.json`；新增插件命令时要同步加权限，否则运行时直接拒绝。
- 麦克风用途说明在 `src-tauri/Info.plist`（`NSMicrophoneUsageDescription`），通过 `bundle.macOS.infoPlist` 合并进 .app。

## 后端结构

```
lib.rs       Tauri 命令、AppState、保存文件、下载编排
asr.rs       SenseVoice 识别 + Silero VAD 分句 + 流式 Zipformer（StreamingSession）
audio.rs     cpal 采集、电平事件、16 kHz 重采样
models.rs    模型目录解析 / 下载（HF → hf-mirror）/ sha256 校验
settings.rs  设置 JSON 持久化
wav.rs       PCM16 WAV 解析（前端载荷）
```

- **命令参数名用 camelCase**（Tauri 2 默认），例如 `audioBase64`、`fileName`。
- 推理是同步阻塞的，一律放进 `tauri::async_runtime::spawn_blocking`，不要阻塞 async 命令的执行器。
- 引擎按 `(language, itn)` 缓存；语言或 ITN 变化时重建。想加“切换语言不重载”，需要走 C API 的 `setConfig`，Rust 绑定没暴露。
- 事件：`audio:level`、`audio:error`、`model:progress`、`engine:state`、`stream:partial`、`stream:segment`、`streaming:state`。改事件名要同步 `src/events.js` 的 `listenEvents()`（事件名只在那一个文件里出现）。
- **两种模式**：`settings.mode = offline | streaming`（界面上叫「整段 / 流式」，两个都是语音模型，区别在识别方式）。整段走 SenseVoice（VAD 分句）；流式走 Zipformer transducer，`start_recording` 先 `ensure_streaming_engine` 再起采集，`run_streaming_core` 每 60ms 从录音缓冲按游标取新采样、重采样后喂给 `StreamingSession`。
- **流式必须按 200ms（3200 样本）分片、且只在 `is_ready()` 为真时 `decode()`**，否则 sherpa-onnx 特征缓冲会越界直接 abort（`features.cc GetFrames`）。这是踩过的坑，别改成小分片或无条件 decode。
- 流式模型是独立的一组 assets（`STREAM_*`）：`streaming-zipformer-zh-int8-2025-06-30` + `silero_vad.onnx`，只覆盖中文；语言选粤/日/韩时前端会自动切回非流式。
- 标点模型（`PUNCT`，CT-Transformer zh-en，281 MB）独立放在 `models/punct/`，默认关闭；开启后 `StreamEmitter` 在每句定稿时调用 `PunctuationEngine::apply`，实时中间结果（partial）不加标点。模型缺失/加载失败时静默降级，不阻断录音。
- **流式收尾必须补 300ms 静音**（`asr.rs` 的 `TAIL_SILENCE_SAMPLES`）：解码器需要尾部上下文才会吐出最后一个字，不补就会丢句尾（“产品评审会”→“产品评审”）。补的静音不计入音频时长。
- **不要用 SenseVoice 覆盖流式结果**（这个功能已经删掉了，别再加回来）：实测（5 条中文样本）修收尾后流式 99%、SenseVoice 91%，SenseVoice 会把“明天”听成“今天”。而且复核要求保留全部音频，和「录音不限时长、内存恒定」直接冲突。
- **SenseVoice 开 ITN 时自己就输出标点**（关掉 ITN 才是纯文本）。标点模型（CT-Transformer）现在只用于**流式模式**给句子补标点；整段模式靠 SenseVoice 自带的 ITN 标点，不要再给整段结果再套一遍标点模型，否则会叠成「。。」。
- 流式链路带 **VAD 门控**：Silero 判到静音就 `force_end()` 提交并 reset，静音段不喂模型（否则 transducer 会在噪声上编出重复字），起音前保留 0.4s 预录避免切字头。
- 模型固定版本（HF commit + sha256 在 `models.rs` 的 `MODEL/TOKENS/VAD` 常量里）。换模型必须同时改 bytes/sha256，否则下载校验会失败。
- **麦克风权限在界面渲染完之后申请**：macOS 是在**第一次碰输入设备**时才弹权限框的，而「列出输入设备」就会触发。所以流程是：启动 → 界面画完（约 0.9s）→ 调 `list_input_devices` → 系统弹授权框。列设备这一步同时把设备列表填好了，下拉框打开即用。
- **别在启动时同步列设备**（`init()` 里直接调）：弹框会打断用户，而且实测弹框期间应用窗口是空白的（进程没有重启，空白的原因没查清）。点麦克风时仍然先 `warm_up_mic` 探一次权限，用户拒绝过的话能给出明确报错。
- **输入设备可选**：`list_input_devices` 命令列出 cpal 输入设备（默认设备带标记），`set_input_device` 存进 settings（None = 系统默认），`audio::start` 按名字挑设备；挑不到就退回默认。下拉框每次展开会重新列一次（`select.js` 的 `onBeforeOpen`），中途插拔设备也能看到。
- **自绘下拉框的 `setItems` 必须在 `UISelect.create()` 之后调用**，反了会拿到 null（启动直接报错）。
- 录音缓冲上限 600 秒（`audio.rs` 的 `MAX_SECONDS`），前端 `MAX_SECONDS` 也要一起改。

## 测试

```bash
cd src-tauri
cargo test --lib                      # 纯逻辑 + 麦克风枚举
cargo test --test asr_smoke -- --nocapture   # 非流式真模型转写（没模型会跳过）
cargo test --test streaming_smoke -- --nocapture  # 流式真模型转写（VOICE_NOTES_STREAMING_MODEL_DIR）
```

`asr_smoke` 需要 `VOICE_NOTES_MODEL_DIR` 指向包含 `model.int8.onnx` / `tokens.txt` 的目录，未设置时自动跳过。

## Windows

- Tauri 不能在 macOS 上交叉打 Windows 包，CI 用 `.github/workflows/build.yml`（windows-latest）出 msi/nsis。
- 代码侧要点：cpal 采样格式已覆盖 F32/I16/U16/I32；Windows 用原生标题栏，`body.macos` 不生效，侧栏顶部占位是 12px；`bundle.windows.webviewInstallMode = downloadBootstrapper`。
- 菜单只在 macOS 构建（`#[cfg(target_os = "macos")]`）；Windows 无自定义菜单。

## 已知坑

- 首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 拉预编译静态库。GitHub 不通时用 `SHERPA_ONNX_ARCHIVE_DIR=/path/to/dir`（目录里放 `sherpa-onnx-v1.13.8-<os>-<arch>-static-lib.tar.bz2`）。
- **避免 webview 重绘时闪白**：`html` 铺上 `var(--bg)`，窗口底色跟着主题走（`syncWindowBackground()` → `setBackgroundColor`，需要 `core:window:allow-set-background-color` 权限）。只给 `body` 上色是不够的。
- macOS 未签名构建的麦克风权限可能需要在「系统设置 → 隐私与安全性 → 麦克风」手动勾选；已签名的正式包才会弹标准授权框。
- 外壳是「侧边栏 + 主区」：`.app[data-view]` 控制 `[data-view-panel]` 显隐，`switchView()` 统一处理导航高亮/标题；录音中不允许切走。新增页面 = 一个 `[data-view-panel]` + 一个 `.nav-item`。
- 拖动窗口靠 `data-tauri-drag-region="deep"`（`.sidebar-top` 与 `.main-head`，按钮自动排除）+ **`core:window:allow-start-dragging` 权限**——这个权限不在 `core:default` 里，删了窗口就拖不动。macOS 侧边栏顶部用 `body.macos .sidebar-top { height: 78px }` 给红绿灯留位，别把品牌压到窗口按钮下面。
- macOS 全屏时红绿灯会自动隐藏、鼠标移到顶部才滑出；侧栏顶部占位**始终**保留 78px，不随全屏变化（否则内容会在进出全屏时跳动）。
- **改 UI 要在浏览器里验**：用 Playwright + 一个假 `window.__TAURI__`（mock `invoke`）把 `src/` 跑起来，检查「没有未翻译的 key」「切换语言/模式后文案都跟着变」「控制台无报错」。截图同理——**要等页面渲染完再截**，否则会截到半成品（语言芯片、下拉框都还没画出来）。
- **harness 的静态服务器用 Node 起，别用 `python -m http.server`**：python 那个会偶发 `ERR_CONNECTION_RESET`（app.js / i18n.js 加载失败），表现是 `state is not defined` 或页面半渲染，很容易误判成代码问题。截图脚本里加一行 `page.on('requestfailed', ...)` 就能立刻看出是服务器抖了。

- 端到端冒烟结果：非流式 5.6s 中文 ≈ 200ms（RTF 0.04）；流式同段音频首字约 1 个分片、整段 0.5s 内出完，无标点。
