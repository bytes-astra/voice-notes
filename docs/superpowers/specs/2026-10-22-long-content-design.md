# 长时间内容支持 —— 设计

日期：2026-10-22
状态：待批准

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

## 用户已做的决策

- 整段模式改成「**边录边按句识别**」
- **去掉 hybrid（流式校正）功能**
- 录音**不设上限**，只做提示
- 长内容**主要是中文**（流式可作主力）

## 目标

- 录音时长不设上限，界面不自动停止；只提示已录制时长。
- 内存**恒定**，不随录音时长增长（两条模式都是）。
- 文件转写不限时长，不再静默截断。
- 两种模式都边录边出字。
- 去掉 hybrid 及其设置项。

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

### 1. 采集缓冲改成消费式

`audio::Recorder` 的 `samples: Arc<Mutex<Vec<f32>>>` 改为：

- 新增 `Recorder::take_new() -> Vec<f32>`：取走并清空全部未消费样本（`std::mem::take`）。调用方拿到的永远是「新增的那部分」。
- 保留安全阀：未消费样本数超过 `MAX_PENDING_SAMPLES`（60s @ 48kHz = 2_880_000）时，丢弃最旧的样本，并发一条 `audio:error` 事件提示「识别跟不上，已丢弃部分音频」。防止 ASR 线程卡死时缓冲无限增长 —— 但丢样本等于丢内容，必须让用户知道，不能静默。
- `append` 不再自己判断 600s 上限（上限改为上面那个安全阀）。

这样两个模式都不再持有历史音频：**内存恒定**。

### 2. 两种模式统一成「ASR 线程消费缓冲」

现状：流式有 ASR 线程（`run_streaming`），整段是停止时同步一次性转写（`transcribe_samples`）。

改法：整段也在 `start_recording` 时起一个 ASR 线程，循环：

```
loop {
    chunk = recorder.take_new()                  // 新采样（设备原始率）
    samples = resample(chunk)                    // → 16k
    整段：喂 VAD → 切出句子 → Engine::transcribe(句子) → 发 stream:segment
    流式：喂现有 StreamingSession（逻辑不变）
    if stop { VAD.flush() 剩余句子；break }
}
```

- 整段的 VAD 配置与现状一致：`threshold 0.5` / `min_silence_duration 0.5` / `min_speech_duration 0.25` / `max_speech_duration 30.0`，`VoiceActivityDetector::create(cfg, 32.0)`。
- 每识别完一句就发 `stream:segment`，payload 的 `committed` 是累计文本 —— 前端已有处理（`src/app.js:1200`），与模式无关，整段模式不需要新的前端事件逻辑。
- 停止时：置 stop 标志 → 线程把 VAD 里剩余的句子 flush 出来识别 → join → 返回 `Transcript`。
- 引擎：`Engine`（SenseVoice）由 ASR 线程独占（`Arc<Engine>` 放进线程）。语言或 ITN 在录音中不可改（语言芯片在录音中已 disable），所以引擎在开始录音时构建一次即可。

**需要新增的接口**（现状没有，文档原先没写）：

- `Engine::recognize(samples) -> String`：**不跑 VAD** 的直接解码。
  原因：现状 `Engine::transcribe` 内部自带 VAD，而新路径已经在外层用 VAD 切好句子，再走一遍是双重 VAD（句子内部还会被再切一次，边界行为会变）。
- `Transcript` 的累计口径：逐句识别没有「一次性」的 `duration_seconds` / `elapsed_ms`。
  新路径自己累加：`duration_seconds` = 各句音频时长之和，`elapsed_ms` = 各句推理耗时之和。历史记录用这两个字段，别丢。
- `start_recording` 必须检查整段模型是否就绪：现状是**停止时**才检查（`transcribe_samples` 里报「模型未就绪」），改成边录边之后，模型没就绪就没法边录边识别，必须在开始录音前拦下来并提示。流式模式已有这个检查（`models::resolve_streaming`）。


### 3. 删除 hybrid

删除范围：

- `settings.hybrid` 字段、`set_hybrid` 命令、`Status.hybrid`。
- `correct_streaming`（`src-tauri/src/lib.rs:772`）及其调用点。
- `src-tauri/src/hybrid.rs` 整个模块、`src-tauri/tests/hybrid.rs`。
- 前端：设置页开关与状态文案、`src/app.js` 的绑定、12 条 i18n 文案（zh + en）。

连带影响：

- **标点模型（CT-Transformer）只对流式模式生效**了 —— 现状下整段模式的标点模型只有 hybrid 复核一条路径在用。设置页文案要改成「流式模式下给句子补标点」；整段模式靠 SenseVoice 自带 ITN 标点。
- 流式模式失去「结果异常时用整段模型兜底」。替代路径：用户自己切到整段模式重录一次。UI 上要在设置里说明这一点。

收益：流式模式不再需要保留完整音频 —— 这是内存恒定的关键。

### 4. 文件转写不限时长

现状：前端 `decodeAudioData` → 重采样 16k → `Math.min(duration, 600)` 截断 → 整条 base64 传给后端（1 小时音频 ≈ 153MB 字符串）。

改法：前端按 **30 秒切片 + 2 秒重叠**逐片调用后端，边转边把文本追加到结果区：

- 每片 base64 约 1.3MB，可接受；逐片调用天然限流。
- 相邻片重叠 2 秒 → 重叠部分的文本会重复 → 拼接时按「最长公共后缀 / 前缀」删除重复（官方 `long_audio_no_vad.py` 同款做法）。
- 显示进度「第 N/M 片」。
- 后端每片仍走现有的整段管线（VAD + 逐句 SenseVoice），语义不变。

### 5. 前端

- 删除 600s 自动停止（`src/app.js:252`）。
- 录音中显示已录制时长；不再有「已达上限」路径。
- 文件转写：进度条 + 分片计数。
- 设置页：删「流式校正」开关；标点模型说明改成只对流式生效。
- 两种模式的实时出字都靠现有 `stream:segment` / `stream:partial` 事件。

### 6. 文档

- README：删掉「单次录音上限 10 分钟」，改成不限时长 + 内存恒定；删掉 hybrid 相关描述；更新两种模式的说明。
- AGENTS.md：记录新的架构（消费式缓冲、统一 ASR 线程）、去掉 hybrid 约定。

## 验证方式

**单元测试（Rust）**

- `take_new` 的消费语义：取走后缓冲为空，再取返回空。
- 安全阀：灌入超过上限的样本，最旧的被丢弃且有警告。
- 切片重叠去重：给「今天天气很好|气很好我们出去」这样的重叠文本，拼接后重复部分只出现一次。
- 整段「边录边识别」与「整条一次识别」结果一致：用现有 fixtures（`src-tauri/tests/fixtures/zh-tail*.wav`）跑两条路径，比对文本。

**真模型测试**

- `asr_smoke` 增加一条：把 5 段 fixtures 拼成一条长音频，走新的边录边路径，断言文本非空且句子数合理。

**手工**

- 录 15 分钟（超过原 10 分钟上限）：出字正常、内存不涨（活动监视器看 footprint）。
- 拖 30 分钟音频：分片转写完整、进度正常。
- 两种模式都验证「边录边出字」。

## 风险

| 风险 | 应对 |
|---|---|
| 整段「边录边识别」与原「整条 VAD 分段」文本不一致 | VAD 本身是流式的，理论上等价；用 fixtures 做等价性测试 |
| 单句推理阻塞 ASR 线程（30s 句子 ≈ 1.2s） | 推理速度远快于实时（25×），不会积压；采集侧继续跑，缓冲短暂增长后被消费 |
| 去掉 hybrid 后流式异常结果没有兜底 | 在设置里写明替代路径（切整段模式） |
| 前端切片边界的切字 | 2 秒重叠 + 文本去重；官方同样承认固定边界会改变切点附近结果 |

## 待用户确认

- **「整段」这个名字**：它不再表示「整条一起转」，而是「用 SenseVoice 逐句识别」。是否改名（例如「精听 / 流式」）。
  建议**不改**：「整段 / 流式」区分的是**模型性质**（SenseVoice 是整段模型、Zipformer 是流式模型），不是 UI 行为。即便改成逐句调用，模型层面它仍是「整段模型」，改名反而增加理解成本。真正变化的是「整段模式现在也边录边出字」，这应该在 UI 上体现（例如提示语、结果区实时刷新），而不是改模型名。

