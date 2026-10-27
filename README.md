## VoiceNotes · 本地语音转文字

![主界面](docs/screenshots/ready.png)

[![Release](https://img.shields.io/github/v/release/bytes-astra/voice-notes.svg)](https://github.com/bytes-astra/voice-notes/releases/latest)

在 Mac / Windows 上把语音转成文字，**音频不出本机**。默认用 **SenseVoiceSmall INT8**（约 228 MB）离线识别，另有**流式模式**（Zipformer，边说边出字）。

Tauri 2 桌面应用，CPU 本地推理，不需要独立显卡；除了下载模型那一次，全程不联网。

### 核心特色

- **点麦克风开始、再点一次结束**；空格键快捷开关，Esc 取消；录音时有实时电平波形和计时。
- **两种识别方式，都是边录边出字**：
  - **整段**：SenseVoiceSmall，支持中 / 英 / 粤 / 日 / 韩 / 自动检测；按停顿分句后逐句识别。
  - **流式**：Zipformer，仅中文；实时出字，中间结果单独一行。
- **流式术语表（热词）**：一行一句填常用说法、人名、地名（最多 100 条），识别时按热词加权，专有名词不容易再听错。
- **标点可选**：流式结果按句自动补标点（CT-Transformer，随流式模型一起下载）。
- **录音时长不限**：用过的音频识别完就释放，录 1 小时和录 1 分钟占的内存一样。
- **可选输入设备**：多麦、外接声卡、回环设备都能选。
- **文件转写**：把音频 / 视频拖进窗口，或按 `⌘O` / `Ctrl+O` 选文件；后端解码（symphonia，纯 Rust，不需要 ffmpeg），支持 mp3 / wav / flac / ogg / m4a / mp4 / mov（视频轨忽略），长文件边解边转。
- **界面语言**：中文 / English，设置里可切。
- **历史记录**：只存本地（最多 100 条），一键复制、保存。
- **保存目录可指定**，默认 `~/Documents/VoiceNotes`（目录名用 ASCII，跨平台工具不用转义；旧版的中文目录「语音转文字」会在启动时自动改名，只在新目录不存在时才搬、不覆盖已有数据），文件是 UTF-8 的 `.txt`。

> 本项目为个人自用，欢迎提 Issue / PR 交流。反馈问题时附上日志文件（位置见「日志」一节），能省掉很多来回。

| 录音中 | 历史记录 | 设置 |
| --- | --- | --- |
| ![录音](docs/screenshots/recording.png) | ![历史](docs/screenshots/history.png) | ![设置](docs/screenshots/settings.png) |

流式模式与模型下载提示：

| 流式 | 流式模型未下载 | 模型未下载 |
| --- | --- | --- |
| ![流式](docs/screenshots/streaming.png) | ![流式下载](docs/screenshots/stream-setup.png) | ![下载模型](docs/screenshots/first-run.png) |

深色主题与英文界面：

| 深色 | English |
| --- | --- |
| ![深色](docs/screenshots/dark.png) | ![英文](docs/screenshots/english.png) |

### 安装

**macOS**

CI 产物（tag `v1.0.0` 对应的 Actions Artifacts）里是 `dmg`，装完把应用拖进「应用程序」即可。

> [!IMPORTANT]
> 包是 **ad-hoc 签名、未公证** —— 首次打开会提示「无法验证开发者」，选「仍要打开」或右键 → 打开。自己构建的 `dist/VoiceNotes.app` 同理。

```bash
sudo xattr -dr com.apple.quarantine /Applications/VoiceNotes.app
```

**Windows**

CI 产物是 `msi` 和 `exe`（NSIS），安装时按提示装 WebView2。模型落在 `%APPDATA%\com.github.bytes-astra.voicenotes\models`。

Tauri 不能在 macOS 上交叉打 Windows 包，要自己出包得在 Windows 机器上装 Rust、Node 18+ 和 Visual Studio Build Tools（C++ 工作负载），然后 `npm run build`；也可以推 tag 让 CI 出。

**Linux**

Tauri 本身支持，但 CI 不出 Linux 包，需要自己构建（依赖 `libasound2-dev`、`libwebkit2gtk-4.1-dev` 等，`cpal` 走 ALSA）。

### 首次运行：下载模型

> [!NOTE]
> 模型不打进安装包，第一次运行要下载。

| 模型 | 大小 | 什么时候需要 |
| --- | --- | --- |
| SenseVoiceSmall INT8 | 约 228 MB | 必装，整段模式用 |
| Silero VAD | 约 2.3 MB | 随上面一起 |
| Zipformer 流式模型 | 约 161 MB | 想用流式模式时 |
| CT-Transformer 标点模型 | 约 281 MB | 随流式模型一起下载 |

1. 启动后顶部出现「下载模型」卡片（模型缺失时都会出现，不只是首次）。
2. 点「下载模型」：**优先走 `hf-mirror.com` 镜像**（国内直连 Hugging Face 经常连不上），镜像失败自动回退官方源；支持断点续传（`.part`），下完做 SHA-256 校验。
3. 想用流式：点模式切换里的「流式」，按提示下载流式 + 标点模型（共约 440 MB）。流式只覆盖中文，选粤语 / 日语 / 韩语会自动切回整段。
4. 已经有本地模型：**设置 → 模型目录 → 选择目录…**，指向一个包含下面文件的目录（随时点「使用应用内模型」切回来）：

   ```
   model.int8.onnx   # 或 model.onnx
   tokens.txt
   silero_vad.onnx   # 必需：缺少它无法分句，这个目录会被判为「未就绪」
   ```

5. 术语表在 **设置 → 术语表** 里填（一行一句），只对流式模式生效。

模型版本是固定的（HF commit + sha256 校验），不会自动更新 —— 换模型需要换应用版本。

来源：[sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17](https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17) · [silero_vad](https://huggingface.co/csukuangfj/vad) · [streaming-zipformer-zh-int8-2025-06-30](https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30)

### 两种模式怎么选

| 模式 | 模型 | 语言 | 特点 |
| --- | --- | --- | --- |
| 整段 | SenseVoiceSmall INT8 | 中 / 英 / 粤 / 日 / 韩 / 自动 | 按句识别，长句更准；分句处会有停顿延迟 |
| 流式 | Zipformer | 仅中文 | 逐字出字，开口后约 0.2 秒出第一个字；支持术语表 |

两种模式都用同一个语音模型，区别只在识别方式：

- **整段**：一句话说完再出这一句的字，识别得更准，但要先等一个停顿。
- **流式**：边说边出字，适合边讲边看；只支持中文。

> [!TIP]
> 要边讲边看就选流式；要长句准确率就选整段。两种模式不能混着选语言 —— 流式只覆盖中文，选了粤语 / 日语 / 韩语会自动切回整段。

各语料的当前实测字符错误率见 [docs/HOW_TO_VERIFY.md](docs/HOW_TO_VERIFY.md)。

### 开发

需要 Rust 1.78+、Node 18+；macOS 还要 Xcode Command Line Tools。

```bash
npm install          # 只装 devDependencies（@tauri-apps/cli + playwright）
npm run dev          # 开发模式
npm run build        # 打包（macOS: .app/.dmg；Windows: .msi/.exe）
npm run check:i18n   # 改文案后跑：检查各语言 key 对齐、HTML/JS 引用的 key 是否存在
npm run check        # 门禁：i18n → clippy → 单元测试
npm run check:ui     # 界面检查：真实 WebKit + 假 __TAURI__
npm run shots        # 重出 README 截图
```

首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 下载对应平台的**预编译静态库**（onnxruntime 等），之后走缓存。

前端是原生 JS + ES Modules，没有打包器、没有框架、不要为 UI 引入构建步骤；界面文案统一放在 `src/locales/*.js` 的字典里（`src/i18n.js` 只放查字典的机制），改完跑一次 `npm run check:i18n`。

完整工程约定、模块边界、提交与发布规范见 [AGENTS.md](AGENTS.md)；质量探针、语料与当前实测基线见 [docs/HOW_TO_VERIFY.md](docs/HOW_TO_VERIFY.md)。

### 已知限制

- 流式模型只支持中文；粤语 / 日语 / 韩语请用整段模式。
- **术语表只对流式生效**：整段模式加术语表会让识别直接崩溃，所以整段模式没有这个开关；改完术语表要重新加载流式模型（约 161 MB），下次录音才生效。[^hotwords]
- **语言选择只是提示，不是过滤器**：选中「中文」后，中英混说里的英文词仍会保留；选错语言反而更差。
- 整段模式一句一句单独识别，上一句的上下文不会带到下一句。
- 整段模式连续说话时要等停顿才出这一句；特别长的句子（超过 90 秒）才会拆成 30 秒一段再合并回来（相邻两段有重叠，避免把词切开）。
- **开头直接是说话时，整段模式偶尔会丢掉第一个字**：音频开头没有停顿，分句的边界正好切在第一个字上。[^vad]
- **整段模式的结果自带标点**，不需要再开标点模型。标点模型只用于流式模式，两个一起用会出现「。。」这样的重复标点。
- **流式模式停止后会多等一小段静音再输出**，否则句子的最后一个字会丢。这段静音不计入录音时长。
- 说话人分离、时间戳导出没做。
- 打包 `.dmg` 需要能调用 `hdiutil`；受限沙箱里可能只产出 `.app`（`--bundles app` 可跳过 dmg）。

[^hotwords]: SenseVoice 上带 beam search 或热词会直接终止识别进程（已复现），不是配置写错的问题。

[^vad]: 分句模型在语音起点前只有约 0.3 秒余量。应用已经做了带门控的回扩补偿，但没有无条件回扩 —— 无条件回扩会把开头噪声也当成语音。

### 隐私

- 音频只在本机处理，不上传；只有下载模型那一次需要联网。
- 历史记录只存在本地，不做任何同步。

### 日志

应用一直写日志，**不需要配置，也没有开关**：

| 系统 | 位置 |
|---|---|
| macOS | `~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log` |
| Windows | `%LOCALAPPDATA%\com.github.bytes-astra.voicenotes\logs\voice-notes.log` |

- 超过 1 MB 会轮转一次，旧文件留成 `voice-notes.log.1`。
- 里面记的是**技术细节**（错误码、系统错误文本、耗时、丢了几秒音频），界面上的提示只给一句话，原因都在日志里。
- 反馈问题时把这个文件发过来就行 —— 前端的异常和关键操作也在同一个文件里。
- 在 Finder 里按 `⌘⇧G` 输入 `~/Library/Logs` 就能找到；或者在终端 `tail -f ~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`。

### 如何自定义与贡献

**如果你想贡献代码或新增特性：**

请阅读 [AGENTS.md](AGENTS.md)。里面包含工程约定、模块边界、参数不变式、提交与 PR 的要求。

**如果你想改质量验证流程，或想知道当前的实测准确率：**

请阅读 [docs/HOW_TO_VERIFY.md](docs/HOW_TO_VERIFY.md)。里面包含质量探针、语料生成、界面检查与当前基线。

**如果你想了解质量调研的背景：**

见 [docs/research/](docs/research/)，先看 [质量路线图](docs/research/2026-10-23-asr-quality-roadmap.md)。

**如果你是 AI AGENT：**

请阅读 [`./AGENTS.md`](./AGENTS.md)。
