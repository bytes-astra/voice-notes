# HOW TO VERIFY

VoiceNotes 的验证手段都在这里：**质量探针**、**长录音排查探针**、**语料**、**界面检查**、**当前基线**。

改任何模型 / 参数 / 热词之前，**先跑探针做前后对比，别凭感觉**。基线数字只写在这一个文件里 —— 复制到别处就会漂。

## 🧪 质量探针

### 跑法

```bash
cd src-tauri
VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
VOICE_NOTES_VAD_ALT=/path/to/silero_vad_v5.onnx \
VOICE_NOTES_HOTWORDS=$'再确认一下\n产品评审会' \
VOICE_NOTES_RULE3=60 \
  cargo test --test quality_probe -- --nocapture --test-threads=1
```

探针只打印不断言，会把 ITN 开关、语言指定方式、流式 greedy/beam/热词、VAD 模型与静音阈值、端点硬切行为在同一段真实音频上的实际表现打出来。参考文本放在 `src-tauri/tests/fixtures/<名字>.txt` 时自动算 CER；它也会扫描 `tests/fixtures/quality/*.wav`。

### 语料

wav 只在本机，不入库：

```bash
node scripts/fetch-quality-corpus.mjs 12   # 真人中文电话录音 + 转写（minds14）
bash scripts/gen-tts-corpus.sh             # TTS 20 句 + 数字句 + 长句 + 噪声变体
```

### 当前基线

zh + ITN 整段模式，含 pre-roll 修复 + 短句合并 + min_silence 0.3，40 条真人语料：

| 语料 | 字符错误率 |
| --- | --- |
| 真人 | **11.28%** |
| 真人 + 噪声 | **2.78%** |
| TTS | **16.39%**（修复前 26.47%） |
| 长句 | **12.50%** |

整段总段数 86（合并前 90）。**TTS 的绝对 CER 不当质量结论** —— 它主要暴露「VAD 切字头」这类相对差异。

端到端冒烟（Apple Silicon）：非流式 5.6 s 中文 ≈ 200 ms（RTF 0.04）；流式同段音频首字约 1 个分片、整段 0.5 s 内出完，无标点。

## 🔍 长录音排查探针

怀疑「录音丢了」（长录音后面少字、句子断在半截）先跑探针，别猜。四个探针在 `src-tauri/tests/`，
**打印 + 断言**（超阈值就红）；共用的小工具在 `src-tauri/tests/common/mod.rs`，探针里写 `mod common;` 引入：

```bash
VOICE_NOTES_MODEL_DIR=... cargo test --test vad_long_probe -- --nocapture      # VAD 吞不吞音频
VOICE_NOTES_MODEL_DIR=... cargo test --test offline_long_probe -- --nocapture   # 整段会话覆盖率
VOICE_NOTES_STREAMING_MODEL_DIR=... cargo test --test streaming_long_probe -- --nocapture
VOICE_NOTES_MODEL_DIR=... cargo test --test resample_probe -- --nocapture       # 48k→16k 重采样
```

四条探针的断言（**超了就是回归**）：

| 探针 | 断言 | 余量是怎么算的 |
| --- | --- | --- |
| `vad_long_probe` | VAD 吞掉的音频 ≤ 故意留的静音 + 每句 0.2 s + 0.5 s | 切句边界上本来就会丢几十毫秒 |
| `offline_long_probe` | 没变成文字的音频 ≤ 故意留的静音 + 1.0 s | 同上 |
| `streaming_long_probe` | 没变成文字的音频 ≤ 1.0 s | 流式没有静音可 excuses |
| `resample_probe` | 重采样一圈后长度比例 ±1%、识别结果差异 ≤ 5% | 比例不对 = 采样率算错了 |

## 📉 质量门禁：识别质量不得退化

`src-tauri/tests/quality_gate.rs` 量的是**生产路径**（`OfflineSession` / `StreamingSession`，
不是裸引擎），语料用 macOS `say` 合成的那批 TTS（文本已知 → 能算 CER）：

```bash
bash scripts/gen-tts-corpus.sh                      # 生成 TTS 语料（不入库）
node scripts/fetch-quality-corpus.mjs               # 可选：抓真人语料（只打印，不设阈值）
cd src-tauri
VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
  cargo test --test quality_gate -- --nocapture --test-threads=1
```

阈值（可用环境变量覆盖）：整段 CER ≤ 25%、流式 CER ≤ 55%、RTF ≤ 1.0
（`VOICE_NOTES_MAX_OFFLINE_CER` / `VOICE_NOTES_MAX_STREAMING_CER` / `VOICE_NOTES_MAX_RTF`）。

**基线（2026-11-03，本机 Apple Silicon，TTS 20 条）**：整段 CER 16.39%、流式 43.28%，
RTF 0.028 / 0.036。阈值留了余量 —— 语料是本机合成的，不同 macOS 版本的 `say` 声线会有细微差别。
**流式的绝对 CER 高不代表流式差**：这个 `say` 声线对 Zipformer 是域外的，
真人语料上两者的差距要小得多（真人语料只打印，不设阈值 —— 每次抓到的样本可能不一样）。

**这个门禁不进 CI**（macOS runner 要下 ~500 MB 模型、跑 10-15 分钟，计费是 ubuntu 的 10 倍）——
换模型或调参数时**手动跑一次**，这是「先量后改」的下限。

## 🧠 内存门禁：不随录音时长增长

`src-tauri/tests/memory_probe.rs`：一个会话跑完 30 分钟音频，在 300 秒和 1800 秒两个点看进程的
**常驻内存**（`ps -o rss=`），增长必须 < 32 MB —— 这 25 分钟的音频本身就有 96 MB，
真把音频攒在内存里的话增长会远超上限。

```bash
VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
  cargo test --test memory_probe -- --nocapture --test-threads=1
```

实测（2026-11-03）：整段 300s 826.7 MB → 1800s 815.5 MB（**降了** 11 MB，分配器把内存还给系统）、
流式 698.4 MB → 698.4 MB。开头从 550 MB 跳到 830 MB 是**一次性预热**（模型 + ONNX arena + VAD 缓冲），
不是随时长增长。

> **别为了这个探针加 libc**：`getrusage` 的 `ru_maxrss` 更准，但加 dev-dependency 会改
> `Cargo.lock` → rust-cache 的 key 跟着变 → CI 里的 sherpa-onnx 预编译库要重下，
> 而那个下载会失败（链接时报「找不到 `sherpa-onnx-c-api`」）。`ps` 是现成的。

2026-10-24 实测结论（176 秒连续语音）：**VAD、整段会话、流式会话、重采样四条路都不丢音频**
（覆盖 175.6/176.0 秒，误差只有句边界）。**队列上限 60 秒是唯一的丢失点** —— 超了会丢**最旧**的
（`SampleQueue::take_new` 里 `drain(..excess)`），并留缺口标记；日志里会打「识别跟不上，丢掉 X 秒音频」。
识别是同步阻塞在消费线程里的（`record.rs` 的 `run_offline` → `asr.rs` 的 `drain()`），但 RTF 只有 0.01，
实际落后不到 1 秒，**离 60 秒上限差得很远** —— 所以「加 buffer」不是解法。

### 缺口与时间轴

丢音频的四个口径（时长含被丢掉的音频、句子时间在真实录音时间轴、缺口排句子前、两种模式同规则）写在哪、为什么，见 [`../AGENTS.md`](../AGENTS.md)「缺口标记」——这里只留验证命令。

带真模型跑一遍端到端（整段与流式各一条）：

```bash
VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
  cargo test --manifest-path src-tauri/Cargo.toml --lib -- gap_marker_reaches_transcript streaming_gap_marker_precedes --nocapture
```

缺 `VOICE_NOTES_STREAMING_MODEL_DIR` 时流式那半会静默跳过（见「单元测试」一节的警告）—— 那是「没验」，不是「过了」。

### 同批多个 endpoint 的真模型验证

本机已有 Zipformer + VAD 时显式运行：

```bash
VOICE_NOTES_STREAMING_MODEL_DIR="$HOME/Library/Application Support/com.github.bytes-astra.voicenotes/models/streaming" \
  cargo test --manifest-path src-tauri/Cargo.toml --test streaming_endpoint_time -- --ignored --nocapture --test-threads=1
```

两条用例默认标为 `ignored`；上面的 `--ignored` 会强制执行。缺少环境变量或模型文件时直接失败，不会静默 `return`。

- `one_batch_multiple_endpoints_match_realtime_and_final_segments`：把 `zh.wav` 重复 3 次、每次后接 3 秒静音，只调用一次 `StreamingSession::push`；`finish` 前必须已有 3 个 endpoint。
- `one_capture_batch_matches_realtime_and_final_segments`：相同音频一次放入 `SampleQueue`，走真实 `run_streaming_core`（含 VAD）；首批回调必须已有 3 个 endpoint。
- 两条都逐段断言 `start < end`、时间单调、各句位于对应音频周期、实时事件与最终 `segments` 的 `text/start/end` 完全相同，并与正常 200ms 喂法逐项对照。

本机实测（2026-11-13）：总时长 25.776s，两条路径与 200ms 对照均得到 `[0.0, 7.0)`、`[9.6, 15.6)`、`[18.2, 24.2)`；两条显式执行结果为 `2 passed; 0 failed; 0 ignored`。这里检查的是后端 `StreamEvent`/回调，不包含 Tauri WebView 事件传输；不把 `segment start` 当成精确的声学起音位置。本次验证未改变模型、参数或生产代码。

## 🧷 单元测试

```bash
npm run check     # 门禁：i18n → 前端依赖图 → 拼句规则 → 前后端契约 → 打包配置 → async 阻塞锁 → clippy(-D warnings) → cargo test --lib
```

- **门禁跑绿 ≠ 流式时间轴验证过**：`src-tauri/src/record/tests.rs` 里几个用例要真模型
  （`VOICE_NOTES_MODEL_DIR` / `VOICE_NOTES_STREAMING_MODEL_DIR`）。没设这两个环境变量时，
  它们**静默跳过**（打印一行「跳过：未设置…」），退出码仍是 0 —— CI 上就是这么跑的。
  本机验证要设上，并且带 `--nocapture` 才看得到跳过了哪几条：

  ```bash
  VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
    cargo test --manifest-path src-tauri/Cargo.toml --lib -- --nocapture
  ```

- `npm run check:deps` 查前端 import 图：单向依赖不能反、也不能成环（链见 AGENTS.md）。
  另外两条：**经典脚本（`i18n.js` / `select.js` / `locales/*.js`）不得被 import** ——
  它们挂 window 上，被 import 会执行两遍（实测：`i18n.js` 执行 2 次，`window.I18N`
  被换成第二个实例）。**当前代码恰好没因此坏掉**（谁生效取决于模块求值顺序），
  但这种依赖求值顺序的隐患不值得留；**每个模块都要登记在层表里** ——
  没登记的模块它的 import 边一条都不查（旧洞）。门禁自己有测试（`scripts/check-deps.test.mjs`）。
- `npm run check:i18n` 查字典对齐 + 引用完整性 + **后端错误码** + **后端裸字符串错误**。
  后端错误码的扫描是**文本级正则**：
  `code("…")` / `code_detail("…")` 都要认，`crate::` 前缀也算（`crate::code_detail("record.tooShort", …)`
  这种写法很常见，排掉冒号就会静默漏网）。裸字符串错误（`Err("加载失败")`、
  `.ok_or("…")`）绕过字典，前端查不到就原文甩给用户 —— 也一起查。
  还要查 `i18n.js` / `select.js` 是不是被写成了 `type="module"`：module 脚本是 deferred 的，
  `app.js`（也是 module）可能先跑，那时 `window.I18N` 还不存在。
  另外查**界面文案里的内部黑话**（线程 / 预热 / 收尾 / 引擎状态 / 解码器 / 采样率 / 推理
  以及对应英文）：技术细节只进日志，不往界面上贴（AGENTS.md「文本分层」）。
  门禁自己有测试（`scripts/check-i18n.test.mjs`），
  改扫描规则要连它一起改 —— 正则是文本级的，漏一个写法就漏一个错误码。
- `npm run check:contract` 查前后端命令名与载荷字段名一致：前端 `invoke('x', { field })` 的
  命令与字段必须在后端存在（`#[tauri::command] fn x(..., field: …)`）。改一边不会让另一边变红 ——
  字段名对不上时 Tauri **静默忽略**，值永远是默认值，界面看着正常但行为是错的。
  还查界面检查的假后端（`scripts/ui/check.mjs`）返回的字段与真后端的 `Status` / `InputDevice`
  一致 —— 历史上出过 mock 写 `is_default`、真后端是 `isDefault`，界面检查全绿、真机上标记不显示。
  门禁自己有测试（`scripts/check-contract.test.mjs`）。
- `npm run check:join` 查前后端拼句规则一致：前端 `core.js` 的 `joinText()` 与后端 `asr.rs` 的
  `join_segments()` 是同一套规则的两份实现（实时出字在前端拼、最终文本在后端拼 ——
  两边不一致时界面上的字和保存下来的字会不一样）。脚本把两边的**判定结构**归一化后比对：
  CJK 码位区间（带端点开闭）、缺口占位符的完整形状（`[[gap:<数字>]]`）、三条拼接规则的表达式。
  只比对子串会漏掉最自然的漂移形状（`..=` 改 `..`、`||` 改 `&&`、少写半个条件、占位符少个 `]`）——
  这几种改法都会被脚本抓住（退出码 1）。
- `npm run check:bundle` 查打包与原生层配置（`scripts/check-bundle.mjs`）：macOS 必须 ad-hoc 签名
  （`bundle.macOS.signingIdentity = "-"`）、必须挂麦克风 entitlement
  （`com.apple.security.device.audio-input`）、`Info.plist` 必须有 `NSMicrophoneUsageDescription`、
  macOS 专属代码必须整体带 `#[cfg(target_os = "macos")]`（`menu.rs` 顶层不许有 `use`，
  `lib.rs` 里 `use menu::…` 要带 cfg）。这些配置改了都不会报错，只在真机上炸 ——
  缺 entitlement 的表现是「能录音、有时长，就是没字」，没 ad-hoc 签名的表现是
  「别的 Mac 从 dmg 装完 Gatekeeper 报已损坏」。门禁自己有测试（`scripts/check-bundle.test.mjs`）。
  真机 / 产物级的那一半由 CI 验：`build.yml` 的 macOS job 在 `npm run build` 之后用
  `codesign -dv`（看 `Signature=adhoc`）和 `codesign -d --entitlements -`（看
  `com.apple.security.device.audio-input`）校验产物。
- `npm run check:async` 查 **async 路径上的阻塞锁**（`scripts/check-async.mjs`）：引擎槽
  （`engine` / `streaming_engine` / `punct_engine`）的**阻塞式** `.lock()` 只能出现在
  `spawn_blocking` 闭包里。引擎加载要好几秒（239 MB / 161 MB / 294 MB），那把锁会被占住；
  async 命令在执行器线程上死等它，整个执行器就卡住 —— 表现是「点了没反应，过几秒才动」。
  `try_lock()` 不算（拿不到就往下走加载路径）。采集器槽 / 会话槽**故意不查**：
  那些临界区只有几行，不存在长等待。
  门禁沿**调用图**从 async 函数出发，所以「async 命令调一个同步取锁助手」（原来的
  `shared_engine`）也会被抓住，而只在 `spawn_blocking` 里调的 `install_engine` 不会。
  门禁自己有测试（`scripts/check-async.test.mjs`，含「把旧的 `shared_engine` 加回真源码会被抓住」）。

## ☁️ 云端模式的验证手段

云端模式（第三种引擎，见 README「云端模式」）的验证**以离线为主**：协议那头用假 provider 或本地假服务端顶上，不需要真服务、不需要 API key，也不下载识别模型；真实 `start_cloud_recording` 在缺少 VAD 时会联网下载约 2.3 MB 的 `silero_vad.onnx`。

**可选的真服务冒烟**：火山（`volcengine::tests::live_smoke`）与阿里（`aliyun::tests::live_smoke`）各有一条用例，凭据只从环境变量读（`VOICE_NOTES_VOLCENGINE_KEY` / `VOICE_NOTES_ALIYUN_KEY`），**没设就跳过**（不打印 key）。它们是**补充**，不作合并门槛 —— 但没跑过真服务就合入的 provider，必须在 README / PR 里标注「未对真实服务验证过」。

- **会话层单测**（`src-tauri/src/asr/cloud/tests.rs`）：时间轴按 16 kHz 采样数推进（丢掉的音频同样占录音时间）、f32→i16 的 clamp、final→`Segment` 映射、**provider 给的时间戳不覆盖本地轴**、provider 要 24 kHz 时上传前升采样、**空 final 不抢下一句的区间**（时间戳不错位）、断连保留已定稿句子。跑法：`cargo test --manifest-path src-tauri/Cargo.toml --lib`。
- **结果与区间配对**：服务端的 final 可能乱序到达，也可能缺一条（识别失败）。会话层按 `item_id` 绑定句子区间（provider 先发 `CommitAck`），不按到达顺序 —— 否则失败点之后每句的时间戳都会错位一格。单测覆盖乱序 final、失败 item 无回执、提交失败后收尾区间合并（`src-tauri/src/asr/cloud/tests.rs` 与 `.../realtime/protocol_tests.rs`）。
- **协议编解码单测**（`src-tauri/src/asr/cloud/realtime/protocol_tests.rs`）：拿官方的示例帧当输入 —— `conversation.item.input_audio_transcription.delta`（字段 `delta`）与 `.completed`（字段 `transcript`）解成 Partial / Final、`input_audio_buffer.commit` 的回执带 `item_id`（用来给 final 按提交顺序排序）、`session.update` 的编码（`audio/pcm` + 24000、`turn_detection` 为 `null` 时由客户端断句、语言参数是 `languages` 复数）。另有**收尾记账**两组：客户端断句（提交后结果没回来就还在等）、服务端断句（迟到的旧 final 不能抹掉新音频的等待），以及**迟到的 `committed` 回执不再排队**（否则队列永远不空，收尾会一直等）。
- **集成测试**（`src-tauri/src/record/tests.rs`）：走 `run_cloud_streaming_core` 的完整编排 —— 中途断连后已定稿句子仍在（`cloud_core_keeps_finalized_sentences_when_disconnected`）、上传丢掉的音频不重复计时（`cloud_core_does_not_double_count_upload_drops`）、**收尾的尾部缺口排在最后一句之后**（`cloud_tail_gap_follows_last_sentence`：第一次上传回一句话、第二次上传丢 1 秒，断言缺口是一个时刻且不并进最后一句）。跑法：`cargo test --manifest-path src-tauri/Cargo.toml --lib record::tests::<name>`。
- **凭据不外泄**：`Status` 序列化里没有 key 明文（`engine` 的单测）、错误详情里的 key 会被抹成 `***`（`sanitize()`，Realtime 与火山各一条协议单测 —— 火山的用会保留 message 的错误码分支，否则断言会恒真）、界面检查断言「密钥不回显」。
  - **日志这一半**：云端模块的日志只许走 `asr/cloud/logging.rs` 的**唯一出口**（`warn` / `info`），写入前用该 provider 的凭据脱敏。两条守卫：`asr::cloud::tests::log_choke_point_redacts_the_credential`（关口的脱敏）+ `asr::cloud::tests::cloud_logs_go_through_the_redacting_choke_point`（**源码级守卫**：运行时读 `src/asr/cloud/` 下各 `.rs` 与 `src/asr/cloud.rs`，出现直接的日志调用就红 —— 日志写进文件与 stderr，测试抓不到返回值，唯一能自动发现「绕过关口」的办法是读源码）。**范围只有云端模块**：`record.rs` 的云端日志只拼字典码，前端那半边靠「key 不回传」。
- **能力声明**（`src-tauri/src/asr/cloud.rs` 的 `Capabilities`）：每个 provider 自报能力（句边界方式、是否需要本地 VAD、中间结果语义、是否支持术语表、上传采样率），按 provider id 静态查的表（`provider_capabilities`）与实例自报值必须一致。合法组合只有两种：**客户端提交 + 需要本地 VAD**、**服务端断句 + 不需要本地 VAD** —— 其余组合拒绝建 provider（「缺 VAD」绝不能被当成「改用服务端断句」）。单测覆盖能力表、组合校验、VAD 前置条件与门控决策的四种组合。
- **中间结果归一器**（`src-tauri/src/asr/cloud/partial.rs`）：各家协议给的中间结果形态不同（增量拼接 / 确认前缀加草稿 / 区间替换），provider 内部归一成「当前未定稿句的累计串」，上层只认这一种语义。归一器按句标识分桶、句定稿时清掉自己的桶；没有句标识的协议走单桶。单测见 `src-tauri/src/asr/cloud/partial/tests.rs`。
- **provider 契约守卫**（各 provider 自己的测试模块）：三家的中间结果形态各有一条守卫，断言两件事 —— ① 上抛的中间结果是当前未定稿句的**累计文本**（句内整体替换，不追加）；② 声明的 `partial_semantics` 与**从样例推导出的**形态一致。三条硬规矩（否则守卫是假的）：**禁 mock provider 当协议样例**（mock 直接注入 `CloudEvent`，没有协议形态）、**期望值必须手写字面量**（从样例里再读一遍就是 `x == x`）、**形态从样例推导而不是只钉常量**（只钉常量的话形态漂移照样过）。样例：火山是两次响应（第二次全量返回、改写最后一条未定句）、Realtime 是两条**同 `item_id`** 的 `.delta`、阿里是同一句的两次「已确认句子 + 草稿」。会话层还有一条零长句顺序守卫（`zero_length_segments_keep_arrival_order`：同批九条 final，首条占区间、后八条零长且同 `start`，文本顺序 = 到达顺序）。清单见 `docs/REVIEW-CHECKLIST.md` 的 RC-K1 / RC-K2 / RC-K4。
- **服务端断句路径**（`server_endpoint` 族）：不本地门控 —— 静音照上传，服务端靠静音判句尾；句子区间按本地轴兜底（「上一句终点 → 本次定稿到达时已喂入会话的音频终点」）。**已知取舍**：该终点会系统性偏长（服务端延迟 + 尾静音）。
- **界面检查**（`npm run check:ui`）覆盖云端：配置保存、状态徽标（已配置 / 未配置）、上传确认框、云端模式发的是 `start_cloud_recording`、**当前服务不支持术语表时给出提示**（术语表是本地流式与云端共用的全局设置，只能提示、不得隐藏入口）、**结果框里的实时句子按本地时间轴排序**（五条排序断言：缺口段先到时句子仍排在缺口前面、同一时刻缺口排在句子前面、同一时刻缺口先到时也在句子前面、乱序到达的句子按起点排、时间递增到达时文本逐字不变），外加**最终转写以后端返回的全文为准**（停止瞬间迟到的句子不混进最终转写）。

- **火山（豆包）协议层**（`src-tauri/src/asr/cloud/volcengine/`）：二进制帧编解码按官方文档逐字节核对（4 字节头 + payload size，大端；消息类型 / flags / 序列号）；结果映射用官方示例 JSON（`result.utterances[]` 的 `definite` → 定稿，最后一条未定句 → 实时行；**不用 `result.text`** —— 那是全会话累计）。热词清洗（控制字符丢弃、超上限（100 条）截断）。
- **火山的真机验证**：用 `VOICE_NOTES_VOLCENGINE_KEY` 环境变量跑 `asr::cloud::volcengine::tests::live_smoke`（没设就跳过，**key 不入库、不进日志**）。已验证通过（2026-11-16）：7 条中间结果 + 1 句定稿「开放时间，早上9点至下午5点。」（fixture `tests/fixtures/zh.wav` 的内容）—— 帧编解码、鉴权、中间结果、末包 → 最终响应、整句断句整条链路都对。资源 ID 用 `volc.seedasr.sauc.duration`（豆包 2.0 小时版）—— **只支持模型 2.0**；填了模型 1.0 的资源 ID 会握手 403 `requested resource not granted`。

- **阿里云百炼（Qwen-Audio 3.1 实时流式）协议层**（`src-tauri/src/asr/cloud/aliyun.rs`）：JSON 控制帧 + 裸二进制音频帧（不需要帧编解码模块）。请求组装（`run-task` / `finish-task`）、服务端事件解码、有状态映射 `AliyunMapper`（已确认句子与草稿分开记）。
  - **映射的四条硬规则**（每条都有单测）：`text` 是整句快照（**替换不追加**）；心跳先判（`sentence_id` 固定 0，否则会误把上一句定稿）；新句出现而上一句未定稿 → **按最后快照防御性定稿**（否则会话层的 Partial 替换语义会让上一句文字永久消失）；已定稿句的后续事件**一律忽略**（防重复 Final，也防迟到的 `end` 把新句的实时行清掉）。
  - **收尾流程有假服务端测试**（`finish_keeps_reading_and_flushes_leftover_packet`）：本地 `TcpListener` + `accept_async` 起一个假 WebSocket 服务端，收到 `finish-task` 后才发两条尾部结果、最后才发 `task-finished`。它守两条规则：**残包要在 `finish-task` 之前发出去**（否则最后 0~99 ms 永不上传、句尾少字）、**`finish-task` 之后要继续收**（发完就关会丢尾部结果）。
  - **阿里真机冒烟**：`VOICE_NOTES_ALIYUN_KEY=... cargo test --manifest-path src-tauri/Cargo.toml --lib asr::cloud::aliyun::tests::live_smoke -- --nocapture`。
    **已验证通过**（2026-11-16，模型 `qwen-audio-3.1-asr-flash-streaming`）：7 条中间结果 + 1 条定稿「开放时间早上9点至下午5点。」（fixture `tests/fixtures/zh.wav`），最后一条中间结果是「开放时间早上9点至下午5」（草稿还没落定，最后的「。」在定稿里）。端点、鉴权、`run-task` 请求体、二进制音频帧、`sentence` + `stash` 的结果结构、`finish-task` → `task-finished` 整条链路都对上了真实服务。这次实测确认了四件：
1. **多句断句正常**：同一段音频喂两遍 → 断成两句（定稿 2 条）。
2. **同一句的连续结果确实越来越完整**（最后一条实时行是定稿文本去掉末尾标点的那一截）。
3. **`heartbeat: true` 没有产生额外事件**。
4. **服务端在句子开头会「漂」**：开头几条中间结果是「太」→「菜放」→「菜放时间在」，之后才修正成「开放时间早上9点」——ASR 的典型开头漂移，我们如实显示，不做特殊处理。

**两点这次没验**：
- **「不传 `language_hints` 时是否自动识别多语种」**：fixture 只有中文，「30 语种」目前只有官方文档依据，要换语种实测。
- **即时热词的实际增益**：fixture 本身就含「开放时间」，加与不加热词看不出差别；要验证增益得用一段**不含该词**的音频。

**注意服务端的识别结果有随机性**：同一段音频两次运行，定稿文本不同（一次「开放时间：早上9点至下午5点。」带冒号，一次「开放时间早上9点至下午5点。」不带）—— 排查识别差异时别把这个当 bug。
  - **官方文档出处**（2026-11-16 核对）：
    - 协议与事件：<https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-websocket-api>、<https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-client-events>、<https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-audio-asr-streaming-server-events>
    - 音频规格与「整句快照」口径：<https://help.aliyun.com/zh/model-studio/automatic-speech-recognition>
    - 模型与语种：<https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-audio-3-1-asr-flash-streaming>、<https://help.aliyun.com/zh/model-studio/asr-model>
    - 术语表（预编译词表，本变更未采用）：<https://www.alibabacloud.com/help/zh/model-studio/vocabulary-http-api>、<https://docs.bailian.console.aliyun.com/zh/model-studio/improve-asr-accuracy>
  - **模型白名单**：只支持 `qwen-audio-3.1-asr-flash-streaming`。参数与结果结构与通用 ASR 协议不同（本模型有 `stash` 草稿、即时热词、方言开关），不按「名字随便填」处理。
  - **计费按 token**（输入 6 元 / 百万、输出 4.5 元 / 百万，北京与新加坡同价），不是按秒。
  - 官方文档有三处口径不一致，实施时以本节为准：`max_sentence_silence` 默认值（本模型客户端事件页 **800** / 旧的 Paraformer SDK 页 800）→ **显式设 800**；`language_hints` 不传时的语义（一处写自动识别、一处写默认 `["zh","en"]`）→ **按「不传 = 自动识别」**；`sentence_id` 是否必填 → **必填**（缺了按兼容路径处理并记 WARN）。

**云端相关测试基线**：`cargo test --lib` = **276**（其中 `asr::cloud` = 124 = 会话层 37 + Realtime 协议层 28 + 归一器 6 + 火山 19 + 阿里 34；协议层含 1 个热词过滤用例）。基线数字以本节为准；改动时**禁止靠删测试达成全绿**（允许合并重复用例，但必须新增等价断言且总数不减）。

> [!NOTE]
> 云端模式**没有质量门禁、没有 CER 基线** —— 识别质量取决于第三方服务，本仓库不设阈值断言。`quality_gate` 那些阈值只管本地两条路径。别把本地基线套到云端上。

## 🖥️ 界面检查

Playwright + 假 `window.__TAURI__`，不需要真应用：

```bash
npm run check:ui     # 165 项检查
npm run shots        # 重出 README 9 张截图
```

- 脚本在 `scripts/ui/`（`check.mjs` / `shots.mjs` / `server.mjs`），假 `window.__TAURI__` 在 `check.mjs` 里。**浏览器不在默认位置时要设 `PLAYWRIGHT_BROWSERS_PATH`**（本机是 `/tmp/pw-browsers`）。
- 静态服务器用 Node 起（`scripts/ui/server.mjs`），**不用 `python -m http.server`**：那个会偶发 `ERR_CONNECTION_RESET`（app.js / i18n.js 加载失败），表现是 `state is not defined` 或半渲染，很容易误判成代码问题。脚本里已经加了 `page.on('requestfailed', ...)`，服务器一抖立刻就能看出来。
- 假 Tauri 的 `event.listen` 会把回调存起来，页面里用 `window.__emit('model:progress', {...})` 主动发事件 —— **后端事件的行为（比如进度按 kind 分流）要靠它测**。
- 截图要**等页面渲染完再截**，否则会截到半成品（语言芯片、下拉框还没画出来）。
- 深色截图用应用自己的主题切换按钮（`#theme-btn`）出，WebKit 里 `prefers-color-scheme` 不可靠。
- **截图 harness 要给 macOS 侧栏垫一层不透明底**（`shots.mjs` 的 `SIDEBAR_MATERIAL_FALLBACK`）：真机上侧栏透明、透出原生材质（NSVisualEffectView），浏览器里没有那一层材质 —— 透明侧栏会直接露出白色画布，深色主题下就成了「白侧栏 + 深色主区」，品牌字都看不见。去掉这行，深色截图立刻坏回去。
- **截图 harness 的 mock status 要跟真实值对得上**：设置页会直接把模型目录、保存目录打在屏幕上。bundle id 是 `com.github.bytes-astra.voicenotes`，默认输出目录是 ASCII 的 `~/Documents/VoiceNotes`。
- **mock 的 `reserve_session` 必须返回会话 ID，`stream:segment` / `stream:partial` 事件必须带同一个 `sessionId`**：前端只认当前会话（`events.js` 里比对 `state.recordingSessionId`），缺了就当成旧会话丢掉 —— 表现是流式截图的结果区一片空白。
- **mock 的字段名要跟后端序列化后的一致**：后端 `InputDevice` 带 `#[serde(rename_all = "camelCase")]`，载荷是 `isDefault` 不是 `is_default`。写错了设备「· 系统默认」标记就不显示 —— 而且断言还可能因为下拉首项本来就叫「系统默认」而**恒真**，等于没测。断言要盯设备项本身（`Mock Mic · 系统默认`），别盯整段文本里有没有「系统默认」。
- **mock 的 `cancel_download` 要把在飞的下载拒掉**：真后端取消后返回的是字典 key `setup.cancelled`（models.rs 里的内部哨兵在命令层翻译过）。mock 不拒那个挂起的 promise 的话，界面永远停在「正在下载」，「已取消」这条断言也测不到。
- **mock 的 `set_hotwords` 返回要带 `hotwordsIgnored`**：真后端回报「有几条含模型不认识的字」（流式模型是单字词表，表外的字让整条术语作废）。少了这个字段，界面永远弹「已保存 N 个术语」，「其中 M 个不会生效」这条提示就测不到。

## 📚 相关文档

- 工程约定、模块边界、参数不变式、踩过的坑：[../AGENTS.md](../AGENTS.md)
- 复审的固定作业面（每轮从「⚠️ / ❌」条目开始，别另起清单）：[REVIEW-CHECKLIST.md](REVIEW-CHECKLIST.md)
