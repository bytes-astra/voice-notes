# 长时间内容支持 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让录音和文件转写都不限时长（内存恒定），两种模式都边录边出字。

**Architecture:** 采集缓冲改成消费式（ASR 线程取走即释放），两种模式共用一个 ASR 线程：流式喂 Zipformer 在线解码，整段喂 Silero VAD 切句后逐句送 SenseVoice。文件转写搬到后端用 symphonia 解码，走同一条管线。hybrid 的「复核」删掉，只留「结果异常」的检测提醒。

**Tech Stack:** Tauri 2 / Rust 2021（MSRV 1.78）/ sherpa-onnx 1.13.8 / symphonia 0.5.5 / 原生 JS 前端（无构建步骤）

**Spec:** `docs/superpowers/specs/2026-10-22-long-content-design.md`

## Global Constraints

- 沙箱环境变量（每次 cargo 命令都要带）：`CARGO_HOME=/Users/hugh/Agents/DeepSeek/.cargo-home CARGO_TARGET_DIR=/Users/hugh/Agents/DeepSeek/.cache/target SHERPA_ONNX_ARCHIVE_DIR=/Users/hugh/Agents/DeepSeek/.cache/sherpa-archives`
- 命令参数一律 camelCase（Tauri 2 默认）；阻塞推理放 `tauri::async_runtime::spawn_blocking` 或专用线程，不放 async 执行器
- 前端是原生 JS，**不引入构建步骤**；所有文案走 `src/i18n.js` 字典（zh-CN + en 两份 key 必须完全对齐）；改文案后跑 `npm run check:i18n`
- 前端 UI 改动必须在浏览器里验（Playwright + 假 `window.__TAURI__`），同步脚本是 `/tmp/uitest/sync.sh`，**必须用它**（否则 mock 被覆盖 → 页面半渲染）
- 事件名不改：`audio:level` / `audio:error` / `stream:partial` / `stream:segment` / `streaming:state` / `engine:state`
- SenseVoice 单次输入上限 30s 是官方限制，不改
- 「整段 / 流式」两个词不改（区分的是模型性质，不是 UI 行为）
- 真模型测试的环境变量：`VOICE_NOTES_MODEL_DIR`（整段）、`VOICE_NOTES_STREAMING_MODEL_DIR`（流式）、`VOICE_NOTES_PUNCT_MODEL`（标点，可选）；缺失时测试自动跳过

## Review Focus

下面这些是规格暗示、但任何单个任务的测试都覆盖不到、而真实用户最可能撞上的失败模式。每条都指明了由哪个任务、用哪个测试钉住它。

1. **结果区的时长/耗时常为 0** —— `Transcript` 序列化成 snake_case，前端读 camelCase（现存 bug，详见 Task 5 Step 3）。→ **Task 5**，加一个断言 `serde_json::to_value(&transcript)` 里 `durationSeconds` 存在的单元测试
2. **录音中途模型/引擎出问题**（模型文件被删、引擎加载失败）→ 用户应该看到明确报错，而不是录音跑完才失败或静默无输出 → **Task 4**，`start_recording` 在整段模型缺失时返回错误（前端 `startRecording` 已有的错误提示路径覆盖）
3. **录音几乎没声音**（静音、极轻的呼吸）→ 应该给出「没识别到内容」而不是空白结果或崩溃 → **Task 4**，`stop_recording` 在 `consumed_samples` 太少时返回「录音太短，至少说 0.2 秒」
4. **用户在一个句子说到一半时停止录音** → 该句也要被识别出来 → **Task 3**，`offline_session_finishes_partial_sentence`（见该任务 Step 1）
5. **文件解码失败 / 格式不支持**（损坏的 mp4、非音频文件）→ 明确报错，不静默产出空结果 → **Task 6**，`decode_to_16k` 对损坏文件返回 `Err`
6. **录音中切换语言或 ITN** → 语言芯片在录音中已禁用（`src/app.js` 的 `chip.disabled = state.phase !== 'idle'`）；引擎在开始录音时构建一次，录音中不换 → **Task 7**，Playwright 用例断言录音状态下语言芯片是 disabled

---

### Task 1: 采集缓冲消费化 + 独立电平环

**Files:**
- Modify: `src-tauri/src/audio.rs`
- Test: `src-tauri/src/audio.rs`（模块内 `#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: 现有 `Recorder`、`append()`、`peak()`、`MAX_SECONDS`
- Produces:
  ```rust
  pub struct SampleQueue { /* Arc<Mutex<Vec<f32>>> */ }
  impl SampleQueue {
      pub fn new() -> Self;
      pub fn push(&self, samples: &[f32]);
      pub fn take_new(&self) -> NewAudio;
  }
  pub struct NewAudio { pub samples: Vec<f32>, pub dropped: u32 }
  impl Recorder {
      pub fn queue(&self) -> SampleQueue;
  }
  pub fn gap_marker(dropped_samples: u32) -> String;   // 例如「……（此处约 3 秒未识别）……」
  ```
  常量：`pub const MAX_PENDING_SAMPLES: usize = 2_880_000;`（48kHz 下 60 秒）

- [ ] **Step 1: 写失败测试**

在 `src-tauri/src/audio.rs` 的测试模块里加：

```rust
#[test]
fn take_new_consumes_samples() {
    let q = SampleQueue::new();
    q.push(&[1.0, 2.0, 3.0]);
    let got = q.take_new();
    assert_eq!(got.samples, vec![1.0, 2.0, 3.0]);
    assert_eq!(got.dropped, 0);
    assert!(q.take_new().samples.is_empty());   // 取过就没有了
}

#[test]
fn take_new_drops_oldest_when_over_limit() {
    let q = SampleQueue::new();
    q.push(&vec![0.0; MAX_PENDING_SAMPLES + 100]);
    let got = q.take_new();
    assert_eq!(got.dropped, 100);
    assert_eq!(got.samples.len(), MAX_PENDING_SAMPLES);
}

#[test]
fn gap_marker_mentions_seconds() {
    assert!(gap_marker(48_000).contains("2"));   // 48000 @48k = 1 秒
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME=/Users/hugh/Agents/DeepSeek/.cargo-home CARGO_TARGET_DIR=/Users/hugh/Agents/DeepSeek/.cache/target cargo test --lib audio::`
Expected: FAIL —— `SampleQueue` / `gap_marker` 未定义

- [ ] **Step 3: 实现**

- 新增 `SampleQueue`，内部 `Arc<Mutex<Vec<f32>>>`；`push(&self, samples: &[f32])` 追加；`take_new(&self) -> NewAudio` 取走全部未消费样本（`std::mem::take`），若取之前长度超过 `MAX_PENDING_SAMPLES` 则先丢最旧的并把丢弃数放进 `dropped`。
- `Recorder` 里的 `samples: Arc<Mutex<Vec<f32>>>` 改成 `queue: SampleQueue`；`Recorder::queue()` 返回 clone。
- `append()` 里对 `cap` 的 600s 判断删掉（上限改由 `SampleQueue` 的 `MAX_PENDING_SAMPLES` 负责）。
- **电平表独立**：`Recorder` 另加 `levels: Arc<Mutex<Vec<f32>>>`（容量 2048 的环形缓冲），`append()` 同时把样本写进去；`peak()` 改读 `levels` 而不是采集缓冲的尾巴。
- `gap_marker(dropped_samples: u32) -> String`：按 `dropped_samples / 48000` 估算秒数（不足 1 秒按 1 秒算），返回形如 `……（此处约 3 秒未识别）……` 的字符串。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... cargo test --lib audio::`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/audio.rs
git commit -m "feat(audio): 采集缓冲改成消费式，电平表独立成环"
```

---

### Task 2: `Engine::recognize`（不跑 VAD 的直接解码）

**Files:**
- Modify: `src-tauri/src/asr.rs`
- Test: `src-tauri/tests/asr_smoke.rs`

**Interfaces:**
- Consumes: `Engine`、`decode()`（模块私有）、`Engine::transcribe`
- Produces: `pub fn recognize(&self, samples: &[f32]) -> String` —— 对一段已经是完整句子的音频直接解码，不做 VAD 切分

- [ ] **Step 1: 写失败测试**

在 `src-tauri/tests/asr_smoke.rs` 加（复用该文件里已有的 `model_paths()` helper 和 `sherpa_onnx::Wave::read`）：

```rust
/// recognize 与 transcribe 对同一段音频给出一致的文本（证明去掉内部 VAD 没改变解码）
#[test]
fn recognize_matches_transcribe_on_one_sentence() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let engine = Engine::new(&paths, "zh", true).expect("加载模型");

    let direct = engine.recognize(wave.samples());
    let batch = engine.transcribe(wave.samples());

    assert!(!direct.trim().is_empty());
    assert_eq!(direct.trim(), batch.text.trim());
}
```


- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... VOICE_NOTES_MODEL_DIR=/Users/hugh/Library/Application\ Support/com.hugh.voicenotes/models cargo test --test asr_smoke recognize_matches -- --nocapture`
Expected: FAIL —— `recognize` 未定义

- [ ] **Step 3: 实现**

在 `impl Engine` 里加：

```rust
/// 对一段已经切好的句子直接解码（不再跑 VAD）。
pub fn recognize(&self, samples: &[f32]) -> String {
    decode(&self.recognizer, samples)
}
```

`decode` 已经是模块内的私有函数（`asr.rs` 里 `fn decode(&self.recognizer, segment.samples())`），直接复用，不要复制实现。

- [ ] **Step 4: 跑测试确认通过**

Run: 同 Step 2 的命令
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/asr.rs src-tauri/tests/asr_smoke.rs
git commit -m "feat(asr): 新增 recognize()，跳过内部 VAD 直接解码"
```

---

### Task 3: 整段模式的「边录边按句识别」ASR 会话

**Files:**
- Modify: `src-tauri/src/asr.rs`
- Test: `src-tauri/tests/asr_smoke.rs`

**Interfaces:**
- Consumes: `Engine::recognize`（Task 2）、`SileroVadModelConfig` / `VoiceActivityDetector`（现有配置在 `asr.rs:66-76`）、`Segment`
- Produces:
  ```rust
  pub enum OfflineEvent { Segment(Segment) }
  pub struct OfflineSession { /* ... */ }
  impl OfflineSession {
      pub fn new(engine: Arc<Engine>, vad: VadModelConfig) -> Self;
      pub fn push(&mut self, samples: &[f32]) -> Vec<OfflineEvent>;
      pub fn finish(&mut self) -> Vec<OfflineEvent>;   // flush VAD 里没成句的剩余音频
      pub fn segments(&self) -> &[Segment];
      pub fn elapsed_ms(&self) -> u64;                  // 纯推理耗时累计
      pub fn consumed_samples(&self) -> usize;          // 已消费的样本数（判断「录得太短」用）
  }
  ```

- [ ] **Step 1: 写失败测试**

在 `src-tauri/tests/asr_smoke.rs` 加（测试文件里需要一个 `vad_config()` helper，参数照抄 `src-tauri/src/asr.rs:66-76` 的现有配置：`threshold 0.5` / `min_silence_duration 0.5` / `min_speech_duration 0.25` / `max_speech_duration 30.0` / `sample_rate 16000` / `window_size 512`；VAD 模型文件用 `model_paths()` 里找到的 `vad`，找不到就 `None`）：

```rust
/// 边录边识别与整条一次识别的文本一致（去掉空白和标点后比字符）
#[test]
fn offline_session_matches_batch_transcribe() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));

    // 路径 A：边录边（按 3200 样本推进）
    let mut session = OfflineSession::new(engine.clone(), vad_config());
    let mut events = Vec::new();
    for chunk in wave.samples().chunks(3200) {
        events.extend(session.push(chunk));
    }
    events.extend(session.finish());
    let live: String = events
        .iter()
        .map(|e| match e {
            OfflineEvent::Segment(s) => s.text.as_str(),
            _ => "",
        })
        .collect();

    // 路径 B：整条一次
    let batch = engine.transcribe(wave.samples());

    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !"，。！？、；：“”‘’（）《》".contains(*c))
            .collect()
    };
    assert!(!norm(&live).is_empty());
    assert_eq!(norm(&live), norm(&batch.text));
}

/// 说到一半停止：finish() 要把没成句的剩余音频也识别出来
#[test]
fn offline_session_finishes_partial_sentence() {
    let Some(paths) = model_paths() else { return };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
    let mut session = OfflineSession::new(engine.clone(), vad_config());
    // 只喂前 1.2 秒（不够 VAD 判静音成句），然后 finish
    let head = &wave.samples()[..(16_000 * 12 / 10).min(wave.samples().len())];
    let _ = session.push(head);
    let events = session.finish();
    let text: String = events.iter().map(|e| match e { OfflineEvent::Segment(s) => s.text.as_str(), _ => "" }).collect();
    assert!(!text.trim().is_empty(), "收尾时应该把没说完的半句也识别出来");
}
```


- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... VOICE_NOTES_MODEL_DIR=... cargo test --test asr_smoke offline_session_matches -- --nocapture`
Expected: FAIL —— `OfflineSession` 未定义

- [ ] **Step 3: 实现**

在 `src-tauri/src/asr.rs` 加 `OfflineSession`：

- 构造：`VoiceActivityDetector::create(&vad_config, MAX_SEGMENT_SECONDS + 2.0)`（与 `Engine::transcribe` 同一套参数：`threshold 0.5` / `min_silence_duration 0.5` / `min_speech_duration 0.25` / `max_speech_duration 30.0` / `sample_rate 16000`）。
- `push(samples)`：按 512 样本窗口喂 `detector.accept_waveform(window)`；`while !detector.empty()` 时取 `detector.front()` 的样本 → `engine.recognize(...)` → 非空则产出 `OfflineEvent::Segment(Segment { text, start, end })` → `detector.pop()`。
  - `start` / `end` 用**累计已消费样本数**换算成秒，与 `Engine::transcribe` 的口径一致。
- `finish()`：`detector.flush()` 后把剩余句子同样识别产出。
- `elapsed_ms`：每次 `recognize` 前后取 `Instant::now()` 差值累加。
- `consumed_samples`：`push` 收到的样本总数 + `finish` 里 flush 的样本数。

- [ ] **Step 4: 跑测试确认通过**

Run: 同 Step 2
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/asr.rs src-tauri/tests/asr_smoke.rs
git commit -m "feat(asr): 整段模式支持边录边按句识别"
```

---

### Task 4: 录音命令改走 ASR 线程（整段 + 流式统一）

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/audio.rs`（`Recorder::stop()` 不再返回全量样本）
- Test: `src-tauri/tests/streaming_smoke.rs`（现有流式真模型测试必须继续通过）

**Interfaces:**
- Consumes: `OfflineSession`（Task 3）、`SampleQueue::take_new`（Task 1）、`StreamingSession`（现有）、`Arc<Engine>` / `Arc<StreamingEngine>` 缓存
- Produces:
  - `AppState` 新增 `offline_session: Mutex<Option<OfflineRun>>`（`OfflineRun { stop: Arc<AtomicBool>, rx: mpsc::Receiver<Transcript> }`），与现有 `stream: Mutex<Option<StreamSession>>` 对称
  - `stop_recording` 在整段模式下**不再**调用 `transcribe_samples`，而是等 ASR 线程收尾
  - `Recorder::stop()` 签名变为 `pub fn stop(self) -> Result<(), String>`（不再返回 `Vec<f32>`）

- [ ] **Step 1: 写失败测试**

在 `src-tauri/tests/streaming_smoke.rs` 加：

```rust
/// 采集缓冲消费化后，流式拿到的音频不能少（缓冲消费不丢样本）
#[test]
fn streaming_via_queue_matches_direct() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

    // 路径 A：直接喂
    let engine = Arc::new(StreamingEngine::new(&paths).expect("加载流式模型"));
    let mut direct = StreamingSession::new(engine.clone(), None);
    let mut direct_events = Vec::new();
    for chunk in wave.samples().chunks(3200) {
        direct_events.extend(direct.push(chunk));
    }
    direct_events.extend(direct.finish());

    // 路径 B：经采集队列消费
    let queue = voice_notes_lib::audio::SampleQueue::new();
    queue.push(wave.samples());
    let mut via_queue = StreamingSession::new(engine, None);
    let mut queue_events = Vec::new();
    loop {
        let got = queue.take_new();
        if got.samples.is_empty() {
            break;
        }
        assert_eq!(got.dropped, 0, "不该丢样本");
        queue_events.extend(via_queue.push(&got.samples));
    }
    queue_events.extend(via_queue.finish());

    let text = |events: &[StreamEvent]| -> String {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Segment(s) => Some(s.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    };
    assert!(!text(&queue_events).trim().is_empty());
    assert_eq!(text(&direct_events), text(&queue_events));
}
```


- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=/Users/hugh/Library/Application\ Support/com.hugh.voicenotes/models/streaming cargo test --test streaming_smoke -- --nocapture`
Expected: FAIL（整段路径已改但流式路径仍用旧的游标 `take_new`，行为未对齐）

- [ ] **Step 3: 实现**

`src-tauri/src/lib.rs`：

- 新增 `fn run_offline(app, engine: Arc<Engine>, queue: SampleQueue, rate: u32, stop: Arc<AtomicBool>) -> Transcript`：循环 `queue.take_new()` → 重采样到 16k（复用 `LinearResampler`，与 `run_streaming_core` 同样写法）→ `session.push(...)` → 每产出一句就 `app.emit("stream:segment", ...)`；`stop` 为真时 `session.finish()` 收尾并返回 `Transcript`。
- **缺口标记**：`take_new()` 返回的 `dropped > 0` 时，把 `audio::gap_marker(dropped)` 作为一个 `Segment` 插进结果（`text` 就是那段标记，`start`/`end` 用当前时间轴），并 `app.emit("audio:error", "识别跟不上，部分音频未转写")`。录音不中断，缺口在文本里看得见。
- `start_recording` 的整段分支：先检查整段模型就绪（`models::resolve(&app, settings.model_dir.as_deref())`，没有就返回「整段模型未就绪，请先下载」），从 `state.engine` 缓存**取走** `Engine`（`Arc`），起线程，存 `AppState.offline_session`。
- `stop_recording` 的整段分支：停采集 → 置 stop → `rx.recv_timeout(20s)` → 把 `Engine` **还回** `state.engine` 缓存 → 返回 Transcript。
- `transcribe_samples` 保留（文件转写与 `warm_up` 仍用），但 `correct_streaming` 的调用点删除（Task 5）。
- `stream:segment` 的载荷改成增量（见 Task 5 的接口），两种模式共用。

`src-tauri/src/audio.rs`：`Recorder::stop()` 不再返回全量样本（改 `Result<(), String>`）；`Recorder::buffer()` 删掉，改用 `Recorder::queue()`。

- [ ] **Step 4: 跑测试确认通过**

Run: 同 Step 2，外加 `cargo test --lib`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/lib.rs src-tauri/src/audio.rs src-tauri/tests/streaming_smoke.rs
git commit -m "feat(recording): 整段模式改走 ASR 线程，两种模式统一"
```

---

### Task 5: 事件载荷改成增量 + hybrid 缩减为检测提醒

**Files:**
- Modify: `src-tauri/src/lib.rs`（`StreamEmitter`、事件载荷、`correct_streaming` 删除）
- Modify: `src-tauri/src/hybrid.rs`（只剩 `is_degenerate`）
- Modify: `src-tauri/src/settings.rs`（`hybrid: bool` → `warn_on_bad: bool`，默认 true）
- Delete: `src-tauri/tests/hybrid.rs`
- Test: `src-tauri/src/hybrid.rs`（模块内单元测试）

**Interfaces:**
- Consumes: `hybrid::is_degenerate`
- Produces:
  - 事件载荷（增量）：
    ```rust
    #[derive(Clone, Serialize)]
    struct SegmentPayload { segment: String, start: f32, end: f32 }
    #[derive(Clone, Serialize)]
    struct PartialPayload { partial: String }
    ```
    `stream:segment` 发 `SegmentPayload`（**不再有 committed 累计全文**）；`stream:partial` 发 `PartialPayload`（同样去掉 committed）。
  - `settings::Settings.warn_on_bad: bool`（默认 true）；`Status.warn_on_bad: bool`
  - 命令 `set_warn_on_bad(enabled: bool) -> Status`（替换 `set_hybrid`）

- [ ] **Step 1: 写失败测试**

在 `src-tauri/src/hybrid.rs` 的测试模块里保留/新增：

```rust
#[test]
fn detects_empty_and_repeated() {
    assert!(is_degenerate("", 3.0));
    assert!(is_degenerate("走走走走走走", 3.0));
    assert!(!is_degenerate("今天下午三点开一个产品评审会", 3.0));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... cargo test --lib hybrid::`
Expected: FAIL —— 旧 `hybrid.rs` 里 `is_degenerate` 的判定口径与新测试不一致（旧的 `punctuate`/`finalize` 测试仍在），或者因为文件已被删而找不到模块

- [ ] **Step 3: 实现**

- `src-tauri/src/hybrid.rs`：只保留 `is_degenerate(text, duration_seconds) -> bool`；删掉 `should_correct` / `punctuate` / `finalize`；保留 `is_degenerate` 的现有判定口径（空、唯一字符占比 < 1/2、字数 < 时长×0.5）。
- 删除 `src-tauri/tests/hybrid.rs`（里面测的 `punctuate` / `finalize` 随功能一起没了）。
- `src-tauri/src/settings.rs`：`hybrid: bool` → `warn_on_bad: bool`，`Default` 里 `true`。
- `src-tauri/src/lib.rs`：
  - 删 `correct_streaming` 函数和它在 `stop_recording` 里的调用；整段/流式都不再跑第二遍 SenseVoice。
  - 删 `set_hybrid` 命令，加 `set_warn_on_bad`；`Status.hybrid` → `Status.warn_on_bad`。
  - `StreamEmitter` 与所有 `stream:segment` / `stream:partial` 发送点改成增量载荷。
  - 停止录音后：如果 `settings.warn_on_bad` 且 `hybrid::is_degenerate(&transcript.text, transcript.duration_seconds)` → 发一条 `engine:state` 之外的新事件？**不要新增事件名** → 复用 `audio:error`？不合适。改为在 `Transcript` 里加字段 `pub quality_warning: Option<String>`，由前端 `showTranscript` 读它弹提示。`Transcript` 是 serde 序列化的，前端拿得到。
- `Transcript` 新增字段 `pub quality_warning: Option<String>`；`Engine::transcribe` 和 `OfflineSession` 产出的 Transcript 都填 `None`，由 `stop_recording` 收尾时统一判定并填。
- **修一个现存 bug**：`Transcript`（`src-tauri/src/asr.rs`）没有 `#[serde(rename_all = "camelCase")]`，序列化出来是 `duration_seconds` / `elapsed_ms`，但前端读的是 `transcript.durationSeconds` / `transcript.elapsedMs`（`src/app.js:338-339`）→ **两个值恒为 undefined，结果区的「音频时长 / 用时 / RTF」和历史记录里的时长现在全是 0**。给 `Transcript` 和 `Segment` 加 `#[serde(rename_all = "camelCase")]`（与 `lib.rs` 里 `Status` 等结构体一致），前端不用改。
- `Segment` 同样加 `#[serde(rename_all = "camelCase")]`（`start` / `end` 是单词，加不加都一样，但保持一致）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... cargo test --lib && cargo test --test asr_smoke -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add -A src-tauri
git commit -m "feat(stream): 事件载荷改增量，hybrid 只留异常检测提醒"
```

---

### Task 6: 后端 symphonia 解码 + `transcribe_file` 命令

**Files:**
- Modify: `src-tauri/Cargo.toml`（加依赖）
- Create: `src-tauri/src/decode.rs`
- Modify: `src-tauri/src/lib.rs`（新命令 + 模块声明）
- Test: `src-tauri/tests/decode_smoke.rs`（新建）

**Interfaces:**
- Consumes: `OfflineSession`（Task 3）、`Engine::recognize`
- Produces:
  ```rust
  // src-tauri/src/decode.rs
  pub struct DecodeInfo { pub sample_rate: u32, pub samples: u64 }
  /// 从磁盘流式解码音频，逐块回调 16 kHz 单声道 f32。
  pub fn decode_to_16k(
      path: &Path,
      mut on_chunk: impl FnMut(&[f32]) -> Result<(), String>,
  ) -> Result<DecodeInfo, String>;
  ```
  命令：`transcribe_file(path: String, language: Option<String>) -> Transcript`（camelCase 参数名）

- [ ] **Step 1: 加依赖并写失败测试**

`src-tauri/Cargo.toml` 加：

```toml
symphonia = { version = "0.5", features = ["all"] }
```

（用 0.5 而不是 0.6：0.6.1 的 MSRV 是 1.85，本项目声明 `rust-version = "1.78"`。）

新建 `src-tauri/tests/decode_smoke.rs`：

```rust
/// 解码一个真实 mp3，采样数与时长符合预期
#[test]
fn decodes_mp3_to_16k() {
    let path = std::path::Path::new("tests/fixtures/sample.mp3");
    if !path.exists() { eprintln!("跳过：缺少 sample.mp3"); return; }
    let mut total = 0usize;
    let info = voice_notes_lib::decode::decode_to_16k(path, |chunk| { total += chunk.len(); Ok(()) }).unwrap();
    assert_eq!(info.sample_rate, 16_000);
    assert!(total > 16_000);   // 至少 1 秒
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... cargo test --test decode_smoke -- --nocapture`
Expected: FAIL —— `voice_notes_lib::decode` 模块不存在

- [ ] **Step 3: 实现**

`src-tauri/src/decode.rs`：

- 用 `symphonia::default::get_probe().format(&path, &FormatOptions::default(), &MetadataOptions::default())` 打开文件；探测失败时回退 `get_probe().probe(...)`。
- 逐包 `format.next_packet()` → `codec.decode_packet(&packet)` → 转成 f32 → 混成单声道 → 用 `symphonia::resample` 或项目已有的 `LinearResampler`（sherpa 的）重采样到 16k → 回调 `on_chunk`。
- 视频轨跳过（symphonia 的 mp4 reader 只给音频轨时要过滤 `packet.track_id()` 与首个音频轨一致）。
- 任何解码错误 → 返回 `Err("无法解码这个文件：{原因}")`（不静默产出空结果）。

`src-tauri/src/lib.rs`：

- `pub mod decode;`
- 新命令 `transcribe_file`：`models::resolve` 取整段模型 → 起 `OfflineSession` → `decode_to_16k(path, |chunk| { session.push(chunk) → 逐句 emit stream:segment })` → 返回 `Transcript`（duration = 解码出的秒数，elapsed = 纯推理耗时）。
- 推理是阻塞的 → 整个函数体放进 `tauri::async_runtime::spawn_blocking`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... cargo test --test decode_smoke -- --nocapture`
Expected: PASS（没有 `sample.mp3` 时跳过也算通过）

- [ ] **Step 5: 提交**

```bash
git add src-tauri/Cargo.toml src-tauri/src/decode.rs src-tauri/src/lib.rs src-tauri/tests/decode_smoke.rs
git commit -m "feat(decode): 用 symphonia 在后端解码音频文件"
```

---

### Task 7: 前端改造

**Files:**
- Modify: `src/app.js`
- Modify: `src/index.html`
- Modify: `src/i18n.js`、`src/locales/en.js`
- Test: Playwright 脚本 `/tmp/uitest/check_long_content.js`（新建，仿 `check_i18n_fix.js` 的写法）

**Interfaces:**
- Consumes: 增量事件载荷（Task 5）、`transcribe_file` 命令（Task 6）、`Transcript.quality_warning`
- Produces: 前端行为
  - 录音不再自动停止（删掉 `MAX_SECONDS` 与 `startTimer` 里的自动停逻辑）
  - `stream:segment` → 追加 `payload.segment` 到结果区；`stream:partial` → 只更新实时行
  - 文件转写走 `invoke('transcribe_file', { path })`，删掉前端 `decodeAudioData` / `encodeWave` / `audioBase64` 的文件路径用法
  - 整段模式录音中显示「正在听…」指示（不依赖 VAD，跟随音量）
  - `Transcript.quality_warning` 非空时弹提示

- [ ] **Step 1: 写失败测试（Playwright）**

新建 `/tmp/uitest/check_long_content.js`（复制 `check_i18n_fix.js` 的骨架：mock `invoke` 已在 `/tmp/ui-harness/mock-tauri.js`）：

```js
// 1. 录音 20 分钟后不会自动停：把 MAX_SECONDS 判断删掉后，用例改成
//    “录音状态下计时器超过 600s 仍然处于 recording 阶段”
// 2. stream:segment 增量载荷 → 结果区是追加而不是覆盖
// 3. 整段模式录音中显示“正在听”指示
```

具体断言：
- 派发 `{ payload: { segment: '第一句。', start: 0, end: 1 } }` 到 `stream:segment` 后，`#text` 的值为 `第一句。`
- 再派发 `{ payload: { segment: '第二句。', start: 1, end: 2 } }` 后，`#text` 的值为 `第一句。第二句。`（**追加**）
- 切到整段模式并 `setPhase('recording')` 后，存在可见的「正在听」元素（`#live-line` 不为空 或 新增的 `#listening` 元素可见）

- [ ] **Step 2: 跑测试确认失败**

Run: `bash /tmp/uitest/sync.sh && cd /tmp/uitest && PLAYWRIGHT_BROWSERS_PATH=/Users/hugh/Agents/DeepSeek/.cache/ms-playwright node check_long_content.js`
Expected: FAIL —— 增量追加与「正在听」都还没有

- [ ] **Step 3: 实现**

`src/app.js`：

- 删 `const MAX_SECONDS = 600` 与 `startTimer()` 里的 `if (elapsed >= MAX_SECONDS ...) stopRecording()`；计时器继续显示时长（后端 `audio:level` 事件带的 `elapsed`）。
- `listen('stream:segment')`：改成追加 `payload.segment` 到 `els.text.value`（用 `state.liveText` 累加，避免每次读 DOM）。
- `listen('stream:partial')`：只更新实时行（`showPartial(payload.partial)`），不再覆盖 `els.text`。
- 文件转写：`pick-file-btn` 与拖放都改成「拿到路径 → `invoke('transcribe_file', { path })`」；删掉 `decodeAudioData` / `OfflineAudioContext` / `encodeWave` / `audioBase64` 在文件路径上的使用（`encodeWave`/`audioBase64` 若没有别的调用方就一并删）。
- 文件转写进度：转写中在结果区上方显示「第 N 句 · 已用时 Xs」（句子数由前端累计收到的 `stream:segment` 事件得出，已用时走计时器）。
- 整段模式录音中：新增一个「正在听…」元素（`#listening`），录音中显示、停止后隐藏；文案走 i18n（`record.listening`）。
- `showTranscript(transcript)`：`if (transcript.qualityWarning) toast(transcript.qualityWarning, 'error')`。

`src/index.html`：加 `#listening` 元素；设置页把「流式校正」开关改成「结果异常时提醒我」（`#warn-toggle` / `#warn-status`），标点模型的说明改成「只对流式模式生效」。

`src/i18n.js` + `src/locales/en.js`：新增 `record.listening`、改 `settings.hybrid*` 为 `settings.warn*`（两份 key 完全对齐）。

- [ ] **Step 4: 跑测试确认通过**

Run: `bash /tmp/uitest/sync.sh && cd /tmp/uitest && PLAYWRIGHT_BROWSERS_PATH=... node check_long_content.js && cd /Users/hugh/Agents/DeepSeek/voice-notes && npm run check:i18n`
Expected: 全部 PASS；i18n 检查通过（2 个语言，key 数一致）

- [ ] **Step 5: 提交**

```bash
git add src/
git commit -m "feat(ui): 录音不限时长，增量出字，整段实时指示，文件转写走后端"
```

---

### Task 8: 文档同步 + 端到端验证

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/screenshots/*.png`（重新出图）

**Interfaces:**
- Consumes: 前面所有任务的最终行为
- Produces: 与实现一致的文档

- [ ] **Step 1: 改文档**

`README.md`：

- 「已知限制」里删掉「单次录音上限 10 分钟；整段模式单句上限 30 秒」，改成：录音不限时长（内存恒定）、整段模式单句 30 秒是 SenseVoice 的模型上限（官方限制）。
- 「两种识别方式」一节：整段改成「边录边按句出字，用 SenseVoice，多语言」；流式补一句「有上下文，适合长录音」。
- 加一段「支持的文件格式」：mp3 / wav / flac / ogg / m4a / mp4 / mov（后端 symphonia 解码，视频轨忽略）。
- 删掉「结果异常时用整段模型复核」的描述，改成「结果异常时提醒」。

`AGENTS.md`：

- 加一条约定：**采集缓冲是消费式的**（ASR 线程取走即释放，内存恒定）；电平表走独立的 2048 样本环，不能读采集缓冲。
- 加一条：**两种模式共用一个 ASR 线程**，整段用 `OfflineSession`（VAD 切句 + `Engine::recognize`），流式用 `StreamingSession`。
- 加一条：**事件载荷是增量的**（`stream:segment` 只带新句子），不要改回累计全文。
- 加一条：**hybrid 只做检测不做复核**；`is_degenerate` 命中只提示不改文本。
- 删掉「不要用 SenseVoice 无脑覆盖流式结果」那条复核相关的约定，保留「流式实测更准」的结论。
- 更新「已知坑」：加上「senseVoice 单次输入 30s 上限」「symphonia 0.5 的 MSRV 与 0.6 的差别」。

- [ ] **Step 2: 出图**

Run: `bash /tmp/uitest/sync.sh && cd /tmp/uitest && PLAYWRIGHT_BROWSERS_PATH=... node shoot_all.js`
Expected: 9 张截图全部重新生成（`docs/screenshots/`）

- [ ] **Step 3: 全量测试**

Run:
```bash
cd src-tauri && CARGO_HOME=... CARGO_TARGET_DIR=... cargo test --lib && \
  cargo test --test asr_smoke -- --nocapture && \
  cargo test --test streaming_smoke -- --nocapture && \
  cargo test --test decode_smoke -- --nocapture
```
Expected: 全部 PASS（`tests/hybrid.rs` 在 Task 5 已删除，命令里不要再有它）

- [ ] **Step 4: 打包并手动验证**

```bash
cd /Users/hugh/Agents/DeepSeek/voice-notes && SHERPA_ONNX_ARCHIVE_DIR=... CARGO_HOME=... CARGO_TARGET_DIR=... npm_config_cache=/tmp/npmcache npx tauri build --bundles app
```
然后启动 `dist/VoiceNotes.app` 手动确认：
- 录一段话，**两种模式都边录边出字**
- 录超过 10 分钟（可以快进计时器验证不自动停），内存不涨
- 拖一个 mp4 进去，能转写
- 流式结果异常时弹「结果可能有问题」的提示

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "docs: 同步长时间内容支持的说明与截图"
```
