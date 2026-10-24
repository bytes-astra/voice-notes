# 转写质量增强路线图（2026-10-23 深度调研汇总）

> 目标：在 **本地离线 + 纯 CPU（macOS arm64 / Windows x64）+ 内存恒定 + 消费式缓冲** 的约束下，
> 找出能真正提升 VoiceNotes 转写质量的手段，并排出落地顺序。
>
> 详细调研（四个方向，各 470–660 行，带一手来源）见同目录：
> - [`2026-10-23-models.md`](./2026-10-23-models.md) —— 可换的 ASR 模型（Qwen3-ASR / FireRed ASR2 / X-ASR / Fun-ASR-Nano / Whisper / Paraformer …）
> - [`2026-10-23-decoding.md`](./2026-10-23-decoding.md) —— 热词、同音词替换、beam search、端点规则、解码参数
> - [`2026-10-23-audio-vad.md`](./2026-10-23-audio-vad.md) —— VAD 模型/参数、降噪、AGC、切句与长音频
> - [`2026-10-23-post-processing.md`](./2026-10-23-post-processing.md) —— 标点、ITN、繁简、说话人分离、LLM 后编辑
>
> **复核**：[`2026-10-23-review.md`](./2026-10-23-review.md)（独立复核，含 3 处修正与无法本地验证清单；下表已按复核结果更新）
> **落地**：[`2026-10-23-optimization-plan.md`](./2026-10-23-optimization-plan.md)（实施批次、改动点、验收标准、回滚）
>
> 调研期间**没有改动任何产品代码**；新增的 `src-tauri/tests/quality_probe.rs` 是量化探针（只打印，不断言）。

## 证据分级（每条建议都标了来源强度）

| 标记 | 含义 |
|---|---|
| ✅ **本地实测** | 在本机用真实模型/真实音频跑过，数字来自本仓库的运行输出 |
| 📄 **一手来源** | 来自官方文档 / 官方仓库 / model card / 论文，**未在本机复现** |
| ⚠️ **待验证** | 方向可信但依赖我们还没有的语料或环境（噪声会议录音、真实人名表） |

---

## 0. 一句话结论

**纯换模型的 CER 增益是「混合的、不必然赢」的**；真正确定性、可落地、且零模型下载成本的收益来自
**① 流式热词 ② 拼音同音词替换 ③ 端点规则的隐藏 bug ④ VAD 版本与参数 ⑤ 标点模型 int8**。
换模型（Qwen3-ASR / FireRed ASR2）是「值得排期」但**需要真实语料先量出基线**的工程，不是本周就能拍板的改动。

---

## 1. P0 —— 零风险、今天就能做

| # | 事项 | 收益 | 代价 | 证据 |
|---|---|---|---|---|
| P0-1 | **标点模型换 int8**：`models.rs` 的 `PUNCT` 从 fp32（294,372,519 B = 281 MiB）换成 int8（**75,519,198 B = 72 MiB**） | 下载体积 **−209 MiB**；质量**轻微下降**（本机 7 句：6 句逐字相同，1 句丢句中逗号）；**int8 并未更快**（26 ms vs 17 ms，单次） | ① 官方 HF 仓库**只有 fp32**，int8 只在 GitHub Releases 与第三方 HF 镜像 → 现行「HF 镜像优先」下载器会 404，**需先解决托管**；② 然后改 3 个常量 | ✅ 体积/质量本机实测；📄 官方 release 资产存在（`-int8.tar.bz2` 64,717,756 B） |
| P0-2 | **Silero VAD v4 → v5**：只换模型文件，不改代码 | 中文会议场景 ROC-AUC 0.83→0.94、多域准确率 0.85→0.91、纯噪声误报 0.51→0.61 | 下载 +0.5 MB；改 3 个常量 | 📄 上游数据 + ✅ 文件已核对：镜像 `csukuangfj/vad` 同一 commit 下 `silero_vad_v5.onnx` = 2,313,101 B / sha256 `6b99cbfd39246b6706f98ec13c7c50c6b299181f2474fa05cbc8046acc274396`（本机已下载并核对 sha）；sherpa 按张量签名自动识别 v4/v5 |
| P0-3 | **文件转写路径的 VAD 缓冲 32 s → 60 s**（`asr.rs:114` 用的是 `MAX_SEGMENT_SECONDS + 2.0`，录音路径是 60 s） | 消除 CircularBuffer 扩容（Overflow 警告 + 整段拷贝） | 1 行 | 📄 sherpa `circular-buffer.cc` 扩容逻辑；✅ 代码里两处确实不一致 |
| P0-4 | ~~别把 `"auto"` 传给 SenseVoice~~ **（复核后撤销）** | — | — | ✅ 复核：v1.13.8 的 SenseVoice `lang2id` 里 `{"auto", 0}` 是合法值，本机无任何错误日志、输出与 `zh` 一致。**现有代码不用改** |
| P0-5 | **保留量化探针** `src-tauri/tests/quality_probe.rs` | 任何「换模型 / 调参 / 加热词」都能先量后改 | 已入库，只打印不失败 | ✅ 已跑通 5 个探针 |

> P0-2 的诚实说明：本地用 3 条**干净短句**样本对比，v4 与 v5 **切句完全一致**（都是 1 段，边界相同）。
> v5 的收益集中在**噪声/多人会议**音频上 —— 我们还没有这类语料，所以这条是「上游数据支持 + 本地无反证」，
> 建议等有真实语料再确认。

## 2. P1 —— 小改动，收益最大（这一档是本次调研的重点）

| # | 事项 | 收益 | 代价 | 证据 |
|---|---|---|---|---|
| **P1-1** | **流式模式接热词**（`hotwords_buf` / `hotwords_file` + `decoding_method="modified_beam_search"`） | **专名/术语命中 48/140 → 94/140**（官方合成集），真实录音 6/30 → 22/30；RTF 基本不变 | 3 行配置 + 词表文件 + 前端「术语表」入口；需 `modified_beam_search`（比 greedy 慢 1.8×，本机实测 3.5s 音频 122ms→130ms） | ✅ **本地实测**：热词表含「再确认一下」时，流式把「我们**在**确认一」纠正成「我们**再**确认一」；📄 官方量化数据 |
| **P1-2** | **流式 `rule3_min_utterance_length` 20 → 60**（或关掉 sherpa endpoint，只留 VAD 门控） | 消除「连续说话每 20 秒被硬切一刀」 | 1 行 | ✅ **本地实测**：连续喂 24.5 s 无停顿语音，endpoint 在 **20.37 s** 触发；改成 60 后 24.5 s 内不再触发 |
| **P1-3** | **加「短句合并」层**（仿 FunASR `merge_vad=True, merge_length_s=15`） | 治「切句太碎」，并给 SenseVoice 更长上下文 | 新增合并器 + 改事件粒度 | 📄 FunASR 官方流水线默认行为；本项目**目前缺这一层** |
| **P1-4** | **`min_silence_duration` 0.5 → 0.2~0.3** | 治「句子拖太长带静音」；Silero 官方默认 0.1 s，我们比默认保守 5 倍 | 参数改动 | ✅ 本地实测：0.3 时句尾从 5.6 s 收到 5.1–5.2 s（少 ~0.4 s 静音尾巴） |
| **P1-5** | **整段模式接 HomophoneReplacer**（`config.hr = HomophoneReplacerConfig{lexicon, rule_fsts}`） | 中文同音错字纠正（官方示例用**正是本项目同款模型**：悬界芯片→玄戒芯片、人工投→人头） | 需 lexicon（1.37 MB）+ 自建 `replace.fst`（要 pynini/Linux 生成） | 📄 官方示例 + crate 源码确认 `OfflineRecognizerConfig.hr` / `OnlineRecognizerConfig.hr` 存在；**流式和整段都生效** |
| P1-6 | 流式 `blank_penalty` 微调 | 抑制「吃字」 | 1 行，先 A/B 再上 | 📄 仅 transducer 生效，blank id 硬编码为 0 |

### 为什么整段模式不能照抄流式那套热词

SenseVoice 是**没有 context graph 的**离线模型，sherpa 明确不支持给它热词：
官方文档原话是 "Only transducer models support hotwords in sherpa-onnx"。
所以**整段模式想修专名，只能走 P1-5 的拼音同音词替换**，或者换模型（见第 4 节）。

## 3. P2 —— 需要真实语料 / 灰度，先量再上

| # | 事项 | 收益 | 代价 / 风险 | 证据 |
|---|---|---|---|---|
| P2-1 | **换整段模型：Qwen3-ASR-0.6B INT8** | 中文 CER **不一定赢**：唯一一份同协议对比表（X-ASR model card）里 SenseVoice-small **5.73 / 6.47 反而略优于 Qwen3-ASR-0.6B 的 5.97 / 6.88**；Qwen3-ASR 在 AISHELL-2 3.15、Fleurs-zh 2.88 等集上赢。**真正的收益是原生支持热词 + 52 语种/22 汉字方言** | 838 MB 下载（现 239 MB）；自回归 LLM 解码；`max_total_len` 默认被钳到 512 会**截断长音频**，`max_new_tokens` 默认 128 必须显式设置；CPU RTF 0.077–0.168 | 📄 官方 README 评测表 + sherpa-onnx 文档页 |
| P2-2 | **换整段模型：FireRed ASR2 AED INT8** | **中文绝对精度最高一档**：AIShell1 0.57% CER、四个普通话集均值 3.05%、19 个方言/口音集均值 11.67%；离线+流式同一套 | 799.7 MB；**无热词接口、无 ITN 标点**（要另配标点）；RTF 0.058–0.333 | 📄 论文 + 官方包 |
| P2-3 | **换流式模型：X-ASR-zh-en INT8** | **离线 + 真流式同一套权重**、自带标点与大小写；流式 1920ms：WenetSpeech net 6.42 / meeting 8.03 | 130.6 MB（比现在 161 MB 还小）；要设 `model_type="zipformer2"` + `modeling_unit="bpe"` + `bpe_vocab`（现状是 cjkchar）；喂数节奏建议从 200 ms 改到其 chunk 尺寸（480 ms） | 📄 官方包 + 模型卡（技术报告 coming soon） |
| P2-4 | **GTCRN 降噪**（sherpa-onnx 已内建 `Offline/OnlineSpeechDenoiser`，**不需要新 crate**） | 噪声环境 PESQ 2.29(RNNoise)→2.87、DNS3 OVRL 2.53→2.70；RTF ≈0.07 | 只在 GitHub Releases，**没有官方 HF 镜像**（与本项目「镜像优先」冲突）；干净语音可能过抑制 → 做成**默认关**的可选开关 | 📄 sherpa-onnx 文档 + crate 源码（`offline_speech_denoiser.rs`） |
| P2-5 | **AGC / 削波提示** | 治「增益过低 VAD 不触发」「增益过高削波」；项目目前**完全没有增益处理**，`LevelRing::peak()` 是 RMS×6 不是真峰值 | 自研约 30 行 | 📄 代码核对 |
| P2-6 | **说话人分离**（`OfflineSpeakerDiarization` 确实存在，RTF 0.11–0.30） | 会议纪要刚需 | `process(&[f32])` 要整段驻留 → **与「不限时长、内存恒定」正面冲突** | 📄 crate 源码 + 官方数据 |

## 4. 明确不做（以及为什么）

| 不做 | 原因 |
|---|---|
| LLM 后编辑（Qwen3-0.6B–4B / llama.cpp） | 639 MB–2.5 GB；逐句 0.5–2 s CPU，**延迟倒挂**；小模型倾向「改写」而非「纠错」，会把一个错字变成一段幻觉。定位成用户主动点的「AI 整理」可以另议，**默认链路不做** |
| ROVER / 二遍识别 / n-gram LM 重打分 | 要整段音频或外接 LM，和消费式缓冲、内存恒定冲突；对 3M 中文小模型收益远小于热词 |
| WeTextProcessing（外挂 ITN） | 官方无 Rust 绑定、无 ONNX 导出（是 FST 规则引擎），第三方 `wetext-rs` 仅 141 下载；SenseVoice 内置 ITN 已覆盖中文数字/标点/大小写 |
| SenseVoice 2025-09-09 版 | 是**粤语微调版**且官方明确「不支持标点」，对普通话是负收益 |
| Whisper large-v3 / turbo | 中文明显弱于现状（WenetSpeech net 9.86 vs 5.73）；体积 537–1019 MB |
| Qwen3-Omni / Qwen-Audio / Omni-ASR | sherpa-onnx 1.13.8 **完全不支持**（crate 里没有对应结构体） |
| NeMo / Citrinet / Parakeet / Canary / MedASR | 全是纯英文或非中文模型 |
| FSMN-VAD | sherpa-onnx 根本不支持（Rust 侧只有 `SileroVadModelConfig` / `TenVadModelConfig`） |
| 调小 `max_speech_duration` 逼 VAD 切句 | 「收紧」参数 0.1 s / 0.9 是 **C++ 硬编码不可配**；调小 = 主动进入激进切分态，中文会被切碎 |
| 引 `webrtc-audio-processing` / `sonora` | 前者要编 C++ WebRTC；后者 **MSRV 1.91，破项目声明的 1.78** |

## 5. 两个必须记住的地雷（本地已复现，会让应用**直接闪退**）

| 地雷 | 现象 | 证据 |
|---|---|---|
| 离线 SenseVoice 上设 `decoding_method="modified_beam_search"` | sherpa 打印 `Only greedy_search is supported at present` 后调 `_Exit(-1)` → **整个进程消失**（Tauri 里就是应用瞬间闪退，没有任何错误提示） | ✅ 本机复现：测试进程 exit status 255 |
| 离线 SenseVoice 上用热词（`create_stream_with_hotwords`） | sherpa 打印 `Only transducer models support contextual biasing` 后同样 `_Exit(-1)` | ✅ 本机复现：exit status 255 |
| Silero VAD v6 | sherpa 只认 v4/v5 张量签名，其它直接 `exit(-1)` | 📄 sherpa 源码 |

> 结论：**热词只能加在流式（transducer）路径上**；整段路径想加解码相关的开关，必须先确认 sherpa 的
> 「Only … is supported at present」检查不会把进程带走。

## 6. 还需要你提供的两样输入（决定 P1/P2 能不能变成结论）

1. **一份真实语料**：10–30 条你日常会录的句子/会议片段（wav）+ 人工听写的正确文本。
   放到 `src-tauri/tests/fixtures/<名字>.wav` 与同名 `<名字>.txt`，探针会自动算 CER。
   没有它，P1-2/P1-4/P2-* 都只能停在「上游数据支持」。
2. **一份术语/人名表**（你常说的产品名、同事名、项目代号）。这是 P1-1 热词唯一真正的验收标准 ——
   本地实测已经证明热词能把「在/再」这类同音错字纠正回来，但**收益大小取决于词表命中率**。

## 7. 建议的落地顺序

1. **今天**：P0-2 / P0-3 / P0-5（VAD v5、32s→60s、探针）——常量/一行改动，风险极低。P0-1 先解决 int8 托管再动（见复核报告第 3 条）。
2. **本周**：P1-1 热词 + P1-2 rule3 —— 收益最大、代码最少；热词需要一个最小可用的术语表入口（先做个「粘贴文本导入」即可）。
3. **有语料后**：P1-3 短句合并、P1-4 min_silence、P1-5 同音词替换 —— 全部先跑探针对比再定参数。
4. **再往后**：P2 换模型 —— 先用同一份语料对比 SenseVoice vs Qwen3-ASR vs FireRed ASR2，
   确认「多花 600–800 MB 下载」是否换得到你能感知的准确率，再决定是否进产品。

## 8. 2026-10-24 复核：哪些还值得做

> 上面 P0/P1/P2 的所有项都逐条用代码核实过状态（本节是核实结果，不是转述文档）。
> 结论：**没有任何一项需要现在执行**。真正的瓶颈是语料，不是代码。

### 8.1 已核实「不需要做」的两项

**P1-6 流式 `blank_penalty` 微调 —— 不做**（实测 A/B，71 条流式语料）

| blank_penalty | long | noisy | real | tts | 总编辑距离 |
|---|---|---|---|---|---|
| 0.0（默认） | 40.00% | 16.67% | **16.24%** | 43.28% | 284/1175 |
| −0.5 / −1.0 | 40.00% | 16.67% | 16.24% | 43.28% | 284/1175 |
| 0.5 | 39.17% | 16.67% | **16.38%** | 41.18% | 279/1175 |
| 1.0 | 40.00% | 18.06% | 16.64% | 40.76% | 282/1175 |

净省 5 个字（0.4%）**全部来自 TTS 合成样本**；40 条真人样本反而变差（16.24% → 16.38%）。
负值完全无效果。差异在噪声量级且方向不一致 → **不做**。

**P2-5 AGC / 削波提示 —— 不做**（实测 + 代码核实，前提就不成立）

- **前提错误**：项目里没有「电平表」这种诊断工具。`audio.rs` 的 `LevelRing::peak()`
  只喂给**波形条动画**（`recorder.js` 的 `4 + round(level*62)` px），既不显示数字也没有阈值告警。
  路线图原文写的「`LevelRing::peak()` 是 RMS×6 不是真峰值」属实，但**改口径对用户没有可感知价值**。
- **削波不影响识别**：真人样本放大 4 倍（削波 2.5%–5.3% 的采样）后 CER **一字未变**。
- **增益过低几乎不发生**：衰减到 −30 dB 时 VAD 仍正常切句；只有 −40 dB 时才有 2/8 个样本
  完全失灵（0 段）。这是「麦克风几乎没收到声音」级别的极端情况。
- 结论：AGC 治不了实际问题，还会带来「说话时突然放大」的听感问题 → **不做**。

### 8.2 因太麻烦 / 阻塞而跳过的四项

| 项 | 跳过理由 |
|---|---|
| P1-5 整段同音词替换（`hr`） | `replace.fst` 需 pynini 生成，**PyPI 上 pynini 只有 manylinux 和 Windows wheel，没有 macOS**（官方文档：macOS 只能走 conda-forge）。本机无 conda/mamba。收益虽可估（语料里 tts-19「玄戒芯片」→识别成「全泄芯片」、tts-20「弓头安装」→「满工重安装」，2/20 样本有专名同音错字），但成本不成比例 |
| P2-4 GTCRN 降噪 | 模型只在 GitHub Releases、**没有官方 HF 镜像**，与本项目「镜像优先」冲突；且现有噪声样本 CER 只有 2.78%（5 条 2 字错），降噪解决不了当前矛盾 |
| P2-1/2/3 换模型 | 838 / 799.7 / 130.6 MB 下载 + 改 `modeling_unit`/`bpe_vocab`/喂数节奏。**已单独成文**：[`2026-10-25-model-switch-options.md`](./2026-10-25-model-switch-options.md) —— 含实测的仓库/commit/bytes/sha256、crate 配置字段、改动面表格、spike 步骤。结论：X-ASR（换流式，168 MiB）**对症流式这个最大短板**，Qwen3-ASR（941 MiB）**唯一能解锁整段热词**，FireRed ASR2（1.15 GiB）无热词无标点 |
| B4 标点换 int8 | 官方 HF 只有 fp32，int8 只在 GitHub Releases → 卡在托管方案（供应链 / 国内可达性）。标点模型**默认关闭**，多数用户不会下载它 |

### 8.3 真正的瓶颈：语料

现有 71 条语料里，**真人样本全部来自 minds14 的「查账户余额」一个场景**（计划自己写了「领域窄」），
没有一条覆盖日常口述或会议。这导致：

- 上面的 P1-6 / P2-5 只能给出「噪声量级」的结论；
- P2-* 的换模型对比（需要同一份语料横比）根本做不了；
- 路线图第 6 节要的两样输入里，**「你日常会录的句子/会议片段」仍然没有**。

**补 10–30 条真实录音（wav + 人工听写文本）比再写任何代码都值钱。** 放到
`src-tauri/tests/fixtures/<名字>.wav` + 同名 `.txt`，探针会自动算 CER。

### 8.4 新增的两个探针

- `src-tauri/tests/level_probe.rs` —— 输入电平敏感性：衰减后 VAD 还切不切得出句子、放大削波后
  CER 变不变、RMS×6 与真峰值差多少。
- `quality_probe.rs` 的 `probe_streaming_blank_penalty` —— blank_penalty 的 A/B。

两个都是「只打印不断言」，跟仓库既有探针一个路子。
