# 排查手册

VoiceNotes 里两类「不报错、但结果不对」的问题 —— **能录音、没字**：麦克风没权限，和音频电平太低被当成非语音丢掉。排查步骤、命令、实测都在这里。

工程约定与其它症状见 [`../AGENTS.md`](../AGENTS.md)，验证手段与基线见 [`HOW_TO_VERIFY.md`](HOW_TO_VERIFY.md)，复审作业面见 [`REVIEW-CHECKLIST.md`](REVIEW-CHECKLIST.md)。

## 麦克风权限排查

症状统一是「能开始录音、有时长统计，但识别不出东西」，日志里电平峰值全是 `0.00000`（真麦克风静音也有底噪，0.005 这种）。三种真因，按顺序排查。

### 1. 打包版缺 microphone entitlement

Tauri 打包签名带 hardened runtime（`codesign -dv` 打出 `flags=0x10002(adhoc,runtime)`），**缺 `com.apple.security.device.audio-input` 时 tccd 根本不弹授权框**，直接打 `Policy disallows prompt … access to kTCCServiceMicrophone denied`，录音拿到的是精确零流；系统设置里手动开开关、`tccutil reset Microphone` 都救不回来。

先看系统日志再动手：

```bash
log show --last 5m --predicate 'subsystem == "com.apple.TCC"' | grep -i voicenotes | grep -i microphone
```

看到 `entitlement … is missing` 就是这个（补 entitlement 重新打包）；看到 `AUTHREQ_PROMPTING` + `authValue=2` 才是真授权成功。

dev 模式的 `target/debug/voice-notes` 是 linker-signed（无 runtime 标志），不需要这条 entitlement —— 所以「开发时好好的、打包就没声」基本就是它，别去怀疑录音代码。

### 2. ad-hoc 重新打包 → 授权记录失效

ad-hoc 签名的 designated requirement **就是代码签名哈希本身**（`codesign -d -r- VoiceNotes.app` 打出 `# designated => cdhash H"…"`），macOS 的 TCC 拿它匹配授权记录 —— 重新打包 → cdhash 变 → 记录对不上 → **静默拒绝**。

- 表现：能开始录音、有时长统计，但识别不出东西；**系统设置 → 麦克风里 VoiceNotes 的开关还是「开」**（那条绑的是上一个构建）。开关开着 ≠ 当前构建有权限。
- 恢复：终端跑 `tccutil reset Microphone com.github.bytes-astra.voicenotes`，再启动应用重新授权。
- 让授权跨构建存活：给应用一个**稳定的签名身份**（自签名证书即可：`openssl req -x509 ... -addext "extendedKeyUsage=codeSigning"` → PKCS#12 导入钥匙串 → `security set-key-partition-list`；或 Apple Development 证书），`bundle.macOS.signingIdentity` 填它 —— designated requirement 从 `cdhash …` 变成 `certificate leaf = H"…"`，跨构建不变。换签名后第一次仍要允许一次，之后那份授权跟着证书走、跨构建有效。

补上 `audio-input` entitlement 后，`tccutil reset` + 重启就会正常弹框（系统日志 `AUTHREQ_PROMPTING` → `AUTHREQ_RESULT authValue=2`）。

### 3. 打包没做签名

`signingIdentity` 留空时，打包版不会弹标准授权框，麦克风权限可能要在系统设置里手动勾选；带上 `audio-input` entitlement 的包才会弹。签名该怎么做见 [`RELEASE.md`](RELEASE.md)「打包与发布」。

## 静默丢音频

**症状**：整段模式录了十几分钟，日志里只有零星几句；中间某几分钟**一句都没有**，也没有任何告警 —— 转写结果里那段时间是空白，用户以为是自己没说话。

**机制（2026-10-28 定案）**：**电平太低**，不是「没人声」。sherpa 的 Silero VAD 触发条件是语音概率**连续**高于阈值满 `min_speech_duration`（`sherpa-onnx/csrc/silero-vad-model.cc` 的 `IsSpeech()`）—— 概率一掉回阈值以下，攒的进度立刻清零。低电平录音的概率恰好在阈值附近抖，于是永远触发不了；而 `voice-activity-detector.cc` 的非语音分支只 `buffer_.Pop(...)`，那段音频**静默消失**：日志、转写、界面上零痕迹（不是「采集缓冲溢出」那种 —— 那种有 `[[gap:N]]` 标记和「识别跟不上」的 WARN）。

**解药**：`audio::Agc`（`src-tauri/src/audio.rs`）—— 目标 RMS 0.02、增益上限 32×、软限幅 ±0.99。**电平够就一个样本不动**，只救偏小的。已接进三条路径：麦克风整段（`record.rs`）、麦克风流式（`record.rs`，在门控 VAD **之前**）、文件转写（`decode.rs`）。

**实测**（`mic_probe` 抓的真实房间声，同一段音频人为衰减；生产 `audio::Agc`）：

| 输入电平 | 不处理 | 经过 AGC |
| --- | --- | --- |
| 正常（峰值 0.637） | 724 字 | 722 字 |
| -25 dB | 465 字 | **723 字** |
| -30 dB | 94 字 | **718 字** |
| -35 dB | **0 字** | **717 字** |

**两条防线**（`src-tauri/src/asr.rs`）：

- `no_segment_warning()`：**有能量、VAD 也没在攒这一句**、且超过 10 秒没切出句子 → 记一条 WARN。判别条件里的「VAD 正在攒」（`detector.detected()`）是关键 —— 连续说话时 VAD 要等到句尾才切段，那段时间没有句子是正常的，别误报。
- `recognize_and_emit()`：识别结果为空时记一条 WARN（原来直接 `return`，音频白丢）。

**排查手法**（别凭感觉，先跑探针）：探针命令、质量门禁的阈值与基线见 [`HOW_TO_VERIFY.md`](HOW_TO_VERIFY.md) —— 改模型 / 换参数 / 加热词之后先跑 `quality_gate`。

`quiet_pipeline_probe` 走**生产的 `record::run_local_offline` 管线**，对照「不经 AGC」与「经 AGC」两条路：干净人声衰减到 ×0.001 时，不经 AGC 是 **0 字**，经管线是 **113 字**。它守住的是「AGC 还接在 `record.rs` 里」—— 单测只测 `Agc` 自己，有人把它从管线里摘掉，单测照样绿。

`mic_probe` 能抓真实麦克风（和 `audio.rs` 同一套设备协商 + 同一个 `LinearResampler`）落成 WAV，再对比「原样 / 去直流 / AGC」三条路径的 VAD 语音比例与字数 —— 它是唯一能拿用户真实音频说话的探针。`VOICE_NOTES_MIC_FILE` 可以喂已存的 WAV 反复迭代。

**教训**：**量电平的影响，采样点要够低**。`gain_probe` 用的是孤立人声，衰减到 **-56.5 dBFS（×0.01）**仍认出 99.2%，看起来「电平无关」—— **那时的电平还没低到阈值以下**：干净人声要衰减到 **×0.001（RMS 0.000149）**才会崩（0 字），真实房间声落在崩溃区（RMS 0.0007）正好对上线上症状。`quiet_pipeline_probe` 用 ×0.001 就是为了**确定性地**复现这件事。⇒ 探针扫电平时要扫到**明显失效**为止，否则「没测出问题」会被误读成「不存在这个问题」。

⇒ 排查这类问题先看两样东西：**保存下来的转写原文**（内容是不是环境里的声音）和**真实音频的电平**（`mic_probe` 抓一段，峰值 / RMS 是多少）。
