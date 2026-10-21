# VoiceNotes

Tauri 2 桌面语音转文字应用，本地跑 SenseVoiceSmall INT8（sherpa-onnx）。

## 运行 / 构建

```bash
npm install
npm run dev            # 开发模式
npm run build          # 打包 → src-tauri/target/release/bundle/
```

- 前端是**原生 JS**：`src/index.html` + `app.js` + `style.css` + `i18n.js`，没有打包器、没有框架、没有 TypeScript。不要为 UI 引入构建步骤。
- **下拉框用自绘组件 `src/select.js`**（`UISelect.create(el, {onChange})`）：原生 `<select>` 在 macOS/Windows 样式不可控。弹层挂在 body 上用 fixed 定位，避免被滚动容器裁掉。
- **所有界面文案走 `src/i18n.js` 的字典**：JS 里用 `t('key', {变量})`，HTML 里给元素加 `data-i18n="key"`（还有 `data-i18n-title/-placeholder/-aria`），启动时 `I18N.apply()` 填充。加语言 = 加一个字典，不要在代码里写死中文。后端也别返回中文展示文案（模型来源返回 `app_data` / `custom` 这样的 token）。
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
- **两种模式**：`settings.mode = offline | streaming`（界面上叫「整段 / 流式」，两个都是语音模型，区别在识别方式）。非流式走 SenseVoice（VAD 分句）；流式走 Zipformer transducer，`start_recording` 先 `ensure_streaming_engine` 再起采集，`run_streaming_core` 每 60ms 从录音缓冲按游标取新采样、重采样后喂给 `StreamingSession`。
- **流式必须按 200ms（3200 样本）分片、且只在 `is_ready()` 为真时 `decode()`**，否则 sherpa-onnx 特征缓冲会越界直接 abort（`features.cc GetFrames`）。这是踩过的坑，别改成小分片或无条件 decode。
- 流式模型是独立的一组 assets（`STREAM_*`）：`streaming-zipformer-zh-int8-2025-06-30` + `silero_vad.onnx`，只覆盖中文；语言选粤/日/韩时前端会自动切回非流式。
- 标点模型（`PUNCT`，CT-Transformer zh-en，281 MB）独立放在 `models/punct/`，默认关闭；开启后 `StreamEmitter` 在每句定稿时调用 `PunctuationEngine::apply`，实时中间结果（partial）不加标点。模型缺失/加载失败时静默降级，不阻断录音。
- **流式收尾必须补 300ms 静音**（`asr.rs` 的 `TAIL_SILENCE_SAMPLES`）：解码器需要尾部上下文才会吐出最后一个字，不补就会丢句尾（“产品评审会”→“产品评审”）。补的静音不计入音频时长。
- **不要用 SenseVoice 无脑覆盖流式结果**：实测（5 条中文样本）修收尾后流式 99%、SenseVoice 91%，SenseVoice 会把“明天”听成“今天”。校正只在流式结果明显异常（空/重复字/字数与时长对不上）时才接管，见 `hybrid::is_degenerate`。
- **SenseVoice 开 ITN 时自己就输出标点**（关掉 ITN 才是纯文本）。所以标点模型只能补在**没有标点**的句子上，否则会叠成「。。」——`hybrid::punctuate` 会跳过已带标点的句子。
- 流式链路带 **VAD 门控**：Silero 判到静音就 `force_end()` 提交并 reset，静音段不喂模型（否则 transducer 会在噪声上编出重复字），起音前保留 0.4s 预录避免切字头。
- 模型固定版本（HF commit + sha256 在 `models.rs` 的 `MODEL/TOKENS/VAD` 常量里）。换模型必须同时改 bytes/sha256，否则下载校验会失败。
- **输入设备可选**：`list_input_devices` 命令列出 cpal 输入设备（默认设备带标记），`set_input_device` 存进 settings（None = 系统默认），`audio::start` 按名字挑设备；挑不到就退回默认。
- 录音缓冲上限 600 秒（`audio.rs` 的 `MAX_SECONDS`），前端 `MAX_SECONDS` 也要一起改。

## 测试

```bash
cd src-tauri
cargo test --lib                      # 纯逻辑 + 麦克风枚举
cargo test --test asr_smoke -- --nocapture   # 非流式真模型转写（没模型会跳过）
cargo test --test streaming_smoke -- --nocapture  # 流式真模型转写（VOICE_NOTES_STREAMING_MODEL_DIR）
```

`asr_smoke` 需要 `VOICE_NOTES_MODEL_DIR` 指向包含 `model.int8.onnx` / `tokens.txt` 的目录，未设置时自动跳过。

## Windows

- Tauri 不能在 macOS 上交叉打 Windows 包，CI 用 `.github/workflows/build.yml`（windows-latest）出 msi/nsis。
- 代码侧要点：cpal 采样格式已覆盖 F32/I16/U16/I32；Windows 用原生标题栏，`body.macos` 不生效，侧栏顶部占位是 12px；`bundle.windows.webviewInstallMode = downloadBootstrapper`。
- 菜单只在 macOS 构建（`#[cfg(target_os = "macos")]`）；Windows 无自定义菜单。

## 已知坑

- 首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 拉预编译静态库。GitHub 不通时用 `SHERPA_ONNX_ARCHIVE_DIR=/path/to/dir`（目录里放 `sherpa-onnx-v1.13.8-<os>-<arch>-static-lib.tar.bz2`）。
- macOS 未签名构建的麦克风权限可能需要在「系统设置 → 隐私与安全性 → 麦克风」手动勾选；已签名的正式包才会弹标准授权框。
- 外壳是「侧边栏 + 主区」：`.app[data-view]` 控制 `[data-view-panel]` 显隐，`switchView()` 统一处理导航高亮/标题；录音中不允许切走。新增页面 = 一个 `[data-view-panel]` + 一个 `.nav-item`。
- 拖动窗口靠 `data-tauri-drag-region="deep"`（`.sidebar-top` 与 `.main-head`，按钮自动排除）+ **`core:window:allow-start-dragging` 权限**——这个权限不在 `core:default` 里，删了窗口就拖不动。macOS 侧边栏顶部用 `body.macos .sidebar-top { height: 78px }` 给红绿灯留位，别把品牌压到窗口按钮下面。
- macOS 全屏时红绿灯会自动隐藏、鼠标移到顶部才滑出；侧栏顶部占位**始终**保留 78px，不随全屏变化（否则内容会在进出全屏时跳动）。
- 端到端冒烟结果：非流式 5.6s 中文 ≈ 200ms（RTF 0.04）；流式同段音频首字约 1 个分片、整段 0.5s 内出完，无标点。
