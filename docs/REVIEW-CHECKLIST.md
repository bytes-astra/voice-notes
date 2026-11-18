# 复审清单（长期不变）

> 这份文档是**复审的固定作业面**。每轮复审从「尚未审过」的区域开始，**不要重新发明检查项**。
>
> 上一版每轮换一份新的 Focus 清单，于是每轮都「发现新问题」—— 其实是一批一直存在的
> 缺陷被不同角度翻出来（见下方「为什么要有这份文档」）。清单固定后，「已审过」才能累积，
> 缺陷池才会变成递减曲线。

**当前规模**：**96 条**不变量、11 组（A–K）—— ✅ 94 条有守卫，⚠️ 3 条，❌ 0 条。

> **云端条目在 K 组**（2026-11-17 落地）：provider 契约、中间结果语义、结果框顺序、凭据脱敏、零长句顺序、上传丢包缺口。
2026-11-14 修复 RC-B6 的门控收尾与预录时间轴问题，作业面 1 → 0；真模型守卫在本地运行。

（历史口径：5 份旧 review plan 一共提出 **26** 个 focus 项 —— 那是「轮次」的粒度；
本文档把它们展开成 **88** 条可逐条核对的条目。两个数各指什么，不要混用。）

## 怎么用

1. **每轮开始**：读本文档 → 挑「状态 = ⚠️ 守卫薄弱」或「❌ 未审」的条目审 → 只审这些。
2. **发现缺陷时**：先在本清单里找对应条目 → 补守卫（断言 / 单测）→ 把状态改成 ✅ 并写上守卫位置。
3. **发现清单里没有的不变量**：在末尾「新增条目」区追加一条，**给它一个稳定 ID**，别另起文档。
4. **每轮结束**：在「复审台账」记一行 —— 日期、审了哪些 ID、新发现、结论。
5. **改守卫位置**：守卫行号会漂。引用尽量写「文件 + 断言名 / 测试函数名」，行号只是辅助。

状态图例：

| 标记 | 含义 |
| --- | --- |
| ✅ | 有自动守卫（UI 断言 / Rust 单测 / 门禁脚本），回归会变红 |
| ⚠️ | 审过，但守卫薄弱 —— 只有一侧有断言，或靠人工/真机验证 |
| ❌ | 未审 / 无守卫 |

---

## 为什么要有这份文档

2026-10-21 ~ 11-03 的 238 个提交里，**78 个是 fix（33%）**。它们不是均匀分布的，而是
集中在 6 个复审日（10-22 / 10-23 / 10-24 / 10-28 / 11-01 / 11-02；对应 5 份 review plan）。
每轮复审用一份**不同**的 Focus 清单，于是每轮换一副
眼镜，在同一个缺陷池里捞出一批「新的」。

三个佐证：

- **缺陷是老的**：40 个近期 fix 做 pickaxe 追溯，可定位的 20 个里 **16 个（80%）是被修代码
  至少 1 天前就存在**。`src/history.js` 的载入逻辑 10-22 引入、11-02 才修，活了 11 天。
- **修复会互相踩脚**：10 天内共享文件的 fix 配对 704 对，其中 **62 对（9%）** 是「后一个 fix
  删掉了前一个 fix 新增的行」。最典型：`d907e5b`（11-02 23:39）给 `result.js` 的
  `appendSegment` 加了「用户改过就追加不重画」的分支，`c674d70`（11-02 23:53，**14 分钟后**）
  在同一函数把它推翻 —— 因为那行没考虑 `[[gap:N]]` 占位符。
- **门禁 54% 是「伤疤」不是「规格」**：`scripts/ui/check.mjs` 的 112 条新增断言里，**60 条
  （54%）写在 fix 提交里** —— 缺陷已被复审发现之后才补的。覆盖面积 = 已经出过事的地方，
  两次事故之间的空白地带没人管。

结论：**发现机制是抽样的，缺陷池是固定的**。清单固定 + 守卫补齐 = 池子开始缩小。

---

## 清单主体

按不变量分组。ID 稳定，勿复用。

### A. 静默失败必须显式报错（9 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-A1 | 下载目录建不出来、下载失败、取消下载，三条路径都必须复位「忙碌」标志 | `engine/tests.rs` 下载守卫在提前返回时复位 / 下载标志互不阻塞；`settings/hotword_tests.rs` rejects_an_unusable_output_dir | ✅ |
| RC-A2 | 模型下载中途取消，必须翻译成字典 key（`setup.cancelled`），不露出内部哨兵串 | `models/tests.rs` cancelled_sentinel_is_only_the_download_cancelled_marker / cancelled_sentinel_with_detail_is_still_cancelled / cancelled_download_returns_the_dictionary_key / cancelling_during_download_is_translated_to_the_dictionary_key；UI `下载取消显示「已取消」` / `下载取消不露出内部哨兵` / `下载取消的进度行不说失败` | ✅ 2026-11-03 |
| RC-A3 | 采集流打不开必须报错（`record.micOpenFailed`），不能静默 | `audio/tests.rs` capture_start_reports_play_failure / play_failure_maps_to_the_mic_open_code | ✅ |
| RC-A4 | VAD 建不起来必须报错，不能 panic | `asr/tests.rs` vad_failure_is_reported_instead_of_panicking | ✅ |
| RC-A5 | 流式引擎加载失败必须报错，**不能显示「已就绪」**、徽标不能永远停在「加载中」 | UI `预热失败不弹「已就绪」` / `预热失败给出错误提示` / `整段预热失败不弹「已就绪」` / `流式引擎报错时徽标是红底` / `流式引擎报错时状态点标红`；`engine/tests.rs` 加载结果映射成引擎状态 | ✅ |
| RC-A6 | 停止录音超时：错误串带细节（`record.stopFailed:细节`）时也要走超时文案，不能把 key 甩给界面 | `record/tests.rs` stop_failed_is_recognised_with_and_without_detail；UI `停止超时带细节仍走超时文案`（`record.streamTimeout:通道关闭` → 「流式识别超时，请重试」，不带错误码也不带细节） | ✅ 2026-11-03 |
| RC-A7 | 停止等待失败不得交出半截结果，且要置 discard | `record/tests.rs` wait_failure_returns_no_partial_result | ✅ |
| RC-A8 | 麦克风故障不能阻断 ASR 收尾，也不能把空结果写历史 | `record/tests.rs` capture_failure_does_not_discard_finished_transcript / capture_failure_without_transcript_returns_error；UI `麦克风异常后回到空闲` / `麦克风异常保留部分结果` / `麦克风异常写入历史` | ✅ |
| RC-A9 | 识别结果为空（VAD 丢音频）必须留痕，不能白丢 | `asr/tests.rs` warns_only_when_speech_went_missing | ✅ |

### B. 缺口与时间轴口径（10 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-B1 | 缺口占位符形状前后端一致（`[[gap:<数字>]]`），占位符前后不加空格 | `scripts/check-join.mjs` + `check-join.test.mjs`；`asr/gap_tests.rs` gap_placeholder_joins_without_spaces | ✅ |
| RC-B2 | 缺口标记落在**缺口终点**（丢掉的音频之后才开始识别），两种模式同一条规则 | `record/tests.rs` gap_segment_marks_the_given_moment；`asr/gap_tests.rs` inserts_gap_in_time_order | ✅ |
| RC-B3 | 同一时刻，缺口排在句子**前面**（否则读起来像句子之后才丢的音频） | `asr/gap_tests.rs` gap_precedes_a_segment_at_the_same_moment；`record/tests.rs` streaming_gap_marker_precedes_the_following_sentence | ✅ |
| RC-B4 | 丢掉的音频占真实录音时间（时长不能少算） | `record/tests.rs` dropped_audio_counts_towards_recorded_time（纯函数，只覆盖整段的算术）/ streaming_dropped_audio_advances_recording_clock（**需真模型**） | ✅ 2026-11-14：两侧都有守卫，本地设 `VOICE_NOTES_*_MODEL_DIR` 跑是绿的（CI 不在本项目验收范围内） |
| RC-B5 | 时间轴换算在**秒域**比较，区间语义 `[start, end)`：起点压缺口要移、终点压缺口不移 | `record/tests.rs` offline_timeline_shifts_by_dropped_audio / offline_timeline_boundary_is_half_open / offline_timeline_is_unchanged_without_drops；`asr/timeline_tests.rs` 全 6 条 | ✅ |
| RC-B6 | 实时事件流与最终结果必须同一套时间刻度（不能混两套）；同批多个 endpoint 按各分片位置定时，后续句子不能因共用批末时钟变成零时长 | `asr/stream_input_tests.rs`、`asr/timeline_tests.rs`：同批 endpoint、余量、预录和缺口的无模型断言；`asr/streaming_time_tests.rs`：门控收尾与缺口边界的真模型断言；`tests/streaming_endpoint_time.rs`：单批多 endpoint 和门控后立即停止的生产路径回归；`tests/streaming_smoke.rs`：收尾句尾及新流识别 | ✅ 2026-11-14：门控收尾先消费 pending、补尾静音并按实际输入终点定稿，再创建新的 stream；空结果不推进上一句终点；预录起点不能退过上一句或前置缺口。真实模型生产路径的零时长用例在修复前失败、修复后通过；人为传入早于上一句终点的旧音频仍钳制为零时长，不能伪造音频区间 |
| RC-B7 | 缺口必须同时进最终文本**和** `segments` | `record/tests.rs` streaming_gap_marker_reaches_transcript（需流式真模型）/ offline_gap_marker_reaches_transcript（需 `VOICE_NOTES_MODEL_DIR`） | ✅ 2026-11-14：两条守卫都在，本地设模型目录跑是绿的 |
| RC-B8 | 流式尾部补的静音不计入时长 | `asr/streaming_time_tests.rs` duration_counts_only_advanced_audio（**需真模型**） | ✅ 2026-11-14：本地设模型目录跑是绿的 |
| RC-B9 | 停止时识别期间采到的尾巴必须被消费（否则句尾丢字） | `record/tests.rs` offline_stop_keeps_the_tail | ✅ |
| RC-B10 | 实时事件流不能重复推进时钟 | `asr/streaming_time_tests.rs` externally_advanced_audio_is_not_counted_again | ✅ 2026-11-14：本地设模型目录跑是绿的 |

### C. 两种模式同一条规则（7 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-C1 | 整段 / 流式的拼句规则逐条一致（CJK 区间带端点开闭、占位符完整形状、三条规则的表达式） | `scripts/check-join.mjs` + `check-join.test.mjs`（14 条变异测试） | ✅ |
| RC-C2 | VAD 参数两模式一致（共有参数；`max_speech_duration` 允许不同 —— 整段 30s 切句 / 流式 20s 门控） | `engine.rs` 的 `vad_config()`（两模式唯一出处）+ `engine/tests.rs` 两模式_vad_参数一致 | ✅ 2026-11-03 |
| RC-C3 | 录音过短的提示两模式走同一套守卫，文案带秒数（「至少说 0.2 秒」而不是「至少说 {seconds} 秒」） | `record/tests.rs` too_short_transcript_reports_seconds；UI `录音过短的提示带秒数`（`record.tooShort:0.2` → 「录音太短，至少说 0.2 秒」） | ✅ 2026-11-03 |
| RC-C4 | 徽标三态两模式一致（不能一个红一个灰） | UI `流式引擎报错时徽标是红底` / `流式引擎报错时状态点标红` | ✅ |
| RC-C5 | 短句合并层两模式共用（短于 3 秒不立刻定稿，间隔超 2 秒不合并） | `asr/merger_tests.rs` 全 6 条 | ✅ |
| RC-C6 | 超长句才切（阈值 90 秒），相邻两段重叠合并 | `asr/merge_tests.rs` 全 4 条 | ✅ |
| RC-C7 | 预录回扩只作用于第一段且有门控（不能无条件回扩） | `asr/pre_roll_tests.rs` pre_roll_only_for_speech_like_prefix | ✅ |

### D. i18n 与文案（11 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-D1 | 各语言 key 必须完全对齐（缺一个就显示 key 名） | `scripts/check-i18n.mjs` | ✅ |
| RC-D2 | 同一个 key 在各语言的占位符集合必须一致 | `check-i18n.mjs` 占位符校验 | ✅ |
| RC-D3 | 新增语言但忘记在 `index.html` 注册 → 门禁必须红 | `check-i18n.mjs` locale 注册检查 + `check-i18n.test.mjs` | ✅ |
| RC-D4 | locale 文件必须排在 `i18n.js` 之前（否则 `window.I18N_LOCALES` 读不到） | `check-i18n.mjs` 脚本顺序检查 | ✅ |
| RC-D5 | 经典脚本（`i18n.js`/`select.js`）不得加 `async`（实测界面直接挂）、不得写成 `type="module"`（挪到 `app.js` 之后就挂）、不得排在 `app.js` 之后 | `check-i18n.mjs` 的经典脚本检查 + `check-i18n.test.mjs` | ✅ 2026-11-03 |
| RC-D6 | 错误文案不暴露技术细节（错误码、本地路径、镜像地址、原始异常） | UI `模型加载失败显示字典文案` / `错误提示不泄露错误码和本地路径` / `下载失败不弹原始错误串` / `未知对象错误不泄露细节` / `未知字符串错误不泄露细节` / `Error 对象不泄露原文` + 3 条「细节写入日志」 | ✅ |
| RC-D7 | 界面文案不带内部黑话（线程 / 预热 / 收尾 / 引擎状态），技术细节只进日志 | `scripts/check-i18n.mjs` 的黑话规则（只查文案的值，不查 key）+ `scripts/check-i18n.test.mjs` 两条自测 | ✅ 2026-11-03 |
| RC-D8 | 切语言不覆盖用户编辑过的文本 | UI `切语言不覆盖用户编辑` / `切英文不覆盖用户编辑` / `连续切两次语言不覆盖用户编辑` / `历史载入后切语言保留用户编辑` | ✅ |
| RC-D9 | 缺口占位符跟着界面语言走（历史里也跟） | UI `缺口占位符初始为中文` / `切英文占位符跟着变` / `连续切两次占位符回到中文` / `历史里的缺口占位符初始为中文` / `历史里的缺口占位符跟着语言变` | ✅ |
| RC-D10 | 界面语言 ≠ 识别语言（6 个识别语言芯片 vs 2 个界面 locale）不能混淆 | UI `界面语言下拉只有三个选项` / `界面语言下拉不混进识别语言` / `切界面语言不改识别语言` | ✅ 2026-11-03 |
| RC-D11 | 界面文案 SHALL 用**中性措辞**：不用口语量词（「一个」「这」「就」）与劝导句（「再试试」「记得点」「换个安静点」）—— 一句中性的话 + 用户看得懂的参数 | **人工守卫**（`check-i18n` 的黑话检查只覆盖技术黑话，口语化没有词表可查）：2026-11-18 逐条过了一遍 `zh-CN.js` / `en.js` 并改掉 24 条口语措辞；改动过的文案见当轮台账 | ⚠️ 2026-11-18 |

### E. 用户编辑、焦点与交互（7 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-E1 | 录音中手改的结果框，下一句识别结果不能整段覆盖（要接在编辑之后） | UI `实时追加不覆盖用户编辑` / `实时追加接在用户编辑之后` / `编辑状态下缺口段显示本地化文案` | ✅ |
| RC-E2 | 停止录音后用后端返回的全文覆盖结果框 | UI `停止后结果框是后端返回的全文` | ✅ |
| RC-E3 | 历史存**原文**（带占位符），渲染时才本地化 —— 否则历史里永远停着旧语言 | UI `历史里的缺口占位符初始为中文` / `历史里的缺口占位符跟着语言变` + `ee3479a` | ✅ |
| RC-E4 | 载入历史必须切回转写页（否则文字写进 `display:none` 的面板，用户看不到） | UI `历史载入切回转写页` / `历史载入内容可见` | ✅ |
| RC-E5 | 纯键盘操作下拉框：Esc 关闭弹层不冒泡（不能连带取消录音）、选中后焦点回按钮（否则下一次空格变成「开始录音」） | UI `下拉 Esc 关闭弹层` / `下拉 Esc 不取消录音` / `选中后焦点回到下拉按钮` / `按钮上的空格不抢去录音` | ✅ |
| RC-E6 | 新 toast 不能被旧的隐藏定时器误伤 | UI `旧 toast 隐藏定时器不隐藏新提示` | ✅ |
| RC-E7 | 输入框里按 Esc 不跳视图 | UI `输入框 Esc 不跳转视图` | ✅ |

### F. 权限、设备与模型就绪（12 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-F1 | 无名设备不列出（按名字挑不中的设备列出来没用） | `audio/tests.rs` unnamed_devices_are_not_listed | ✅ |
| RC-F2 | 设备名即身份，不能拿展示文案当身份传后端 | UI `默认设备标记使用真实后端字段` / `失效设备显示系统默认` / `失效设备同步后端设置` | ✅ |
| RC-F3 | 截断的流式模型与 VAD 不得显示就绪 | `models/tests.rs` truncated_app_data_model_is_not_ready / model_without_vad_is_not_usable / streaming_model_requires_complete_assets | ✅ |
| RC-F4 | 过大 `.part` 要清掉重下，正常断点续传用 Range | `models/tests.rs` oversized_partial_file_is_replaced_by_a_fresh_download | ✅ |
| RC-F5 | 两条下载线各自有标志，可以同时跑；取消要指明取消哪一条 | `engine/tests.rs` 下载标志互不阻塞；`command_tests.rs` cancel_download_targets_the_named_line / unknown_download_kind_cancels_nothing | ✅ |
| RC-F6 | 认不出的 `kind` 不能猜着取消（猜错会停掉另一个正在跑的下载） | `command_tests.rs` unknown_download_kind_cancels_nothing | ✅ |
| RC-F7 | 录音启动预留的锁，退出时必须复位 | `engine/tests.rs` 录音启动预留锁配置且退出复位 | ✅ |
| RC-F8 | 取消录音不碰「正在启动」标志（那是 start 命令的 guard 独占） | `command_tests.rs` cancel_does_not_release_the_starting_flag | ✅ |
| RC-F9 | 麦克风权限改成点麦克风时才申请，且界面渲染完再列设备（避免启动就弹权限框打断用户） | UI `启动时不申请麦克风权限`（启动后 `warm_up_mic` 调了 0 次）/ `列设备时界面已经画完`（列设备那一刻界面已有语言芯片） | ✅ 2026-11-03 |
| RC-F10 | 落盘文件名不覆盖已有文件 | `output/tests.rs` writes_unique_files_without_overwriting | ✅ |
| RC-F11 | 术语表按模型词表过滤：表外字整条作废；读不到词表时全部保留 | `engine/tests.rs` 词表只收单字符号 / 词表外的字整条热词作废 / 读不到词表时热词全部保留；UI `术语表提示词表外字有几条`（`Status.hotwordsIgnored` → 「其中 2 个包含模型词表之外的字，不会生效」） | ✅ 2026-11-03 |
| RC-F12 | 录音中和启动中的语言 / ITN 锁定 | `command_tests.rs` recognition_config_is_locked_while_recording；UI `录音中语言与 ITN 锁定` / `启动中语言与 ITN 锁定` / `启动中不发送语言或 ITN 变更` / `停止后语言与 ITN 解锁` | ✅ |

### G. macOS 原生层与打包（5 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-G1 | 侧栏材质、窗口外观、窗口标题跟着主题走 | UI `切主题同步窗口外观` / `切回浅色同步窗口外观` / `切语言同步原生窗口标题`（假 `__TAURI__` 补上 `setTheme` / `setTitle`，此前这两个调用在 harness 里静默 no-op） | ✅ 2026-11-03 |
| RC-G2 | macOS 根背景保持透明（材质才透得出来） | UI `macOS 根背景保持透明` | ✅ |
| RC-G3 | 打包版必须带麦克风 entitlement，签名用 ad-hoc | `scripts/check-bundle.mjs`（ad-hoc 签名 / 麦克风 entitlement / Info.plist 用途说明）+ `scripts/check-bundle.test.mjs`；产物级：`build.yml` 的 macOS job 用 `codesign` 验 `Signature=adhoc` 与 `com.apple.security.device.audio-input` | ✅ 2026-11-03 |
| RC-G4 | `install_menu` 的 import 必须跟 `cfg`，否则 Windows 编译失败 | `scripts/check-bundle.mjs`（`menu.rs` 顶层不许有 `use`、函数必须带 cfg；`lib.rs` 的 `use menu::…` 必须带 cfg）；`check.yml` 的 `rust-windows` job 每个 PR 都 `cargo check --all-targets` | ✅ 2026-11-03 |
| RC-G5 | Windows 上侧栏品牌行不能被切掉 | UI `Windows 下不加 macOS 材质类` / `Windows 下侧栏品牌行不被切掉`（Windows UA 另开一页：顶部容器要装得下品牌行） | ✅ 2026-11-03 |

### H. 事件契约与数据流（8 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-H1 | 事件载荷增量：分段只带新句，前端自己追加（累计全文在长会话里是 O(n²)） | UI `旧会话分段被忽略` / `旧会话中间结果被忽略` / `取消竞态不写历史` / `取消竞态结果仍为空` | ✅ |
| RC-H2 | 取消后迟到的停止结果不能复活已取消的录音 | UI `取消竞态不写历史` / `取消竞态结果仍为空` | ✅ |
| RC-H3 | 致命错误保留非空部分结果并写历史；非致命错误（缺口提示）用普通提示样式 | UI `麦克风异常保留部分结果` / `麦克风异常写入历史` / `缺口提示不是错误样式` / `缺口提示照常显示` / `部分结果警告不被完成提示覆盖` | ✅ |
| RC-H4 | 下载进度按 `kind` 分流，不串面板 | UI `流式进度显示在流式面板` / `流式进度不串到整段面板` / `整段进度显示在整段面板` / `整段进度不串到流式面板` | ✅ |
| RC-H5 | 文件转写：过期任务退出仍复位、预留失败恢复空闲、转写中拒绝载入历史 | UI `过期文件任务退出仍复位` / `文件预留失败恢复空闲` / `文件预留失败后可录音` / `转写中载入历史被拒绝` | ✅ |
| RC-H6 | 前后端命令名与载荷字段必须一致（`isDefault` / `sessionId` / `warning`） | `scripts/check-contract.mjs`（命令名 + 载荷字段 + mock 载荷）+ `check-contract.test.mjs` | ✅ 2026-11-03 |
| RC-H7 | 前端 import 图单向、不能成环；经典脚本不得被 import（实测会执行两遍、`window.I18N` 被换实例）；每个模块都要登记 | `scripts/check-deps.mjs`（四条规则）+ `scripts/check-deps.test.mjs`（9 条） | ✅ 2026-11-03 |
| RC-H8 | 同一时刻多句不丢（说话→静音→再说话） | UI `说话→静音→再说话：三句都在` | ✅ |

### I. 质量与性能（4 条 · 守卫全空白）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-I1 | 识别质量不得退化（整段 CER、实时性 RTF） | `src-tauri/tests/quality_gate.rs`：量**生产路径**（`OfflineSession` / `StreamingSession`），整段 CER ≤ 25%、流式 CER ≤ 55%、RTF ≤ 1.0（环境变量可覆盖）；基线整段 16.39% / 流式 43.28% | ✅ 2026-11-03（**本地跑**：要真模型 + TTS 语料；CI 不在验收范围内，见 RC-I2 的口径） |
| RC-I2 | 流式路径的质量门禁必须**可本地复现**（不依赖 CI） | `src-tauri/tests/quality_gate.rs`（流式 CER ≤ 55%、RTF ≤ 1.0）+ `src-tauri/tests/memory_probe.rs` + `src-tauri/tests/streaming_endpoint_time.rs`；跑法见 `docs/HOW_TO_VERIFY.md` 的「质量门禁」一节 | ✅ 2026-11-14 **改口径**：CI 不在本项目的验收范围内，真模型守卫一律本地跑（实测流式 CER 43.28% / 最差 RTF 0.036，均在阈值内）。2026-11-03 曾定「流式路径进 CI」，2026-11-14 撤下 —— 不变量从「进 CI」改为「可本地复现」，ID 保留 |
| RC-I3 | 长录音不得丢音频（VAD / 整段会话 / 流式会话 / 重采样四条路） | 四个长录音探针现在**带断言**：`vad_long_probe`（吞掉的 ≤ 故意静音 + 每句 0.2 s + 0.5 s）、`offline_long_probe`（未覆盖 ≤ 故意静音 + 1 s）、`streaming_long_probe`（未覆盖 ≤ 1 s）、`resample_probe`（长度比例 ±1%、识别差异 ≤ 5%） | ✅ 2026-11-03 |
| RC-I4 | 内存不随录音时长增长（消费式缓冲） | `src-tauri/tests/memory_probe.rs`：一个会话跑 30 分钟，300 s 与 1800 s 两点的常驻内存（`ps -o rss=`）增长 < 32 MB（这 25 分钟音频本身就有 96 MB）；实测整段 −11.2 MB、流式 +0.0 MB。另有 `audio/tests.rs` take_new_consumes_samples / pending_limit_follows_sample_rate | ✅ 2026-11-03（**不加 libc**：改 `Cargo.lock` 会让 CI 重下 sherpa-onnx 预编译库而失败） |

### J. 并发与原子性（3 条）

> 这三条来自 2026-11-04 的并发核实：Tauri 的 async 命令跑在 tokio 多线程 runtime 上，
> 加上三个显式 OS 线程（麦克风、整段识别、流式识别）和三个跨线程共享的 Mutex
> （`recorder` / `stream` / `offline_session`），并发是真实存在的。
> 三条缺陷的实际可达性都被上一轮**夸大**过（要锁被 poison / 要两个任务乱序），
> 但都值得加固 —— 加固的收益是「这类缺陷以后写不出来」，不是「现在正在烧火」。

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-J1 | 采集器槽与会话槽必须**一起**登记：要么都写成功，要么都不写。只写采集器不写会话 → 识别线程永远收不到 stop（线程泄漏、永久占着 239 MB / 161 MB 引擎）、麦克风不关、语言/ITN 永久锁死，下一次 start 撞「已经在录音」 | `record.rs` 的 `register_slots`（先拿齐两把锁再写，锁序 采集器 → 会话）；`record/tests.rs` 会话槽坏了采集器槽也不能被写 / 采集器槽坏了会话槽也不能被写 / 登记成功两个槽都有 / 已经在录音时登记要报占用 / 会话槽占着时登记要报占用 | ✅ 2026-11-04 |
| RC-J2 | 装引擎必须用**当前**设置：读设置与装引擎要在同一把锁内，陈旧判定必须覆盖**所有**影响识别的设置（整段 = 模型目录 / 语言 / ITN，流式 = 热词）。只比语言和 ITN 的话，换了模型目录照样复用旧引擎，新设置被静默吞掉 | `engine.rs` 的 `install_engine`（拿槽锁 → 锁内读设置 → 比身份 → 复用或重建）+ `OfflineEngineKey` / `StreamingEngineKey`；`engine/tests.rs` 设置没变就复用引擎 / 换了模型目录就重建引擎 / 换了语言就重建引擎 / 换了热词就重建流式引擎 / 等锁期间设置改了也要用新设置 | ✅ 2026-11-04 |
| RC-J3 | async 路径上不许**阻塞式**取引擎锁：引擎加载要好几秒（239 MB / 161 MB / 294 MB），那把锁会被占住；async 命令在执行器线程上死等，整个执行器就卡住（用户看着像界面冻住）。`try_lock()` 不算 | `scripts/check-async.mjs`（沿调用图从 async 函数出发，引擎槽的 `.lock()` 只许出现在 `spawn_blocking` 闭包里）+ 9 条自测（含「把旧的 `shared_engine` 加回真源码会被抓住」）；`load_engine` 直接返回 `Arc<Engine>`，不再有 `shared_engine` 的二次取锁 | ✅ 2026-11-04 |

### K. 云端模式（20 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-K1 | provider 声明的中间结果语义 SHALL 与协议实际形态一致（`Preview` 允许确认前缀为空）。**声明必须有约束力** —— 只钉住源码里的常量、形态漂移照样过，所以形态要从**协议样例本身**推导再与声明比较 | 三家各一条（真实协议样例，禁 mock）：`asr/cloud/volcengine/tests.rs::partial_is_a_whole_sentence_replacement`、`asr/cloud/realtime/protocol_tests.rs::deltas_accumulate_into_the_sentence_so_far`、`asr/cloud/aliyun/tests.rs::declared_partial_semantics_matches_samples` | ✅ 2026-11-17 |
| RC-K2 | 上层看到的中间结果 SHALL 是当前未定稿句的累计文本（句内整体替换，不追加）。期望值必须是**手写字面量**，不能从样例里再读一遍（那是 `x == x`） | 三家的 provider 守卫（火山 / Realtime / 阿里：第二次更新**改写**第一次，「替换」与「追加」的结果才不同）+ 阿里的**字面量**那条 `asr/cloud/aliyun/tests.rs::partial_is_confirmed_prefix_plus_draft`（另两条是推导式，不带字面量）+ `asr/cloud/tests.rs::delta_accumulates_into_partial`（会话层不自己累加）+ **生产链路**：`asr/cloud/realtime/protocol_tests.rs::deltas_accumulate_on_the_production_path`（假 WebSocket 服务端跑真实 run 循环 —— 自建归一器的守卫守不住「provider → 上抛」那段接线） | ✅ 2026-11-17 |
| RC-K3 | 结果框里的实时句子顺序 SHALL 以本地时间轴为准，与最终转写一致；同 `start` 时缺口在前 | `scripts/ui/check.mjs` 的「缺口先到时句子仍排在缺口前面」/「同一时刻缺口排在句子前面」/「时间递增到达时文本逐字不变」三条断言（**RC-B3 的前端那半边** —— RC-B3 的后端守卫不覆盖 `result.js`；「与最终转写一致」是两条规则同源，界面断言只验实时行） | ✅ 2026-11-16 |
| RC-K4 | 同批多句的零长区间 SHALL 以**到达顺序**为准（服务端断句族收尾时一次性 drain 出多条 final：首条占收尾那段区间，第 2 条起退化为零长、同 `start`） | `asr/cloud/tests.rs::zero_length_segments_keep_arrival_order`（九条 final 同批到达：首条 `start < end`、后八条零长且同 `start`、文本顺序 = 到达顺序）—— **这条守的是「顺序」，不是区间准确性**：零长区间是已知取舍（今天无用户可见影响；触发条件与选型见 `AGENTS.md` 与 OpenSpec change `cloud-server-endpoint-spans`） | ✅ 2026-11-17 |
| RC-K5 | 上传丢包所占的时间 SHALL 计入真实录音时长，缺口位置 SHALL 在真实录音时间轴上；连续丢包并成一条；**缺口只上报一次** | `asr/cloud/tests.rs::upload_drop_records_gap_position`（缺口位置）、`asr/cloud/tests.rs::consecutive_drops_merge_into_one_gap`（并成一条）、`record/tests.rs::cloud_core_does_not_double_count_upload_drops`（时长不虚增 + `[[gap:1]]` **只出现一次**）、`record/tests.rs::cloud_tail_gap_follows_last_sentence`（收尾尾部缺口排在最后一句之后） | ✅ 2026-11-17 |
| RC-K6 | 凭据 SHALL NOT 出现在错误详情 / 状态回传 / 日志 | 错误详情：`asr/cloud/realtime/protocol_tests.rs::error_detail_redacts_secret`、`asr/cloud/realtime/protocol_tests.rs::server_error_redacts_secret`、`asr/cloud/volcengine/tests.rs::server_error_message_redacts_secret`（**helper 级守卫**：只钉 `sanitize` / `error_detail` 这两个函数，**不覆盖 run 循环的调用点**）；状态回传：`engine/tests.rs::cloud_status_exposes_bool_not_key`（连接列表只回传**掩码** —— `settings/cloud_tests.rs::mask_secret_never_reveals_the_whole_secret`）；**日志**：`asr/cloud/tests.rs::log_choke_point_redacts_the_credential`（关口的脱敏）+ `asr/cloud/tests.rs::cloud_logs_go_through_the_redacting_choke_point`（**源码级守卫**：云端模块里出现 `crate::log` 或 `crate::code_detail` 就红 —— 两条路都写日志文件却不脱敏；`crate::code_detail` 内部就是 `log::error`，云端模块一律用 `logging::code_detail` 先脱敏再委托）。**已知边界**：`eprintln!` 只进 stderr 不进日志文件；脱敏只保证凭据的**精确子串**不出现。前端那半边靠**「key 不回传」**：`Status` 里没有 key 明文，凭据只有 `cloud_configured` 布尔 | ✅ 2026-11-17 |
| RC-K7 | 服务端断句族 SHALL NOT 依赖客户端 commit 定句，也不要求本地 VAD | `asr/cloud/tests.rs::server_endpoint_provider_does_not_commit`、`volcengine/tests.rs::capabilities_are_server_endpoint`、`record/tests.rs::gate_follows_capabilities` | ✅ 2026-11-17（登记既有） |
| RC-K8 | 有 `item_id` 的 final SHALL 按 `item_id` 绑定，SHALL NOT 按到达顺序配对；**没有 `item_id` 时才按提交顺序（`pending` 队列）兜底**；空 final / 提交失败的区间也必须被消费或合并，不能留给下一句 | `asr/cloud/tests.rs::out_of_order_final_keeps_its_span`、`asr/cloud/tests.rs::failed_item_without_ack_keeps_spans_aligned`（前两条守 id 绑定）、`asr/cloud/tests.rs::empty_final_does_not_steal_next_span`、`asr/cloud/tests.rs::failed_commit_merges_span_into_the_last_one`（后两条守无 id 兜底与区间合并） | ✅ 2026-11-17（登记既有） |
| RC-K9 | 映射器的**去重表只收有内容的定稿**：空文本的定稿 SHALL NOT 占住去重键 —— 否则同 id 的**后续真实文本**会被早退整段丢弃（只留一条 WARN） | `asr/cloud/aliyun/tests.rs::empty_final_does_not_block_later_text_for_the_same_id` | ✅ 2026-11-17 |
| RC-K10 | 映射器的**防御性定稿 SHALL 与本句有没有文本无关**：新句（哪怕文本为空）+ `sentence_end` 时，上一句 SHALL 照样按快照 emit 成 Final；上一句只当过 Partial 的话，下一个 Partial 一覆盖就彻底消失 | `asr/cloud/aliyun/tests.rs::a_new_id_with_empty_text_still_finalizes_the_previous_sentence`、`asr/cloud/aliyun/tests.rs::a_missing_id_empty_end_still_finalizes_the_previous_sentence`（服务端违约不发 `sentence_id` 时同样不能丢上一句）、`asr/cloud/aliyun/tests.rs::a_draft_without_id_does_not_survive_the_next_update`（草稿缺 id 时按「属于本句」处理，过期草稿不重复拼接） | ✅ 2026-11-17 |
| RC-K11 | 防御性定稿的**取舍**：按快照定稿后那个 id 进了去重表 → 服务端随后对同一 id 的**完整** `sentence_end` 会被早退丢弃 → 定稿文本是快照而不是服务端最终文本。**宁可少几个标点，也不要同一句出两条 Final**（重复句子更糟） | `asr/cloud/aliyun/tests.rs::new_sentence_finalizes_previous_by_snapshot`（按快照定稿）+ `asr/cloud/aliyun/tests.rs::finalized_sentence_ignores_later_events`（已定稿的后续事件忽略） | ✅ 2026-11-17（登记既有取舍） |
| RC-K13 | 云端配置 SHALL 以**连接列表**保存：每条一整套配置（provider / 地址 / 凭据 / 模型），切 provider SHALL NOT 清空其它 provider 的连接；`Status` SHALL 只回传**掩码**、SHALL NOT 回传凭据明文 | `settings/cloud_tests.rs::switching_provider_keeps_the_other_providers_profile`（切走不清空别人的连接）+ `settings/cloud_tests.rs::mask_secret_never_reveals_the_whole_secret`（掩码不泄露原文）+ 界面断言「列表显示掩码、不显示明文」/「切到火山没有清掉阿里那条」 | ✅ 2026-11-18 |
| RC-K14 | 凭据 SHALL 能被**显式清除**（凭据明文不回传 → 界面没法「读出来再改」，只能清）；清除 SHALL 只动凭据，SHALL NOT 连带提交编辑区里其它未保存的字段 | `settings/cloud_tests.rs::clearing_the_credential_only_touches_the_credential`（只清凭据）+ 界面断言「编辑已存凭据的连接时能看到「清除凭据」」/「清完凭据后列表里不再有掩码」/「新增连接时没有「清除凭据」」 | ✅ 2026-11-18 |
| RC-K15 | 每条连接 SHALL 有**唯一 id**：空 id 或重复 id 必须拆开 —— 重复 id 会让按 id 匹配的删/改/切换全部串台 | `settings/cloud_tests.rs::empty_ids_are_reassigned_to_unique_values` + `duplicate_ids_are_reassigned_to_unique_values` | ✅ 2026-11-18 |
| RC-K16 | 换 provider 且备注留空时，备注名 SHALL 跟**新** provider 走（否则列表里「名字是 OpenAI Realtime、实际连阿里」） | `settings/cloud_tests.rs::changing_the_provider_uses_the_new_provider_default_label` | ✅ 2026-11-18 |
| RC-K17 | 删连接前 SHALL **二次确认**：凭据明文永不回传 → 删掉就找不回来 | 界面断言「删除连接前要确认」（确认框文案含该配置的名字） | ✅ 2026-11-18 |
| RC-K18 | 不存在的连接 id SHALL **报错**，SHALL NOT 静默当成功（删/清都要报错） | `settings/cloud_tests.rs::unknown_profile_is_reported_instead_of_silently_ignored` | ✅ 2026-11-18 |
| RC-K19 | 云端配置编辑区 SHALL **就地展开**在被编辑的那一行下面（新增时在「＋ 添加配置」上方）；保存 / 取消 / 清除凭据 SHALL 用**语义图标**（✓ 保存、× 取消、橡皮擦清除）—— 不用「保存到文件」「返回上一级」这类会读成别的动作的图标；破坏性的那枚 SHALL 与主操作分开、hover 才变红 | 界面断言「编辑区就地展开在被编辑的那一行下面」；**图标语义与配色只有人工守卫**（截图核对，断言 SVG 形状会太脆） | ⚠️ 2026-11-18 |
| RC-K20 | 云端配置**列表行**的三个位置 SHALL 各有各的语义：状态点只是**指示**（实心 = 使用中）SHALL NOT 是控件；「设为使用中」SHALL 绑在**整行**上（点空白处也行），点编辑 / 删除 SHALL NOT 顺带切换使用中；行尾动作图标 SHALL hover 这行才显形，使用中的那行一直显；**键盘聚焦到行内按钮时 SHALL 同样显形**（按钮只是 `opacity:0`，仍在 Tab 序列里 —— 少了 `:focus-within` 焦点就落在看不见的按钮上）；图标点击区 SHALL 不小于 32px | 界面断言「点整行能切换使用中的那条」/「圆点只是指示、不可点」/「动作图标 hover 才显形」/「键盘聚焦时动作图标也显形」/「图标按钮点击区不小于 32px」/「点编辑不会顺带切换使用中」；**状态点的形状与「使用中」徽章的配色只有人工守卫**（截图核对，断言 SVG 形状会太脆） | ⚠️ 2026-11-18 |
| RC-K12 | 云端 provider 在**协议层**丢弃的术语数 SHALL 回传界面：术语表是本地流式与云端共用的设置，但各家限制不同（阿里官方规范 ≤ 15 字、火山 100 条上限、Realtime 禁特定字符）—— 被丢掉的用户得知道，不能只记一条日志 | `asr/cloud/tests.rs::cloud_hotwords_ignored_counts_protocol_level_drops`（每个 provider 都有分支 + 正常词不丢）+ 界面断言「术语表提示云端协议层丢弃几条」 | ✅ 2026-11-17 |

---

## 下一轮该审什么

⚠️ + ❌ 共 **3 条**（RC-D11 界面文案的中性措辞只有人工守卫 —— 口语化没有词表可查，`check-i18n` 查不出来，每轮改文案时顺带过一遍；RC-K19 图标语义与配色只有人工守卫 —— 断言 SVG 形状会太脆，靠截图核对；RC-K20 状态点的形状与「使用中」徽章的配色同样只有人工守卫）。历史：2026-11-03 补完四条门禁后从 21 条降到 17 条，同日补四条 UI 断言后降到 13 条，
再补完原生层与打包后降到 9 条，补完质量门禁后降到 3 条；2026-11-13 复核降级三条回到 6 条，
2026-11-14 修完 RC-B6 后清零；2026-11-17 加了 K 组后回到 1 条，同日补上 RC-K6 的日志守卫后清零）。

作业面为零 **不等于**「不用再审了」，只表示没有「已知守卫薄弱 / 未审」的条目。下一轮从这两处起：

1. **找新不变量**：读生产路径（`record.rs` 的收尾、`asr.rs` 的时钟、`engine.rs` 的加载），
   发现清单里没有的约束，追加到本文末尾并给它稳定 ID —— 不要临时另起 Focus 清单。
2. **全量重审**：代码大改后（改模型、改时间轴、改并发结构），旧守卫可能仍在但已不覆盖新逻辑，
   按组 A–K 过一遍比只盯增量更可靠。

### 已完成（2026-11-04 · 并发三件）

| ID | 做了什么 | 在哪 |
| --- | --- | --- |
| RC-J1 | 两个槽（采集器 / 会话）改成一起登记：新增泛型 `register_slots`（先拿齐两把锁再写，锁序 采集器 → 会话）；两处 `start_*` 改用它，登记失败时置 `stop` / `discard` 让已起的识别线程退出 | `src-tauri/src/record.rs`、`src-tauri/src/lib.rs`、`src-tauri/src/record/tests.rs` |
| RC-J2 | 引擎有了「身份」（`OfflineEngineKey` = 模型目录 / 语言 / ITN，`StreamingEngineKey` = 热词）；新增 `install_engine`：拿槽锁 → **锁内**读设置 → 比身份 → 复用或重建。装引擎不再用进锁前捕获的快照 | `src-tauri/src/engine.rs`、`src-tauri/src/engine/tests.rs` |
| RC-J3 | `load_engine` 直接返回 `Arc<Engine>`，删掉 `shared_engine` 的阻塞取锁；新增门禁 `scripts/check-async.mjs`：沿调用图从 async 函数出发，引擎槽的阻塞 `.lock()` 只许出现在 `spawn_blocking` 闭包里 | `src-tauri/src/engine.rs`、`src-tauri/src/lib.rs`、`scripts/check-async.mjs`、`scripts/check-async.test.mjs`、`package.json`、`.github/workflows/check.yml` |

这轮的修法都是「**把失败点前移 / 让竞态在结构上不成立**」，不是给症状打补丁：
两槽登记不再有中间态；装引擎不再有「读设置」与「装引擎」之间的缝；
async 路径上不再有第二把引擎锁。

### 已完成（2026-11-03）

| ID | 做了什么 | 在哪 |
| --- | --- | --- |
| RC-H7 | `check-deps.mjs` 重写：加「经典脚本不得被 import」「每个模块都要登记」两条规则，补 `check-deps.test.mjs`（9 条） | `scripts/check-deps.mjs`、`scripts/check-deps.test.mjs` |
| RC-C2 | 两模式 VAD 配置收敛到唯一出处 `engine.rs` 的 `vad_config()`，加单测断言除 `max_speech_duration` 外全一致 | `src-tauri/src/engine.rs`、`src-tauri/src/engine/tests.rs` |
| RC-H6 | 新增 `scripts/check-contract.mjs`：前后端命令名 + 载荷字段名 + mock 载荷字段一致性，配 11 条自测 | `scripts/check-contract.mjs`、`scripts/check-contract.test.mjs` |
| RC-D5 | `check-i18n.mjs` 加经典脚本的三条硬约束：**不得加 `async`**（实测界面直接挂）、**不得写成 `type="module"`**（挪到 `app.js` 之后就挂）、**不得排在 `app.js` 之后**。附：`app.js` 必须保留 `type="module"`。配 4 条自测 | `scripts/check-i18n.mjs`、`scripts/check-i18n.test.mjs` |
| RC-A2 / RC-A6 / RC-C3 / RC-F11 | 补四条 UI 断言（各一条）：下载取消显示「已取消」、停止超时带细节仍走超时文案、录音过短的提示带秒数、术语表提示词表外字有几条。mock 侧补：`cancel_download` 让在飞的下载以 `setup.cancelled` 失败、`set_hotwords` 回 `hotwordsIgnored` | `scripts/ui/check.mjs` |
| RC-G1 / RC-G5 | 假 `__TAURI__` 补 `setTheme` / `setTitle`，加三条断言（切主题、切回浅色、切语言同步窗口标题）；再用一个 Windows UA 另开一页，验侧栏品牌行没被切掉 | `scripts/ui/check.mjs` |
| RC-G3 / RC-G4 | 新增 `scripts/check-bundle.mjs`：ad-hoc 签名、麦克风 entitlement、Info.plist 用途说明、macOS 专属代码的 cfg（`menu.rs` 顶层不许有 `use`，`lib.rs` 的 `use menu::…` 必须带 cfg）。CI 两头都补上：`check.yml` 加 `rust-windows` job（每个 PR `cargo check --all-targets`），`build.yml` 的 macOS job 在 build 后用 `codesign` 验签名与 entitlement | `scripts/check-bundle.mjs`、`scripts/check-bundle.test.mjs`、`.github/workflows/check.yml`、`.github/workflows/build.yml` |
| RC-I1 | 新增 `src-tauri/tests/quality_gate.rs`：量**生产路径**（`OfflineSession` / `StreamingSession`）的 CER 与 RTF，带阈值断言（整段 ≤ 25%、流式 ≤ 55%、RTF ≤ 1.0）。语料列举与归一化挪到 `tests/common/mod.rs`（探针与门禁共用一份） | `src-tauri/tests/quality_gate.rs`、`src-tauri/tests/common/mod.rs` |
| RC-I3 | 四个长录音探针从「只打印」改成「打印 + 断言」：VAD 吞掉的、整段未覆盖的、流式未覆盖的、重采样后的长度与识别差异，各有阈值 | `src-tauri/tests/vad_long_probe.rs`、`offline_long_probe.rs`、`streaming_long_probe.rs`、`resample_probe.rs` |
| RC-I4 | 新增 `src-tauri/tests/memory_probe.rs`：一个会话跑 30 分钟音频，300 s / 1800 s 两点的峰值内存增长 < 32 MB。加了 `libc` 作为 dev-dependency（本来就在依赖树里，不增加编译成本） | `src-tauri/tests/memory_probe.rs`、`src-tauri/Cargo.toml` |
| RC-D7 | `check-i18n.mjs` 加黑话规则：界面文案里出现线程 / 预热 / 收尾 / 引擎状态 / 解码器 / 采样率 / 推理（或对应英文）就红（只查文案的值，不查 key）。配 2 条自测 | `scripts/check-i18n.mjs`、`scripts/check-i18n.test.mjs` |
| RC-D10 / RC-F9 | 补四条 UI 断言：界面语言下拉只有三个选项、不混进识别语言、切界面语言不改识别语言；启动时不申请麦克风权限、列设备时界面已经画完 | `scripts/ui/check.mjs` |

顺带补上的两条：**后端裸字符串错误**（`Err("加载失败")` 绕过字典，原文会甩到界面）
现在由 `check:i18n` 查；**假后端漏字段**（`Status.hotwordsIgnored`）由 `check:contract` 查出并已补上。

补 UI 断言时**撞出一个真缺陷**：下载被取消后，进度行写的是「下载失败」——
用户自己点的取消，界面却说失败。已修：`runDownload` 按错误码区分取消与失败
（`core.js` 新增 `errorCode()`，跟 `fmtError` 共用同一处切分），取消时进度行写「下载已取消」。

### RC-B6 收尾复核

门控切到静音时，旧实现先按整批录音时钟定稿，却把不足一个分片的 pending 留给
重置后的识别器。停止时旧音频可能被识别为一条零时长的新句。
`streaming_endpoint_time::gated_stop_keeps_pending_in_the_previous_sentence` 用真模型
覆盖生产路径；修复前实际出现 `3.8s → 3.8s`，修复后使用真实输入终点并保留句尾。

先前将原因概括为「重放分片早于上一句终点」不够准确；trait 是测试手段，不是修复本身。
当前回归不证明任意不合法的旧音频输入都能产生非零区间：确无可用时间时仍保留钳制。

### 口径说明（2026-11-14）

- **CI 不在本项目的验收范围内**：真模型守卫一律本地跑（设 `VOICE_NOTES_MODEL_DIR` /
  `VOICE_NOTES_STREAMING_MODEL_DIR`），跑法见 `docs/HOW_TO_VERIFY.md`。
  守卫「没环境变量就静默跳过」是**本地跑的取舍**，不是缺陷 —— 2026-11-13 曾据此把
  RC-B4 / RC-B6 / RC-B7 从 ✅ 降为 ⚠️，2026-11-14 按实际状态改回。
- 因此「守卫在 CI 上等于不存在」**不再**是降级理由；判定标准回到
  「这条不变量有没有断言钉住」。
- `RC-B8` / `RC-B10`（需真模型环境变量）：守卫本身在（`asr/streaming_time_tests.rs`），
  设了模型目录就能跑，本地已跑绿。

---

## 复审台账

| 日期 | 审了哪些 ID | 当天 fix 数 | 结论 |
| --- | --- | --- | --- |
| 2026-10-22 | （前端拆分 + 长内容方案，未成文） | 11 | 清单雏形 |
| 2026-10-23 | （初版：取消录音、切语言、模型状态、下载标志、截断模型、VAD panic） | 15 | 清单雏形，未成文 |
| 2026-10-24 | （整库评审 6 项） | 10 | 同上 |
| 2026-10-26 | （录音生命周期 + 全库审查） | 2 | 同上 |
| 2026-10-28 | （时间轴、续传、toast、编辑文本、焦点、导入图） | 10 | 同上 |
| 2026-11-01 | （时长、徽标、停止超时、下载取消、拼句） | 6 | 同上 |
| 2026-11-02 | （录音中编辑、含占位符的历史、键盘焦点、预热假成功、新增语言注册） | 13 | 同上 |
| 2026-11-03 | RC-H7 / RC-C2 / RC-H6 / RC-D5（补四条门禁）+ 后端裸字符串错误 + mock 载荷字段 + RC-A2 / RC-A6 / RC-C3 / RC-F11（补四条 UI 断言）+ RC-G1 / RC-G3 / RC-G4 / RC-G5（原生层与打包）+ RC-I1 / RC-I3 / RC-I4 / RC-D7 / RC-D10 / RC-F9（质量门禁与文案守卫） | 4 条门禁 + 4 条 UI 断言 + 1 条打包门禁 + 1 条质量门禁 + 1 条内存门禁 | **清单成文**（72 条）；补四条门禁 —— 缺口 21 → 17；再补四条 UI 断言（撞出并修掉「下载取消却说失败」）—— 17 → 13；再补原生层与打包 —— 13 → 9；再补质量门禁与文案守卫 —— 9 → 3（RC-I2 是明确决定不做的取舍） |
| 2026-11-04 | RC-J1 / RC-J2 / RC-J3（并发与原子性：两槽登记、引擎快照与清空、async 阻塞锁） | 1 条并发门禁 + 10 条 Rust 单测 | 清单 72 → **75 条**（10 组）；⚠️+❌ 仍是 3 条 —— 这轮补的是**已有作业面之外**的新不变量（J 组），作业面没变大 |
| 2026-11-13 | RC-B4 / RC-B6 / RC-B7 / RC-B8 / RC-B10（流式时钟与缺口）+ `install_engine` 的配置读取 | 0（本轮只做核实） | 复现口径：`cargo test --lib` 不设环境变量时，B4/B6/B7 的用例 0.00s 静默返回却仍算 passed → 三条从 ✅ 降为 ⚠️；作业面 3 → **6 条**。真模型补跑：`streaming_` 前缀 8 条 + `streaming_time` 2 条全绿；`npm run check`（120 条单测）与 `npm run check:ui`（140/140）全绿 |
| 2026-11-13 | RC-B6（同批多个 endpoint 的分片计时） | 1 条代码修复 + 7 条无模型回归（已随 645c87a 落库） | 分片携带真实采样终点，endpoint 不再共用批末时钟；保留 endpoint 后 pending 的句子归属，不让下一轮 `advance_dropped(0)` 跳过其时间；跨批余量、缺口、预录、尾包与空输入都有断言；新用例先 RED 后 GREEN，`npm run check` 汇总 127 passed；真模型端到端未执行 |
| 2026-11-13 | RC-B6（真模型端到端补验） | 0（新增 2 条真模型测试，未改生产代码） | 本机现有 Zipformer + VAD，25.776s 音频单批触发 3 个 endpoint；StreamingSession 与 run_streaming_core 的实时事件/最终 segments 均为 `[0,7)`、`[9.6,15.6)`、`[18.2,24.2)`，与 200ms 喂法逐项一致；显式执行 `2 passed; 0 failed; 0 ignored`，clippy all-targets 通过；不含 Tauri WebView 传输验证，不改变 CI 取舍 |
| 2026-11-14 | RC-B6 零时长残边 + 流式探针时钟 + 端到端用例宽限（独立复审 4 条） | 3 条代码修复 + 3 条无模型回归 | ① `Timeline::commit_at` 收 `audio_start`：重放分片整段早于句起点时退到分片音频起点，不退进上一句（`last_end` 兜底）；② `quality_gate` / `memory_probe` 的流式测量补 `advance_samples`，时间戳不再全零；③ `vad_is_speech` 逐窗取「或」，整批含语音时不再被批尾静音门控丢弃 —— **实测口径**：5.59s 语音 + 3s 尾静音喂成一整批时，批末状态是 `false`（整批语音被丢掉），逐窗「或」是 `true`；这个差别只在**批很大**时出现（60ms 粒度下两者相同），而端到端用例那 25.776s 单批是靠 700ms 宽限喂进去的（旧代码同样 3/3 通过）—— 「端到端用例不再依赖宽限」这句成立，但不等于旧代码过不了该用例；同一轮改正「未 commit」注记（该批已随 645c87a 落库） |
| 2026-11-14 | RC-B6（自查 4 条提交：句子起点语义 + 句起点倒退） | 1 条代码修复 + 4 条无模型回归 | ① 句子起点改用**这句最早那片音频**（`Timeline::sentence_audio_start`）：原来取「触发定稿的那一片」，句子横跨多片时前半段音频落在区间外，界面上这句话短一截；② 句起点只前进不后退：重放分片定稿时原来把 `segment_start` 改成那片分片的终点（比当前句起点更早），下一句会从那一片开始，区间里混进不属于它的音频；③ 余量（跨批 pending）只标记「这句有内容」，不拿它当句子起点 —— 余量可能横跨一个洞，位置不精确；④ `detected()` 是**状态量**实测确认（144 批里 74 批为 true，首个 true 在第 17 批），逐窗取「或」的前提成立。回归覆盖：跨两片的重放句、句起点不倒退（`asr/timeline_tests.rs` / `asr/stream_input_tests.rs`）；`cargo test --lib` 133 passed、真模型端到端 `2 passed`、clippy all-targets 通过、`npm run check` 六道门禁 fail 0 |
| 2026-11-14 | RC-B4 / RC-B6 / RC-B7 / RC-B8 / RC-B10 / RC-I2（**改口径**：CI 不在验收范围内） | 0（只改清单状态，未动生产代码） | 真模型守卫**本地跑就是绿的**（实测：`cargo test --lib` 6 条真模型守卫全 ok；`quality_gate` 整段 CER 16.39% / 流式 43.28%，最差 RTF 0.036；`memory_probe` 整段 300s→1800s 涨 6.7 MB、流式涨 0.3 MB；`npm run check:ui` 140/140）—— 2026-11-13 那次降级的唯一理由是「守卫在 CI 上等于不存在」，按新口径不成立，故 RC-B4 / RC-B7 / RC-B8 / RC-B10 改回 ✅；RC-I2 从「流式路径进 CI」改为「流式路径的质量门禁可本地复现」，ID 保留。**作业面 6 → 1**，剩下的 RC-B6 是真缺陷（重放分片早于上一句终点时仍为零时长），不是覆盖问题 |
| 2026-11-14 | RC-B6（门控收尾与真实输入时间） | 代码修复 + 回归测试（未提交） | 真模型生产路径在修复前复现零时长；收尾前消费 pending 并冲尾上下文，按真实输入终点定稿；空结果不占用已提交句子的时间，预录不能跨前置缺口。`npm run check`（140 passed）、`streaming_endpoint_time`（3 passed）、`streaming_smoke`（8 passed，含热词配置）、`streaming_long_probe` 与 `quality_gate` 通过 |
| 2026-11-17 | RC-K1 / RC-K2 / RC-K4 / RC-K5 / RC-K6 / RC-K7 / RC-K8（云端模式 K 组）+ RC-K3（登记批 A 已落地的三条界面断言） | 0（本轮只补守卫与文档，未动生产代码） | 清单 75 → **83 条**（11 组 A–K）；新增 3 条守卫（K1 / K2 / K4）+ 补强 1 条断言（K5 的「`[[gap:1]]` 只出现一次」）+ 登记 5 条既有（K3 / K5 其余三条 / K6 的错误详情与状态回传 / K7 / K8）；**RC-K6 的日志那半边仍 ⚠️**（凭据不出现**在日志里**没有任何守卫）；作业面 0 → **1 条**。负向验证（七种改坏都让对应守卫变红，已恢复）：火山声明改成 `Delta`、Realtime 声明改成 `Preview`、阿里声明改成 `Delta`、归一器的 `Delta` 臂改成替换、火山改用 `result.text`、排序逆序、排序按文本 |
| 2026-11-17 | RC-K4（服务端断句族「同批多句 → 后续句区间退化为零长」这个取舍的单独评估） | 0（只做评估与文档，未动生产代码） | 评估结论：**今天没有用户可见影响** —— 结果框不显示逐句时间、导出是纯文本、落盘不存区间、顺序不乱（RC-K4 守的是顺序，不是区间准确性）。三条修法各有代价：本地 VAD 要改「服务端断句族不需要本地 VAD」这条能力契约；provider 时间戳兜底要**定一套口径**（两家 `begin` 差 230 ms、Realtime 没有时间戳）；本地平分剩余区间要改会话层核心（影响所有 provider）而且边界仍是编造的 → **暂不修**。实测（四种情形 × **各 3 轮、12 组数字逐位一致**（可复现）：两家 × 有/无 1.5 秒尾静音，同一个单句 fixture）：两家的 `end` 都落在**语音末点（~5100 ms）与音频末点（7092 ms）之间**，而**本地轴把尾巴静音整段算进句子**、直接顶到 7092（阿里补静音后 `end` 反而变小，机制未查） —— 偏差方向是「本地轴偏长」，终点差 0 / 12 ms（无尾静音）到 **1.28 / 1.71 s**（有尾静音）；两家对同一起音点的 `begin` 差 ~230 ms（火山 572 / 阿里 800），**互相校准不了**；Realtime 根本没有时间戳。**四组样本、同一个 fixture，不是常数**。将来触发条件：导 SRT·VTT / 显示逐句时间 / 按句跳转 —— 届时选 provider 时间戳。评估全文见 OpenSpec change `cloud-server-endpoint-spans` |
| 2026-11-17 | RC-K6（云端凭据不进日志：守卫从无到有） | 0（收口不是修 bug —— 原来的调用点就脱敏了，缺的是**守卫**） | 云端模块的日志改走 `asr/cloud/logging.rs` 的**唯一出口**（写入前用该 provider 的凭据脱敏），12 处 `crate::log::` 调用全部改走；两条新守卫：关口的脱敏（纯函数）+ **源码级守卫**（云端模块里出现直接的日志调用就红 —— 日志写进文件与 stderr，测试抓不到，唯一能自动发现「绕过关口」的办法是读源码）。RC-K6 由 ⚠️ 改 ✅，作业面 1 → **0 条**；清单仍 83 条（11 组 A–K）。负向验证（三种改坏都变红，已恢复）：关口去掉脱敏、provider 里加一行 `crate::log::warn`、`use crate::log;` 之后写 `log::warn` |
| 2026-11-17 | RC-K9 / RC-K10（外部复审发现：阿里映射器两处丢文本）+ 日志关口的两个口子 | 2 条代码修复（`fix(asr)`；**随 v1.6.0 发布出去的是缺陷，修复在 v1.6.0 之后** —— v1.6.1 尚未发版）+ 3 条回归测试 | 两份**独立的**外部复审（两个 Orca session）各自实测复现了同一处：① 空文本的定稿把 `finalized` 去重表毒化成空串 → 同 id 的后续真实文本被整段丢弃（只留一条 WARN）；② 新句 + 空文本 + `sentence_end` → 上一句拿不到 Final，文本只剩一条 Partial，下一个 Partial 一覆盖就彻底消失（与 AGENTS.md 写下的「防御性定稿」规则冲突）。修法：空文本不进去重表；防御性定稿的判定只看 id、与本句有没有文本无关（「无法核对」那一支仍要求带文本 —— 心跳形状靠它守住）。顺带收口：火山的 `map_error_code` 改成把凭据交给关口脱敏（不再靠调用点自觉）；源码级日志守卫补「按导入的名字判断」（`use crate::{log, …};` 漏网）；草稿缺 id 时按「属于本句」处理。负向验证（**四种改坏都变红**，已恢复：退回「空文本进重表」、退回「防御性定稿只看 id」、退回「草稿无 id 按属于本句处理」、加 `use crate::{log,…}` 绕过）。清单 83 → **85 条**（11 组 A–K），作业面仍 0 条 |
| 2026-11-17 | RC-K10 / RC-K11（第四份外部复审发现 aa8ceff 的残留：缺 sentence_id 的空收尾事件仍会丢上一句）+ RC-K6 日志守卫的两个绕过口 | 1 条代码修复（`fix(asr)`，第六次外部复审）+ 1 条回归测试 | 三份外部复审后又有第四份独立复审撞见同一处映射器缺陷的**另一半**（服务端违约不发 `sentence_id` 时，收尾事件认不出是哪一句 → 上一句的快照被丢掉）。修在 ⑥（按「快照归谁」分两种）；④ 的「无法核对」那一支**保持不变** —— 心跳形状（id=0 但没标 heartbeat）靠它守住，去掉条件会打破既有守卫。负向验证：退回 ⑥ → 变红。顺带把日志守卫的两个绕过口堵上（多行成组导入、任意路径前缀的 `log::` 调用）。清单 86 条，作业面仍 0 条 |
| 2026-11-17 | RC-K1 / RC-K2 / RC-K6（三块交叉复核后的补强：契约守卫盖住生产接线与协议解析、日志关口盖住 `code_detail`） | 0（补的是守卫与结构，没修 bug） | 交叉复核（三个 Haiku 子 agent + 逐条 mutation 验证）找出两类洞：① **契约守卫只守了 provider 内部映射**，没守「provider → 上抛」的接线（把 Realtime 的生产调用点绕开归一器 → 276 条测试全绿）与「协议帧 → Sentence」的解析（把阿里的 stash 解析丢掉 → 276 条全绿）；② **日志关口有第二条没人守的路** —— `crate::code_detail` 内部就是 `log::error` → 写日志文件，云端模块 33 处，守卫的片段表匹配不到它（**今天没有凭据泄漏**：三家的服务端文本进 `code_detail` 前都过了 `sanitize`）。补法：Realtime 加**假 WebSocket 服务端**守卫跑真实 run 循环；阿里的守卫改喂**真实协议 JSON**（经 `decode_server_event` → `map_sentence`）；关口补 `code_detail` 出口、守卫片段表换成 `crate::log` + `crate::code_detail`、33 处改走云端自己的出口、跳过 `mock.rs`。负向验证（**五种改坏都变红**）：Realtime 绕开归一器、阿里丢 stash、`crate::code_detail` 直调、`use crate::log;` 之后 `log::warn`、`crate::log::write`。清单：RC-K2 补登记阿里的字面量守卫与生产链路守卫；RC-K6 的日志那半边改成准确说法 + 已知边界（`eprintln!` 只进 stderr；脱敏只保证精确子串）。AGENTS.md：「客户端提交族不可达」→「协议正常时不可达」、触发条件统一成通式、`Status` 的措辞收准 |
| 2026-11-17 | RC-K12（云端术语表被协议层丢弃没有用户可见提示）+ RC-K6 日志守卫的第三、四轮收紧 | 1 条功能修复（`feat` 性质：Status 加字段 + 界面提示）+ 1 条 Rust 守卫 + 1 条界面断言 | 第三份外部复审发现：**云端术语表被协议层丢弃时用户看不到任何提示** —— 术语表是本地流式与云端共用的设置，但各家协议限制不同（阿里官方规范 ≤ 15 字、火山 100 条上限、Realtime 禁特定字符），而 `settings::clean_hotwords` 允许最长 32 字 → 一条 20 字的中文术语在云端会被**静默丢掉**，界面却只说「已保存」。修法：`Status.cloudHotwordsIgnored` 按当前 provider 算（用 `create_provider` 里**同一批过滤函数**），界面在云端丢弃时给**云端专属**的提示（与本地流式的「含模型不认识的字」分开）。同轮还收了日志守卫的两轮：子 agent 复核找出 `use crate::log::{warn};` / `use crate::log::*;` 两种**真漏网**（按导入名判会漏），改成「凡从 `crate::log` 导入的一律不许」；以及行尾注释 / 字符串字面量里的 `log::warn` 会**误伤**，改成扫**代码骨架**（去掉注释、清空字符串内容）。负向验证（四种改坏都变红，已恢复）：两种新导入写法、行尾注释误伤、字符串字面量误伤。清单 86 → **87 条**（11 组 A–K），作业面仍 0 条 |
| 2026-11-18 | RC-K13（云端配置改成连接列表）+ RC-K6（凭据只回传掩码） | 1 条功能改动（后端 `feat(asr)` + 前端 `feat(ui)`）+ 3 条 Rust 守卫 + 8 条界面断言 | 设置页的云端配置从**一张平铺表单**改成**一份连接列表**（参考用户给的中转服务密钥面板形态：列表 + 掩码 + 「使用中」+ 可增删改）。解决四个具体痛点：① 切 provider 清空一切（`app.js` 的 onChange 显式清空地址/密钥/模型/资源标识）→ 每家各一条独立连接；② 密钥零反馈（password 框 + 「已保存的密钥不会显示出来」）→ 列表显示掩码 `sk-c…HePU`；③ 模型/资源标识手打会踩雷（阿里模型走白名单、火山填模型 1.0 的 id 会握手 403）→ 按 provider 给候选项；④ 一个 provider 只能一套配置 → 可增删改多条。**借形态不借字段**：参考图的「并发不限 / 无消费上限」是中转服务的**配额**语义，桌面端一次只连一个、算不出来 → 不做。**不做旧格式迁移**（旧 5 个平铺字段被忽略，用户重填）—— 两套真值来源迟早不一致。**不做复制明文密钥**（要开显式命令回传明文，动 RC-K6 的口径，值得单独评估）。负向验证：`mask_secret` 对 1 字符密钥曾 panic（已修 → 固定点数）、`CloudProfileView` 漏 `rename_all=camelCase` 导致前端读不到字段（已修）。清单 87 → **88 条**（11 组 A–K），作业面仍 0 条 |
| 2026-11-18 | RC-K13（连接列表复审）+ RC-K14 ~ RC-K18（本轮新增）+ RC-D11（文案中性措辞） | 3 条代码修复（id 唯一 / 备注跟 provider / 凭据可清除）+ 1 条新命令（`clear_cloud_profile_key`）+ 1 条前端二次确认 + 24 条界面文案中性化 | 对 v1.7.0 的连接列表做了一次独立复审，查出四个真缺陷：① **界面已经无法清除凭据** —— `status.js` 把空串转成 `undefined`（=「不改」），而旧的「清除凭据」按钮在改成连接列表时被删掉 → 用户只能删掉整条配置才能清掉 key（后端其实支持空串=清空，能力够不着）；② `clean_cloud_profiles` 的 id 去重用了**一次性快照** → 两条空 id 拿到同一个新 id（实测 `["p-3","p-3"]`）→ 删一条删两条、改一条改另一条；③ 换 provider 时备注名按**旧** provider 取（`provider_label` 在赋值之前求值）→ 列表里「名字是 OpenAI Realtime、实际连阿里」；④ 删连接没有二次确认 → 凭据明文永不回传，删掉即不可恢复。修法与守卫：新增 `clear_cloud_profile_key` 命令（**只动凭据**，不连带提交编辑区里未保存的字段）+ 界面「清除凭据」按钮；id 候选集改成「已定下来的 ∪ 原始列表」；`profile.provider` 先赋值再取默认名；删连接前 `confirmDialog`；另加 RC-K18（不存在的 id 要报错，不静默）。文案那轮按「一句中性的话 + 用户看得懂的参数」把 24 条口语措辞改成中性（`record.empty` / `history.empty` / `model.offlineMissing` / `settings.hotwordsHint` 等），并把界面的「连接」改成「**配置**」（「连接」是技术词，且与旁边的「云端服务」下拉打架）、下拉标签改成「服务商」。清单 88 → **94 条**（11 组 A–K）；作业面 0 → **1 条**（RC-D11 只有人工守卫：口语化没有词表可查，`check-i18n` 查不出来）。验证：`cargo test --lib` **295 passed / 0 failed**（290 → 295）；`npm run check:ui` **182/182 通过**（177 → 182）；`check:i18n` / `check:deps` / `check:contract` 通过 |
| 2026-11-18 | RC-K19（云端配置编辑区的图标语义与版式） | 3 处界面调整（保存 / 取消的图标语义化 + 「清除凭据」改图标按钮 + 编辑区就地展开与动作行重排） | 看截图发现三个问题：① **保存用的是「保存到文件」的向下箭头**（`saveIcon` = 箭头落进托盘），在这里会被读成「下载」→ 换成 `checkIcon`（✓）；② **取消用的是左箭头**（`backIcon`，原本的理由是「叉是关闭/删除」）—— 取消在表单里就是**收起这次编辑**，不删任何东西（删是垃圾桶那枚），左箭头「返回上一级」是误导 → 换成 `closeIcon`（×）；③ **「清除凭据」是文字按钮，跟两个 28px 图标按钮挤在左下角**，破坏性操作还紧挨保存 → 改成橡皮擦图标（`clearIcon`，跟垃圾桶分开）、靠左并 hover 才变红，保存 / 取消靠右；④ 编辑区原本固定挂在列表下方，点第二行的铅笔、表单却出现在列表底下，视线要来回找 → 改成**就地展开在被编辑那一行的下面**（新增时在「＋ 添加配置」上方，`placeEditor()`），列表重建后重新放置。新增两条界面断言（「编辑区就地展开在被编辑的那一行下面」「清除凭据前要确认」）—— 清除凭据也要确认：明文永不回传，清掉就找不回来。`npm run check:ui` 182 → **184/184 通过**；清单 94 → **95 条**，作业面 1 → **2 条**（RC-K19 的图标语义只有人工守卫：断言 SVG 形状太脆，靠截图核对） |
| 2026-11-18 | RC-K20（云端配置列表行的图标语义与交互） | 4 处界面调整（状态点改纯指示 + 点整行设为使用中 + 行尾动作 hover 才显形 + 图标点击区 28px→32px） | 看图发现列表行里三个位置都在抢注意力：`●/○` 是**字符**当按钮（大小随字号漂、深色下发灰），铅笔与垃圾桶同一种灰看不出轻重，「使用中」用描边胶囊跟图标不在同一视觉层。改成 B 档：① 状态点换成 SVG 实心/空心点并**降级为纯指示**（`aria-hidden`，不再是控件）；② 「设为使用中」绑到**整行**（事件委托取**最近**的 `[data-action]` —— 点在编辑/删除上就是那两个动作，不会顺带切换使用中），键盘可达性靠名称+明细那个按钮（同 `history-main` 的做法）；③ 行尾动作默认 `opacity:0`，hover 这行才显形，使用中的那行一直显；④ `.icon-btn.sm` 28px → 32px（点击目标太小）。「使用中」徽章改浅底胶囊（`--accent-soft` 底 + `--accent` 字）。新增五条界面断言（「点整行能切换使用中的那条」「圆点只是指示、不可点」「动作图标 hover 才显形」「图标按钮点击区不小于 32px」「点编辑不会顺带切换使用中」）。`npm run check:ui` 184 → **188/188 通过**；清单 95 → **96 条**，作业面 2 → **3 条**（RC-K20 的状态点形状与徽章配色仍只有人工守卫） |
| 2026-11-19 | RC-K20（加固）+ RC-K14 / K15 / K16 / K18（复核） | 5 处修复（1 处功能性 + 4 处文档/口径） | 对上一轮那批未提交改动做了一次独立复审：门禁全绿（`cargo test --lib` 295 / `check:ui` 188），并用 **mutation** 验证守卫真的有效 —— 后端改坏 4 处（备注名回退旧 provider、id 去重退回快照、清凭据连带清地址、不存在 id 静默成功）→ **恰好** 5 条对应用例变红、其余 290 条不动；前端改坏 2 处（取消整行的 `data-action`、编辑区固定挂底部）→ **恰好** 3 条界面断言变红。查出 5 个问题：① **行尾动作 `opacity:0` 但仍在 Tab 序列里，且 `.cloud-profile-actions` 漏了 `:focus-within`**（`.history-actions` 是有的）→ 键盘用户 Tab 到铅笔 / 垃圾桶时焦点落在两个看不见的按钮上；补规则 + 新增断言「键盘聚焦时动作图标也显形」；② 清单**分组标题**的条数没跟着涨（`D. i18n 与文案` 10 → 11、`K. 云端模式` 13 → 20，标题合计 88 而表格实际 96 —— 台账里写的 96 是对的）；③ README 截图是这批 UI 改动**之前**出的（行布局 / hover 才显形的动作 / 32px 图标 / 徽章胶囊 / 编辑区卡片全变了）→ 重出；④ `HOW_TO_VERIFY.md` 仍写「重出 README 9 张截图」（实际 10 张）；⑤ `deleteCloudProfile` / `clearCloudProfileKey` 直接 `await confirmDialog(...)` 没包 try/catch（都由 `void` 调用，插件 IPC 抛错就是未处理 rejection，而 `confirmCloudUpload` 是包了的）→ 抽 `confirmDestructive()`：打不开按「没同意」处理，原因进日志。顺手：截图脚本改成**先 hover 未使用那一行**再截（不 hover 的话图里根本没有「怎么改、怎么删」这两个入口），README 同步补「移到那一行才出现」。**测试自身的坑**：新断言第一次是红的（`聚焦后=0`），不是 CSS 没生效 —— 是 `transition: opacity 0.12s` 还没跑完就读了计算值，加 250ms 等待。`npm run check:ui` 188 → **189/189 通过**；清单仍 **96 条**（新断言挂在 RC-K20 下），作业面仍 3 条 |

「当天 fix 数」一列是 `git log` 实测值（`git log --since=<date> --until=<date>` 统计以 `fix` 开头的提交），
不是估算 —— 它同时也是「这轮复审的产出」的度量。
