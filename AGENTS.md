# VoiceNotes

Tauri 2 桌面语音转文字应用，本地跑 SenseVoiceSmall INT8（sherpa-onnx）。

## 运行 / 构建

```bash
npm install
npm run dev            # 开发模式
npm run build          # 打包 → src-tauri/target/release/bundle/
```

- 前端是**原生 JS**：`src/index.html` + `app.js` + `style.css`，没有打包器、没有框架、没有 TypeScript。不要为 UI 引入构建步骤。
- 前端通过 `window.__TAURI__.*` 调插件（`withGlobalTauri: true`），调用面在 `src/app.js` 顶部集中解构。
- 权限声明在 `src-tauri/capabilities/default.json`；新增插件命令时要同步加权限，否则运行时直接拒绝。
- 麦克风用途说明在 `src-tauri/Info.plist`（`NSMicrophoneUsageDescription`），通过 `bundle.macOS.infoPlist` 合并进 .app。

## 后端结构

```
lib.rs       Tauri 命令、AppState、保存文件、下载编排
asr.rs       SenseVoice 识别 + Silero VAD 分句 + 流式 Zipformer（StreamingSession）
audio.rs     cpal 采集、电平事件、16 kHz 重采样
models.rs    模型目录解析 / 下载（HF → hf-mirror）/ sha256 校验
settings.rs  设置 JSON 持久化
wav.rs       PCM16 WAV 解析（前端载荷）
```

- **命令参数名用 camelCase**（Tauri 2 默认），例如 `audioBase64`、`fileName`。
- 推理是同步阻塞的，一律放进 `tauri::async_runtime::spawn_blocking`，不要阻塞 async 命令的执行器。
- 引擎按 `(language, itn)` 缓存；语言或 ITN 变化时重建。想加“切换语言不重载”，需要走 C API 的 `setConfig`，Rust 绑定没暴露。
- 事件：`audio:level`、`audio:error`、`model:progress`、`engine:state`、`stream:partial`、`stream:segment`、`streaming:state`。改事件名要同步 `src/app.js` 的 `listenEvents()`。
- **两种模式**：`settings.mode = offline | streaming`。非流式走 SenseVoice（VAD 分句）；流式走 Zipformer transducer，`start_recording` 先 `ensure_streaming_engine` 再起采集，`run_streaming_core` 每 60ms 从录音缓冲按游标取新采样、重采样后喂给 `StreamingSession`。
- **流式必须按 200ms（3200 样本）分片、且只在 `is_ready()` 为真时 `decode()`**，否则 sherpa-onnx 特征缓冲会越界直接 abort（`features.cc GetFrames`）。这是踩过的坑，别改成小分片或无条件 decode。
- 流式模型是独立的一组 assets（`STREAM_*`），固定中英双语 Zipformer；语言选粤/日/韩时前端会自动切回非流式。
- 模型固定版本（HF commit + sha256 在 `models.rs` 的 `MODEL/TOKENS/VAD` 常量里）。换模型必须同时改 bytes/sha256，否则下载校验会失败。
- 录音缓冲上限 600 秒（`audio.rs` 的 `MAX_SECONDS`），前端 `MAX_SECONDS` 也要一起改。

## 测试

```bash
cd src-tauri
cargo test --lib                      # 纯逻辑 + 麦克风枚举
cargo test --test asr_smoke -- --nocapture   # 非流式真模型转写（没模型会跳过）
cargo test --test streaming_smoke -- --nocapture  # 流式真模型转写（VOICE_NOTES_STREAMING_MODEL_DIR）
```

`asr_smoke` 需要 `VOICE_NOTES_MODEL_DIR` 指向包含 `model.int8.onnx` / `tokens.txt` 的目录，未设置时自动跳过。

## 已知坑

- 首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 拉预编译静态库。GitHub 不通时用 `SHERPA_ONNX_ARCHIVE_DIR=/path/to/dir`（目录里放 `sherpa-onnx-v1.13.8-<os>-<arch>-static-lib.tar.bz2`）。
- macOS 未签名构建的麦克风权限可能需要在「系统设置 → 隐私与安全性 → 麦克风」手动勾选；已签名的正式包才会弹标准授权框。
- 拖动窗口靠 `data-tauri-drag-region`；macOS 顶栏给红绿灯留了 84px（`body.macos`），改布局别把左侧内容压到窗口按钮下面。
- 端到端冒烟结果：非流式 5.6s 中文 ≈ 200ms（RTF 0.04）；流式同段音频首字约 1 个分片、整段 0.5s 内出完，无标点。
