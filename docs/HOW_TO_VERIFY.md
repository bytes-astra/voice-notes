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

`src-tauri/tests/memory_probe.rs`：一个会话跑完 30 分钟音频，在 300 秒和 1800 秒两个点看
**峰值内存**（`getrusage` 的 `ru_maxrss`），增长必须 < 32 MB —— 这 25 分钟的音频本身就有 96 MB，
真把音频攒在内存里的话增长会远超上限。

```bash
VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
  cargo test --test memory_probe -- --nocapture --test-threads=1
```

实测（2026-11-03）：整段 300s 826.6 MB → 1800s 830.9 MB（+4.4 MB）、流式 +0.0 MB。
开头从 550 MB 跳到 830 MB 是**一次性预热**（模型 + ONNX arena + VAD 缓冲），不是随时长增长。

2026-10-24 实测结论（176 秒连续语音）：**VAD、整段会话、流式会话、重采样四条路都不丢音频**
（覆盖 175.6/176.0 秒，误差只有句边界）。**队列上限 60 秒是唯一的丢失点** —— 超了会丢**最旧**的
（`SampleQueue::take_new` 里 `drain(..excess)`），并留缺口标记；日志里会打「识别跟不上，丢掉 X 秒音频」。
识别是同步阻塞在消费线程里的（`record.rs` 的 `run_offline` → `asr.rs` 的 `drain()`），但 RTF 只有 0.01，
实际落后不到 1 秒，**离 60 秒上限差得很远** —— 所以「加 buffer」不是解法。

### 缺口与时间轴

丢音频时的三个口径都在单测里（不需要真模型；过滤器也会匹配到需要真模型的用例 —— 那几条没设环境变量时静默跳过，见下面「单元测试」一节的警告）：

- **录音时长含被丢掉的音频**：`record.rs` 的 `recorded_seconds()` = 喂进识别器的采样 + 被丢掉的采样。
  否则识别跟不上时界面上的时长会短一截。
- **句子时间在真实录音时间轴上**：整段模式的 VAD 只见过喂进去的音频，句子时间是「喂进去的时间轴」。
  `record.rs` 的 `to_real_time()` 按缺口位置换算，缺口标记落在**缺口终点**。
  换算在**秒域**比较（不绕道「秒 → 采样下标」—— f32 往返在长录音上会差 ±1 个采样）。
- **同一时刻缺口排在句子前面**：缺口终点与它后面第一句的起点相同，`asr.rs` 的 `insert_gaps()`
  显式让缺口优先（稳定排序会把后 extend 的缺口排到句子后面，文本变成「句子[[gap:3]]」）。
- **两种模式同一条规则**：流式会话的句子起点也要按丢掉的时间后移（`asr.rs` 的 `Timeline`），
  否则流式会排出「句子[[gap:4]]」而整段是「[[gap:4]]句子」。

带真模型跑一遍端到端（整段与流式各一条）：

```bash
VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
  cargo test --manifest-path src-tauri/Cargo.toml --lib -- gap_marker_reaches_transcript streaming_gap_marker_precedes --nocapture
```

缺 `VOICE_NOTES_STREAMING_MODEL_DIR` 时流式那半会静默跳过（见上面的警告）—— 那是「没验」，不是「过了」。

## 🧷 单元测试

```bash
npm run check     # 门禁：i18n → 前端依赖图 → 拼句规则 → 前后端契约 → 打包配置 → clippy(-D warnings) → cargo test --lib
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

## 🖥️ 界面检查

Playwright + 假 `window.__TAURI__`，不需要真应用：

```bash
npm run check:ui     # 140 项检查
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
- 质量调研与优化方案：[research/](research/)
