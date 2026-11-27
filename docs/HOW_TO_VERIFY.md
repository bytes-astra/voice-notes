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
不是随时长增长。**基线数字只写在这里** —— 探针的源码注释不抄一份（抄过一次，已经漂了）。

> **别为了这个探针加 libc**：`getrusage` 的 `ru_maxrss` 更准，但加 dev-dependency 会改
> `Cargo.lock` → rust-cache 的 key 跟着变 → CI 里的 sherpa-onnx 预编译库要重下，
> 而那个下载会失败（链接时报「找不到 `sherpa-onnx-c-api`」）。`ps` 是现成的。

2026-10-24 实测（176 秒连续语音）：**VAD、整段会话、流式会话、重采样四条路都不丢音频**
（覆盖 175.6/176.0 秒，误差只有句边界）。**唯一的丢失点是队列上限 60 秒** —— 超了丢**最旧**的
（`SampleQueue::take_new` 的 `drain(..excess)`）并留缺口标记，日志里打「识别跟不上，丢掉 X 秒音频」。
识别同步阻塞在消费线程里（`record.rs` 的 `run_local_offline` → `asr.rs` 的 `drain()`），但 RTF 只有 0.01、
实际落后不到 1 秒，**离 60 秒上限差得很远** —— 所以「加 buffer」不是解法。

### 缺口与时间轴

丢音频的四个口径（时长含被丢掉的音频、句子时间在真实录音时间轴、缺口排句子前、两种模式同规则）写在哪、为什么，见 [`ASR.md`](ASR.md)「缺口标记」——这里只留验证命令。

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
- `one_capture_batch_matches_realtime_and_final_segments`：相同音频一次放入 `SampleQueue`，走真实 `run_local_streaming`（含 VAD）；首批回调必须已有 3 个 endpoint。
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

- `npm run check:deps` 查前端 import 图：单向依赖不能反、也不能成环（链见 [`FRONTEND.md`](FRONTEND.md)）。
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
  以及对应英文）：技术细节只进日志，不往界面上贴（[`FRONTEND.md`](FRONTEND.md)「文本分层」）。
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
- `npm run check:frontend` 跑**前端纯函数的单元测试**（`scripts/frontend/`，`node --test`）：
  把 `src/` 里的纯函数**直接 import 进来**跑真实断言 —— 格式化（`fmtDuration` / `fmtClock` /
  `fmtBytes`）、拼句（`joinText`）、缺口占位符（`isGapMarker` / `gapText` / `localizeText` /
  `gapMarkerTexts`）、错误码（`errorCode` / `fmtError`）、文件名（`buildFileName`）。
  **与 `check:join` 互补、都留着**：`check:join` 比的是前后端两份实现的**结构**（两边同时改错成
  一样它也不红），这层测的是**行为对不对**（输出等于手写的期望值）。期望值一律手写字面量，
  不从被测函数推导（推导出来的期望恒真）。
  **沙箱**（`scripts/frontend/sandbox.mjs`）给 Node 补上 `window` / `document` / `navigator` /
  `localStorage`，并按浏览器的顺序加载**真的字典**（`src/locales/*.js` 再 `src/i18n.js`，用
  `new Function` —— 它们是浏览器脚本不是 ES module，`import()` 会失败；跟 `check-i18n.mjs`
  同一个手法）。字典是真的才能测出「中文全角不加空格、英文半角加空格」这类依赖文案内容的规则。
  沙箱**自己也有守卫**（`scripts/frontend/sandbox.test.mjs`）：字典非空、`t` 查得到 key、
  **切语言对已 import 的模块生效**、两次 import 拿到不同实例 —— 沙箱坏了测试会静默地测错东西。
  测试文件**不在 `src/` 下**：`src/` 是 Tauri 的 `frontendDist`（会被打进 app bundle），
  且 `check:deps` 规则四要求 `src/` 下每个 `.js` 都登记。
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
  `spawn_blocking` 闭包里。引擎加载要好几秒（228 MB / 162 MB / 281 MB，与 README 同一口径 ——
  界面按 1024 进制显示），那把锁会被占住；
  async 命令在执行器线程上死等它，整个执行器就卡住 —— 表现是「点了没反应，过几秒才动」。
  `try_lock()` 不算（拿不到就往下走加载路径）。采集器槽 / 会话槽**故意不查**：
  那些临界区只有几行，不存在长等待。
  门禁沿**调用图**从 async 函数出发，所以「async 命令调一个同步取锁助手」（原来的
  `shared_engine`）也会被抓住，而只在 `spawn_blocking` 里调的 `install_engine` 不会。
  门禁还有**第二条规则**：阻塞的构造器（`Engine::new` / `OfflineSession::new` /
  `VoiceActivityDetector::create` 等，建它们要载 onnx、建 ORT session）不许建在
  async 路径上。
  **豁免只给「不是执行器」的线程**：`spawn_blocking` 闭包与专用 `std::thread`
  闭包。**执行器的 spawn 不豁免** —— `tauri::async_runtime::spawn(async move {…})`
  与 `tokio::spawn(…)` 跑的是执行器上的异步任务（tauri 的 `async_runtime::spawn`
  就是 `tokio::spawn` 到全局 tokio runtime），阻塞在那儿一样占执行器。判据是
  **参数形态**：闭包（`move || {…}`）只可能是 OS 线程，`async` 块 / future 是执行器任务。
  门禁自己有测试（`scripts/check-async.test.mjs`，含「把旧的 `shared_engine` 加回真源码会被抓住」
  与「执行器 spawn 的豁免真的不在（mutation）」）。
- `npm run check:fmt` 查 **Rust 源码格式**：`cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`。
  格式不查不会让功能出错，但会让**真正的 diff 淹没在空白改动里** —— review 时分不清
  哪些是格式化、哪些是真改动。门禁挂在 `npm run check` 里（排在 `check:rust` 前 —— 格式先过，
  再编译）。仓库用 rustfmt 默认配置，**没有** `rustfmt.toml`。
  - **2026-11-24 起格式已统一**：一次性跑 `cargo fmt --all`（46 个文件 / 220 处），
    **单独成提交**（不改逻辑），门禁在它之后挂上。
  - **注意**：`cargo fmt` 只重排空白、换行与尾逗号，**不改语义** —— 但它会让
    「用逐字文本匹配做 mutation」的自测悄悄失效（匹配不上 → 什么都没改 → 假绿）。
    写这类自测要用宽松正则，并**断言替换真的发生了**。

## ☁️ 云端模式的验证手段

云端模式（第三种引擎，见 README「云端模式」）的验证**以离线为主**：协议那头用假 provider 或本地假服务端顶上，不需要真服务、不需要 API key，也不下载识别模型；真实 `start_cloud_recording` 在缺少 VAD 时会联网下载约 2.3 MB 的 `silero_vad.onnx`。

**可选的真服务冒烟**：火山（`volcengine::tests::live_smoke`）与阿里（`aliyun::tests::live_smoke`）各有一条用例，凭据只从环境变量读（`VOICE_NOTES_VOLCENGINE_KEY` / `VOICE_NOTES_ALIYUN_KEY`），**没设就跳过**（不打印 key）。它们是**补充**，不作合并门槛 —— 但没跑过真服务就合入的 provider，必须在 README / PR 里标注「未对真实服务验证过」。

- **会话层单测**（`src-tauri/src/asr/cloud/tests.rs`）：时间轴按 16 kHz 采样数推进（丢掉的音频同样占录音时间）、f32→i16 的 clamp、final→`Segment` 映射、**provider 给的时间戳不覆盖本地轴**、provider 要 24 kHz 时上传前升采样、**空 final 不抢下一句的区间**（时间戳不错位）、断连保留已定稿句子。跑法：`cargo test --manifest-path src-tauri/Cargo.toml --lib`。
- **结果与区间配对**：服务端的 final 可能乱序到达，也可能缺一条（识别失败），会话层按 `item_id` 绑定句子区间（口径见 [`CLOUD.md`](CLOUD.md)「云端模式」）。单测覆盖乱序 final、失败 item 无回执、提交失败后收尾区间合并（`src-tauri/src/asr/cloud/tests.rs` 与 `.../realtime/protocol_tests.rs`）。
- **协议编解码单测**（`src-tauri/src/asr/cloud/realtime/protocol_tests.rs`）：拿官方的示例帧当输入 —— `conversation.item.input_audio_transcription.delta`（字段 `delta`）与 `.completed`（字段 `transcript`）解成 Partial / Final、`input_audio_buffer.commit` 的回执带 `item_id`（用来给 final 按提交顺序排序）、`session.update` 的编码（`audio/pcm` + 24000、`turn_detection` 为 `null` 时由客户端断句、语言参数是 `languages` 复数）。另有**收尾记账**两组：客户端断句（提交后结果没回来就还在等）、服务端断句（迟到的旧 final 不能抹掉新音频的等待），以及**迟到的 `committed` 回执不再排队**（否则队列永远不空，收尾会一直等）。
- **集成测试**（`src-tauri/src/record/tests.rs`）：走 `run_cloud_streaming` 的完整编排 —— 中途断连后已定稿句子仍在（`cloud_core_keeps_finalized_sentences_when_disconnected`）、上传丢掉的音频不重复计时（`cloud_core_does_not_double_count_upload_drops`）、**收尾的尾部缺口排在最后一句之后**（`cloud_tail_gap_follows_last_sentence`：第一次上传回一句话、第二次上传丢 1 秒，断言缺口是一个时刻且不并进最后一句）。跑法：`cargo test --manifest-path src-tauri/Cargo.toml --lib record::tests::<name>`。
- **凭据不外泄**：`Status` 序列化里没有 key 明文（`engine` 的单测）、错误详情里的 key 会被抹成 `***`（`sanitize()`，Realtime 与火山各一条协议单测 —— 火山的用会保留 message 的错误码分支，否则断言会恒真）、界面检查断言「密钥不回显」。
  - **连接列表只回传掩码**：`Status.cloudProfiles` 里每条是 `{id, label, provider, baseUrl, model, keyMask}`，**明文不回传**（掩码算法见 [`CLOUD.md`](CLOUD.md)「云端模式」）。守卫：`settings/cloud_tests.rs::mask_secret_hides_the_middle` / `mask_secret_never_reveals_the_whole_secret`；界面断言「列表显示掩码、不显示明文」。
  - **每家一条独立连接**：切 provider 不再清空别人的配置（旧行为是「切换会清掉地址与凭据」）。守卫：`settings/cloud_tests.rs::switching_provider_keeps_the_other_providers_profile`。
  - **日志这一半**：云端模块的日志只许走 `asr/cloud/logging.rs` 的关口（`warn` / `info` / `code_detail`），写入前用该 provider 的凭据脱敏。两条守卫：`asr::cloud::tests::log_choke_point_redacts_the_credential`（关口的脱敏）+ `asr::cloud::tests::cloud_logs_go_through_the_redacting_choke_point`（**源码级守卫**：运行时读 `src/asr/cloud/` 下各 `.rs` 与 `src/asr/cloud.rs`，出现 `crate::log` 或 `crate::code_detail` 就红 —— **两条路都写日志文件**（`crate::code_detail` 内部就是 `log::error`）却不脱敏，所以云端模块一律用 `logging::code_detail` 先脱敏再委托）。守卫**跳过**测试文件（`tests.rs` / `*_tests.rs` / `tests/` 目录）、`mock.rs`（`#[cfg(test)]`）与关口自己（`logging.rs`）。
    - **已知边界**：见 [`CLOUD.md`](CLOUD.md)「云端模式」—— `eprintln!` 只进 stderr 不进日志文件；脱敏只保证凭据的**精确子串**不出现（变形、URL 编码不保证）。
    - **范围只有云端模块**：`record.rs` 的云端日志只拼字典码，前端那半边靠「key 不回传」。
- **云端术语表的协议层丢弃要回传界面**（`Status.cloudHotwordsIgnored`）：术语表是本地流式与云端共用的设置，但**各家的协议限制不同**（阿里官方规范 ≤ 15 字、火山 100 条上限、Realtime 禁特定字符）—— 被丢掉的用户得知道，不能只记一条日志。`asr/cloud/tests.rs::cloud_hotwords_ignored_counts_protocol_level_drops` 守着计数（每个 provider 都有分支 + 正常词不丢）。
- **能力声明**（`src-tauri/src/asr/cloud.rs` 的 `Capabilities`）：provider 自报能力（句边界方式、是否需要本地 VAD、中间结果语义、是否支持术语表、上传采样率），静态表（`provider_capabilities`）与实例自报值必须一致；合法组合只有两种 —— **客户端提交 + 需要本地 VAD**、**服务端断句 + 不需要本地 VAD**，其余拒绝建 provider（「缺 VAD」绝不能被当成「改用服务端断句」）。单测覆盖能力表、组合校验、VAD 前置条件与门控决策的四种组合。
- **中间结果归一器**（`src-tauri/src/asr/cloud/partial.rs`）：各家协议形态与归一档见 [`CLOUD.md`](CLOUD.md)「云端模式」。归一器按句标识分桶、句定稿时清掉自己的桶；没有句标识的协议走单桶。单测见 `src-tauri/src/asr/cloud/partial/tests.rs`。
- **provider 契约守卫**（各 provider 自己的测试模块）：三家的中间结果形态各有一条守卫，断言两件事 —— ① 上抛的中间结果是当前未定稿句的**累计文本**（句内整体替换，不追加）；② 声明的 `partial_semantics` 与**从样例推导出的**形态一致。三条硬规矩（否则守卫是假的）：**禁 mock provider 当协议样例**（mock 直接注入 `CloudEvent`，没有协议形态）、**期望值必须手写字面量**（从样例里再读一遍就是 `x == x`）、**形态从样例推导而不是只钉常量**（只钉常量的话形态漂移照样过）。样例：火山是两次响应（第二次全量返回、改写最后一条未定句）、Realtime 是两条**同 `item_id`** 的 `.delta`、阿里是同一句的两次「已确认句子 + 草稿」。**生产链路**另有守卫：`asr::cloud::realtime::protocol_tests::deltas_accumulate_on_the_production_path` —— 假 WebSocket 服务端回两帧 `.delta`，跑**真实 run 循环**，断言上抛的是「你好，世界」而不是「，世界」（自建归一器的守卫守不住「provider → 上抛」那段接线：把生产调用点绕开归一器，276 条测试全绿）。会话层还有一条零长句顺序守卫（`zero_length_segments_keep_arrival_order`：同批九条 final，首条占区间、后八条零长且同 `start`，文本顺序 = 到达顺序）。清单见 `docs/REVIEW-CHECKLIST.md` 的 RC-K1 / RC-K2 / RC-K4。
- **服务端断句路径**（`server_endpoint` 族）：不本地门控（静音照上传，服务端靠静音判句尾），句子区间按本地轴兜底 —— 口径见 [`CLOUD.md`](CLOUD.md)「云端模式」。**已知取舍**：该终点会系统性偏长（服务端延迟 + 尾静音）。
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

**云端相关测试基线**：`cargo test --lib` = **327**（其中 `asr::cloud` = 139，含 6 条云端质量门禁用例）。基线数字以本节为准；改动时**禁止靠删测试达成全绿**（允许合并重复用例，但必须新增等价断言且总数不减）。

## ☁️🧪 云端质量门禁：认得准不准

`src-tauri/src/asr/cloud/quality/tests.rs`（`#[cfg(test)]`，只在 `--lib` 测试构建里编译）走**生产路径** `CloudStreamingSession`（建连 → 20 ms 分批喂 → 收定稿 → 收尾），用 TTS 语料算聚合 CER，并断言最后一句的本地轴终点贴着音频末点。

```bash
export VOICE_NOTES_VOLCENGINE_KEY=$(python3 -c "import json,os;d=json.load(open(os.path.expanduser('~/Library/Application Support/com.github.bytes-astra.voicenotes/settings.json')));print([p['api_key'] for p in d['cloud_profiles'] if p['provider']=='volcengine'][0])")
export VOICE_NOTES_ALIYUN_KEY=$(python3 -c "import json,os;d=json.load(open(os.path.expanduser('~/Library/Application Support/com.github.bytes-astra.voicenotes/settings.json')));print([p['api_key'] for p in d['cloud_profiles'] if p['provider']=='aliyun'][0])")
export VOICE_NOTES_STREAMING_MODEL_DIR="$HOME/Library/Application Support/com.github.bytes-astra.voicenotes/models/streaming"  # 可选：算本地基线
bash scripts/gen-tts-corpus.sh          # 没语料时先生成（不入库）
cd src-tauri
cargo test --lib cloud_quality -- --nocapture --test-threads=1
cargo test --lib last_sentence_end -- --nocapture --test-threads=1
cargo test --lib server_endpoint_offset -- --nocapture --test-threads=1
```

- **没设某家的 key → 跳过那一家**（打印「跳过：未设置 …」）；两家都没设 → 整个用例跳过。**跳过 ≠ 通过**。
- 语料缺失同样跳过；`tts-` 语料必须带同名 `.txt`（算 CER 用）。
- 阈值：火山 TTS 聚合 ≤ **65%**、阿里 ≤ **20%**（各按 2026-11-28 实测 +10pp 取整；`VOICE_NOTES_MAX_CLOUD_CER` 可统一覆盖）；真人 fixture `tests/fixtures/zh.wav` ≤ **25%**。
- 相对断言：设了 `VOICE_NOTES_STREAMING_MODEL_DIR` 时，云端 TTS 聚合不得比**本地流式基线 + 20pp** 更差。
- 终点断言（`last_sentence_end_tracks_the_local_audio_clock`）：第一条 TTS 语料 + 1.5 s 静音，最后一句的**本地轴**终点与音频末点之差 ≤ **3 s**。**它守的是本地 16 kHz 时钟**（终点没被截断/漂走）。mutation 自证：把 `force_end()` 的 `commit_at(self.fed_end, …)` 改成 `fed_end - 4 * 16_000` → 用例红（差 -4.00 s）；< 3 s 的截断它抓不住。
- 服务端终点偏差（`server_endpoint_offset_stays_bounded`）：`zh.wav` + 1.5 s 静音，`offset = 本地轴终点 − 服务端 end`，`|offset| ≤ **3 s**`。**这条守的是 provider 侧**（服务端时间戳与本地轴差多少），详见下一节。mutation 自证：上界 3 → 1 s → 两家都红（+1.28 / +1.71）。
- **喂入粒度**：门禁默认 20 ms/批（`VOICE_NOTES_CLOUD_CHUNK` 可改），生产 `record.rs` 的云端循环是 **60 ms/批**；**线上帧长与两者都无关** —— provider 自己重新攒包：火山 200 ms（`AUDIO_PACKET_BYTES = 6400`）、阿里 100 ms（`= 3200`）。实测把 20 ms 改成 200 ms，两家聚合 CER 几乎不变（见下）。

**实测（2026-11-28，本机 Apple Silicon，TTS 20 条 + 真人 1 条，真 key，跑两轮看方差）**：

| provider | TTS 聚合 CER | 真人 `zh.wav` | 终点差 |
| --- | --- | --- | --- |
| 火山（豆包 2.0 小时版） | **54.20% / 54.20%** | 0% | 0.00 s |
| 阿里（Qwen-Audio 3.1 流式） | **6.30% / 7.14%** | 0% | 0.00 s |
| 本地流式基线（同一批 TTS） | 43.28% | — | — |

**火山在 `say` 合成音上很差，这是语料域外，不是驱动 bug —— 单变量实验证据（2026-11-28）**：

| 实验 | 只改的变量 | 火山聚合 CER | 阿里聚合 CER |
| --- | --- | --- | --- |
| 基线 | 1 s 尾静音 / 20 ms 批 | 54.20% | 6.30%（另两轮 7.14%） |
| A · 收尾 | 尾静音 1 s → **3 s** | **54.62%** | **6.72%** |
| B · 分包 | 每批 320 → **3200 采样（200 ms）** | **54.20%** | **7.14%** |

两个变量都改不动结果：收尾不是没等够，分包也不是驱动问题（provider 本来就会重新攒成 200 / 100 ms 的线上帧）。同样四条「100%」的文件（tts-06 / 12 / 19 / 20）在 A、B 里仍然一个事件都不回 —— 事件流 dump 确认**连 Partial 都没有**，不是「只缺定稿」。同一条音频本地整段模型 0%、阿里 0~35%，而**本地流式 Zipformer 也认得很差**（tts-06 本地流式出「手段现在太多了」、tts-12 出「新闻回本方案软」）—— 说明是这批 Flo 合成音对**流式模型**域外，阿里的大模型才扛得住。所以**两家各用一个阈值**，TTS 绝对值不当质量结论 —— 门禁的「牙齿」是真人 fixture 那一条（两家实测 0%，上限 25%）。

**这个门禁不进 CI**（要真 key、要联网、跑一次约 90 s 且服务端计费）—— 与 `quality_gate.rs` 同一取舍。改云端协议 / 重采样 / 收尾顺序时手动跑一次。

### 服务端终点偏差怎么测（`server_endpoint_offset_stays_bounded`）

[`CLOUD.md`](CLOUD.md) 那张表的**两半**现在都有守卫：本地轴那一半由 `last_sentence_end_tracks_the_local_audio_clock` 守，服务端那一半由这条守（BL-13 / change `2026-11-28-provider-timestamps`）。

- **数据从哪来**：两家 provider 把响应里的句级时间戳填进 `CloudEvent::Final` 的 `start_ms` / `end_ms`（火山 `utterances[].start_time` / `end_time`、阿里 `sentence.begin_time` / `end_time`，**毫秒**）。生产路径**不读**它们 —— 句子时间仍只由本地 16 kHz 时钟决定。
- **怎么读出来**：`quality::TimestampTap` 是只读捕获包装（只在 `#[cfg(test)]` 编译）：把 `CloudProvider` 的调用原样转发，只在 `connect` 时抄一份 `Final` 的时间戳；`CloudStreamingSession` 收到的还是同一串事件。测试**不改生产路径**。
- **条件**：`tests/fixtures/zh.wav`（5.592 s）+ 1.5 s 尾静音（音频末点 7.092 s），与 `CLOUD.md` 的表一致；`offset = 本地轴终点 − 服务端 end`（正数 = 本地轴偏长）。
- **判定**：`|offset| ≤ 3 s`（与 `ENDPOINT_SLACK_SECONDS` 同口径）。**双向**都判 —— provider 时间戳单位错（秒当毫秒）会以另一个方向越界。
- **跳过 / 失败**：没设某家 key → 打印「跳过」；两家都没设 → 整体跳过。**provider 的定稿里没有 `end_ms` 算失败**（路线 A 的契约：两家都要填），不是跳过。
- **源码守卫**：`session_never_reads_provider_timestamps` 扫描 `src/asr/cloud.rs` 的 `impl CloudStreamingSession` 块（去掉注释 / 字符串），不许出现 `start_ms` / `end_ms`。mutation：在会话层绑定 `end_ms` → 红。
- **实测（2026-11-28，真 key）**：火山 `本地轴 7.09 s / 服务端 5.81 s`，差 **+1.28 s**；阿里 `本地轴 7.09 s / 服务端 5.38 s`，差 **+1.71 s** —— 与 `CLOUD.md` 的 1.28 / 1.71 s 逐位一致，同时证实字段单位是毫秒。

> [!NOTE]
> 这条守的是「偏差不许涨过 3 s」，不是精度断言（服务端有随机性）。若它突然变红，先看打印出的服务端 `end` 是否是毫秒量级（~5000）—— 若变成个位数，说明服务端改了字段单位或字段名（provider 解析会得到 `None` → 也会红，并提示「没有服务端 end 时间戳」）。

## 🖥️ 界面检查

Playwright + 假 `window.__TAURI__`，不需要真应用：

```bash
npm run check:ui     # 239 项检查
npm run shots        # 重出 README 10 张截图
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
- **mock 的 `set_hotwords` 返回要带 `hotwordsIgnored`**：真后端回报「有几条包含模型词表之外的字」（流式模型是单字词表，表外的字让整条术语作废）。少了这个字段，界面永远弹「已保存 N 个术语」，「其中 M 个不会生效」这条提示就测不到。

## 📚 相关文档

- 工程约定、模块边界、参数不变式、踩过的坑：[../AGENTS.md](../AGENTS.md)（总索引；专题文档：[FRONTEND.md](FRONTEND.md) / [BACKEND.md](BACKEND.md) / [ASR.md](ASR.md) / [CLOUD.md](CLOUD.md) / [RELEASE.md](RELEASE.md)）
- 复审的固定作业面（每轮从「⚠️ / ❌」条目开始，别另起清单）：[REVIEW-CHECKLIST.md](REVIEW-CHECKLIST.md)
- 工程改进待办（门禁缺口、结构简化、中期升级）：[BACKLOG.md](BACKLOG.md)
- 排查步骤（能录音没字 / 静默丢音频）：[TROUBLESHOOTING.md](TROUBLESHOOTING.md)
