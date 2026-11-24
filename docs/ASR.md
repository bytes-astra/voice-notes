# 识别约定（VoiceNotes）

缺口标记的时间轴口径、整段模式、流式模式、模型与下载 —— 改识别相关代码前先读这里；云端模式另见 [`CLOUD.md`](CLOUD.md)。

工程约定的总索引在 [`../AGENTS.md`](../AGENTS.md)；验证手段与基线在 [`HOW_TO_VERIFY.md`](HOW_TO_VERIFY.md)。

## 缺口标记

识别跟不上丢掉音频时，后端在文本里插 `[[gap:秒数]]`（`audio::gap_marker`），前端渲染时用 `core.js` 的 `localizeText()` 换成 `t('record.gap', {seconds})`。后端不写中文标记。

- **丢掉的音频占真实录音时间**：两种模式统一 —— `record.rs` 的 `recorded_seconds()` = 喂入 + 丢掉（以前只算喂入，识别跟不上时录音时长会短一截）。
- **时间轴统一到真实录音时间**：`record.rs` 的 `to_real_time()` 按 `drops`（缺口位置 + 累计丢掉量）把句子时间换算到真实时间轴，缺口标记落在**缺口终点**（`gap_at()`）。实时事件流发射前也过它 —— 别让同一事件流里混两套刻度。
- **时间轴换算在秒域比较**：`drops` 与句子的 `start`/`end` 都是秒，直接比秒才精确；绕道「秒 → 下标」会在长录音上因 f32 往返差 ±1 个采样，边界压在缺口上时整句错位。区间语义 `[start, end)`：起点上的缺口要移，终点上的不移 —— 这组不对称有单测守着。
- **同一时刻缺口排在句子前面**（`asr.rs` 的 `insert_gaps()`）：缺口终点与后一句起点相同时，`sort_by` 是稳定排序、gaps 又是后 extend 的 —— 不显式让缺口优先，缺口会排到句子**后面**，文本变成「句子[[gap:3]]」。
- **两种模式同一条规则**：流式句子起点也要按丢掉的时间后移（`asr.rs` 的 `Timeline::advance_dropped()`）：洞在这句**之前**（识别器还没吃到这句的音频）→ 起点挪到洞的终点；洞在这句**里面** → 起点不动，句子的跨度自然把空档包住。不挪的话流式是「句子[[gap:4]]」而整段是「[[gap:4]]句子」。`Timeline` 独立成结构是为了脱开模型做单测 —— 真模型用例没设环境变量时会跳过，这条规则不能只靠它们守。

## 整段模式

- `OfflineSession` 用 VAD 切句、逐句 `Engine::recognize()`，边录边出字。
- **VAD 缓冲给 60 秒**（`VoiceActivityDetector::create(&vad, 60.0)`，在 `asr.rs` 的 `OfflineSession::new`）。sherpa 的 `max_speech_duration` 不是硬切，它会把 VAD 参数调严（min_silence 0.5→0.1、threshold 0.5→0.9 逼它找静音），连续说话时找不到，句子能到 1 分钟以上。给 32 秒会触发 CircularBuffer 扩容（Overflow 警告 + 整段拷贝）。
- **默认整句一次识别**（最准，`asr.rs` 的 `MAX_SINGLE_SHOT_SECONDS` / `merge_transcripts`）。SenseVoice 的「30 秒上限」实测不是硬约束。**切分阈值是 90 秒**（`MAX_SINGLE_SHOT_SECONDS`），不是官方的 30 秒 —— 只有超过 90 秒才切 30 秒窗口 + 2 秒重叠（`merge_transcripts` 合并），防编码器注意力平方级吃显存。提前切反而更差，而且会把边界上的词劈开。
- **VAD 会切掉开头第一个字**：音频开头直接是语音（没前导静音，比如 TTS、剪过的文件）时 sherpa VAD 的内部 padding 只有 ≈314 ms（「今天下午」→「天下午」）。`asr.rs` 的 `should_pre_roll` 只在「第一段 + 起点 < 0.6 s + 段前 RMS > 段 RMS × 0.3」时把起点回扩 0.4 s。回扩保持**带门控** —— 无门控回扩会把开头噪声当语音（真人样本多出「您好」）。
- **ITN 自带标点**（ITN 开关在 `engine.rs` 的 `Engine::new`）：开 ITN 的 SenseVoice 自己就输出标点（关 ITN 才是纯文本）。标点模型只用于流式，叠在整段上会变成「。。」。
- **短句合并**（`asr.rs` 的 `SegmentMerger`）：VAD 切出来的句子短于 3 秒（`MERGE_MIN_SECONDS`）不立刻定稿，等下一句来了**在音频层**合成一句再识别 —— 短句单独送 SenseVoice 时上下文太少，容易把词认错。两段之间隔超过 2 秒（`MERGE_MAX_GAP_SECONDS`）就不合并，否则「说完停顿十秒再开口」会被连成一句。**录音和文件转写两条生产路径都走 `OfflineSession`**，所以行为一致；`Engine::transcribe`（只给测试/探针用）也接了同一个合并器，否则两边结果对不上。
- **VAD 参数整段和流式必须一致**：两边都走 `engine.rs` 的同一个 `vad_config()` 构造（`threshold` / `min_silence_duration` / `min_speech_duration` / `window_size` 等共有参数只写一份，改一边忘了另一边 → 同一段音频在两个模式里切出不同数量的句子）。**唯一允许不同的是 `max_speech_duration`**：整段 30 秒（用来切句）、流式 20 秒（门控的耐心上限）—— 两条路径用它的地方不同，**不要统一**。`engine/tests.rs` 的 `两模式_vad_参数一致` 守着这条。

## 流式模式

- **分片恒为 200 ms（3200 样本），且只在 `is_ready()` 为真时 `decode()`**（`asr.rs` 的 `StreamingSession`）。改小分片或无条件 decode 都会让 sherpa-onnx 特征缓冲越界直接 abort（`features.cc GetFrames`）。`start_streaming_recording` 先 `ensure_streaming_engine` 再起采集（`lib.rs`）。
- **收尾补 300 ms 静音**（`TAIL_SILENCE_SAMPLES`，在 `asr.rs`）：解码器需要尾部上下文才吐出最后一个字，不补就丢句尾（「产品评审会」→「产品评审」）。补的静音不计入时长。
- **VAD 门控**（在 `record.rs` 的 `run_local_streaming`）：Silero 判到静音就 `force_end()` 提交并 reset，静音段不喂模型（否则 transducer 会在噪声上编出重复字）；起音前留 0.4 s 预录避免切字头。
- 流式模型是独立的一组 assets（`models.rs` 的 `STREAM_*`：`streaming-zipformer-zh-int8-2025-06-30` + `silero_vad.onnx`），只覆盖中文；选粤/日/韩时前端自动切回整段。**流式的 `silero_vad.onnx` 与整段的是同一个 v5 文件**（`STREAM_VAD` 和 `VAD` 指向同一 commit 下的 `silero_vad_v5.onnx`）—— 两种模式的切句边界要一致。
- **流式结果就是最终结果**（不要用 SenseVoice 覆盖它，这个功能已删）：实测流式明显更准，而且覆盖要保留全部音频，和「不限时长、内存恒定」直接冲突。
- **流式文本要从 token 列表重建，别用 sherpa 的 `text` 字段**（`asr.rs` 的 `text_from_tokens`）：流式模型词表只有 1426 个汉字，表外的字（番、茄、腩、苹、莓、酪……）只能拆成**字节 token** `<0xE7><0x95><0xAA>` 拼；sherpa 的符号表把 `<0xE7>` 还原成**裸字节**放进 `text`（`symbol-table.cc` 的 `is_bpe_with_byte_fallback_` 分支），切分点落在半个字上时 `text` 就是非法 UTF-8，sherpa 自己的 `Utf8ToUtf32()` 会把它**固化成 U+FFFD** —— 界面上就是「�」。**token 列表是无损的**：从它重建，末尾没解完的半个字先扣住、等下一帧补齐。`clean()` 里另有兜底（丢掉 U+FFFD 并记 WARN）。回归测试 `streaming_smoke::keeps_out_of_vocab_chars_intact` + fixture `tests/fixtures/zh-oov.wav`（macOS TTS 造的「番茄牛腩。苹果派要苹果。草莓奶酪。」）。**整段模式没这个问题** —— SenseVoice 的词表没有字节 token。
- **术语表（热词）**（`settings.rs` 清洗，`engine.rs` 的 `StreamingEngine::new` 用它）：`settings.hotwords`（≤ 100 条、单条 ≤ 32 字，`settings::clean_hotwords` 清洗）。非空时 `StreamingEngine` 用 `modified_beam_search` + `config.hotwords_buf`（短音频约慢 6%），为空保持 greedy。**只能用在流式** —— SenseVoice 上给热词或 beam 会 `_Exit(-1)` 杀进程（已复现）。改词表会清空流式引擎，下次录音重载 162 MB（流式这一组 assets，界面按 1024 进制显示）。设置页「填入示例」用的是 `src-tauri/assets/hotwords.txt`。
  - **热词要按模型的词表过滤**（`engine.rs` 的 `streaming_vocab_chars` / `split_hotwords_by_vocab`）：流式模型是 cjkchar **单字**词表（1426 字），sherpa 的 `EncodeHotwords` **逐字**查表 —— 任何一个字不在表里，**整条热词**就被跳过（只往 stderr 打一句 `Cannot find ID for token`，界面上看不出为什么没生效）。喂给 sherpa 之前先剔掉含表外字的条目，并记 WARN 列出是哪几条；`Status.hotwordsIgnored` 把这个数给界面，保存后提示「其中 N 个包含模型词表之外的字，不会生效」。**读不到词表时全部放行** —— 别因为词表读不出来就把用户的术语表清空。
- 标点模型（`PUNCT`，CT-Transformer zh-en，281 MB，`engine.rs` 的 `PunctuationEngine`）独立放在 `models/punct/`，默认关闭；开启后 `StreamEmitter` 在每句定稿时调 `PunctuationEngine::apply`，实时中间结果不加标点。模型缺失/加载失败时静默降级，不阻断录音。

## 模型与下载

- 模型版本固定：HF commit + sha256 + bytes 写在 `models.rs` 的 `MODEL` / `TOKENS` / `VAD` / `STREAM_*` / `PUNCT` 常量里。**换模型必须同时改 bytes 和 sha256**，否则下载校验失败。
- **下载走镜像优先**（`[MIRROR, HF]`）：国内直连 huggingface.co 经常连不上，镜像失败再回退官方源。顺序保持镜像优先。
- **模型目录必须有 `silero_vad.onnx`**：整段分句、文件转写都靠 VAD，`models::resolve()` 会直接跳过没有 VAD 的目录（否则录完只得空结果）。
- 已存在且大小正确的文件直接跳过下载；`.part` + Range 支持断点续传；下完做 sha256 校验。应用数据目录里的文件按**字节数**判定是否就绪（`app_data_file_ok`，含流式 VAD），所以换 VAD 版本后会自动重下；截断文件不能显示「已就绪」。下载忙碌标志用 `DownloadFlagGuard` 在所有退出路径复位，别在目录创建前裸 `?` 留下永久 busy。
