# 后端约定（VoiceNotes）

后端是 Rust（Tauri 2）。模块分层、命令与运行时、事件契约、落盘、日志、音频采集 —— 改后端前先读这里。

工程约定的总索引在 [`../AGENTS.md`](../AGENTS.md)；验证手段与基线在 [`HOW_TO_VERIFY.md`](HOW_TO_VERIFY.md)。

## 后端分层

- **lib.rs 只放命令层**：新逻辑先找对应模块放 —— `record.rs`（录音编排）、`engine.rs`（引擎与状态）、`asr.rs`（识别会话）、`audio.rs`（采集与重采样）、`decode.rs`（解码）、`models.rs`（模型下载）、`output.rs`（落盘）、`settings.rs`（设置）、`log.rs`（日志）、`menu.rs`（macOS 菜单）。命令本身保持薄：一个命令别同时干两件事（录音按模式拆成 `start_streaming_recording` / `start_offline_recording` 两个独立命令）。
- **`record.rs` 不认识 Tauri**：结果通过 `emit` 回调交出去，所以它能被单测直接调用。`AppHandle` 不进 `record.rs` —— 实在要发事件就在 lib.rs 那层包一下。
- **名字别撞**：`record.rs` 是「录音怎么跑」，`asr.rs` 里的 `OfflineSession` / `StreamingSession` 是「识别会话」，两回事 —— 所以前者不叫 session.rs。两种录音共用 `RecordingRun { stop, discard, rx }`，停止统一走 `finish_recording`：麦克风故障也要先等 ASR 收尾，不能当取消丢文本。
- **两个槽一起登记**（`record.rs` 的 `register_slots`）：采集器槽（`recorder`）与会话槽（`stream` / `offline_session`）必须**一起**写 —— 只写采集器不写会话，识别线程就永远收不到 stop（线程泄漏、麦克风不关、语言/ITN 永久锁死）。函数先拿齐两把锁再写，任一把锁被 poison 时两个槽都不变；锁序固定 采集器 → 会话。登记失败时，已经起起来的识别线程要靠 `stop` / `discard` 标志让它退出。
- **单元测试不进源文件**：测试放同目录子文件夹，`src/asr.rs` 里写 `#[cfg(test)] mod tests;`，实现放 `src/asr/tests.rs`（一个模块有多个测试组就是 `src/asr/gap_tests.rs` 这样另起文件）。crate root 例外 —— `lib.rs` 的子模块落在 `src/` 根下（`src/command_tests.rs`）。**拆出去仍是单元测试**：`use super::*` 照样能摸到父模块的私有项，`cargo test --lib` 照样跑得到，门禁不用改。别为了「看起来整齐」搬到 `src-tauri/tests/` —— 那是集成测试，只碰得到 `pub` 项，而且 `cargo test --lib` 不再覆盖它。

## 命令与运行时

- **命令参数名用 camelCase**，如 `audioBase64`、`fileName`。
- **推理是同步阻塞的**，一律放进 `tauri::async_runtime::spawn_blocking`，别阻塞 async 执行器。
- **引擎按「身份」缓存**：整段的身份是 `(模型目录, language, itn)`，流式是 `hotwords`（`engine.rs` 的 `OfflineEngineKey` / `StreamingEngineKey`）。任何一项变了就重建 —— 原来只看 `(language, itn)`，换模型目录会被静默吞掉。想「切语言不重载」得走 C API 的 `setConfig`，Rust 绑定没暴露。录音中语言和 ITN 前后端都锁定；自定义模型目录变更由 `set_model_dir` 清缓存。
- **装引擎要锁内读设置**（`engine.rs` 的 `install_engine`）：拿引擎槽的锁 → **在锁内**读设置 → 比身份 → 复用或重建。设置是在锁外被改的（`set_model_dir` / `set_hotwords`），读设置和装引擎之间只要有缝，旧快照就会把新设置顶掉。`load_engine` 直接返回 `Arc<Engine>`，不要再加一个「二次取锁」的助手（async 路径上阻塞等引擎锁会把整个执行器卡住）。
- **文件解码在后端**：`decode::decode_to_16k()` 逐块解码，内存恒定。symphonia 锁 **0.6**（要求 Rust 1.85；本项目 MSRV 以整棵依赖树为准，声明 **1.90**）。

## 事件契约

- **事件名只出现在 `src/events.js`**：`audio:level`、`audio:error`、`model:progress`、`engine:state`、`stream:partial`、`stream:segment`、`streaming:state`。改名字要同步前端。
- **会话 ID**：录音/文件都先 `reserve_session`，启动/文件命令携带 `sessionId`，分段、partial、采集错误带同一个 ID；前端只接收当前会话，超时同时置 discard 阻止旧线程继续发射。
- **`audio:error` 的载荷是对象**：`{ code, detail?, seconds?, fatal, sessionId? }`。`code` 是字典 key，`fatal: false` 表示「提示但不中断」（比如丢掉音频的缺口提示 —— 录音要继续）。致命 `record.micError` 由前端停止录音并保存非空部分结果；用户主动 Esc 才走取消并清空，其他致命错误仍取消。
- **停止时的采集故障走 `Transcript.warning`**：`finish_recording` 先让识别线程收尾、保住非空转写，再由 `stop_recording` 把 `record.micError` 写进 `Transcript.warning`；正常转写不带这个字段（`skip_serializing_if` 保证载荷不变）。前端据此提示「麦克风发生异常」而不是「完成」，也不丢结果。
- **事件载荷是增量的**：`stream:segment` 只带新句子，`stream:partial` 只带中间结果。累计全文在长时间会话里是 O(n²) 的传输与重绘。前端 `core.js` 的 `joinText()` 与后端 `asr::join_segments()` 规则必须一致（中日韩之间直接连，其余加空格）—— `npm run check:join` 守着这条。
- **计时器只有一个写者**（`els.timer`）：前端 `recorder.js` 的 `startTimer` 用 `state.startedAt` 起算，**后端不再报 elapsed**（`audio:level` 载荷里没有这个字段）。两个时钟的原点差几毫秒（后端从采集线程起算，前端从命令返回起算）—— 显示值会在两个数之间来回跳，看着像秒数在退。
- **结果框的实时追加不能夺走用户的光标**：整值写（`el.value = …`）会把光标推到末尾、还清空撤销栈。`result.js` 的 `writeResultText` 分两种：后缀扩展走 `execCommand('insertText')` 增量插入（插完还原选区），其余情况整值写但把光标放回原位。守卫是 UI 断言「实时追加后光标还在原处」。
- **云端连接列表重绘不能弄丢焦点**：行按 id 复用（`status.js` 的 `cloudRows`），只改内容不换节点；`syncRowOrder` 只在顺序真变了时才动节点（移动节点会让落在里面的焦点丢失）；行内的名称 / 明细是不可聚焦的 `div`，**mousedown 的默认行为会把焦点移走** —— `app.js` 拦下它。
- **拖拽用 Tauri 的 `onDragDropEvent`**：Tauri v2 默认拦截系统拖放，HTML5 `drop` 事件在真应用里不触发。`tauri.conf.json` 的 `dragDropEnabled` 必须保持 `true` —— 关掉它 wry 就不装拖放 handler，`onDragDropEvent` 永远不触发，拖拽转写直接变成死代码（而且没有 HTML5 兜底）。
- **模型下载是两条独立的线**（可以同时下）：各自有 `downloading` / `cancel_download` 标志（`engine.rs` 的 `AppState` 里 `downloading_streaming` / `cancel_download_streaming`）。共用一个标志的话，整段在下载时点流式下载会直接报 busy 失败。
  - 进度事件是同一个名字（`model:progress`），载荷里带 `kind`（`"offline"` / `"streaming"`，由 `lib.rs` 的 `EventSink` 塞进去）—— **前端必须按 kind 分流**，别看「当前是不是在下载」那种状态，否则两个面板会互相显示对方的进度（看起来就像流式在重复下载整段模型）。
  - `cancel_download` 命令带 `kind` 参数，指明取消哪一个。

## 落盘与目录

- **转写结果的默认输出目录用 ASCII**（`settings::default_output_dir` → `~/Documents/VoiceNotes`）：中文目录名在终端、脚本、跨平台工具里处处要转义。旧版的中文目录「语音转文字」由 `settings::migrate_output_dir()` 在启动时一次性改名（只在新目录不存在时才搬，不覆盖已有数据）。
- **打开文件夹用自己的 `open_output_dir` 命令，不用 opener 插件**：插件的 `opener:allow-open-path` 权限是「without any pre-configured scope」—— 不配 scope 的话任何路径都被拒（报 "Not allowed to open path"）；而保存目录是用户可改的（`settings.output_dir`），静态 scope 盖不全。后端自己解析出真实路径再 `open`/`explorer`/`xdg-open` 更省事（应用自己的命令不需要 ACL 权限，`__allow_command` 直接放行）。

## 日志

**逐句都要记日志**：整段录音记 `识别出一句：起 → 止，N 字`；流式（`record.rs` 的 `run_local_streaming`）记 `流式识别出一句` / `流式收尾识别出一句`，文件转写（`lib.rs` 的 `transcribe_file`）记 `文件识别出一句` —— 只记边界和字数、不记内容，配合总时长就能算出哪一段没被覆盖。**缺口占位符不算句子**（那是「丢了音频」的标记，不是识别结果）。

**日志一直开着，不需要用户配置**：`log.rs`（`log::info` / `warn` / `error`）写到系统标准日志目录 —— macOS `~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，Windows `%LOCALAPPDATA%\com.github.bytes-astra.voicenotes\logs\voice-notes.log`；**超 1 MB 或跨天**轮转一次，历史留成带时间戳的文件名（`voice-notes.log.2026-11-26-14-03-22`）并保留最近 **5 份**，同时镜像 stderr。没有开关、没有设置页 —— 用户反馈问题直接把这个文件发过来。前端那边用 `client_log` 命令写进同一个文件（`src/tauri.js` 的 `logEvent()`），这样一份文件就是完整的一份记录。

- **文件名带时间戳是为了让「哪段时间出的问题」能直接引用**：用户说「11 月 24 日那次」，维护者直接拿对应的那份，不用在一堆 `.log.1` 里猜。
- **跨天也轮转**（不只超 1 MB）：否则一天没写满 1 MB 时日志全挤在一个文件里，没有可单独拿出来的「时间段」。
- **判定抽成纯函数** `should_rotate(size, modified, now)` —— 跨天靠**当前日志文件最后一次写入的时间**判定（应用重启后依然成立）；拿不到那个时间就只看超限，不误轮转。
- **清理在轮转之后立刻做**，删超出份数的最旧文件；旧格式的 `voice-notes.log.1` 解析不出时间戳 → 排序时当作最旧 → 升级后自然被淘汰。
- **撞名不覆盖**：同一秒轮转两次时给历史文件加 `-1` / `-2` 计数器，既有文件的内容不会被覆盖。


## 音频采集

- **采集缓冲是消费式的**（`audio.rs`）：`audio::SampleQueue` 的 `take_new()` 取走即释放，内存恒定（录 1 小时和 1 分钟占一样多）。**电平表走独立的 `LevelRing`**（2048 样本）—— 采集缓冲没有历史了，音量表去读它会恒读 0。
- **安全阀**（`audio.rs`）：未消费采样超过 `MAX_PENDING_SECONDS × 设备实际采样率`（60 秒，采样率不能写死 48k）时丢最旧的，并用 `audio::gap_marker()` 在结果里留可见缺口，不能静默丢。
- **停止录音时 `Recorder::stop()` 只关流、不取采样**（`audio.rs` 的 `stop`；收尾取样在 `record.rs`）：句尾最后一段要留给识别线程收尾时 `take_new()`。主线程先取走会切掉最后一个字（离线/流式都中招）。
- **麦克风权限在界面渲染完之后申请**：macOS 是第一次碰输入设备才弹权限框，而「列出输入设备」就会触发。流程：启动 → 界面画完（约 0.9 s）→ `list_input_devices` → 弹框（这一步同时把设备列表填好）。
- **启动时不列设备**：弹框会打断用户，实测弹框期间窗口是空白的。点麦克风时仍先 `warm_up_mic` 探权限，用户拒绝过要能给出明确报错。
- **输入设备可选**：`list_input_devices` 列出 cpal 输入设备（默认设备带标记），`set_input_device` 存 settings（None = 系统默认），`audio::start` 按名字挑，挑不到退回默认。下拉框每次展开重新列一次（`select.js` 的 `onBeforeOpen`），中途插拔也能看到。**没有名字的设备直接不列**（`audio::device_entry`）：按名字挑不中它，列出来只会让界面出现一条选不中的项 —— 设备名是身份，不能拿展示文案（本地化回退标签）当身份传给后端。
- **输入电平自动增益**（`audio::Agc`）：在**重采样之后、VAD 之前**提电平；电平够就一个样本不动，只救偏小的。参数、三条接入路径与实测见 [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md)「静默丢音频」。
- 录音**没有时长上限**。

## Rust 侧测试与探针

`cargo test --lib`（纯逻辑），以及 `src-tauri/tests/` 下的 `asr_smoke`（整段真模型，需 `VOICE_NOTES_MODEL_DIR`）、`streaming_smoke`（流式，需 `VOICE_NOTES_STREAMING_MODEL_DIR`）、`streaming_endpoint_time`（流式时间区间，真模型 + `--ignored`）、`decode_smoke`（后端解码全链路）、`quality_probe`（质量探针，只打印不断言）、`quality_gate`（质量门禁，**带阈值断言**：整段/流式的 CER 与 RTF）、`memory_probe`（内存门禁：常驻内存不随录音时长增长）。长录音四探针（`vad_long_probe` / `offline_long_probe` / `streaming_long_probe` / `resample_probe`）**打印 + 断言**：超阈值就是回归。其余探针（`gain_probe` / `noise_probe` / `mic_probe` / `level_probe` / `quiet_pipeline_probe`）的用途与跑法见 [`HOW_TO_VERIFY.md`](HOW_TO_VERIFY.md)；完整清单以 `src-tauri/tests/` 目录为准。
