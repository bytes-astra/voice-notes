# 长时间内容支持 —— 设计

日期：2026-10-22
状态：已批准（用户逐项确认）

## 意图与背景

用户要求：**支持长时间的内容** —— 录音和文件转写都不限时长，内容连续、不丢。

现状有两个限制：

1. **单次录音 10 分钟**：前端计时到 600s 自动停止录音（`src/app.js:252`），后端采集缓冲 cap 也是 600s（`src-tauri/src/audio.rs:15,278`）。文件转写还有 `Math.min(buffer.duration, MAX_SECONDS)`（`src/app.js:857`）静默截断。
2. **整段模式单句 30 秒**：SenseVoice 单次输入上限（官方限制），由 Silero VAD 的 `max_speech_duration: 30.0`（`src-tauri/src/asr.rs:73`）强制切段。

已经确认的关键事实（决定了整个方案）：

- **整段模式本来就是分段识别**：录完 → 整条音频喂 VAD → 切成 ≤30s 的段 → 逐段送 SenseVoice → 拼接。30 秒限制、逐段拼接、跨段丢上下文，这些**在现状里已经存在**；唯一的区别是所有句子都在录音**结束之后**才被识别。
- **SenseVoice 在 sherpa-onnx 里是无状态的**：逐次独立调用，不跨次共享上下文，也不输出 token 级时间戳。因此「跨段上下文续接」在整段模式里做不到，只能拼接。
- **Zipformer transducer 是有状态的**：逐帧解码，没有单次输入长度上限，天然连续。
- **采集缓冲在流式模式只增不删**：`take_new`（`src-tauri/src/lib.rs:402`）只推进游标，从不释放已消费的样本，内存会一路涨到约 115MB（48kHz f32，600s）。
- **文件转写现在是整条解码进内存**：`decodeAudioData(await file.arrayBuffer())`（`src/app.js:856`），1 小时 48k 立体声 float32 ≈ 1.4GB，WebView 大概率直接失败。前端切片只解决 IPC 体积，不解决解码内存。

## 用户已做的决策

| 决策 | 选择 |
|---|---|
| 整段模式 | 改成「边录边按句识别」 |
| hybrid（流式校正） | 去掉**复核**，但保留「结果异常」的**检测提醒** |
| 录音时长 | 不设上限，只提示 |
| 长内容语言 | 主要是中文（流式可作主力） |
| 文件解码 | **搬到后端 Rust**（symphonia，纯 Rust，无需 ffmpeg） |
| 整段出字粒度 | 保持 30s 一句，**加「正在听」指示**（不依赖 VAD） |
| 识别跟不上时 | 丢最旧的音频，**在文本里标记缺口** + 弹提示，录音不中断 |
| 「整段」命名 | **不改**（区分的是模型性质，不是 UI 行为；改名还要迁移 `settings.mode`） |

## 目标

- 录音时长不设上限，界面不自动停止；只提示已录制时长。
- 内存**恒定**，不随录音时长增长（两条模式都是）。
- 文件转写不限时长，不再静默截断；mp4/mov 在 Windows 也能转（顺带修掉）。
- 两种模式都边录边出字。
- 去掉 hybrid 的**复核**行为，保留质量**检测提醒**。

## 非目标

- 不改 SenseVoice 的 30 秒单次输入上限（官方限制，超了精度掉）。
- 不做跨段上下文续接（SenseVoice 无状态，流式不需要）。
- 不改「流式只支持中文」的限制。
- 本次不引入录音文件留存 / 回放功能。

## 约束

- sherpa-onnx 1.13.8：`OfflineRecognizer`、`OnlineRecognizer`、`VoiceActivityDetector`、`LinearResampler`。
- `OfflineRecognizer` / `OnlineRecognizer` 都实现了 `Send + Sync`，引擎可以放进 ASR 线程。
- Tauri 2：命令参数 camelCase；阻塞推理不放 async 执行器。
- 现有事件名不改：`audio:level` / `audio:error` / `stream:partial` / `stream:segment` / `streaming:state` / `engine:state`。
- 前端是原生 JS，无构建步骤；所有文案走 i18n 字典。

## 设计

### 1. 采集缓冲改成消费式 + 独立的电平环

`audio::Recorder` 的 `samples: Arc<Mutex<Vec<f32>>>` 改为：

- 新增 `Recorder::take_new() -> Vec<f32>`：取走并清空全部未消费样本（`std::mem::take`）。调用方拿到的永远是「新增的那部分」。
- **电平表要独立**：`peak()`（`audio.rs:354`）现在读的是同一个缓冲的尾巴，缓冲改成消费式后音量表会恒读 0。改法：`append` 同时写进一个独立的 2048 样本环缓冲（`levels`），`peak()` 改读它。
- 安全阀：未消费样本数超过 `MAX_PENDING_SECONDS * sample_rate`（60 秒，按**设备实际采样率**算，不能写死 48kHz 的样本数）时，丢弃最旧的样本，**在转写文本里插入缺口标记**（如 `……（此处约 3 秒未识别）……`），并发 `audio:error` 事件弹提示。录音不中断，但缺口是显式可见的，绝不静默丢。

两个模式都不再持有历史音频：**内存恒定**。

### 2. 两种模式统一成「ASR 线程消费缓冲」

现状：流式有 ASR 线程（`run_streaming`），整段是停止时同步一次性转写（`transcribe_samples`）。

改法：整段也在 `start_recording` 时起一个 ASR 线程，循环：

```
loop {
    chunk = recorder.take_new()                  // 新采样（设备原始率）
    samples = resample(chunk)                    // → 16k
    整段：喂 VAD → 切出句子 → Engine::recognize(句子) → 发 stream:segment（增量）
    流式：喂现有 StreamingSession（逻辑不变）
    if stop { VAD.flush() 剩余句子；break }
}
```

- 整段的 VAD 配置与现状一致：`threshold 0.5` / `min_silence_duration 0.5` / `min_speech_duration 0.25` / `max_speech_duration 30.0`，`VoiceActivityDetector::create(cfg, 32.0)`。
- **两种模式的 VAD 参数不同是刻意的**：流式侧 `max_speech_duration: 20.0`（`asr.rs:271`），旁边还有 transducer 自己的 `rule3_min_utterance_length = 20.0` —— 流式要短句、响应快；整段侧 30s 是因为 SenseVoice 在 ≤30s 区间最准。别当 bug「修」成一致。
- **出字粒度保持 30s**：句子由 VAD 在静音 0.5s 处切出，连续说话最长 30s 才出一句（SenseVoice 的擅长区间，识别最准）。为免用户以为卡死，界面上加一个**不依赖 VAD 的「正在听…」指示**（跟随实时音量/计时器）。
- 停止时：置 stop 标志 → 线程把 VAD 里剩余的句子 flush 出来识别 → join → 返回 `Transcript`。
- 引擎：ASR 线程启动时从 `state.engine` 缓存**取走** `Engine`（SenseVoice），停止时**还回**缓存 —— 文件转写随后可以复用已加载的模型，不必重新加载 200MB。
- 语言 / ITN 在录音中不可改（语言芯片录音中已 disable），引擎在开始录音时构建一次。

**需要新增的接口**（现状没有）：

- `Engine::recognize(samples) -> String`：**不跑 VAD** 的直接解码。
  原因：现状 `Engine::transcribe` 内部自带 VAD，而新路径已经在外层用 VAD 切好句子，再走一遍是双重 VAD（句子内部还会被再切一次，边界行为会变）。
- `Transcript` 的累计口径：逐句识别没有「一次性」的 `duration_seconds` / `elapsed_ms`。
  新路径自己累加：`duration_seconds` = 墙钟录音时长，`elapsed_ms` = 各句纯推理耗时之和（RTF 仍然有意义）。历史记录用这两个字段。
- **两个计数器，口径必须分开**：`recorded_samples`（送进 `push` 的样本总数，**不含** `finish()` flush 的 VAD 残留）用来判断「录音太短」；`recognized_samples`（VAD 切出并识别的句子的样本数之和）用来判断「没识别到内容」。混在一起会让「录音时长」算错（VAD 残留多半是静音）。
- `start_recording` 必须检查整段模型是否就绪：现状是**停止时**才检查（`transcribe_samples` 里报「模型未就绪」），改成边录边之后必须在开始录音前拦下来并提示。流式模式已有这个检查（`models::resolve_streaming`），整段补上同样的（`models::resolve`）。

### 3. hybrid：删复核，留检测

- 删除：`correct_streaming`（`src-tauri/src/lib.rs:772`）及其调用点、`settings.hybrid` 字段、`set_hybrid` 命令、`Status.hybrid`。
- **保留**：`hybrid.rs` 缩减成一个纯检测模块 —— `is_degenerate(text, duration)` 判定「空结果 / 重复字 / 字数与时长明显对不上」。
- 触发时**只提示、不改文本**：弹一条提示「结果可能有问题（识别异常），建议切到整段模式重录」。
- 设置页：开关从「结果异常时用整段模型复核」改成「结果异常时提醒我」。

收益：流式模式不再需要保留完整音频 —— 这是内存恒定的关键。

连带影响：

- **标点模型（CT-Transformer）只对流式模式生效**了 —— 现状下整段模式的标点模型只有 hybrid 复核一条路径在用。设置页文案改成「流式模式下给句子补标点」；整段模式靠 SenseVoice 自带 ITN 标点。

### 4. 文件转写搬到后端（symphonia）

新增依赖 `symphonia`（纯 Rust 解码器，无需 ffmpeg / 系统库）。

- 新命令 `transcribe_file(path: String, language: Option<String>) -> Transcript`：后端从磁盘解码 → 16k 单声道 → **走同一条 ASR 管线**（VAD + 逐句 SenseVoice）→ 边识别边发 `stream:segment` 增量事件 → 返回最终 Transcript。
- 解码是流式的（按包读），**内存恒定**：1 小时文件不会整条进内存。
- 支持格式：mp3 / wav / flac / ogg / m4a(aac) / mp4 / mov（视频轨跳过）。**mp4/mov 在 Windows 上也能转了** —— 这是一直挂着的待办，本设计顺带解决。
- 前端：文件选择 → 拿到路径 → `invoke('transcribe_file', { path })`；实时文本和进度靠 `stream:segment` 事件；删掉前端 `decodeAudioData` 那套（含 `encodeWave` / `audioBase64` 的文件路径用法）。
- 长文件转写显示进度（第 N 句 / 已用时）。

### 5. 事件载荷改成增量

`stream:segment` 现在带 `committed`（累计全文）。多小时会话（约 1000 句）下这是 O(n²) 的 IPC 传输 + O(n²) 的 textarea 重写，webview 会卡。

改法：载荷带**新句子**（`segment` 字段）+ 该句的时间戳，前端追加到结果区；不再传累计全文。

### 6. 前端

- 删除 600s 自动停止（`src/app.js:252`）。
- 录音中显示已录制时长；整段模式加「正在听…」指示（不依赖 VAD，跟随音量）。
- 文件转写：实时文本 + 进度（第 N 句 / 已用时）。
- 设置页：hybrid 开关改成「结果异常时提醒我」；标点模型说明改成只对流式生效。
- 两种模式的实时出字都靠 `stream:segment` / `stream:partial`（增量载荷）。

### 7. 文档

- README：删掉「单次录音上限 10 分钟」，改成不限时长 + 内存恒定；更新两种模式的说明；删掉 hybrid 复核的描述；加上文件格式支持列表。
- AGENTS.md：记录新的架构（消费式缓冲 + 电平环、统一 ASR 线程、增量事件、后端解码）、去掉 hybrid 复核约定。

## 验证方式

**单元测试（Rust）**

- `take_new` 的消费语义：取走后缓冲为空，再取返回空。
- 电平环：`peak()` 在缓冲被消费后仍能读到电平。
- 安全阀：灌入超过上限的样本，最旧的被丢弃、**缺口标记被插入**、有警告事件。
- `Engine::recognize` 与 `Engine::transcribe` 在同一段音频上结果一致（证明去掉内部 VAD 没改变解码）。
- symphonia 解码：用一个真实 mp3/m4a 样本，解出的采样数与时长符合预期。

**真模型测试**

- 整段「边录边识别」与「整条一次识别」的文本等价性：用现有 fixtures（`src-tauri/tests/fixtures/zh-tail*.wav`）跑两条路径，**去掉空白与标点后比字符**（允许分句不同）。
- 缺口标记：人为让 ASR 线程停顿，验证文本里出现标记。

**手工**

- 录 15 分钟（超过原 10 分钟上限）：出字正常、内存不涨（活动监视器看 footprint）。
- 拖 30 分钟音频 / 一个 mp4 视频：转写完整、进度正常。
- 两种模式都验证「边录边出字」和「正在听」指示。

## 风险

| 风险 | 应对 |
|---|---|
| 整段「边录边识别」与原「整条 VAD 分段」文本不一致 | VAD 本身是流式的，理论上等价；用 fixtures 做等价性测试（归一化后比字符） |
| 单句推理阻塞 ASR 线程（30s 句子 ≈ 1.2s） | 推理速度远快于实时（25×），不会积压；采集侧继续跑，缓冲短暂增长后被消费 |
| 增量事件改成追加后，用户在录音中编辑结果区会被覆盖 | 录音中结果区本就是实时输出；行为与现状一致（现状是用 committed 整体覆盖） |
| symphonia 解码某些容器失败 | 解码失败要明确报错（不静默截断）；不支持的格式给出可读提示 |
| 去掉 hybrid 复核后流式异常结果没有兜底 | 保留检测提醒，引导用户切整段模式重录 |
