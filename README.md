# VoiceNotes · 本地语音转文字

用 **SenseVoiceSmall (INT8)** 在本机把语音转成文字，另有**流式模式**（中文 Zipformer，边说边出）。Tauri 2 桌面应用，macOS / Windows / Linux 通用，默认不联网（只有首次下载模型时需要网络）。

参考了 DSH 的语音输入实现（`client-ui-voice-input` + `speech-to-text-sensevoice`）：同样用 Silero VAD 切句 + sherpa-onnx 跑 SenseVoice，同样在前端重采样成 16 kHz 单声道 PCM16。

界面是「左侧导航 + 右侧主区」：转写、历史、设置各一页。

![主界面](docs/screenshots/ready.png)

| 录音 | 历史 | 设置 |
| --- | --- | --- |
| ![录音](docs/screenshots/recording.png) | ![历史](docs/screenshots/history.png) | ![设置](docs/screenshots/settings.png) |

首次运行（下载模型 / 复用已有模型，截图见 `docs/screenshots/first-run.png`）：

![首次运行](docs/screenshots/first-run.png)

流式模式（边说边出，中间结果在计时旁边滚动）与首次切换时的下载提示：

| 流式 | 流式模型未下载 |
| --- | --- |
| ![流式](docs/screenshots/streaming.png) | ![流式下载](docs/screenshots/stream-setup.png) |

## 功能

- **点一下开始、再点一下结束**，Esc 取消，空格键快捷开关；录音时有实时电平波形与计时。
- **VAD 自动分句**：长录音按语音段切开逐段识别，静音段被跳过，结果按中英文习惯拼接。
- **中 / 英 / 粤 / 日 / 韩 / 自动** 语言选择，ITN（数字、标点）可开关。
- **文件转写**：把音频/视频拖进窗口，或按 `⌘O` / `Ctrl+O` 选文件；浏览器解码后重采样再送模型（mp3、m4a、wav、webm、mp4… 取决于系统解码器）。
- **历史记录**（本地 localStorage，最多 100 条），一键复制、保存。
- **保存目录可指定**，默认 `文档/语音转文字/`，文件为 UTF-8 的 `.txt`。
- 完全离线：模型在本机运行，音频不出机器。

## 首次运行：下载模型

SenseVoiceSmall INT8 约 **228 MB**，不会打进安装包：

1. 启动后顶部会出现「首次使用：下载语音模型」。
2. 点 **下载模型**：优先 Hugging Face，失败自动切换 `hf-mirror.com`；支持断点续传（`.part`），下载完做 SHA-256 校验。
3. **流式模型**单独下载（约 161 MB，仅在切到「流式」模式时需要）：点模式切换里的「流式」，按提示下载；流式模型只覆盖中文，选粤语/日语/韩语会自动切回非流式。
4. **标点模型**（可选，约 281 MB）：设置 → 流式标点 → 下载，下完自动开启，给流式结果按句补标点；不装也不影响识别。
5. 也可以点 **使用已有模型目录**，指向一个包含以下文件的目录（设置里随时可以点「使用应用内模型」切回来）：

```
model.int8.onnx   # 或 model.onnx
tokens.txt
silero_vad.onnx   # 可选：没有也能转写，只是少了 VAD 分句
```

模型来源（固定 commit）：[sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17](https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17) · [silero_vad](https://huggingface.co/csukuangfj/vad)。

## 已经打好的包

`dist/VoiceNotes.app` 是 macOS (arm64) 的 ad-hoc 签名构建，双击即可运行（未公证，首次打开若被拦，右键 → 打开）。模型不入包，首次运行按上面的流程下载。

## 开发

需要 Rust 1.78+、Node 18+，macOS 需要 Xcode Command Line Tools。

```bash
npm install          # 只装 @tauri-apps/cli
npm run dev          # 开发模式
npm run build        # 打包（macOS: .app/.dmg；Windows: .msi/.exe）
```

首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 下载对应平台的**预编译静态库**（onnxruntime 等），之后走缓存。

## 准确率取舍

同一段音频，两个模型的实测对比（5 条中文样本，字符级准确率）：

| | 结果 |
| --- | --- |
| 流式（Zipformer + 收尾补静音） | **99%** |
| 非流式（SenseVoice + ITN） | 91%（“明天”听成“今天”） |

所以最终文本默认用**流式**的结果；只有在流式明显异常（空结果、重复字、字数跟音频时长对不上）时，才自动用 SenseVoice 复核一次（设置里可关）。流式收尾会补 300ms 静音把最后一个字冲出来，否则句尾会被丢掉。

标点：非流式用的 SenseVoice 开 ITN 时**自己就带标点**；流式模型不带标点，用可选的标点模型（CT-Transformer）补。两者不会叠加。

## Windows 打包

Tauri **不能在 macOS 上交叉打 Windows 安装包**（需要 Windows 的 MSVC 工具链、NSIS/WiX 和 WebView2）。两条可行路径：

1. **Windows 机器**：装 Rust、Node 18+、Visual Studio Build Tools（C++ 工作负载），然后
   ```bash
   npm install
   npm run build
   ```
   产物在 `src-tauri/target/release/bundle/msi/*.msi` 与 `nsis/*.exe`。
2. **GitHub Actions**：仓库已带 `.github/workflows/build.yml`，推送 tag（`v*`）或在 Actions 里手动运行，会同时产出 macOS（dmg）和 Windows（msi/exe），在 Artifacts 里下载。

Windows 包使用 `downloadBootstrapper` 安装 WebView2；应用内下载的模型与 macOS 一致，落在 `%APPDATA%\com.hugh.voicenotes\models`。

### 平台说明

- **macOS**：麦克风权限写在 `src-tauri/Info.plist`（`NSMicrophoneUsageDescription`），首次录音时系统会弹授权框；未签名的本地构建在「系统设置 → 隐私与安全性 → 麦克风」里需要手动勾选。
- **Windows**：使用 WebView2（安装包会引导下载）；`cpal` 走 WASAPI，无需额外权限。
- **Linux**：需要 `libasound2-dev`、`libwebkit2gtk-4.1-dev` 等 Tauri 依赖（`cpal` 走 ALSA）。

## 结构

```
src/                     前端（原生 JS，无构建）
  index.html             应用外壳（侧边栏 + 三个视图）
  app.js                 视图切换、录音/转写/历史/设置
  style.css              深浅色主题样式
src-tauri/
  src/lib.rs             Tauri 命令、状态、保存
  src/asr.rs             SenseVoice 推理 + Silero VAD 分句 + 流式 Zipformer（带 VAD 门控）
  src/audio.rs           cpal 采集、电平、重采样
  src/models.rs          模型定位/下载/校验
  src/settings.rs        设置持久化
  src/wav.rs             PCM16 WAV 解析
scripts/make-icon.py     生成图标源图
```

## 测试

```bash
cd src-tauri
cargo test --lib                  # 逻辑、WAV 解析、麦克风枚举、重采样
cargo test --test asr_smoke       # 真模型转写 tests/fixtures/zh.wav（缺模型自动跳过）
cargo test --test streaming_smoke # 流式识别，100ms 切片模拟麦克风
VOICE_NOTES_PUNCT_MODEL=/path/to/punct/model.onnx cargo test --lib punctuat  # 标点

# 需要真实网络的用例（默认跳过）
VOICE_NOTES_DOWNLOAD_TEST=1 cargo test --lib downloads_vad -- --nocapture
VOICE_NOTES_RECORD_TEST=1 cargo test --lib records_microphone -- --nocapture
```

`asr_smoke` 需要 `VOICE_NOTES_MODEL_DIR`，`streaming_smoke` 需要 `VOICE_NOTES_STREAMING_MODEL_DIR`（流式四个文件：`encoder.int8.onnx`/`decoder.int8.onnx`/`joiner.int8.onnx`/`tokens.txt`），未设置时自动跳过。
实测（Apple Silicon）：5.6 秒中文音频转写耗时约 200 ms，实时率约 0.04×。

## 已知限制

- 非流式模式是**准实时**：录音结束后统一识别（SenseVoice 是非流式模型）。要边说边出字请切「流式」。
- 流式模式只支持**中文**，准确率低于 SenseVoice，适合快速记录；定稿建议切回非流式重录或用文件转写。标点是可选模型（见上），且只加在定稿句上，实时中间结果不带标点。
- 单次录音上限 10 分钟；非流式单句（VAD 切出来的一段）上限 30 秒。
- 首次转写需要把 239 MB 模型读进内存，约 1–2 秒；之后常驻复用。
- 说话人分离、时间戳导出未做。
- 打包 `.dmg` 需要能调用 `hdiutil`；在受限沙箱里 `npm run build` 可能只产出 `.app`（`--bundles app` 可跳过 dmg）。
