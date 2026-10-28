# Review Findings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复审查中确认的问题，保持识别模型参数与现有架构不变。

**Architecture:** 后端沿用薄命令层、纯录音编排和 ASR 会话。流式真实时间轴归入 `StreamingSession`，门控 VAD 不保留音频。前端在现有模块中保存最小渲染状态，不新增依赖。

**Tech Stack:** Rust、serde、sherpa-onnx、原生 ES Modules、Playwright WebKit。

**Spec:** `docs/superpowers/specs/2026-10-28-review-fixes-design.md`

## Global Constraints

- Tauri 事件载荷 camelCase；命令参数与前端字段不变。
- 推理与 200ms 分片规则不变，不改模型或 VAD 参数。
- 前端单向依赖，无打包器、框架、TypeScript。
- 每项修复先有可观察失败测试，再实现。
- 不提交构建产物或截图；不执行发布流程。

## Review Focus

- 静音、预录、尾部和缺口不能让时间轴重复推进；真实模型冒烟覆盖。
- 过大 `.part` 与正常断点续传；本地 HTTP 测试覆盖 Range。
- 新 toast 与旧隐藏定时器交错；浏览器 fake timers 覆盖。
- 编辑文本、加载历史、切换语言的边界；浏览器覆盖。
- 流式尾部静音不计时长；真模型测试覆盖。

---

### Task 1: 事件契约与下载自愈

**Files:** `src-tauri/src/audio.rs`, `src-tauri/src/models.rs`, `scripts/ui/check.mjs`, `scripts/ui/shots.mjs`

**Interfaces:** serde 输出 `sessionId`、`isDefault`；`fetch_one` 对外签名不变。

- [ ] 写 serde 字段断言和本地 HTTP  oversized `.part` 下载测试；运行确认失败。
- [ ] 为两个结构体添加 camelCase，修正 mock；下载前清理 oversize，大小校验失败清理 `.part`。
- [ ] 运行 `cargo test --manifest-path src-tauri/Cargo.toml --lib serializes_camel_case`、下载测试及 UI 契约测试。
- [ ] 提交后端契约与下载修复。

### Task 2: 流式时间轴与 VAD 队列

**Files:** `src-tauri/src/asr.rs`, `src-tauri/src/record.rs`, `src-tauri/src/lib.rs`

**Interfaces:** `StreamingSession::advance_time(&[f32])`、`elapsed_seconds() -> f32`；其他公开接口不变。

- [ ] 写真实模型的静音时间轴/缺口顺序测试，以及 VAD 队列清理测试；运行确认旧实现失败。
- [ ] 会话增加真实输入计数，录音中每块只 `advance_time` 一次，commit/时长使用真实计数；每次 VAD 判断后 `clear()`。
- [ ] 流式停止命令添加与整段相同的过短和空文本检查。
- [ ] 运行 Rust 全套测试与真模型流式冒烟/长录音探针。
- [ ] 提交流式修复。

### Task 3: 前端状态与文本渲染

**Files:** `src/core.js`, `src/result.js`, `src/app.js`, `src/history.js`, `src/files.js`, `src/recorder.js`, `scripts/ui/check.mjs`

**Interfaces:** 保留 `showTranscript` 等现有导出；动态元信息可由 `applyLocale` 重绘。

- [ ] 浏览器添加用户编辑、meta、toast、历史无 id、文件过期复位/进度、Esc 测试，确认旧实现失败。
- [ ] 保留编辑，在本地化前拼接，重绘动态 meta，清理 toast 内层定时器，finally 无条件复位，逐条删除无 id 历史，输入区 Esc 不导航。
- [ ] 运行 `npm run check:ui`，修复回归。
- [ ] 提交前端状态修复。

### Task 4: 设备与下载进度

**Files:** `src/status.js`, `src/select.js`, `src/events.js`, `scripts/ui/check.mjs`

**Interfaces:** `loadDevices()` 缺失已保存设备时选择 null；不改变下拉公开 API。

- [ ] 添加设备缺失/空缓存/下载空来源浏览器测试，确认失败。
- [ ] 同步缺失设备到系统默认，提供标签回退，空来源不显示缓存。
- [ ] 运行完整 UI 检查。
- [ ] 提交设备与进度修复。

### Task 5: 全量验证与审查

- [ ] 检查 diff、运行 `npm run check` 和 `npm run check:ui`。
- [ ] 使用本机模型运行流式测试和长录音探针，确认没有测试被误跳过。
- [ ] 独立审查全部改动，重要问题按 TDD 修复。
- [ ] 汇总变更、验证和剩余限制；不合并、不推送。
