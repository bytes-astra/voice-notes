# 语音转写后处理调研（2026-10-23）

> **主题**：识别完文本之后，还有哪些手段能提升「转写结果」的整体质量（标点、数字规范化、繁简、说话人、术语纠错、LLM 后编辑），并且必须能在**本地离线、纯 CPU、无需 GPU 的桌面应用**里落地。
>
> **项目**：VoiceNotes（Tauri 2 桌面应用，仓库 `/Users/hugh/Agents/DeepSeek/voice-notes`）
> **现状基线**：整段模式 = SenseVoiceSmall INT8（自带 ITN，输出标点）；流式模式 = zipformer transducer + 可选 CT-Transformer zh-en 标点模型（281 MiB，独立下载，默认关闭）。
> **Rust 栈**：`sherpa-onnx = "1.13.8"`，crate 源码位于
> `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-onnx-1.13.8/src/`
> 与 `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-onnx-sys-1.13.8/src/`。
>
> 本文所有 API 字段名均以**上述本地 crate 源码**为准；所有模型体积、指标均以**官方仓库 / model card / 官方文档**为准。
> 本次调研**未修改仓库代码、未执行 git 操作、未在本机下载任何模型**。

---

## 结论摘要

| 手段 | 预期收益 | 体积 / 内存 | CPU 代价 | 侵入性 | 可行性 |
|---|---|---|---|---|---|
| **A. 标点模型换 int8**（CT-Transformer zh-en fp32→int8） | 质量基本不变，下载 **281 MiB → 72 MiB** | −74% 下载体积；运行时内存同量级 | 无变化（int8 更快） | 极低：只改 `models.rs` 的 `PUNCT` 常量 | ✅ **立即做** |
| **B. 术语纠错：sherpa-onnx 内置 HomophoneReplacer** | 中文同音错词纠正（悬界→玄戒、人工投→人头） | lexicon 1.37 MB + 规则 FST 0.2 MB | jieba 分词 + pinyin + FST，毫秒级 | 中：`OfflineRecognizerConfig.hr` / `OnlineRecognizerConfig.hr` | ✅ **推荐**（流式 + 整段都支持） |
| **C. 术语纠错：Hotwords / contextual biasing** | 领域热词（产品名、人名）识别率提升 | 0（内存里放字符串即可） | 需 `modified_beam_search`，比 greedy 慢 | 中 | ⚠️ **仅流式模式可用**（SenseVoice 不支持） |
| **D. ITN：沿用 SenseVoice 内置 ITN** | 中文数字→阿拉伯数字、句末标点、英文首字母大写 | 0（已在 228 MB 模型里） | 0 | 极低 | ✅ **保持现状**，不要另起 WeTextProcessing |
| **E. ITN：WeTextProcessing（规则式）** | 补齐金额/日期/量词/百分比/车牌/火车号 | FST 1.36 MB（zh）+ 规则表 | 极低（FST 查表） | 高：官方无 Rust 绑定，第三方 crate 仅 141 下载 | ⚠️ **暂不做**（见 §2.3） |
| **F. 说话人分离（Diarization）** | 会议纪要场景刚需 | 1.5 MB（int8 分割）+ 26–28 MB（嵌入） | RTF ≈ 0.11–0.30（官方实测） | **高**：需整段音频，与「内存恒定/不限时长」冲突 | ⚠️ **仅在另开「录音到文件」管线时做** |
| **G. 英文大小写 + 英文标点（OnlinePunctuation）** | 英文句首大写、句末标点 | 7.1 MB int8 + 146 KB bpe.vocab | ~13 ms/句（官方实测） | 低 | ✅ **推荐**（流式英文场景） |
| **H. 繁简转换（OpenCC）** | 繁体→简体 | 需系统装 OpenCC C++ 库 | 极低 | 高：跨平台打包 C++ 依赖 | 🔸 **可选**，量小时不值得 |
| **I. 中文数字↔阿拉伯（`chinese-number` crate）** | 纯数字转换 | 0 | 极低 | 低 | 🔸 **噪音**（ITN 已覆盖，且更准） |
| **J. 置信度 / 低可信标注** | 标出可疑片段 | 0 | 0 | 中：需绕开 crate 的 serde 结构 | ⚠️ **整段(SenseVoice)拿不到，流式(transducer)可以** |
| **K. LLM 后编辑（Qwen3 0.6B–4B，llama.cpp）** | 纠错 + 润色 + 摘要 | 639 MB – 2.5 GB（GGUF） | 每句 ~0.5–2 s CPU | 很高 | 🔸 **默认不做**（见 §5.4 诚实结论） |

> **复核注记（2026-10-23）**：标点 int8 实测 75,519,198 B（72 MiB，省 209 MiB）；本机 7 句对比 **6 句逐字相同、1 句丢句中逗号**（质量轻微下降）；单次测量 int8 **没有更快**（26 ms vs 17 ms）。另外官方 HF 仓库只有 fp32，int8 只在 GitHub Releases 与第三方镜像 —— 换 int8 前要先解决托管。

### 一句话结论（如果只看一条）

**在「本地离线 + 纯 CPU」这个约束下，最划算的三件事是：①把 CT-Transformer 换成 int8 版（省 209 MiB）；②打开 sherpa-onnx 内置的 HomophoneReplacer 做中文术语纠错；③流式模式补上英文大小写/标点。**
LLM 后编辑在纯 CPU 桌面场景下**收益/风险比很差**：小模型会「改写」而不是「纠错」，把一个错字变成一段幻觉，比留着错字更糟。

### 与现有架构的冲突点（必须先看的三条）

1. **标点模型现在是 fp32**：`src-tauri/src/models.rs:104-109` 的 `PUNCT` 指向 `...vocab272727-2024-04-12/.../model.onnx`，`bytes: 294_372_519`（281 MiB）。官方同时提供 int8（72 MiB）。
2. **流式模型支持 hotwords，但整段的 SenseVoice 不支持**：官方文档明写 *"Only transducer models support hotwords in sherpa-onnx. That is, only models from Offline transducer models and Online transducer models support hotwords."* 所以「术语纠错」这条能力**天然只在流式模式可用**，整段模式要另走 HomophoneReplacer。
3. **Diarization 需要整段音频**：`OfflineSpeakerDiarization::process(&self, samples: &[f32])` 一次性吃完整波形，与本项目「录音不限时长、内存恒定」的设计直接冲突。

---

## 逐项详述

### 1. 标点

#### 1.1 sherpa-onnx 到底支持哪些标点模型（按 crate 源码 + 官方 release 双重确认）

**crate 源码只有两个标点类，且每个只有一种模型槽位：**

`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-onnx-1.13.8/src/offline_punctuation.rs`

```rust
// L29-34
pub struct OfflinePunctuationModelConfig {
    pub ct_transformer: Option<String>,   // ← 只有这一个模型槽位
    pub num_threads: i32,
    pub debug: bool,
    pub provider: Option<String>,
}
// L60-62
pub struct OfflinePunctuationConfig { pub model: OfflinePunctuationModelConfig }
// L97
pub fn add_punctuation(&self, text: &str) -> Option<String>
```

`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-onnx-1.13.8/src/online_punctuation.rs`

```rust
// L33-38
pub struct OnlinePunctuationModelConfig {
    pub cnn_bilstm: Option<String>,        // ← 只有这一个模型槽位
    pub bpe_vocab: Option<String>,
    pub num_threads: i32,
    pub debug: bool,
    pub provider: Option<String>,
}
// L108 附近
pub fn add_punctuation(&self, text: &str) -> Option<String>
```

**官方 `punctuation-models` release 标签（`k2-fsa/sherpa-onnx`，published 2024-04-12）的全部资产只有 3 个**（GitHub Releases API 核对）：

| 资产 | 压缩包 | 解压后模型 | 支持语言 | 能力 |
|---|---|---|---|---|
| `sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12.tar.bz2` | 279.0 MB | `model.onnx` **281 MiB** | 中 + 英 | 标点 |
| `sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8.tar.bz2` | 64.7 MB | `model.int8.onnx` **72 MiB** | 中 + 英 | 标点 |
| `sherpa-onnx-online-punct-en-2024-08-06.tar.bz2` | 30.7 MB | `model.int8.onnx` 7.1 MB + `bpe.vocab` 146 KB | **仅英文** | 标点 **+ 大小写** |

> **关于「SplitPunct」：sherpa-onnx 生态里不存在这个模型。** 上表是官方 release 的完整资产列表（只有 3 个），且 crate 的配置结构里只有 `ct_transformer` / `cnn_bilstm` 两个槽位，没有任何第二种离线标点模型类型。arXiv 检索 `"SplitPunct"` 也返回 0 条记录。**结论：不要按 SplitPunct 做技术选型。**

> **官方 release 自 2024-04-12 起未再更新过标点模型**（遍历 `k2-fsa/sherpa-onnx` 全部 release，只有 `punctuation-models` 这个 tag 含标点模型资产）。截至 2026-10-23，中文标点**没有更新的官方选择**。

**其他相关模型（非 sherpa-onnx 官方）：**
- FunASR 侧的中文标点模型 `iic/punc_ct-transformer_zh-cn-common-vocab272727-pytorch`（本项目 CT-Transformer 的上游），model card 同时提供 `model.pt`（292 MB PyTorch）。
- FunASR 还提供 `ct-punc`、`ct-punc-c`、`fa-zh`（时间戳）等变体，但**均未导出到 sherpa-onnx**。

#### 1.2 中文标点质量 / 体积 / CPU

**质量（官方 model card，2026-10-20 仍在更新）** — `iic/punc_ct-transformer_zh-cn-common-vocab272727-pytorch`：

| precision | recall | f1_score |
|---|---|---|
| 53.8 | 60.0 | **56.5** |

- 训练数据约 **33M samples**；评测集为「自采集通用领域业务场景数据 20000+ samples」。
- 论文：*Controllable Time-delay Transformer for Real-Time Punctuation Prediction and Disfluency Detection*，ICASSP 2020，<https://arxiv.org/abs/2003.01309>。
- Model card 自述适用场景：*"适合对文本数据进行标点预测，文本长度不限"*；运行平台支持 Linux-x86_64 / Mac / Windows。
- ⚠️ **F1 只有 56.5** —— 这是本报告里最该记住的数字之一：CT-Transformer 的标点**并没有到「可以直接信」的程度**，它的价值在于「把裸文本变得可读」，不是「给出权威断句」。

**CPU 代价（官方文档实测输出）**：
- `sherpa-onnx-offline-punctuation` 对一句中文：`Elapsed seconds: 0.014 s`；中英混合：`0.010 s`。
- 结论：**毫秒级**，流式句尾补标点完全无压力。

**体积**：
- fp32 = 281 MiB（项目当前用的就是这个）；int8 = 72 MiB。
- **省 209 MiB，代价接近零** —— 这是本次调研里 ROI 最高的一条。

#### 1.3 能否「流式增量应用」

这是最容易被误解的一点，必须区分两个概念：

**(a) `OnlinePunctuation` 不是有状态增量模型。** 查 C++ 实现
`sherpa-onnx/csrc/online-punctuation-cnn-bilstm-impl.h`：

```cpp
// L54
std::string AddPunctuationWithCase(const std::string &text) const override { ... }
// L64-72：把整段文本切成句子，堆成一个 {n, kMaxSeqLen(200)} 的张量
// L78：一次 Forward
auto pair = model_.Forward(std::move(token_ids), std::move(valid_ids), std::move(label_len));
```

`const` 方法、无跨调用状态 → 它是**「一段文本一次性进、一次性出」**，只是模型结构（CNN-BiLSTM，来自 Edge-Punct-Casing，<https://arxiv.org/abs/2407.13142>）本身适合低延迟场景。
**所以 sherpa-onnx 里没有「流式增量标点」这种东西**；官方叫 online 的那个模型只对英文有效。

**(b) `OfflinePunctuation`（CT-Transformer）同样是一次性。**
`sherpa-onnx/csrc/offline-punctuation-ct-transformer-impl.h`：`max_len = 200`，长文本在 `len >= max_len` 时按逗号切分（`if (dot_index == -1 && len >= max_len && comma_index != -1)`）后分段推理。

**本项目当前的用法是正确的**：在 `src-tauri/src/asr.rs:648-650`，`StreamEmitter` 只在**句子定稿**时调 `punctuation.apply()`，实时中间结果（partial）不加标点。CT-Transformer 一次性作用于一整句，正是它的工作方式。

#### 1.4 对本项目「整段 SenseVoice ITN / 流式 CT-Transformer」现状的组合建议

| 组合 | 评价 |
|---|---|
| 整段 = SenseVoice 自带 ITN 标点 | ✅ **保持**。官方文档：`This model supports punctuations if you set use_itn to 1.` |
| 整段结果**再**套一遍 CT-Transformer | ❌ **不要做**。SenseVoice ITN 已输出 `。，、`，CT-Transformer 会再插一遍 → `。。`（与 AGENTS.md 一致）。若真要叠，必须先做「剥离已有标点」的前置处理。 |
| 流式 = CT-Transformer（现状） | ✅ **保持**，这是 sherpa-onnx 中文流式标点的唯一选择。 |
| 流式改成「也用 SenseVoice 打标点」 | ❌ **不可行/无意义**。①SenseVoice 是**非流式**模型（OfflineRecognizer），不能逐块喂；②AGENTS.md 已实测记录：SenseVoice 覆盖流式结果后准确率反而从 99% 掉到 91%（且把「明天」听成「今天」）；③那需要保留全部音频做二次识别，与「内存恒定」冲突。**结论不变。** |
| 流式 = 改用 `OnlinePunctuation`（Edge-Punct-Casing） | ❌ 中文不适用（该模型**只支持英文**）。 |
| **CT-Transformer fp32 → int8** | ✅ **推荐**。省 209 MiB，质量损失可忽略（标点 F1 56.5 的模型，量化噪声远小于模型本身的误差）。 |

**具体落地建议（第 1 条）**：
`src-tauri/src/models.rs:104-109` 的 `PUNCT` 常量换成
`/csukuangfj/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8/resolve/<commit>/model.int8.onnx`，
`bytes` 改为 ~75_497_472（72 MiB），并同步改 sha256 —— AGENTS.md 明确要求「换模型必须同时改 bytes/sha256」。

---

### 2. ITN

#### 2.1 SenseVoice 内置 ITN 到底做了什么（官方实测输出，一手）

来自 sherpa-onnx 官方 [SenseVoice 预训练模型文档](https://k2-fsa.github.io/sherpa/onnx/sense-voice/pretrained.html) 的 CLI 输出：

**关闭 ITN（`use_itn=0`）：**
```json
{"text": "开饭时间早上九点至下午五点", "tokens":["开","饭","时","间","早","上","九","点","至","下","午","五","点"]}
{"text": "the tribal chieftain called for the boy and presented him with fifty pieces of gold"}
```

**开启 ITN（`use_itn=1`）：**
```json
{"text": "开放时间早上9点至下午5点。", "tokens":["开","放","时","间","早","上","9","点","至","下","午","5","点","。"]}
{"text": "The tribal chieftain called for the boy and presented him with 50 pieces of code."}
```

**由此可直接读出 SenseVoice ITN 的能力边界：**

| 能力 | 覆盖情况 | 证据 |
|---|---|---|
| 中文数字 → 阿拉伯数字 | ✅ | 九→9、五→5 |
| 句末标点 | ✅ | 补 `。` |
| 英文首字母大写 | ✅ | `the`→`The` |
| 英文数字 → 阿拉伯数字 | ✅ | `fifty`→`50` |
| 固定搭配纠错 | ✅（意外收获） | `开饭时间`→`开放时间` |
| **⚠️ 会引入错误** | ❌ | `fifty pieces of gold` → `fifty pieces of **code**`（ITN 把 gold 听/转成了 code） |

> **这是本次调研最重要的一条负面结论**：ITN 不是无损的。官方示例里它自己就把 `gold` 变成了 `code`。**任何在 ITN 之后再叠加 ITN 的方案（SenseVoice ITN + WeTextProcessing 双跑）都可能把这个错误放大两次**，而且用户无法知道哪个版本才是对的。

#### 2.2 crate 侧的开关

`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-onnx-1.13.8/src/offline_asr.rs:223-227`

```rust
pub struct OfflineSenseVoiceModelConfig {
    pub model: Option<String>,
    pub language: Option<String>,
    pub use_itn: bool,          // ← ITN + 标点 一起开关
}
```

`use_itn` 同时控制**数字规范化**和**标点输出**，无法分开。项目已有 `settings.itn`（引擎按 `(language, itn)` 缓存），这一层不用动。

**注意**：2025-09-09 发布的粤语版 `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09` 官方明确 *"This model does not support punctuations."* 若将来换模型，标点能力会一起消失。

#### 2.3 WeTextProcessing：能力、能否进 Rust、体积与开销

**规则覆盖（一手，按官方仓库目录枚举）：**

| 目录 | 规则文件 |
|---|---|
| `itn/chinese/rules/` | `cardinal`, `char`, `date`, `fraction`, `license_plate`, `math`, `measure`, `money`, `postprocessor`, `time`, `train_number`, `whitelist` |
| `itn/english/rules/` | `cardinal`, `char`, `date`, `decimal`, `electronic`, `measure`, `money`, `ordinal`, `punctuation`, `telephone`, `time`, `whitelist`, `word` |

**相对 SenseVoice ITN 的增量覆盖（中文侧）：** 金额（money）、量词（measure）、日期（date）、分数（fraction）、车牌（license_plate）、火车车次（train_number）、数学式（math）。
**SenseVoice ITN 没有这些**；反之英文侧 WeTextProcessing 有 `telephone` / `electronic` / `ordinal` / `decimal`，SenseVoice ITN 也没有。

**体积与开销：**
- 官方 FST 资产（WeTextProcessing release `WeTextProcessing`）：`zh_itn_tagger.fst` **1.29 MB** + `zh_itn_verbalizer.fst` **0.07 MB**；更新的 `release-graph-v1.0.4.1.zip` 1.61 MB（含多语言，2024-08-01）。仓库最新 tag 为 `v1.2.0`（2026-06-10）。
- 纯 FST 查表，**无模型、无神经网络、CPU 开销可忽略**。这是它相对 LLM 的核心优势。

**能否在 Rust 里用？—— 诚实的结论：不顺路。**

| 路线 | 现状 |
|---|---|
| 官方 Python 包 | `pip install WeTextProcessing`，`weitn --text "二点五平方电线"` |
| 官方 C++ runtime | 仓库 `runtime/`，CMake 构建，`processor_main --tagger zh_itn_tagger.fst --verbalizer zh_itn_verbalizer.fst --text ...` |
| **官方 Rust 绑定** | ❌ **不存在**。`runtime/bindings/` 目录下**只有 `android/`**。 |
| **ONNX 导出** | ❌ **不存在**。WeTextProcessing 是 **FST / OpenFST 规则引擎**，不是神经网络，没有 ONNX 版本可言。（模型导出这条路在这里根本不适用。） |
| 第三方 Rust crate | ⚠️ `wetext-rs` 0.1.2（crates.io，发布 2025-12-30），API：`Normalizer::new("path/to/fsts", NormalizerConfig::new().with_lang(Language::Zh))`，`Language::{Zh, En, Ja}`，支持 TN/ITN。**但 crates.io 只有 141 次下载、仅 1 个版本**，属于实验性质；且仍然需要把 FST 文件放到磁盘上。 |

**结论：WeTextProcessing 进 Rust 只有「手写 FFI 绑官方 C++ runtime」或「赌 `wetext-rs`」两条路，都不划算。**

**那「开饭时间早上9点至下午5点」这类口语时间表达呢？**

- SenseVoice 内置 ITN **已经**产出 `开放时间早上9点至下午5点。`（官方文档实测，见 §2.1）。
- WeTextProcessing 中文 ITN 的 `time.py` / `date.py` 会给出更结构化的结果（如「上午九点」→「09:00」这类带上下午语义的归一），**但需要 Python 或 FFI**。
- **对本项目的性价比判断**：口语时间的**主要错误源是 ASR 本身听错（「开放」听成「开饭」ITN 顺手救回来了，「明天」听成「今天」ITN 救不回来）**，而不是 ITN 的表达形式。再上一套 ITN 引擎解决的只是「表达形式」，收益远小于风险（两套 ITN 串联可能互相打架）。

**⇒ 建议：整段模式维持 SenseVoice 内置 ITN，不引入 WeTextProcessing。**

---

### 3. 中文规范化

逐项过一遍「哪些值得做、哪些是噪音」：

| 项 | 工具 | 结论 | 依据 |
|---|---|---|---|
| **繁简转换** | OpenCC（<https://github.com/BYVoid/OpenCC>，10,073 stars，2026-10-22 仍有提交） | 🔸 **可选，但成本被低估** | Rust 绑定 `opencc-rust` 2.0.2（magiclen/opencc-rust）**必须先编译安装 OpenCC C++ 库**（`pkg-config` + `OPENCC_LIB_DIRS`/`OPENCC_LIBS`，且要链接 `opencc` + `marisa`）。对一个要出 macOS + Windows 安装包的 Tauri 应用，这是**引入 C++ 依赖链 + 跨平台打包坑**。且 ASR 输出本来就是简体，繁体输入场景罕见。 |
| **中文数字 ↔ 阿拉伯** | `chinese-number` 0.8.2（3,868,013 次下载，2026-06-28） | 🔸 **噪音** | ① SenseVoice ITN 已经做，且做得更准（能区分「九点」的时间语境）；② WeTextProcessing 的 cardinal/date/time 规则更完整；③ 纯文本替换没有上下文，容易把「三思」改成「3思」。 |
| **标点全半角** | 自己写 | 🔸 **小噪音，但顺手** | ASR 基本不产全角标点；只需在「英文句号」和「中文句号」之间做一次统一（半角 `.` 后面补空格）。成本极低，可作为纯字符串清洗的一部分。 |
| **英文大小写** | sherpa-onnx `OnlinePunctuation`（Edge-Punct-Casing） | ✅ **值得做（仅英文流式）** | 模型 `sherpa-onnx-online-punct-en-2024-08-06`，`model.int8.onnx` **7.1 MB** + `bpe.vocab` 146 KB；官方实测 `Elapsed seconds: 0.013 s`，输出 `how are you i am fine thank you` → `How are you? I am fine. Thank you.` —— **一次调用同时解决标点和大小写**。代价：只支持英文。 |
| **儿化 / 语气词** | WeTextProcessing TN 的 `remove_erhua=True` | ❌ **不做** | 那是 **TN（TTS 前处理）** 的选项，作用是把「事儿」还原成「事」以便 TTS 朗读。对 ASR **输出**做这件事等于**主动删除用户说的话**。 |

> 关于「儿化」的常见误解：语音识别输出「事儿」「玩儿」是**忠实还原口语**，不是错误。用户要的是可读的转写，不是被「规范化」过的文本。

---

### 4. 说话人分离（Diarization）

#### 4.1 sherpa-onnx 支持情况（按 crate 源码确认 —— 是的，存在）

`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-onnx-1.13.8/src/offline_speaker_diarization.rs`（整文件即为 diarization 模块，273 行）：

```rust
// L13-17
pub struct OfflineSpeakerSegmentationPyannoteModelConfig {
    pub model: Option<String>,
    pub window_shift_ratio: f32,      // 默认 0.1
}
// L42-47
pub struct OfflineSpeakerSegmentationModelConfig {
    pub pyannote: OfflineSpeakerSegmentationPyannoteModelConfig,
    pub num_threads: i32, pub debug: bool, pub provider: Option<String>,
}
// L75-80
pub struct FastClusteringConfig {
    pub num_clusters: i32,            // 默认 -1 = 自动
    pub threshold: f32,               // 默认 0.5
    pub compute_confidence: bool,     // 默认 false
}
// L103-109
pub struct OfflineSpeakerDiarizationConfig {
    pub segmentation: OfflineSpeakerSegmentationModelConfig,
    pub embedding: SpeakerEmbeddingExtractorConfig,   // speaker_embedding.rs:17-22
    pub clustering: FastClusteringConfig,
    pub min_duration_on: f32,         // 默认 0.3
    pub min_duration_off: f32,        // 默认 0.5
}
// L173 / L178 / L185
pub fn sample_rate(&self) -> i32
pub fn set_config(&self, config: &OfflineSpeakerDiarizationConfig)
pub fn process(&self, samples: &[f32]) -> Option<OfflineSpeakerDiarizationResult>
// L225-237
pub fn num_speakers(&self) -> i32
pub fn num_segments(&self) -> i32
pub fn sort_by_start_time(&self) -> Vec<OfflineSpeakerDiarizationSegment>
// L143-148
pub struct OfflineSpeakerDiarizationSegment {
    pub start: f32, pub end: f32, pub speaker: i32, pub confidence: f32,
}
```

**架构 = 分割（pyannote）+ 说话人嵌入（CAM++/3D-Speaker/NeMo）+ 快速聚类**，三件套各自可换。

> 注：官方 `HomophoneReplacerConfig`（在 `online_asr.rs:238-241`）里 `dict_dir` 已被标记 `// not used any more internally`，现在只剩 `lexicon` 和 `rule_fsts` 两个有效字段。

#### 4.2 可用模型与体积（官方 release 资产）

**分割模型**（release `speaker-segmentation-models`，2024-09-29）：

| 资产 | 体积 | 说明 |
|---|---|---|
| `sherpa-onnx-pyannote-segmentation-3-0.tar.bz2` | 7.0 MB | `model.onnx` 5.7 MB / `model.int8.onnx` **1.5 MB**；源自 <https://huggingface.co/pyannote/segmentation-3.0> |
| `sherpa-onnx-reverb-diarization-v1.tar.bz2` | 10.9 MB | ReverbDiarization |
| `sherpa-onnx-reverb-diarization-v2.tar.bz2` | 254.1 MB | v2（偏大） |

**说话人嵌入模型**（release `speaker-recongition-models`）—— 这里就是 **CAM++**：

| 资产 | 体积 |
|---|---|
| `wespeaker_en_voxceleb_CAM++.onnx` / `_LM.onnx` | 29.3 MB |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx` | **28.3 MB**（中文+英文，3D-Speaker CAM++） |
| `3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx` | 28.3 MB |
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx` | 39.6 MB |
| `nemo_en_titanet_small.onnx` | 40.3 MB |
| `nemo_en_titanet_large.onnx` | 101.4 MB |
| `wespeaker_zh_cnceleb_resnet34.onnx` | 26.5 MB |

**最小可用组合：pyannote int8（1.5 MB）+ CAM++ zh_en advanced（28.3 MB）≈ 30 MB。**

**关于 pyannote / CAM++ 本身：**
- pyannote 的官方 diarization pipeline（`pyannote/segmentation-3.0` + `pyannote/webrtcvad` + `pyannote/speaker-diarization-3.1`）**实际使用需要 GPU**；sherpa-onnx 做的是 **ONNX 移植版**，CPU 可跑（见下方 RTF）。本项目「无需 GPU」的约束下，只能走 sherpa 的 ONNX 版，**不能直接用 pyannote 原版 pipeline**。
- CAM++ 是 3D-Speaker 的说话人验证模型，在 sherpa-onnx 里扮演「嵌入提取器」角色，不是独立 diarization 模型。原始论文：*An Enhanced Network with Speaker Reduction for Long Distance Speaker Verification*（CAM++，3D-Speaker 仓库）。

#### 4.3 CPU 实时性（官方文档实测，56.861 s 四人中文音频）

| 组合 | Elapsed | **RTF** |
|---|---|---|
| pyannote fp32 + 3D-Speaker eres2net_base fp32 | 16.870 s | **0.297** |
| pyannote int8 + 3D-Speaker eres2net_base int8 | 13.679 s | **0.241** |
| pyannote fp32 + NeMo titanet_small fp32 | 6.756 s | **0.119** |
| pyannote int8 + NeMo titanet_small int8 | 6.231 s | **0.110** |
| （reverb-diarization-v1 同配置） | 25.7 / 22.3 / 11.5 / 9.7 s | 0.452 / 0.393 / 0.202 / 0.170 |

**RTF 0.11–0.30 → 比实时快 3–9 倍。CPU 实时性完全没问题。**

#### 4.4 对「会议纪要」的价值与本项目的冲突

**价值：高。** 会议纪要的核心痛点就是「谁说的」，而当前项目输出的是一坨没有说话人的纯文本，**在多说话人场景下基本不可用于纪要**。Diarization + ASR 的组合是行业标准做法（FunASR 也是这么组合的，见其 model card 里 `spk_model="cam++"`）。

**但对本项目有三个硬冲突：**

1. **架构冲突（最致命）**：`process(&self, samples: &[f32])` 需要**完整波形一次性在内存里**。本项目 AGENTS.md 明确规定「录音不限时长、内存恒定」，且整段模式是「边录边出字」的 `OfflineSession` 流。要做 diarization，必须新增一条「先录到文件 → 停止后再 diarize」的管线。
   - 历史教训：AGENTS.md 记录了「不要用 SenseVoice 覆盖流式结果」被删掉的原因之一正是「复核要求保留全部音频，和『录音不限时长、内存恒定』直接冲突」。**Diarization 会把这个冲突重新引进来。**
2. **说话人编号不稳定**：`num_clusters = -1` 时靠 `threshold`（默认 0.5）自动估计人数，官方示例反而推荐对已知人数用 `--clustering.num-clusters=4`、对未知用 `--clustering.cluster-threshold=0.9`。**阈值很敏感**，同一个人在不同录音里可能拿到不同编号。纪要产品需要「说话人 A/B/C」的稳定命名，用户得自己改名。
3. **置信度低**：`OfflineSpeakerDiarizationSegment.confidence` 依赖 `FastClusteringConfig.compute_confidence`（默认 `false`），聚类置信度不等于「这段一定是他」的置信度。

**⇒ 结论：**
- **如果**本产品要往「会议纪要」方向走，diarization 是**绕不开**的，值得单独立项做（需要新的「录音到文件 → 离线后处理」管线）。
- **如果**当前定位是「个人速记」，diarization 是**过度设计**：30 MB 模型 + 整段音频驻留 + 说话人编号不稳定，换来的是单人场景下零收益。
- **务实的中间态**：先用「静音间隔 + 说话人嵌入相似度」做**轻量说话人切换检测**（只判断「换人了没」，不聚类），成本 28 MB、不需要整段驻留、也能支撑「分段 + 换人标记」。这个思路可以以后再评估。

---

### 5. LLM 后编辑（Qwen3 / Llama 3.2 / gemma 等本地小模型）

#### 5.1 Rust 侧的接法（现成 crate，2026-10-23 状态）

| crate | 最新版 | 最后更新 | crates.io 下载量 | 说明 |
|---|---|---|---|---|
| `llama-cpp-2` | 0.1.160 | 2026-10-15 | 1,691,209 | llama.cpp 的 Rust 绑定（GGUF 直接读） |
| `mistralrs` | 0.8.1 | 2026-04-02 | 226,408 | 「Fast, flexible LLM inference.」 |
| `candle-core` | 0.11.0 | 2026-06-26 | 10,378,806 | 纯 Rust ML 框架 |
| `ort` | 2.0.0-rc.15 | 2026-10-19 | 23,064,599 | ONNX Runtime 的 Rust 封装 |

**注意版本状态**：`ort` 2.0 仍是 **rc**（release candidate）；`llama-cpp-2` 迭代极快（2026-10-15 刚发版）。任何一条都会给 Tauri 的 `Cargo.lock` 和交叉编译（macOS 本地 + Windows CI）带来持续维护成本。

**Tauri + sherpa-onnx 有官方先例**：`k2-fsa/sherpa-onnx/tauri-examples/`（`hello_world`、`non-streaming-speech-recognition-from-file`、`non-streaming-speech-recognition-from-microphone`），官方文档页 <https://k2-fsa.github.io/sherpa/onnx/tauri/index.html>；sherpa-onnx 还有专门的 `tauri` release 标签（2026-08-25）。
**但 sherpa-onnx 本身没有文本 LLM** —— 它是 ASR/TTS/VAD/KWS/说话人等语音引擎，**不提供「文本后编辑」模型**。所以 LLM 后编辑必然要引入**第二套推理栈**（llama.cpp / ONNX Runtime / Candle）。

**Tauri + 本地 LLM 的先例**（说明工程上可行，但不是 ASR 后编辑场景）：
- `benitomartin/tauri-app-bundle`（*Tauri LLM Chat App*）
- `armanisadeghi/matrx-local` 的 *Local LLM Inference Integration* 文档（Tauri 桌面端接本地 LLM）

#### 5.2 模型体积（一手，HuggingFace 文件大小）

| 模型 | 文件 | 体积 |
|---|---|---|
| Qwen3-0.6B | `Qwen3-0.6B-Q8_0.gguf` | **639.4 MB** |
| Qwen3-1.7B | `Qwen3-1.7B-Q8_0.gguf` | **1,834.4 MB** |
| Qwen3-4B | `Qwen3-4B-Q4_K_M.gguf` | **2,497.3 MB** |
| Qwen3-4B | `Qwen3-4B-Q8_0.gguf` | 4,280.4 MB |

> Qwen3 官方发布（<https://qwenlm.github.io/blog/qwen3/>，2025-04-29）：稠密模型 0.6B / 1.7B / 4B / 8B / 14B / 32B，**Apache 2.0**；支持 **hybrid thinking**（`enable_thinking=False` 即非思考模式，快速输出）；官方推荐本地部署工具就包含 llama.cpp。

#### 5.3 CPU 速度（一手实测）

**目前最权威的一份公开 CPU 实测是 Arm 自己发布的**：Qwen3-1.7B-Base Q4_K_M，**vivo X300 手机 CPU**（C1-Ultra/Premium/Pro，Android 16，llama.cpp + `ggml CPU_REPACK`，`-march=armv8.6-a+dotprod+i8mm`，4 线程，128 prompt / 128 generated，5 次预热 + 20 次实测）：

| 指标 | 优化版 |
|---|---|
| **Decode 吞吐** | **34.11 tok/s** |
| **TTFT**（首 token 延迟） | **809.12 ms** |
| p50 延迟（128 prompt + 128 generated） | 4564.02 ms |
| 模型体积 | 1127.98 MB |
| 峰值内存 | 2311.20 MB |
| MMLU 准确率 | 61.05%（基线 61.23%，量化损失 0.18 pp） |

> 来源：`Arm/qwen3-1-7b-base-q4-k-m-llamacpp-vivo-x300`（HuggingFace model card）。

**换算到桌面 CPU（一句话要多久）：**
- 手机 CPU 都能跑 34 tok/s，桌面 CPU（笔记本/台式）通常是这个的 **2–4 倍**，即 **70–140 tok/s**（1.7B Q4_K_M）。
- 一句 20–40 字的中文 ≈ 30–60 token → **改写一句约 0.3–0.9 s**（桌面）。
- 加上 prefill：把「系统提示 + 术语表 + 这一句」拼成 300–600 token 的 prompt，TTFT 在手机上是 0.8 s，桌面 **约 0.2–0.4 s**。
- **合计：桌面 CPU 上逐句后编辑约 0.5–1.3 s/句。** 对「边录边出字」的场景，这是**比识别本身还慢**的延迟（整段模式实测 RTF 0.04，5.6 s 音频只要 200 ms）。
- 4B Q4_K_M（2.5 GB）再慢约 2.5 倍 → **约 1.5–3 s/句**，内存峰值 >4 GB。

**关于线程**：llama.cpp 官方性能排查文档特别提醒，**线程数设过大会让 CPU 过饱和、反而更慢**（实测例：30B q4_0 在 7 核机上 `-t 7` 只有 1.7 tok/s，`-t 1` 反而 5.5 tok/s）。

#### 5.4 诚实的结论：值不值得？

**默认路径：不值得。** 理由，按严重程度排序：

1. **风险不对等 —— 这是决定性的。**
   ASR 的目标是**忠实**。一个错字，用户看得见、能自己改。而 LLM 后编辑是**重写**：0.6B–1.7B 模型在「纠错」任务上极易把「开饭时间」改成「开放时间」（对）、把「悬界芯片」改成「玄戒芯片」（也许对）、把「明天」改成「今天」（错）、把没听清的词**补全成一个通顺但虚构的句子**。
   **LLM 改写后的文本，用户无法区分「模型修对的」和「模型编的」。** 这对笔记类产品是负价值。
   → 而且项目里已经有**两个**专门为「忠实」设计的组件在做同样的事（SenseVoice ITN、CT-Transformer 标点），它们是**分类/规则模型**，输出可预期；LLM 是**生成模型**，输出不可预期。

2. **延迟倒挂。** 逐句 0.5–1.3 s 的后处理，和「边录边出字」的产品体验直接冲突。要么阻塞出字（体验崩），要么攒一大段再处理（失去实时性）。

3. **体积/内存代价与收益不匹配。** 为了「句首大写 + 补个逗号」引入 639 MB–2.5 GB 模型；同一件事 Edge-Punct-Casing 用 **7.1 MB** 就能做（而且是**同时**做标点 + 大小写）。

4. **第二套推理栈的维护成本。** `llama-cpp-2` 0.1.160 / `ort` 2.0.0-rc.15 都是快速迭代或 RC 状态；Windows CI 交叉编译 llama.cpp 还要编 C++ 静态库。

5. **3 个模型文件 × 多语言 × 多量化 = 组合爆炸**，下载管理、磁盘占用、首启加载时间都要重新设计。

**什么条件下值得：**

| 条件 | 说明 |
|---|---|
| ✅ **作为独立的「整理/摘要」功能**，而非「转写后处理」 | 「把这段口语整理成会议纪要」「提取待办事项」「分条」——这是**加价值**，不是**修错误**，LLM 的幻觉风险由用户复核兜住（输出是草稿，不是转写稿）。 |
| ✅ **用户显式触发** | 按钮式操作，用户明知道这是 AI 改写，随时可回退到原文。不放在默认路径上。 |
| ✅ **可换云端/本地** | 本地是默认，但允许用户指向自己的 Ollama / OpenAI 兼容端点（llama.cpp 的 `llama-server` 提供 OpenAI 兼容 API）。这样小模型能力不足的用户有出路。 |
| ✅ **必须用 ≥4B** | 0.6B/1.7B 的指令遵循能力做「只改错字、不动其他」这种保守指令非常不可靠。4B Q4_K_M ≈ 2.5 GB，内存峰值 >4 GB。 |
| ⚠️ **必须逐句 + 强约束** | 给「原文 + 术语表 + 只输出改后文本」的极简 prompt，关闭 thinking（`enable_thinking=False`），输出做 diff 后只应用高置信改动。 |

**⇒ 最终建议：把 LLM 后编辑定位成「可选的、用户主动触发的『AI 整理』功能」，用 llama.cpp / `llama-cpp-2`，模型至少 4B，且必须与原始转写并列展示。绝不要放进默认转写链路。**

---

### 6. 输出层：置信度 / 低可信标注 / 二次编辑

#### 6.1 crate 源码确认：`OfflineRecognizerResult` 有哪些字段

`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sherpa-onnx-1.13.8/src/offline_asr.rs:617-624`：

```rust
#[derive(Clone, Debug, Deserialize)]
/// Recognition result returned by [`OfflineStream::get_result`].
pub struct OfflineRecognizerResult {
    pub text: String,
    pub tokens: Vec<String>,
    pub timestamps: Option<Vec<f32>>,
    pub durations: Option<Vec<f32>>,
}
```

**只有 4 个字段。`ys_probs` 不存在。** 同理，
`online_asr.rs:435-445`：

```rust
#[derive(Clone, Debug, Deserialize)]
pub struct RecognizerResult {
    pub text: String,
    pub tokens: Vec<String>,
    pub timestamps: Option<Vec<f32>>,
    pub segment: Option<i32>,
    pub start_time: Option<f32>,
    pub is_final: bool,
}
```

> 流式结果里**有** `segment` / `start_time` / `is_final` —— 这三个字段对「按句做后处理」「定位句子时间轴」很有用。

#### 6.2 但是 `ys_probs` 在底层是有的 —— 只是被 serde 丢掉了

`offline_asr.rs:733-745`：

```rust
pub fn get_result(&self) -> Option<OfflineRecognizerResult> {
    unsafe {
        let cstr = sys::SherpaOnnxGetOfflineStreamResultAsJson(self.ptr);
        ...
        let s = CStr::from_ptr(cstr).to_string_lossy().into_owned();
        sys::SherpaOnnxDestroyOfflineStreamResultJson(cstr);
        serde_json::from_str(&s).ok()          // ← 只反序列化到 4 个字段
    }
}
```

C API 的 JSON 实际包含的字段，**在官方文档的 CLI 输出里可以直接看到**：

```json
{"lang": "<|zh|>", "emotion": "<|NEUTRAL|>", "event": "<|Speech|>",
 "text": "开放时间早上9点至下午5点。",
 "timestamps": [0.72, 0.96, ...],
 "durations": [],
 "tokens": ["开","放",...],
 "ys_log_probs": [],
 "words": []}
```

**关键事实：**
1. JSON 里字段名叫 **`ys_log_probs`**（不是 `ys_probs`），且 Rust 结构体**没有**这个字段 → serde 默认**静默忽略未知字段**，数据在 C 层被丢掉了。
2. **`ys_log_probs` 在 SenseVoice 上是空数组 `[]`** —— SenseVoice 的 ONNX 导出没有产出 ys 分支的 log-prob。
   → **结论：对本项目的整段模式（SenseVoice），即使把字段暴露出来，也拿不到 token 级置信度。**
3. `ys_log_probs` 是 **transducer** 模型（zipformer）特有的。在本项目的**流式模式**里它应该是实心的 —— 但 `RecognizerResult` 同样没有这个字段。

**要拿到流式 token 置信度，两条路：**

- **路 A（可行，但要多加一个直接依赖）**：直接调 C API，自己定义结构体。
  ```rust
  // 自己加依赖：sherpa-onnx-sys = "1.13.8"（版本必须与 sherpa-onnx 一致）
  #[derive(serde::Deserialize)]
  struct RawOnlineResult {
      text: String,
      tokens: Vec<String>,
      timestamps: Option<Vec<f32>>,
      ys_log_probs: Option<Vec<f32>>,   // ← 自己加（注意是 log_probs，不是 probs）
      // ...
  }
  // 然后 sys::SherpaOnnxGetOnlineStreamResultAsJson(recognizer_ptr, stream_ptr)
  //     + sys::SherpaOnnxDestroyOnlineStreamResultJson(ptr)
  ```
  `sherpa-onnx-sys-1.13.8/src/online_asr.rs` 已导出这两个符号（crate 内部用法见 `sherpa-onnx-1.13.8/src/online_asr.rs:422-429`）。
  ⚠️ **注意**：`sherpa-onnx-1.13.8/src/lib.rs` 只 `pub use` 了各业务模块（L290-306），**没有 re-export `sherpa_onnx_sys`**，所以走这条路必须在本项目 `Cargo.toml` 里**显式加 `sherpa-onnx-sys = "1.13.8"`**（版本必须严格对齐，否则会拉进两份 C 静态库）。
  ⚠️ 另外 raw JSON 里可能还有 `words` / `segment` / `start_time` 等字段，serde 会忽略未知字段，安全。
- **路 B（最省事）**：给 sherpa-onnx 上游提 PR，给 `RecognizerResult` 加 `pub ys_log_probs: Option<Vec<f32>>`。改动只有一行（serde 会自动填充）。**这是最干净的做法。**

#### 6.3 时间戳：能用

- `timestamps: Option<Vec<f32>>` —— 官方 SenseVoice 示例中是**实心的**（每 token 一个起始时间，单位秒）。
- `durations: Option<Vec<f32>>` —— 官方示例中为**空**。
- ⇒ **token 级时间戳可以直接用**（做「点文字跳到音频位置」是可行的），但**整段模式按 VAD 分句**（`asr.rs` 的 `OfflineSession`）时，需要把每句的 token 时间戳与句起点对齐才能得到全局时间轴。
- ⚠️ **注意 §5 的 HomophoneReplacer 副作用**：官方文档的示例里，开启 HR 后 `text` 变成了「玄戒芯片」，但同一份 JSON 的 `tokens` 仍然是 `["悬","界",...]` —— **文本被替换了，token 数组没有同步**。这意味着 **HR 之后 `tokens` / `timestamps` 与 `text` 不再一一对应**。如果要做「点击词高亮时间轴」，这是一个必须处理的坑。

#### 6.4 「低可信片段标注 / 二次编辑」值不值得做？

**值得做的（低风险、纯前端/纯逻辑）：**

| 功能 | 实现 | 价值 |
|---|---|---|
| **点词回跳音频** | 用 `timestamps` + `tokens`（+ VAD 句起点）算全局时间轴 | 高。笔记软件的核心交互 |
| **段落/句子时间戳展示** | 现有 `stream:segment` 已带 `start`/`end` | 中 |
| **二次编辑 + 保留原文** | 编辑结果存到历史，标注「已编辑」 | 中。转写稿本来就要能改 |
| **疑似错词提示** | **规则驱动**：用户词表（hotwords / 术语表）里的词若不在输出中 → 提示「可能没识别到『XX』」；或用 ASR 自带的 `hotwords` 机制在识别阶段就纠 | 中高。**比置信度更可靠** |

**不建议做的：**

- **基于 `ys_log_probs` 的置信度高亮** —— ① 整段模式（SenseVoice）**根本拿不到**（§6.2）；② 流式模式能拿到，但「token log-prob 低」≠「用户听错了」，transducer 的 beam search 里 log-prob 受语言模型影响，阈值极难标定；③ 对中文而言，逐 token 的置信度和「这个词是不是术语」的关联很弱。
- **把置信度做成「红色波浪线」** —— 会给用户大量误报（正常口语里低分词很常见），且 sherpa 没有做置信度标定的公开工作，阈值只能靠拍脑袋。

**⇒ 更划算的替代方案：**
用**「用户词表 + 规则」**代替「模型置信度」做低可信提示：
1. 用户在设置里维护「专有名词表」（人名、产品名、公司名）；
2. 流式模式把这些词喂给 sherpa 的 **hotwords**（`OnlineRecognizerConfig.hotwords_buf: Option<Vec<u8>>` / `hotwords_file` / `hotwords_score`，`online_asr.rs:272-274`）→ 识别阶段就纠；
3. 整段模式用 **HomophoneReplacer**（`lexicon` + `rule_fsts`）纠同音错词；
4. 输出层再做一条规则：「词表里的词一个都没出现 → 提示可能漏识别」。

**这套组合 100% 是规则/分类模型驱动的，可预期、零幻觉、体积 1.5 MB 级别**，比 LLM 靠谱得多。

---

## 不建议做的（及原因）

| # | 不建议做 | 原因（含一手依据） |
|---|---|---|
| 1 | **在整段（SenseVoice）结果上再套 CT-Transformer 标点** | SenseVoice `use_itn=1` 时输出已含 `。，、`（官方 SenseVoice 文档 JSON 输出可见）；CT-Transformer 再插一遍 → `。。`。AGENTS.md 已明文禁止。 |
| 2 | **用 SenseVoice 覆盖流式结果**（含「流式也用 SenseVoice 打标点」的变体） | ① SenseVoice 是 **OfflineRecognizer**，无法逐块流式；② AGENTS.md 实测：流式 99% vs SenseVoice 91%，且「明天」被听成「今天」；③ 需要保留全部音频，与「录音不限时长、内存恒定」冲突。 |
| 3 | **引入 WeTextProcessing 作为 ITN 主力** | ① 官方**无 Rust 绑定**（`runtime/bindings/` 只有 `android/`）、**无 ONNX 导出**（它是 FST 规则引擎，不是神经网络）；② 唯一 Rust 实现 `wetext-rs` 仅 141 次下载 / 单版本；③ SenseVoice ITN 已覆盖中文数字+标点+首字母大写（官方实测 `九→9`、`五→5`、补 `。`、`the→The`），**而且它会犯错**（官方示例 `gold→code`），双跑 ITN 只会放大错误。 |
| 4 | **把 LLM（Qwen3 0.6B/1.7B）放进默认转写链路** | ① 生成模型会改写/编造内容，错误不可见且不可预期 —— 对「忠实转写」是负价值；② 桌面 CPU 逐句 0.5–1.3 s（依据：Arm 实测手机 CPU 上 1.7B Q4_K_M = 34.11 tok/s / TTFT 809 ms），延迟倒挂；③ 为补标点+大写引入 639 MB–2.5 GB 模型，而 Edge-Punct-Casing 只要 7.1 MB；④ 多一套 llama.cpp/ORT 推理栈 + Windows 交叉编译负担。**如果要做，定位成用户主动触发的「AI 整理」功能，且至少 4B。** |
| 5 | **为「中文标点」去找 SplitPunct 之类的模型** | 官方 `punctuation-models` release 只有 3 个资产，crate 配置只有 `ct_transformer` / `cnn_bilstm` 两个槽位；arXiv 检索 `"SplitPunct"` = 0 条。**该名字在 sherpa-onnx 生态里不存在。** |
| 6 | **把 `ys_probs` 当作整段模式的置信度来源** | crate 的 `OfflineRecognizerResult` 只有 `text/tokens/timestamps/durations`；底层 JSON 里的字段名是 `ys_log_probs`，且 **SenseVoice 的输出中它是 `[]`**（官方文档可见）。即使把字段暴露出来也拿不到值。流式（transducer）才有。 |
| 7 | **做「逐 token 置信度高亮 / 红色波浪线」** | 见 §6.4：无标定的阈值 + SenseVoice 无此数据 = 大量误报。改用「用户词表 + hotwords + HomophoneReplacer」的规则式提示。 |
| 8 | **做儿化/语气词「规范化」** | WeTextProcessing 的 `remove_erhua` 是 **TN（TTS 前处理）** 选项，用在 ASR **输出**上等于主动删除用户说的话。「事儿」「玩儿」是忠实口语，不是错误。 |
| 9 | **引入 OpenCC 做繁简转换（当前阶段）** | `opencc-rust` **必须先编译安装 OpenCC C++ 库**（`pkg-config` + `opencc` + `marisa`），对 macOS + Windows 双平台 Tauri 打包是实打实的 C++ 依赖链；且 ASR 输出本就是简体，繁体输入是长尾场景。**除非有明确的繁体用户诉求，否则不值得。** |
| 10 | **用 `chinese-number` 之类做中文数字↔阿拉伯转换** | 纯文本替换没有上下文，容易误改；SenseVoice ITN 已在识别层做了更好的版本。属于重复劳动且更差。 |
| 11 | **现在就给 diarization 加进主链路** | `process(&[f32])` 要整段音频驻留，与「内存恒定/不限时长」冲突；说话人编号靠 `clustering.threshold` 估计、跨录音不稳定；单人场景零收益。**要做就单独立项，且走「录音到文件 → 离线后处理」管线。** |
| 12 | **用 `OnlinePunctuation` 给中文补标点** | 该模型（Edge-Punct-Casing）**只支持英文**（官方文档：*"This model supports only English."*），且 C++ 实现 `AddPunctuationWithCase` 是 `const`、无跨调用状态 —— 它不是增量流式模型，是「一段文本一次性进一次性出」。 |

---

## 参考来源清单

> 全部为一手来源（官方仓库 / 官方 model card / 官方文档 / 官方 release API / 本地 crate 源码）。

### A. sherpa-onnx（标点、说话人、热词、Tauri）

1. 官方标点预训练模型文档（CT-Transformer / Edge-Punct-Casing，体积、CPU 耗时实测）— <https://k2-fsa.github.io/sherpa/onnx/punctuation/pretrained_models.html>
2. 官方标点模型 release 资产（仅 3 个模型）— <https://github.com/k2-fsa/sherpa-onnx/releases/tag/punctuation-models>
3. sherpa-onnx 仓库（2026-10-22 仍有提交）— <https://github.com/k2-fsa/sherpa-onnx>
4. 官方 Tauri 桌面应用文档 + 示例仓库 — <https://k2-fsa.github.io/sherpa/onnx/tauri/index.html> ｜ <https://github.com/k2-fsa/sherpa-onnx/tree/master/tauri-examples>
5. 官方 SenseVoice 预训练模型文档（**ITN 开关前后的实测输出**、`use_itn` 与标点的关系、RTF、粤语版不支持标点）— <https://k2-fsa.github.io/sherpa/onnx/sense-voice/pretrained.html>
6. 官方说话人分离文档（RTF 0.110–0.297 实测、pyannote-3.0 / reverb 模型、聚类参数）— <https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/models.html>
7. 官方说话人分离 release 资产 — <https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-segmentation-models>
8. 官方说话人嵌入 release 资产（CAM++ / 3D-Speaker / NeMo）— <https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models>
9. 官方 Hotwords / contextual biasing 文档（**「Only transducer models support hotwords」**、`modified_beam_search` 要求、modeling-unit、boosting score 语法）— <https://k2-fsa.github.io/sherpa/onnx/hotwords/index.html>
10. 官方 HomophoneReplacer（拼音词组匹配替换）文档与示例（悬界→玄戒、人工投→人头）— <https://k2-fsa.github.io/sherpa/onnx/homophone-replacer/index.html> ｜ release 资产（`lexicon.txt` 1.37 MB、`replace.fst` 0.25 MB、`dict.tar.bz2` 4.06 MB）— <https://github.com/k2-fsa/sherpa-onnx/releases/tag/hr-files>
11. 本地 crate 源码（字段名以此为准）：
    - `sherpa-onnx-1.13.8/src/offline_punctuation.rs`（`OfflinePunctuationModelConfig.ct_transformer`）
    - `sherpa-onnx-1.13.8/src/online_punctuation.rs`（`OnlinePunctuationModelConfig.cnn_bilstm` / `bpe_vocab`）
    - `sherpa-onnx-1.13.8/src/offline_speaker_diarization.rs`（`OfflineSpeakerDiarizationConfig` 全字段）
    - `sherpa-onnx-1.13.8/src/offline_asr.rs`（`OfflineRecognizerResult` L617-624、`OfflineSenseVoiceModelConfig.use_itn` L223-227、`hotwords_file`/`hotwords_score` L563-564、`get_result` L733-745）
    - `sherpa-onnx-1.13.8/src/online_asr.rs`（`RecognizerResult` L435-445、`hotwords_buf` L272-274、`HomophoneReplacerConfig` L238-241）
    - `sherpa-onnx-1.13.8/src/speaker_embedding.rs`（`SpeakerEmbeddingExtractorConfig` L17-22）
    - `sherpa-onnx-sys-1.13.8/src/offline_asr.rs`（`SherpaOnnxGetOfflineStreamResultAsJson` L278）
    - `sherpa-onnx-1.13.8/src/lib.rs`（L290-306 只 re-export 各业务模块，**未 re-export `sherpa_onnx_sys`**）
12. sherpa-onnx C++ 实现（说明 OnlinePunctuation 无状态、CT-Transformer 的 200 字切分）：
    - `sherpa-onnx/csrc/online-punctuation-cnn-bilstm-impl.h`
    - `sherpa-onnx/csrc/offline-punctuation-ct-transformer-impl.h`
    - `sherpa-onnx/csrc/homophone-replacer.cc`

### B. SenseVoice / FunASR

13. SenseVoice 仓库（已迁至 QwenAudio）— <https://github.com/QwenAudio/SenseVoice>
14. FunASR 仓库（`ct-punc` / `spk_model="cam++"` 组合用法）— <https://github.com/modelscope/FunASR>
15. **CT-Transformer 官方 model card**（precision 53.8 / recall 60.0 / **f1 56.5**、33M 训练样本、适用范围）— <https://www.modelscope.cn/models/iic/punc_ct-transformer_zh-cn-common-vocab272727-pytorch>
16. CT-Transformer 论文（ICASSP 2020，Controllable Time-delay Transformer）— <https://arxiv.org/abs/2003.01309>
17. Edge-Punct-Casing 论文（英文标点 + 大小写，online-punct-en 的来源）— <https://arxiv.org/abs/2407.13142>
18. pyannote segmentation-3.0 — <https://huggingface.co/pyannote/segmentation-3.0>

### C. ITN / 规范化

19. WeTextProcessing 官方仓库（Apache-2.0，2026-07-29 有提交；`itn/chinese/rules`、`itn/english/rules` 规则清单；`runtime/` C++ 实现；`runtime/bindings/` **仅 android**）— <https://github.com/wenet-e2e/WeTextProcessing>
20. WeTextProcessing FST 资产（`zh_itn_tagger.fst` 1.29 MB、`zh_itn_verbalizer.fst` 0.07 MB）— <https://github.com/wenet-e2e/WeTextProcessing/releases/tag/WeTextProcessing>
21. `wetext-rs` 0.1.2（第三方 Rust 移植，141 次下载，API：`Normalizer` / `NormalizerConfig` / `Language`）— <https://docs.rs/wetext-rs/0.1.2/wetext_rs/> ｜ <https://crates.io/crates/wetext-rs>
22. OpenCC 官方仓库（10,073 stars，2026-10-22 有提交）— <https://github.com/BYVoid/OpenCC>
23. `opencc-rust` 2.0.2（**需预编译 OpenCC C++ 库**）— <https://github.com/magiclen/opencc-rust> ｜ <https://docs.rs/opencc-rust/>
24. `chinese-number` 0.8.2（3,868,013 次下载）— <https://crates.io/crates/chinese-number>

### D. LLM 后编辑

25. Qwen3 官方发布博客（稠密模型 0.6B/1.7B/4B/8B/14B/32B，Apache 2.0，hybrid thinking，`enable_thinking=False`，官方推荐 llama.cpp 做本地部署）— <https://qwenlm.github.io/blog/qwen3/>
26. **Arm 官方 CPU 实测**（Qwen3-1.7B-Base Q4_K_M，vivo X300 手机 CPU：34.11 tok/s decode、TTFT 809.12 ms、1127.98 MB、峰值内存 2311.20 MB、MMLU 61.05%）— <https://huggingface.co/Arm/qwen3-1-7b-base-q4-k-m-llamacpp-vivo-x300>
27. Qwen3 GGUF 官方文件体积 — <https://huggingface.co/Qwen/Qwen3-0.6B-GGUF> ｜ <https://huggingface.co/Qwen/Qwen3-1.7B-GGUF> ｜ <https://huggingface.co/Qwen/Qwen3-4B-GGUF>
28. llama.cpp 仓库（GGUF、k-quant、CPU 后端）— <https://github.com/ggml-org/llama.cpp>
29. llama.cpp 官方性能排查文档（**线程数过大会让 CPU 过饱和反而更慢**；实测 -t 7 = 1.7 tok/s vs -t 1 = 5.5 tok/s）— <https://github.com/ggml-org/llama.cpp/blob/master/docs/development/token_generation_performance_tips.md>
30. `llama-cpp-2` 0.1.160 — <https://crates.io/crates/llama-cpp-2>
31. `mistralrs` 0.8.1 — <https://crates.io/crates/mistralrs>
32. `candle-core` 0.11.0 — <https://crates.io/crates/candle-core>
33. `ort` 2.0.0-rc.15 — <https://crates.io/crates/ort>
34. Tauri + 本地 LLM 工程先例 — <https://github.com/benitomartin/tauri-app-bundle> ｜ <https://github.com/armanisadeghi/matrx-local/blob/main/docs/local-llm-inference-integration.md>

### E. 本仓库现状核对（本次调研读取，未修改）

35. `src-tauri/src/models.rs:101-109` — `PUNCT` 常量：`csukuangfj/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12/.../model.onnx`，`bytes: 294_372_519`（281 MiB，**fp32**），sha256 已固定。
36. `src-tauri/src/asr.rs:413-440` — `PunctuationEngine`（包装 `OfflinePunctuation`，`apply()` 失败时原样返回）。
37. `src-tauri/src/asr.rs:648-650` — `StreamEmitter` 仅在句段定稿时调用 `punctuator.apply(&self.partial)`，partial 不加标点。
