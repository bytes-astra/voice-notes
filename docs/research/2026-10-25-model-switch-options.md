# 换模型备选方案（2026-10-25 核实版）

> 目的：把「换 ASR 模型」从路线图里的一行，拆成**随时可以执行**的一份文档 —— 需要时照着做，
> 不用重新调研。
>
> 本文的**所有文件大小与 sha256 都是 2026-10-25 从 Hugging Face API 实测拉取的**，
> 仓库路径、commit、文件名都是真实的（不是从文档转述）。配置字段名是从本机
> `sherpa-onnx-1.13.8` 的 crate 源码里读出来的。

## 0. 先说结论

| 候选 | 换的是 | 下载量（实测） | sherpa 1.13.8 支持 | 关键价值 | 关键代价 |
|---|---|---|---|---|---|
| **X-ASR** | 流式（+离线） | **168 MiB** | ✅ 原生（`model_type`/`modeling_unit`/`bpe_vocab` 字段都有） | **对症**：流式 CER 16.24% 明显高于整段 11.28%；自带动词性模型 + 自带标点 + 中英混说 | 不是 cjkchar，喂数节奏要改；要重写流式的 tokens 假设 |
| **Qwen3-ASR 0.6B** | 整段 | **941 MiB** | ✅ 原生（`OfflineQwen3ASRModelConfig`） | **整段模式终于能用热词**（`hotwords` 字段） | 自回归 LLM 解码，慢；`max_total_len`/`max_new_tokens` 要显式调 |
| **FireRed ASR2** | 整段 | **1.15 GiB** | ✅ 原生（`OfflineFireRedAsrModelConfig`） | 中文绝对精度最高一档 | **没有热词接口、没有标点**；体积最大；下载比现有整段模型大 3 倍 |

**建议顺序：先试 X-ASR**（体积和现状同量级、对症流式这个最大短板），**Qwen3-ASR 放第二个**
（唯一能解锁整段热词的），**FireRed ASR2 最后**（收益没实测过、体积最大、还缺标点）。

## 1. 为什么要换：现状的实测短板

同一批语料（71 条 wav，整段 vs 流式，2026-10-24 探针）：

| 类别 | 整段（SenseVoice） | 流式（Zipformer） |
|---|---|---|
| 真人（40 条） | **11.28%** | **16.24%** |
| 真人+噪声（5 条） | 2.78% | 16.67% |
| TTS（20 条） | 16.39% | 43.28% |
| 长句（1 条） | 12.50% | 40.00% |

**流式是整段的 1.4 倍（相对）** —— 这是现在最大的质量缺口，也是 X-ASR 对症的地方。

另外两条换模型的动机：

- **整段模式没有热词**。`sherpa-onnx` 明确只给 transducer 支持热词（给 SenseVoice 设
  `modified_beam_search` 或 `create_stream_with_hotwords` 会 `_Exit(-1)` 杀进程，已复现）。
  术语表功能现在**只在流式生效**，整段录专名必错。Qwen3-ASR 的 `hotwords` 字段能解这个。
- **多语种**。现在流式只覆盖中文，选粤/日/韩会退回整段。Qwen3-ASR / X-ASR 都是中英双码
  甚至多语种。

## 2. 候选一：X-ASR（换流式，推荐先试）

### 2.1 模型文件（HF API 实测，commit `bbc3ebbf8f134848f804967f3a3b69f981308a85`）

仓库：`csukuangfj2/sherpa-onnx-x-asr-zipformer-transducer-zh-en-int8-2026-06-03`

| 文件 | bytes | sha256 |
|---|---|---|
| `encoder-epoch-99-avg-1.int8.onnx` | 161,744,450 | `2964d4afd257762931184b15fac04bcae79158b6b84fd9441fc33c8eb5a360ab` |
| `decoder-epoch-99-avg-1.onnx` | 11,309,084 | `c6a045cd52d4d2693c19218f404be6e37b6130e1244814ce31fa2188e4355cb5` |
| `joiner-epoch-99-avg-1.int8.onnx` | 2,581,422 | `82e88dad29301e29144565cd3b7347fa163f07ddb30ea60346b7c4c536adf4ba` |
| `bpe.model` | 359,499 | `f3858eb428e082d7e89f13b0711c7a2ce195fa98dd5b80879645d346fb3e01e7` |
| `tokens.txt` | 59,240 | （非 LFS，无 sha） |

**合计 176,053,695 B ≈ 168 MiB** —— 比现在的流式模型（161,141,793 B）**只大 15 MB**。

还有两个变体仓库：
- `csukuangfj2/sherpa-onnx-x-asr-zipformer-transducer-zh-en-punct-int8-2026-06-03` —— **自带标点**，
  可以省掉现在 281 MiB 的 CT-Transformer 标点模型
- `csukuangfj2/sherpa-onnx-x-asr-zipformer-transducer-zh-en-2026-06-03` —— fp32

上游：`GilgameshWind/X-ASR-zh-en`（HF tags 明确写了 `sherpa-onnx` / `streaming-asr` / `offline-asr` /
`x-asr-zipformer-transducer` / `code-switching`）。

### 2.2 crate 配置（1.13.8 原生支持，不用升 crate）

`OnlineModelConfig`（`sherpa-onnx-1.13.8/src/online_asr.rs:128`）里有这三个字段：

```rust
pub model_type: Option<String>,      // "zipformer2"
pub modeling_unit: Option<String>,   // "bpe"（现在是 "cjkchar"）
pub bpe_vocab: Option<String>,       // bpe.model 的路径
```

所以 `StreamingEngine::new` 要加：

```rust
config.model_config.model_type = Some("zipformer2".into());
config.model_config.modeling_unit = Some("bpe".into());
config.model_config.bpe_vocab = Some(paths.bpe_vocab.clone());
```

### 2.3 改动面

| 文件 | 改什么 |
|---|---|
| `models.rs` | 加 `STREAM_X_ASR_*` 常量（5 个 Asset）；`StreamingPaths` 加 `bpe_vocab` 字段 |
| `engine.rs` | `StreamingEngine::new` 加上面三个字段；`modeling_unit` 从 cjkchar 变 bpe |
| `asr.rs` | `StreamingSession` 的 `TAIL_SILENCE_SAMPLES` / 收尾逻辑可能要重测（bpe 的解码边界不一样） |
| `record.rs` | **喂数节奏**：现在是 200 ms（3200 样本）一片，X-ASR 的官方建议是它自己的 chunk 尺寸（480 ms） |
| `lib.rs` | `download_streaming_model` 多下 `bpe.model`；`status` 报新模型 |

### 2.4 风险

- **`modeling_unit` 从 cjkchar 变 bpe** 是行为级变更：热词的 `hotwords_buf` 语义会变（bpe 词表下
  热词是子串匹配，匹配规则不一样），现在实测有效的热词效果**要重新验证**。
- 端点规则（`rule1/2/3`）在 bpe 上的行为未知，要用 `probe_streaming_endpoint_cut` 重新量。
- 现在的流式是「纯中文」，X-ASR 是中英双码，**中英混说的输入行为会变**。

## 3. 候选二：Qwen3-ASR 0.6B INT8（换整段，解锁热词）

### 3.1 模型文件（HF API 实测，commit `68818b2313fe77bd06f6a7c5068ff3ef59d02b8a`）

仓库：`csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25`

| 文件 | bytes | sha256 |
|---|---|---|
| `conv_frontend.onnx` | 44,148,281 | `d22dc4423e0940e49884e903d2ea2f7e5567c14fc1aed97e4e26d6b8f208ef9e` |
| `encoder.int8.onnx` | 182,491,662 | `60748d3e6744a57c9c91e1b17424a6c2990567e8adceb0783940c03ed98fa9d9` |
| `decoder.int8.onnx` | 755,914,231 | `4f6885be5959ae26af3089d38ee7972c5fafbeeb1cf8d5e76eab6d8b61ca5771` |
| `tokenizer/vocab.json` | 2,776,833 | `ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910` |
| `tokenizer/merges.txt` | 1,671,853 | （非 LFS，无 sha） |
| `tokenizer/tokenizer_config.json` | 12,487 | `4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c` |

**合计 987,015,347 B ≈ 941 MiB** —— 是现有整段模型（239,233,841 B / 228 MiB）的 **4 倍**。

上游：`Qwen/Qwen3-ASR-0.6B`（HF 上是 safetensors 格式，**不能直接用**；要用的是上面
`csukuangfj2` 导出的 ONNX 版）。

### 3.2 crate 配置

`OfflineQwen3ASRModelConfig`（`sherpa-onnx-1.13.8/src/offline_asr.rs:325`）：

```rust
pub conv_frontend: Option<String>,
pub encoder: Option<String>,
pub decoder: Option<String>,
pub tokenizer: Option<String>,
pub max_total_len: i32,      // 默认 512  ← 会截断长音频
pub max_new_tokens: i32,     // 默认 128  ← 中文长句不够
pub temperature: f32,        // 默认 1e-6
pub top_p: f32,              // 默认 0.8
pub seed: i32,               // 默认 42
pub hotwords: Option<String>,  // ← 整段热词就是靠这个
```

**两个必须显式设的参数**：

- `max_total_len`：默认 512 帧（~32 s @16k），**超了直接截断**。本项目整段模式是「边录边按句
  识别」，单句一般 < 30 s，但 VAD 偶发长句（实测能到 1 分钟以上）会中招 → **必须调大**
  （建议 ≥ 3000，覆盖 180 s）。
- `max_new_tokens`：默认 128 token。中文 30 s 约 100–150 字，**128 会被截断** → 建议 ≥ 512。

### 3.3 改动面

| 文件 | 改什么 |
|---|---|
| `models.rs` | 加 `QWEN3_*` 常量（6 个 Asset）；`ModelPaths` 结构要扩展（现在只有 `model`/`tokens`/`vad` 三个字段，装不下 conv_frontend + encoder + decoder + tokenizer 四件套） |
| `engine.rs` | `Engine::new` 现在是 SenseVoice 专用（`config.model_config.sense_voice`），要抽象出「模型种类」分支 |
| `asr.rs` | 整段模式的热词要接到 `config.model_config.qwen3_asr.hotwords`（**注意它是文件路径，不是 buf**） |
| `lib.rs` | `download_model` / `load_engine` / `status` 全部要按模型种类分支；前端要有模型选择 |

### 3.4 风险

- **慢**。自回归 LLM 解码，官方 CPU RTF 0.077–0.168（SenseVoice 约 0.01 量级）。
  本项目的整段模式是「边录边按句识别」，延迟直接乘上去。
- **和「内存恒定 / 消费式缓冲」的架构不冲突**（还是逐句识别），但**延迟会变差**。
- `max_total_len` / `max_new_tokens` 配错就是「长句被静默截断」，**这类 bug 很难靠肉眼发现**，
  必须用探针量。
- 热词走文件路径 → 要新增一个「把术语表写成文件」的流程（现在的术语表是设置项里的字符串数组）。

## 4. 候选三：FireRed ASR2（整段，中文精度最高一档）

### 4.1 模型文件（HF API 实测，commit `374cff185e952c40fcf2f6da972a3b6cf340608d`）

仓库：`csukuangfj2/sherpa-onnx-fire-red-asr2-zh_en-int8-2026-02-26`

| 文件 | bytes | sha256 |
|---|---|---|
| `encoder.int8.onnx` | 817,286,833 | `54048d66b6e8f3c80ea7ce95efe794587b0fd81d7271651d0decd3803852ae82` |
| `decoder.int8.onnx` | 417,291,928 | `b840ce7196ae4a14d05ae84bbf56082b6b61ccec5610fda907dddbcea37354ff` |
| `tokens.txt` | 79,172 | （非 LFS，无 sha） |

**合计 1,234,657,933 B ≈ 1.15 GiB**。

另有 CTC 单文件版：`csukuangfj2/sherpa-onnx-fire-red-asr2-ctc-zh_en-int8-2026-02-25`
（对应 `config.model_config.fire_red_asr_ctc.model`）。

上游：`FireRedTeam/FireRedASR2-AED`（36 likes / 1180 downloads / arxiv:2603.10420）、
`FireRedTeam/FireRedASR2-LLM`。

### 4.2 crate 配置

```rust
// transducer 版
config.model_config.fire_red_asr = OfflineFireRedAsrModelConfig { encoder, decoder };
// CTC 版
config.model_config.fire_red_asr_ctc = OfflineFireRedAsrCtcModelConfig { model };
```

（`sherpa-onnx-1.13.8/src/offline_asr.rs:143` / `:311`）

### 4.3 风险

- **没有热词接口**（transducer 版的 `hotwords` 在 Rust 绑定里没暴露），所以**解决不了整段专名问题**。
- **没有标点**。整段现在靠 SenseVoice 自带 ITN 出标点，换了就没标点了，得另配标点模型
  （现在那个 281 MiB 的 CT-Transformer 可以配，但体积就上去了）。
- 体积最大（1.15 GiB），是现有整段模型的 5 倍。
- 官方精度数字（AIShell1 0.57% CER 等）**跟我们的语料不可比**（不同测试集、不同协议）。

## 5. 怎么执行：先做 spike，别直接改产品代码

**换模型是大事，但第一步不该动产品代码。** 正确顺序：

### 步骤 1：写一个独立探针（不改产品代码）

新建 `src-tauri/tests/model_switch_probe.rs`，只打印不断言：

```rust
// 1. 加载新模型（配置从上面抄）
// 2. 跑 tests/fixtures/quality/ 全语料
// 3. 算 CER，输出分类汇总（real / noisy / tts / long / num）
// 4. 额外打印：耗时、RTF、段数
```

**验收口径**：跟现在的基线并排比 —— 整段 `real 11.28%`、流式 `real 16.24%`。
注意 TTS 语料只能做相对对比（路线图原话：「别拿 TTS 的绝对 CER 当质量结论」）。

### 步骤 2：算清楚四个数

| 指标 | 为什么重要 |
|---|---|
| CER（分 real / noisy / tts / long） | 质量本身 |
| RTF | 决定用户等不等得起（整段是边录边出字，RTF 直接乘到延迟上） |
| 段数 | 句子完整度（现在的 2×2 矩阵里，段数是唯一有区分度的指标） |
| 下载量 | 决定用户会不会放弃下载 |

### 步骤 3：过了这关再谈接进产品

接进产品要动的东西见各节的「改动面」表格。**这一步不急** —— 先把数拿到。

## 6. 硬前提：没有语料，换了也测不出

现有的 71 条语料里，**40 条真人样本全部来自 minds14 的「查账户余额」一个场景**（领域窄），
另外 20 条是 TTS 合成音。拿这份语料横比三个模型，只能得到「在电话客服场景下谁好」，
不能得到「谁更适合我日常的口述」。

**换模型之前，先补 10–30 条你日常真的会录的句子**（wav + 人工听写文本），放到
`src-tauri/tests/fixtures/<名字>.wav` + 同名 `.txt`，探针会自动算 CER。
这一步的成本远低于换模型本身，而且**换完模型后你还需要它来验收**。

## 7. 回滚

- 每种新模型做成**独立的下载项**（像现在的 `MODEL` / `STREAM_*` / `PUNCT` 一样），
  用户可以在设置里切回 SenseVoice。
- `settings.mode` 已经有 `offline | streaming` 的概念，加模型种类是同类改动。
- 所有改动单独成 commit，`git revert` 单个 commit 即可回退。
- **不要**在同一个 commit 里既换模型又改 VAD 参数 —— 出问题分不清是谁的锅。

## 8. 数字汇总（2026-10-25 HF API 实测，与路线图原文有出入）

| 模型 | 路线图原文 | **实测** | 差异 |
|---|---|---|---|
| Qwen3-ASR 0.6B int8 | 838 MB | **987,015,347 B（941 MiB）** | 路线图偏小 |
| FireRed ASR2 int8 | 799.7 MB | **1,234,657,933 B（1.15 GiB）** | 路线图偏小很多 |
| X-ASR int8 | 130.6 MB | **176,053,695 B（168 MiB）** | 路线图只算了 encoder？ |

**另外一条容易踩的坑**：这三个仓库的作者是 **`csukuangfj2`**，不是 `csukuangfj`
（现有的 SenseVoice / Zipformer / 标点模型都在 `csukuangfj` 名下）。写 `models.rs` 的
`Asset.path` 时别写错作者目录。
