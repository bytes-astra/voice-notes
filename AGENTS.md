# VoiceNotes

Tauri 2 桌面语音转文字应用。整段模式跑 SenseVoiceSmall INT8，流式模式跑 Zipformer transducer，都在本地 CPU 上推理（sherpa-onnx 1.13.8）。

## 命令

```bash
npm install
npm run dev              # 开发模式
npm run build            # 打包 → src-tauri/target/release/bundle/
npm run check:i18n       # 改文案后必跑：各语言 key 对齐 + HTML/JS 引用的 key 存在
npm run check            # 门禁：i18n → clippy（-D warnings）→ cargo test --lib
npm run check:ui         # 界面检查：真实 WebKit + 假 __TAURI__，47 项
npm run shots            # 重出 README 的 9 张截图
```

**门禁在 CI 里**（`.github/workflows/check.yml`，推 main / 提 PR 都跑）：前端 job 用 ubuntu（i18n + 界面检查），Rust job 用 macOS（clippy + 单元测试）—— 这个项目的 Rust 端在 Linux 上没验证过（cpal 要 ALSA 头文件），别把没支持的平台塞进 CI。

```bash
cd src-tauri
cargo test --lib                      # 纯逻辑 + 麦克风枚举
cargo test --test asr_smoke           # 整段真模型转写（需 VOICE_NOTES_MODEL_DIR）
cargo test --test streaming_smoke     # 流式真模型转写（需 VOICE_NOTES_STREAMING_MODEL_DIR）
cargo test --test decode_smoke        # 后端解码 + 文件全链路转写
cargo test --test quality_probe -- --nocapture --test-threads=1   # 质量探针，只打印不断言
```

## 前端约定

- **原生 JS + ES Modules**，没有打包器、没有框架、没有 TypeScript。不要为 UI 引入构建步骤。
- **模块依赖方向（不能反，也不能成环）**：`core.js` / `tauri.js` → `theme.js` / `export.js` → `history.js` → `result.js` → `status.js` → `recorder.js` → `files.js` / `events.js` → `app.js`。`core.js` / `tauri.js` 不 import 任何业务模块；`status.js` 不 import `recorder.js`（所以 `idleHint()` 放 core）；`result.js` 不 import `files.js`；`history.js` 不 import `result.js`。
- **`i18n.js` / `select.js` 保持经典脚本**（挂 `window.I18N` / `window.UISelect`），在 `index.html` 里必须排在 `<script type="module" src="app.js">` 之前。**别 import 它们** —— 会被重新执行一遍、覆盖 `window.I18N`，表现为「切语言后部分文案不跟着变」。
- **`state` / `els` 只在 `core.js` 创建一次**，其他模块 `import` 引用（live binding 是同一个对象）。
- **所有界面文案走 `src/i18n.js`**：JS 用 `t('key', {变量})`，HTML 用 `data-i18n` / `-title` / `-placeholder` / `-aria`，启动时 `I18N.apply()` 填充。不要在代码里写死中文，后端也不要返回中文展示文案（模型来源返回 `app_data` / `custom` 这种 token）。
- **字典文件在 `src/locales/*.js`，机制在 `src/i18n.js`**：每种语言一个文件（中文是 `zh-CN.js`，英文是 `en.js`），自己在 `window.I18N_LOCALES` 上注册。**加语言 = 加一个字典文件 + 在 `index.html` 里加一行 `<script src="locales/xx.js"></script>`** —— 必须排在 `i18n.js` 之前（`i18n.js` 启动时读 `window.I18N_LOCALES`，晚加载就没了）。`i18n.js` 里不放任何文案。
- **后端错误只返回字典 key**：用 `crate::code("record.empty")` / `crate::code_detail("record.micError", e)`（lib.rs 里定义），得到 `"record.empty"` / `"record.micError:<细节>"`。前端 `core.js` 的 `fmtError()` 查字典翻译。**新增错误码必须同时加进 `src/locales/zh-CN.js` 和 `src/locales/en.js`**，`npm run check:i18n` 会校验后端引用的码在字典里。
- **用户提示与技术细节分开**（两条线，别混）：
  - **界面文案 = 一句中性的话 + 用户看得懂的参数**（字典里只留 `{value}` / `{seconds}` 这类占位符）。别口语化（「识别跟不上，丢掉了…」这种），也别把内部黑话写给用户（线程、预热、收尾、引擎状态、断句）—— 用「麦克风发生异常」「识别已停止」这种说法。
  - **技术细节（系统错误文本、路径、采样格式名）只进日志**：`code_detail()` 会把细节写进日志文件（`~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，见后端约定里那条）；**字典里不要留 `{error}` 占位符**（`{error}` 只用来装「嵌套的干净文案」，比如 `result.fileFailed` 里那句已经翻译过的原因）。
  - 字典里查不到的错误码 → 界面显示 `error.unknown`，**不要把错误码或细节原样甩给用户**。
- **缺口标记用占位符**：识别跟不上丢掉音频时，后端在文本里插 `[[gap:秒数]]`（`audio::gap_marker`），前端渲染时用 `core.js` 的 `localizeText()` 换成 `t('record.gap', {seconds})`。**后端不要直接写中文标记**。占位符前后拼句不加空格（后端 `asr::join_segments` 与前端 `joinText()` 规则一致）。
- **字典文案在渲染时取**：存 key，渲染时 `t(key)`。模块顶层取 `t()` 会在切语言时不更新（语言芯片、阶段提示、视图标题都踩过）。视图标题现在是 `VIEW_KEYS` + `viewTitle()`。
- **随状态变的标签**（麦克风「开始录音 / 结束录音」）用单独的渲染函数（如 `renderMicLabel()`），并在 `applyLocale()` 里重调一次 —— `I18N.apply()` 只认 HTML 上的静态 `data-i18n-aria`。
- **同一个控件的文案不要按模式变**：麦克风下方的空闲提示两种模式统一写「点击麦克风开始录音」，模式差异由「整段 / 流式」芯片的副标题说明。
- **下拉框用自绘组件 `src/select.js`**（`UISelect.create(el, {onChange})`）：原生 `<select>` 在 macOS/Windows 样式不可控。弹层挂 body 上用 fixed 定位，避免被滚动容器裁掉。`setItems` 必须在 `create()` 之后调用，反了拿到 null。
- Tauri 插件调用面集中在 `src/tauri.js` 解构，其他模块从那里 import。新增插件命令要同步在 `src-tauri/capabilities/default.json` 加权限，否则运行时直接拒绝。
- **macOS 侧栏的原生材质**：`lib.rs` 的 setup 里 `set_effects(Effect::Sidebar)`（NSVisualEffectView），配合 `style.css` 里 `body.macos` 的透明规则 —— 侧栏不画底色，系统材质从后面透出来，主区自己刷不透明底色。
  - **不用开 `transparent: true`**：Tauri 内置的 `set_effects` 自己就把 webview 处理成透明了。开了反而更糟 —— 那是全局窗口属性，Windows 也会变 layered 窗口（DWM 投影会没），而 Tauri 没有平台专属的窗口配置（`tauri.windows.conf.json` 的合并是 RFC 7386，数组整体替换，改一个字段得抄整份 window 配置）。
  - **材质的明暗跟的是窗口外观**（NSWindow.appearance），不是网页的 `data-theme` —— 切主题时必须同步 `setTheme('Dark'|'Light')`（`theme.js` 的 `syncWindowTheme()`），否则系统浅色 + 应用暗色 = 浅色材质配深色字。
  - **`setTheme` 需要 `core:window:allow-set-theme` 权限**，漏了会被 ACL 直接拒绝，而前端那行 `.catch` 会把错误吞掉 —— 表现是「暗色下材质不变」且毫无提示。失败要 `logEvent` 记进日志。
  - Liquid Glass（macOS 26+ 的 NSGlassEffectView）试过，效果不如经典材质，已回退。
- **窗口标题跟界面语言走**：`tauri.conf.json` 的 `title` 只是启动时的默认值（写 `VoiceNotes`，别写死中文），运行时由 `applyLocale()` 调 `getCurrentWindow().setTitle(t('app.title'))` 设置（需要 `core:window:allow-set-title` 权限）。
- 外壳是「侧边栏 + 主区」：`.app[data-view]` 控制 `[data-view-panel]` 显隐，`switchView()` 统一处理导航高亮/标题，录音中不允许切走。新增页面 = 一个 `[data-view-panel]` + 一个 `.nav-item`。

## 后端约定

```
lib.rs       Tauri 命令与事件载荷（只放命令层，业务逻辑去对应模块）
record.rs    录音编排：采集队列 → 识别会话 → 出字（纯逻辑，不认识 Tauri）
engine.rs    三个引擎的构造（整段 / 流式 / 标点）+ 加载 + AppState + Status
asr.rs       识别会话（Offline/Streaming Session）+ 转写数据结构 + 文本规则
audio.rs     cpal 采集、消费式采集缓冲、电平环、16 kHz 重采样 + 缺口占位符格式
decode.rs    symphonia 后端解码（mp3/wav/flac/ogg/m4a/mp4/mov，视频轨忽略）
models.rs    模型目录解析 / 下载（镜像优先）/ sha256 校验
output.rs    转写结果落盘（文件名清洗 + 同名自动加序号，不覆盖）
settings.rs  设置 JSON 持久化
log.rs       应用日志（写文件 + stderr 镜像）
menu.rs      macOS 原生菜单（只有 macOS 编）
```

- **lib.rs 只放命令层**：一个文件混着三层（命令 / 业务 / 基础设施）会长得很快 —— 之前它 1821 行，现在约 970 行。新逻辑先找对应模块放：`record.rs`（录音编排）、`engine.rs`（引擎与状态）、`output.rs`（落盘）。命令本身保持薄：一个命令别同时干两件事（`start_recording` 已按模式拆成 `start_streaming_recording` / `start_offline_recording`）。
- **`record.rs` 不认识 Tauri**：结果通过 `emit` 回调交出去，所以它能被单测直接调用。**别往里塞 `AppHandle`** —— 实在要发事件就在 lib.rs 那层包一下。
- **名字别撞**：`record.rs` 是「录音怎么跑」，`asr.rs` 里的 `OfflineSession` / `StreamingSession` 是「识别会话」，两回事 —— 所以前者不叫 session.rs。缺口占位符（`[[gap:N]]`）的**格式**只在 `audio.rs` 一处（构造 + 解析都在那儿），别在别的模块再写一遍正则。

- **命令参数名用 camelCase**（Tauri 2 默认），如 `audioBase64`、`fileName`。
- **推理是同步阻塞的**，一律放进 `tauri::async_runtime::spawn_blocking`，别阻塞 async 执行器。
- 引擎按 `(language, itn)` 缓存，语言或 ITN 变化时重建。想「切语言不重载」得走 C API 的 `setConfig`，Rust 绑定没暴露。
- **事件名只出现在 `src/events.js`**：`audio:level`、`audio:error`、`model:progress`、`engine:state`、`stream:partial`、`stream:segment`、`streaming:state`。改名字要同步前端。
- **`audio:error` 的载荷是对象**：`{ code, detail?, seconds?, fatal }`。`code` 是字典 key，`fatal: false` 表示「提示但不中断」（比如丢掉音频的缺口提示 —— 录音要继续），前端只在 `fatal` 为真时才取消录音。
- **事件载荷是增量的**：`stream:segment` 只带新句子，`stream:partial` 只带中间结果。别改回累计全文（长时间会话是 O(n²) 的传输与重绘）。前端 `core.js` 的 `joinText()` 与后端 `asr::join_segments()` 规则必须一致（中日韩之间直接连，其余加空格）。
- 文件解码在后端：`decode::decode_to_16k()` 逐块解码，内存恒定。symphonia 锁 **0.5**（0.6 要 Rust 1.85，会破本项目声明的 1.78）。
- **整段和流式的模型下载是两条独立的线**（可以同时下）：各自有 `downloading` / `cancel_download` 标志（`engine.rs` 的 `AppState` 里 `downloading_streaming` / `cancel_download_streaming`）。**共用一个标志的话，整段在下载时点流式下载会直接报 busy 失败**。
  - 进度事件是同一个名字（`model:progress`），所以载荷里带 `kind`（`"offline"` / `"streaming"`，由 `lib.rs` 的 `EventSink` 塞进去）—— **前端必须按 kind 分流**，别看「当前是不是在下载」那种状态，否则两个面板会互相显示对方的进度（看起来就像流式在重复下载整段模型）。
  - `cancel_download` 命令带 `kind` 参数，指明取消哪一个。
- **转写结果的默认输出目录用 ASCII**（`settings::default_output_dir` → `~/Documents/VoiceNotes`）：中文目录名在终端、脚本、跨平台工具里处处要转义。旧版的中文目录「语音转文字」由 `settings::migrate_output_dir()` 在启动时一次性改名（只在新目录不存在时才搬，不覆盖已有数据）。
- **打开文件夹用自己的 `open_output_dir` 命令，不要用 opener 插件**：插件的 `opener:allow-open-path` 权限是「without any pre-configured scope」—— **不配 scope 的话任何路径都被拒**（报 "Not allowed to open path"）；而保存目录是用户可改的（`settings.output_dir`），静态 scope 盖不全。后端自己解析出真实路径再 `open`/`explorer`/`xdg-open` 更省事（应用自己的命令不需要 ACL 权限，`__allow_command` 直接放行）。
- **日志一直开着，不需要用户配置**：`log.rs`（`log::info` / `warn` / `error`）写到系统标准日志目录 —— macOS `~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，Windows `%LOCALAPPDATA%\com.github.bytes-astra.voicenotes\logs\voice-notes.log`；超 1 MB 轮转一次保留 `.1`，同时镜像 stderr。**没有开关、没有设置页** —— 用户反馈问题直接把这个文件发过来。前端那边用 `client_log` 命令写进同一个文件（`src/tauri.js` 的 `logEvent()`），这样一份文件就是完整的一份记录。
- **技术细节只进日志，不进界面文案**：`code_detail()` 已经把细节写进日志了，界面那边只显示字典里的干净文案（见「用户提示与技术细节分开」那条）。前端把错误抛给界面时同理：toast 里放干净文案，细节用 `logEvent('ERROR', …)` 记下来。
- 拖拽用 Tauri 的 `onDragDropEvent`：Tauri v2 默认拦截系统拖放，HTML5 `drop` 事件在真应用里不触发。**所以 `tauri.conf.json` 的 `dragDropEnabled` 必须保持 `true`** —— 关掉它 wry 就不装拖放 handler，`onDragDropEvent` 永远不触发，拖拽转写直接变成死代码（而且没有 HTML5 兜底）。

## 两种模式

`settings.mode = offline | streaming`，界面上叫「整段 / 流式」——**两个都是语音模型**，区别在识别方式。

**整段（SenseVoice）**

- `OfflineSession` 用 VAD 切句、逐句 `Engine::recognize()`，边录边出字。（会话在 `asr.rs`，引擎在 `engine.rs`）
- **VAD 缓冲给 60 秒**（`VoiceActivityDetector::create(&vad, 60.0)`，在 `asr.rs` 的 `OfflineSession::new`）。sherpa 的 `max_speech_duration` 不是硬切，是「把 VAD 参数调严」（min_silence 0.5→0.1、threshold 0.5→0.9 逼它找静音），连续说话时找不到，句子能到 1 分钟以上。给 32 秒会触发 CircularBuffer 扩容（Overflow 警告 + 整段拷贝）。
- **默认整句一次识别**（最准，`asr.rs` 的 `MAX_SINGLE_SHOT_SECONDS` / `merge_transcripts`）。SenseVoice 的「30 秒上限」实测不是硬约束：喂 55 秒 / 130 秒连续语音准确率都 98.6%。只有超过 `MAX_SINGLE_SHOT_SECONDS`（90 秒）才切 30 秒窗口 + 2 秒重叠（`merge_transcripts` 合并），防编码器注意力平方级吃显存。**别为了「符合官方限制」就切** —— 切了反而更差（97.9%），而且会把边界上的词劈开。
- **VAD 会切掉开头第一个字**：音频开头直接是语音（没前导静音，比如 TTS、剪过的文件）时 sherpa VAD 的内部 padding 只有 ≈314 ms（「今天下午」→「天下午」）。`asr.rs` 的 `should_pre_roll` 只在「第一段 + 起点 < 0.6 s + 段前 RMS > 段 RMS × 0.3」时把起点回扩 0.4 s。**不要改成无门控回扩** —— 实测会把开头噪声当语音（真人样本多出「您好」）。
- **ITN 自带标点**（ITN 开关在 `engine.rs` 的 `Engine::new`）：开 ITN 的 SenseVoice 自己就输出标点（关 ITN 才是纯文本）。标点模型只用于流式，别给整段再套一遍，否则叠成「。。」。

**流式（Zipformer transducer）**

- `StreamingSession` 200 ms（3200 样本）一片喂（`asr.rs`），`start_recording` 先 `ensure_streaming_engine` 再起采集（`lib.rs`）。
- **必须按 200 ms 分片、且只在 `is_ready()` 为真时 `decode()`**（`asr.rs` 的 `StreamingSession`），否则 sherpa-onnx 特征缓冲越界直接 abort（`features.cc GetFrames`）。别改成小分片或无条件 decode。
- **收尾必须补 300 ms 静音**（`TAIL_SILENCE_SAMPLES`，在 `asr.rs`）：解码器需要尾部上下文才吐出最后一个字，不补就丢句尾（「产品评审会」→「产品评审」）。补的静音不计入时长。
- **VAD 门控**（在 `record.rs` 的 `run_streaming_core`）：Silero 判到静音就 `force_end()` 提交并 reset，静音段不喂模型（否则 transducer 会在噪声上编出重复字）；起音前留 0.4 s 预录避免切字头。
- 流式模型是独立的一组 assets（`models.rs` 的 `STREAM_*`：`streaming-zipformer-zh-int8-2025-06-30` + `silero_vad.onnx`），只覆盖中文；选粤/日/韩时前端自动切回整段。
- **不要用 SenseVoice 覆盖流式结果**（这个功能已删，别加回来）：修收尾后流式 99%、SenseVoice 91%，而且覆盖要保留全部音频，和「不限时长、内存恒定」直接冲突。
- **术语表（热词）**（`settings.rs` 清洗，`engine.rs` 的 `StreamingEngine::new` 用它）：`settings.hotwords`（≤ 100 条、单条 ≤ 32 字，`settings::clean_hotwords` 清洗）。非空时 `StreamingEngine` 用 `modified_beam_search` + `config.hotwords_buf`（短音频约慢 6%），为空保持 greedy。**只能用在流式** —— SenseVoice 上给热词或 beam 会 `_Exit(-1)` 杀进程（已复现）。改词表会清空流式引擎，下次录音重载 161 MB。设置页「填入示例」用的是 `src-tauri/assets/hotwords.txt`。
- 标点模型（`PUNCT`，CT-Transformer zh-en，281 MB，`engine.rs` 的 `PunctuationEngine`）独立放在 `models/punct/`，默认关闭；开启后 `StreamEmitter` 在每句定稿时调 `PunctuationEngine::apply`，实时中间结果不加标点。模型缺失/加载失败时静默降级，不阻断录音。

## 音频与采集

- **采集缓冲是消费式的**（`audio.rs`）：`audio::SampleQueue` 的 `take_new()` 取走即释放，内存恒定（录 1 小时和 1 分钟占一样多）。**电平表走独立的 `LevelRing`**（2048 样本）—— 采集缓冲没有历史了，音量表去读它会恒读 0。
- **安全阀**（`audio.rs`）：未消费采样超过 `MAX_PENDING_SECONDS × 设备实际采样率`（60 秒，采样率不能写死 48k）时丢最旧的，并用 `audio::gap_marker()` 在结果里留可见缺口，不能静默丢。
- **停止录音时 `Recorder::stop()` 只关流、不取采样**（`audio.rs` 的 `stop`；收尾取样在 `record.rs`）：句尾最后一段要留给识别线程收尾时 `take_new()`。主线程先取走会切掉最后一个字（离线/流式都中招）。
- **麦克风权限在界面渲染完之后申请**：macOS 是第一次碰输入设备才弹权限框，而「列出输入设备」就会触发。流程：启动 → 界面画完（约 0.9 s）→ `list_input_devices` → 弹框（这一步同时把设备列表填好）。
- **别在启动时同步列设备**：弹框会打断用户，实测弹框期间窗口是空白的。点麦克风时仍先 `warm_up_mic` 探权限，用户拒绝过要能给出明确报错。
- **输入设备可选**：`list_input_devices` 列出 cpal 输入设备（默认设备带标记），`set_input_device` 存 settings（None = 系统默认），`audio::start` 按名字挑，挑不到退回默认。下拉框每次展开重新列一次（`select.js` 的 `onBeforeOpen`），中途插拔也能看到。
- 录音**没有时长上限**。

## 模型与下载

- 模型版本固定：HF commit + sha256 + bytes 写在 `models.rs` 的 `MODEL` / `TOKENS` / `VAD` / `STREAM_*` / `PUNCT` 常量里。**换模型必须同时改 bytes 和 sha256**，否则下载校验失败。
- **下载走镜像优先**（`[MIRROR, HF]`）：国内直连 huggingface.co 经常连不上，镜像失败再回退官方源。别改回 HF 优先。
- **模型目录必须有 `silero_vad.onnx`**：整段分句、文件转写都靠 VAD，`models::resolve()` 会直接跳过没有 VAD 的目录（否则录完只得空结果）。
- 已存在且大小正确的文件直接跳过下载；`.part` + Range 支持断点续传；下完做 sha256 校验。
- 应用数据目录里的文件按**字节数**判定是否就绪（`app_data_file_ok`），所以换 VAD 版本后会自动重下。

## 验证

**改任何模型 / 参数 / 热词之前，先跑探针做前后对比，别凭感觉。**

```bash
VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
VOICE_NOTES_VAD_ALT=/path/to/silero_vad_v5.onnx \
VOICE_NOTES_HOTWORDS=$'再确认一下\n产品评审会' \
VOICE_NOTES_RULE3=60 \
  cargo test --test quality_probe -- --nocapture --test-threads=1
```

探针只打印不断言，会把 ITN 开关、语言指定方式、流式 greedy/beam/热词、VAD 模型与静音阈值、端点硬切行为在同一段真实音频上的实际表现打出来。参考文本放在 `src-tauri/tests/fixtures/<名字>.txt` 时自动算 CER；它也会扫描 `tests/fixtures/quality/*.wav`。

语料（仅本机，wav 不入库）：

```bash
node scripts/fetch-quality-corpus.mjs 12   # 真人中文电话录音 + 转写（minds14）
bash scripts/gen-tts-corpus.sh             # TTS 20 句 + 数字句 + 长句 + 噪声变体
```

当前基线（zh + ITN 整段，含 pre-roll 修复；40 条真人语料）：真人 **11.41%** / 真人+噪声 **1.39%** / TTS **15.55%**（修复前 26.47%）/ 长句 **15.00%**。**别拿 TTS 的绝对 CER 当质量结论** —— 它主要暴露「VAD 切字头」这类相对差异。

**怀疑「录音丢了」先跑探针，别猜**（四个探针在 `src-tauri/tests/`，只打印不断言；共用的小工具在 `src-tauri/tests/common/mod.rs`，探针里写 `mod common;` 引入）：

```bash
VOICE_NOTES_MODEL_DIR=... cargo test --test vad_long_probe -- --nocapture      # VAD 吞不吞音频
VOICE_NOTES_MODEL_DIR=... cargo test --test offline_long_probe -- --nocapture   # 整段会话覆盖率
VOICE_NOTES_STREAMING_MODEL_DIR=... cargo test --test streaming_long_probe -- --nocapture
VOICE_NOTES_MODEL_DIR=... cargo test --test resample_probe -- --nocapture       # 48k→16k 重采样
```

2026-10-24 实测结论（176 秒连续语音）：**VAD、整段会话、流式会话、重采样四条路都不丢音频**
（覆盖 175.6/176.0 秒，误差只有句边界）。**队列上限 60 秒是唯一的丢失点** —— 超了会丢**最旧**的
（`SampleQueue::take_new` 里 `drain(..excess)`），并留缺口标记；日志里会打「识别跟不上，丢掉 X 秒音频」。
识别是同步阻塞在消费线程里的（`record.rs` 的 `run_offline` → `asr.rs` 的 `drain()`），但 RTF 只有 0.01，
实际落后不到 1 秒，**离 60 秒上限差得很远** —— 所以「加 buffer」不是解法。

**改 UI 要在浏览器里验**（Playwright + 假 `window.__TAURI__`）：

```bash
npm run check:ui     # 47 项检查
npm run shots        # 重出 README 9 张截图
```

- 脚本在 `scripts/ui/`（`check.mjs` / `shots.mjs` / `server.mjs`），假 `window.__TAURI__` 在 `check.mjs` 里。**浏览器不在默认位置时要设 `PLAYWRIGHT_BROWSERS_PATH`**（本机是 `/tmp/pw-browsers`）。
- 静态服务器用 Node 起（`scripts/ui/server.mjs`），**别用 `python -m http.server`**：那个会偶发 `ERR_CONNECTION_RESET`（app.js / i18n.js 加载失败），表现是 `state is not defined` 或半渲染，很容易误判成代码问题。脚本里已经加了 `page.on('requestfailed', ...)`，服务器一抖立刻就能看出来。
- 假 Tauri 的 `event.listen` 会把回调存起来，页面里用 `window.__emit('model:progress', {...})` 主动发事件 —— **后端事件的行为（比如进度按 kind 分流）要靠它测**。
- 截图要**等页面渲染完再截**，否则会截到半成品（语言芯片、下拉框还没画出来）。
- 深色截图用应用自己的主题切换按钮（`#theme-btn`）出，WebKit 里 `prefers-color-scheme` 不可靠。
- 调研与优化方案在 `docs/research/`（先看 `2026-10-23-asr-quality-roadmap.md`）。

## 打包与发布

- **macOS 必须 ad-hoc 签名**：`tauri.conf.json` 的 `bundle.macOS.signingIdentity` 必须是 `"-"`。不设时 Tauri 不做 codesign，链接器留下的是 `linker-signed`（`Sealed Resources=none`、Info.plist 未绑定），`codesign --verify` 失败；别的 Mac 从 dmg 装完 Gatekeeper 报「已损坏」（不是「未验证开发者」）。签名后仍提示「无法验证开发者」，可右键打开或 `xattr -dr com.apple.quarantine`；要免提示只能 Developer ID + 公证。
- **打包版必须带麦克风 entitlement**：`src-tauri/Entitlements.plist` 里的 `com.apple.security.device.audio-input`，通过 `bundle.macOS.entitlements` 挂上。Tauri 打包签名带 hardened runtime（`codesign -dv` 打出 `flags=0x10002(adhoc,runtime)`），**缺这条 entitlement 时 tccd 根本不弹授权框**，直接打 `Policy disallows prompt … access to kTCCServiceMicrophone denied`，录音拿到的是精确零流；系统设置里手动开开关、`tccutil reset Microphone` 都救不回来。
  - 表现和下面那条「ad-hoc 丢权限」一模一样（能开始录、没字、提示 `record.empty`），但原因不同 —— **先看系统日志再动手**：`log show --last 5m --predicate 'subsystem == "com.apple.TCC"' | grep -i voicenotes | grep -i microphone`；看到 `entitlement … is missing` 就是这个（补 entitlement 重新打包），看到 `AUTHREQ_PROMPTING` + `authValue=2` 才是真授权成功。
  - dev 模式的 `target/debug/voice-notes` 是 linker-signed（无 runtime 标志），不需要这条 entitlement —— 所以「开发时好好的、打包就没声」基本就是它，别去怀疑录音代码。
- **ad-hoc 签名 = 每次重新打包都会丢麦克风权限**：ad-hoc 签名的 designated requirement **就是代码签名哈希本身**（`codesign -d -r- VoiceNotes.app` 打出 `# designated => cdhash H"…"`），macOS 的 TCC 拿它匹配授权记录 —— 重新打包 → cdhash 变 → 记录对不上 → **静默拒绝，麦克风喂精确的零流**。
  - 表现：能开始录音、有时长统计，但识别不出东西；**系统设置 → 麦克风里 VoiceNotes 的开关还是「开」**（那条绑的是上一个构建）。别被这个误导 —— 开关开着 ≠ 当前构建有权限。
  - 恢复：终端跑 `tccutil reset Microphone com.github.bytes-astra.voicenotes`，再启动应用重新授权。
  - 让授权跨构建存活：给应用一个**稳定的签名身份**（自签名证书即可：`openssl req -x509 ... -addext "extendedKeyUsage=codeSigning"` → PKCS#12 导入钥匙串 → `security set-key-partition-list`；或 Apple Development 证书），`bundle.macOS.signingIdentity` 填它 —— designated requirement 从 `cdhash …` 变成 `certificate leaf = H"…"`，跨构建不变。
  - 换签名后第一次仍要允许一次，之后那份授权跟着证书走、跨构建有效。**（2026-10-23 更正）** 以前记的「换签名也不弹框、只能手动开开关」是缺 `audio-input` entitlement 造成的，补上后 `tccutil reset` + 重启就会正常弹框（系统日志 `AUTHREQ_PROMPTING` → `AUTHREQ_RESULT authValue=2`）。
  - 排查：日志里的电平峰值全是 `0.00000`（不是 0.005 这种底噪）就是被拒了；真麦克风静音也有底噪。
- 打包 `.dmg` 需要能调 `hdiutil`，受限沙箱里可能只产出 `.app`（`--bundles app` 可跳过 dmg）。
- Tauri **不能在 macOS 上交叉打 Windows 包**，CI 用 `.github/workflows/build.yml`（windows-latest）出 msi/nsis。
- 推 tag（`v*`）触发 CI，同时产出 macOS（dmg）和 Windows（msi/exe），在 Artifacts 下载。
- Windows 代码侧：cpal 采样格式已覆盖 F32/I16/U16/I32；**没有 hiddenTitle**（macOS 专属），用原生标题栏；`body.macos` 不生效时侧栏顶部高度跟着内容走（写死 12px 会切掉 30px 高的品牌行）；`bundle.windows.webviewInstallMode = downloadBootstrapper`。菜单只在 macOS 构建（`#[cfg(target_os = "macos")]`）。

## 已知坑

- 首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 拉预编译静态库；GitHub 不通用 `SHERPA_ONNX_ARCHIVE_DIR=/path/to/dir`（放 `sherpa-onnx-v1.13.8-<os>-<arch>-static-lib.tar.bz2`）。
- **避免 webview 重绘时闪白**：`html` 铺上 `var(--bg)`，窗口底色跟主题走（`syncWindowBackground()` → `setBackgroundColor`，需要 `core:window:allow-set-background-color` 权限）。只给 `body` 上色不够。
- 拖动窗口靠 `data-tauri-drag-region="deep"`（`.sidebar-top` 与 `.main-head`，按钮自动排除）+ **`core:window:allow-start-dragging` 权限** —— 这权限不在 `core:default` 里，删了窗口就拖不动。
- macOS 侧栏顶部用 `body.macos .sidebar-top { height: 78px }` 给红绿灯留位，全屏时红绿灯会自动隐藏但占位**始终保留 78px**（否则进出全屏内容会跳）。Windows 没有这个占位。
- macOS 未签名构建的麦克风权限可能要手动勾选；正式签名包才弹标准授权框。
- 端到端冒烟：非流式 5.6 s 中文 ≈ 200 ms（RTF 0.04）；流式同段音频首字约 1 个分片、整段 0.5 s 内出完，无标点。
