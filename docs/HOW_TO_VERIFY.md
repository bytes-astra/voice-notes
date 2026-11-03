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

怀疑「录音丢了」（长录音后面少字、句子断在半截）先跑探针，别猜。四个探针在 `src-tauri/tests/`，只打印不断言；共用的小工具在 `src-tauri/tests/common/mod.rs`，探针里写 `mod common;` 引入：

```bash
VOICE_NOTES_MODEL_DIR=... cargo test --test vad_long_probe -- --nocapture      # VAD 吞不吞音频
VOICE_NOTES_MODEL_DIR=... cargo test --test offline_long_probe -- --nocapture   # 整段会话覆盖率
VOICE_NOTES_STREAMING_MODEL_DIR=... cargo test --test streaming_long_probe -- --nocapture
VOICE_NOTES_MODEL_DIR=... cargo test --test resample_probe -- --nocapture       # 48k→16k 重采样
```

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
npm run check     # 门禁：i18n → 前端依赖图 → 拼句规则 → clippy(-D warnings) → cargo test --lib
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
- `npm run check:i18n` 查字典对齐 + 引用完整性 + **后端错误码**。后端错误码的扫描是**文本级正则**：
  `code("…")` / `code_detail("…")` 都要认，`crate::` 前缀也算（`crate::code_detail("record.tooShort", …)`
  这种写法很常见，排掉冒号就会静默漏网）。门禁自己有测试（`scripts/check-i18n.test.mjs`），
  改扫描规则要连它一起改 —— 正则是文本级的，漏一个写法就漏一个错误码。
- `npm run check:join` 查前后端拼句规则一致：前端 `core.js` 的 `joinText()` 与后端 `asr.rs` 的
  `join_segments()` 是同一套规则的两份实现（实时出字在前端拼、最终文本在后端拼 ——
  两边不一致时界面上的字和保存下来的字会不一样）。脚本把两边的**判定结构**归一化后比对：
  CJK 码位区间（带端点开闭）、缺口占位符的完整形状（`[[gap:<数字>]]`）、三条拼接规则的表达式。
  只比对子串会漏掉最自然的漂移形状（`..=` 改 `..`、`||` 改 `&&`、少写半个条件、占位符少个 `]`）——
  这几种改法都会被脚本抓住（退出码 1）。

## 🖥️ 界面检查

Playwright + 假 `window.__TAURI__`，不需要真应用：

```bash
npm run check:ui     # 111 项检查
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

## 📚 相关文档

- 工程约定、模块边界、参数不变式、踩过的坑：[../AGENTS.md](../AGENTS.md)
- 复审的固定作业面（每轮从「⚠️ / ❌」条目开始，别另起清单）：[REVIEW-CHECKLIST.md](REVIEW-CHECKLIST.md)
- 质量调研与优化方案：[research/](research/)
