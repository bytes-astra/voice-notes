# 复审清单（长期不变）

> 这份文档是**复审的固定作业面**。每轮复审从「尚未审过」的区域开始，**不要重新发明检查项**。
>
> 上一版每轮换一份新的 Focus 清单，于是每轮都「发现新问题」—— 其实是一批一直存在的
> 缺陷被不同角度翻出来（见下方「为什么要有这份文档」）。清单固定后，「已审过」才能累积，
> 缺陷池才会变成递减曲线。

**当前规模**：**72 条**不变量、9 组（A–I），分 3 档 —— ✅ 51 条有自动守卫 / ⚠️ 12 条守卫薄弱 /
❌ 9 条无守卫。**21 条（⚠️+❌）就是下一轮的作业面。**

（历史口径：5 份旧 review plan 一共提出 **26** 个 focus 项 —— 那是「轮次」的粒度；
本文档把它们展开成 **72** 条可逐条核对的条目。两个数各指什么，不要混用。）

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
每轮复审用一份**不同**的 Focus 清单（见 `docs/superpowers/plans/*review*.md`），于是每轮换一副
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
| RC-A2 | 模型下载中途取消，必须翻译成字典 key（`setup.cancelled`），不露出内部哨兵串 | `models/tests.rs` cancelled_sentinel_is_only_the_download_cancelled_marker / cancelled_sentinel_with_detail_is_still_cancelled / cancelled_download_returns_the_dictionary_key / cancelling_during_download_is_translated_to_the_dictionary_key | ⚠️ 后端有守卫；**UI 层无「已取消」提示断言** |
| RC-A3 | 采集流打不开必须报错（`record.micOpenFailed`），不能静默 | `audio/tests.rs` capture_start_reports_play_failure / play_failure_maps_to_the_mic_open_code | ✅ |
| RC-A4 | VAD 建不起来必须报错，不能 panic | `asr/tests.rs` vad_failure_is_reported_instead_of_panicking | ✅ |
| RC-A5 | 流式引擎加载失败必须报错，**不能显示「已就绪」**、徽标不能永远停在「加载中」 | UI `预热失败不弹「已就绪」` / `预热失败给出错误提示` / `整段预热失败不弹「已就绪」` / `流式引擎报错时徽标是红底` / `流式引擎报错时状态点标红`；`engine/tests.rs` 加载结果映射成引擎状态 | ✅ |
| RC-A6 | 停止录音超时：错误串带细节（`record.stopFailed:细节`）时也要走超时文案，不能把 key 甩给界面 | `record/tests.rs` stop_failed_is_recognised_with_and_without_detail | ⚠️ 只有后端；UI 未验「超时」文案 |
| RC-A7 | 停止等待失败不得交出半截结果，且要置 discard | `record/tests.rs` wait_failure_returns_no_partial_result | ✅ |
| RC-A8 | 麦克风故障不能阻断 ASR 收尾，也不能把空结果写历史 | `record/tests.rs` capture_failure_does_not_discard_finished_transcript / capture_failure_without_transcript_returns_error；UI `麦克风异常后回到空闲` / `麦克风异常保留部分结果` / `麦克风异常写入历史` | ✅ |
| RC-A9 | 识别结果为空（VAD 丢音频）必须留痕，不能白丢 | `asr/tests.rs` warns_only_when_speech_went_missing | ✅ |

### B. 缺口与时间轴口径（10 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-B1 | 缺口占位符形状前后端一致（`[[gap:<数字>]]`），占位符前后不加空格 | `scripts/check-join.mjs` + `check-join.test.mjs`；`asr/gap_tests.rs` gap_placeholder_joins_without_spaces | ✅ |
| RC-B2 | 缺口标记落在**缺口终点**（丢掉的音频之后才开始识别），两种模式同一条规则 | `record/tests.rs` gap_segment_marks_the_given_moment；`asr/gap_tests.rs` inserts_gap_in_time_order | ✅ |
| RC-B3 | 同一时刻，缺口排在句子**前面**（否则读起来像句子之后才丢的音频） | `asr/gap_tests.rs` gap_precedes_a_segment_at_the_same_moment；`record/tests.rs` streaming_gap_marker_precedes_the_following_sentence | ✅ |
| RC-B4 | 丢掉的音频占真实录音时间（时长不能少算） | `record/tests.rs` dropped_audio_counts_towards_recorded_time / streaming_dropped_audio_advances_recording_clock | ✅ |
| RC-B5 | 时间轴换算在**秒域**比较，区间语义 `[start, end)`：起点压缺口要移、终点压缺口不移 | `record/tests.rs` offline_timeline_shifts_by_dropped_audio / offline_timeline_boundary_is_half_open / offline_timeline_is_unchanged_without_drops；`asr/timeline_tests.rs` 全 6 条 | ✅ |
| RC-B6 | 实时事件流与最终结果必须同一套时间刻度（不能混两套） | `record/tests.rs` streaming_duration_includes_leading_silence / streaming_duration_includes_audio_arriving_after_stop | ✅ |
| RC-B7 | 缺口必须同时进最终文本**和** `segments` | `record/tests.rs` streaming_gap_marker_reaches_transcript / offline_gap_marker_reaches_transcript | ✅ |
| RC-B8 | 流式尾部补的静音不计入时长 | `asr/streaming_time_tests.rs` duration_counts_only_advanced_audio（**需真模型**，无环境变量时静默跳过） | ⚠️ 依赖真模型 |
| RC-B9 | 停止时识别期间采到的尾巴必须被消费（否则句尾丢字） | `record/tests.rs` offline_stop_keeps_the_tail | ✅ |
| RC-B10 | 实时事件流不能重复推进时钟 | `asr/streaming_time_tests.rs` externally_advanced_audio_is_not_counted_again | ⚠️ 依赖真模型 |

### C. 两种模式同一条规则（7 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-C1 | 整段 / 流式的拼句规则逐条一致（CJK 区间带端点开闭、占位符完整形状、三条规则的表达式） | `scripts/check-join.mjs` + `check-join.test.mjs`（14 条变异测试） | ✅ |
| RC-C2 | VAD 参数两模式一致（`min_silence` 等） | —— | ⚠️ **无守卫**：`check:join` 只管拼句，不管 VAD 参数 |
| RC-C3 | 录音过短的提示两模式走同一套守卫，文案带秒数（「至少说 0.2 秒」而不是「至少说 {seconds} 秒」） | `record/tests.rs` too_short_transcript_reports_seconds | ⚠️ 只有后端；**UI 未验提示文案** |
| RC-C4 | 徽标三态两模式一致（不能一个红一个灰） | UI `流式引擎报错时徽标是红底` / `流式引擎报错时状态点标红` | ✅ |
| RC-C5 | 短句合并层两模式共用（短于 3 秒不立刻定稿，间隔超 2 秒不合并） | `asr/merger_tests.rs` 全 6 条 | ✅ |
| RC-C6 | 超长句才切（阈值 90 秒），相邻两段重叠合并 | `asr/merge_tests.rs` 全 4 条 | ✅ |
| RC-C7 | 预录回扩只作用于第一段且有门控（不能无条件回扩） | `asr/pre_roll_tests.rs` pre_roll_only_for_speech_like_prefix | ✅ |

### D. i18n 与文案（10 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-D1 | 各语言 key 必须完全对齐（缺一个就显示 key 名） | `scripts/check-i18n.mjs` | ✅ |
| RC-D2 | 同一个 key 在各语言的占位符集合必须一致 | `check-i18n.mjs` 占位符校验 | ✅ |
| RC-D3 | 新增语言但忘记在 `index.html` 注册 → 门禁必须红 | `check-i18n.mjs` locale 注册检查 + `check-i18n.test.mjs` | ✅ |
| RC-D4 | locale 文件必须排在 `i18n.js` 之前（否则 `window.I18N_LOCALES` 读不到） | `check-i18n.mjs` 脚本顺序检查 | ✅ |
| RC-D5 | `i18n.js` / `select.js` 必须排在 `app.js` 之前（否则 `ReferenceError: UISelect is not defined`） | —— | ❌ **无守卫**：`check-i18n.mjs` 只查 locales → i18n.js，没查 → app.js |
| RC-D6 | 错误文案不暴露技术细节（错误码、本地路径、镜像地址、原始异常） | UI `模型加载失败显示字典文案` / `错误提示不泄露错误码和本地路径` / `下载失败不弹原始错误串` / `未知对象错误不泄露细节` / `未知字符串错误不泄露细节` / `Error 对象不泄露原文` + 3 条「细节写入日志」 | ✅ |
| RC-D7 | 界面文案不带内部黑话（线程 / 预热 / 收尾 / 引擎状态），技术细节只进日志 | —— | ⚠️ 靠人工审；`check-i18n` 只查 key 存在与占位符，不查文案质量 |
| RC-D8 | 切语言不覆盖用户编辑过的文本 | UI `切语言不覆盖用户编辑` / `切英文不覆盖用户编辑` / `连续切两次语言不覆盖用户编辑` / `历史载入后切语言保留用户编辑` | ✅ |
| RC-D9 | 缺口占位符跟着界面语言走（历史里也跟） | UI `缺口占位符初始为中文` / `切英文占位符跟着变` / `连续切两次占位符回到中文` / `历史里的缺口占位符初始为中文` / `历史里的缺口占位符跟着语言变` | ✅ |
| RC-D10 | 界面语言 ≠ 识别语言（6 个识别语言芯片 vs 2 个界面 locale）不能混淆 | —— | ❌ 无守卫（文档已区分，界面未断言） |

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
| RC-F9 | 麦克风权限改成点麦克风时才申请，且界面渲染完再列设备（避免启动就弹权限框打断用户） | —— | ❌ **无守卫**：靠流程约定，`check:ui` 无法验证 |
| RC-F10 | 落盘文件名不覆盖已有文件 | `output/tests.rs` writes_unique_files_without_overwriting | ✅ |
| RC-F11 | 术语表按模型词表过滤：表外字整条作废；读不到词表时全部保留 | `engine/tests.rs` 词表只收单字符号 / 词表外的字整条热词作废 / 读不到词表时热词全部保留 | ⚠️ 只有后端；UI 未验「其中 N 个含模型不认识的字」提示 |
| RC-F12 | 录音中和启动中的语言 / ITN 锁定 | `command_tests.rs` recognition_config_is_locked_while_recording；UI `录音中语言与 ITN 锁定` / `启动中语言与 ITN 锁定` / `启动中不发送语言或 ITN 变更` / `停止后语言与 ITN 解锁` | ✅ |

### G. macOS 原生层与打包（5 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-G1 | 侧栏材质、窗口外观、窗口标题跟着主题走 | —— | ❌ **无守卫**：`check:ui` 的假 `__TAURI__` 只提供 `setBackgroundColor`/`startDragging`，缺 `setTheme`/`setTitle`，这些调用在 harness 里静默变成 no-op |
| RC-G2 | macOS 根背景保持透明（材质才透得出来） | UI `macOS 根背景保持透明` | ✅ |
| RC-G3 | 打包版必须带麦克风 entitlement，签名用 ad-hoc | —— | ❌ 无守卫：只能真机验（`npm run build` + `codesign`） |
| RC-G4 | `install_menu` 的 import 必须跟 `cfg`，否则 Windows 编译失败 | —— | ❌ 无守卫：CI 不跑 Windows 编译 |
| RC-G5 | Windows 上侧栏品牌行不能被切掉 | —— | ⚠️ 靠 CSS 人工确认 |

### H. 事件契约与数据流（8 条）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-H1 | 事件载荷增量：分段只带新句，前端自己追加（累计全文在长会话里是 O(n²)） | UI `旧会话分段被忽略` / `旧会话中间结果被忽略` / `取消竞态不写历史` / `取消竞态结果仍为空` | ✅ |
| RC-H2 | 取消后迟到的停止结果不能复活已取消的录音 | UI `取消竞态不写历史` / `取消竞态结果仍为空` | ✅ |
| RC-H3 | 致命错误保留非空部分结果并写历史；非致命错误（缺口提示）用普通提示样式 | UI `麦克风异常保留部分结果` / `麦克风异常写入历史` / `缺口提示不是错误样式` / `缺口提示照常显示` / `部分结果警告不被完成提示覆盖` | ✅ |
| RC-H4 | 下载进度按 `kind` 分流，不串面板 | UI `流式进度显示在流式面板` / `流式进度不串到整段面板` / `整段进度显示在整段面板` / `整段进度不串到流式面板` | ✅ |
| RC-H5 | 文件转写：过期任务退出仍复位、预留失败恢复空闲、转写中拒绝载入历史 | UI `过期文件任务退出仍复位` / `文件预留失败恢复空闲` / `文件预留失败后可录音` / `转写中载入历史被拒绝` | ✅ |
| RC-H6 | 前后端命令名与载荷字段必须一致（`isDefault` / `sessionId` / `warning`） | —— | ❌ **无守卫**：mock 与 Rust 各写一份，改一边不会变红 |
| RC-H7 | 前端 import 图单向、不能成环 | `scripts/check-deps.mjs` | ⚠️ `select.js` / `i18n.js` **不在层表里 → 完全豁免**；且该脚本**没有自测**（另两个门禁都有） |
| RC-H8 | 同一时刻多句不丢（说话→静音→再说话） | UI `说话→静音→再说话：三句都在` | ✅ |

### I. 质量与性能（4 条 · 守卫全空白）

| ID | 不变量 | 守卫 | 状态 |
| --- | --- | --- | --- |
| RC-I1 | 识别质量不得退化（整段 CER、实时性 RTF） | —— | ❌ **无守卫**：`quality_probe.rs` 等 12 个探针只打印不断言，`npm run check` 与 CI 对「识别变差了」零反应 |
| RC-I2 | 流式路径必须有 CI 覆盖 | —— | ❌ **无守卫**：`streaming_smoke` 从未被 workflow 调用；lib 里的流式用例无环境变量时静默跳过 |
| RC-I3 | 长录音不得丢音频（VAD / 整段会话 / 流式会话 / 重采样四条路） | `src-tauri/tests/` 四个长录音探针（只打印不断言） | ⚠️ 需真模型 + 人工读输出 |
| RC-I4 | 内存不随录音时长增长（消费式缓冲） | `audio/tests.rs` take_new_consumes_samples / pending_limit_follows_sample_rate | ⚠️ 只测了队列语义，没测端到端内存曲线 |

---

## 下一轮该审什么

⚠️ + ❌ 共 **21 条**，按「补起来最划算」排序：

### 第 1 优先：补门禁（一次投入，永久生效）

| ID | 要补什么 | 为什么划算 |
| --- | --- | --- |
| RC-H7 | `check-deps.mjs` 的 `LAYERS` 登记 `select.js` / `i18n.js`；补 `check-deps.test.mjs` | 另两个门禁都有自测，它没有；两条正则的事 |
| RC-C2 | 「两模式 VAD 参数一致」的结构门禁 | `check:join` 已经示范了「把两边归一化后比对」的模式，照抄即可 |
| RC-H6 | 前后端命令名与载荷字段的契约校验 | mock 与 Rust 各写一份，改一边不会变红 —— 这是「跨端」缺陷反复出现的根源 |
| RC-D5 | `i18n.js` / `select.js` 必须排在 `app.js` 之前 | `check-i18n.mjs` 已经会查 locales → i18n.js，加一条 → app.js 的规则 |

### 第 2 优先：补 UI 断言（后端已有守卫，只差界面这一侧）

`RC-A2`（下载取消的「已取消」提示）、`RC-A6`（停止超时的超时文案）、`RC-C3`（过短提示带秒数）、
`RC-F11`（术语表词表外字的提示数）—— 各补一条 `check.mjs` 断言即可。

### 第 3 优先：原生层与打包

`RC-G1`（mock 缺 `setTheme` / `setTitle`，导致材质、窗口外观、窗口标题在 harness 里静默 no-op）、
`RC-G3`（麦克风 entitlement）、`RC-G4`（`install_menu` 的 `cfg`）、`RC-G5`（Windows 侧栏品牌行）。
前两条加 mock API / CI 步骤即可；`RC-G3` 只能真机验。

### 第 4 优先：质量门禁（唯一能改变曲线形状的一类）

`RC-I1`（CER / RTF 不得退化）、`RC-I2`（流式路径进 CI）、`RC-I3`（长录音不丢音频）、
`RC-I4`（内存不随时长增长）。其余改动都是把单点缺陷抓得更早，这几条是让「缺陷池变小」。

### 其余

`RC-D7`（文案不带黑话）、`RC-D10`（界面语言 ≠ 识别语言）、`RC-F9`（麦克风权限申请时机）、
`RC-B8` / `RC-B10`（需真模型环境变量，CI 上静默跳过）—— 靠人工审或补文档说明，暂不占门禁位。

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
| 2026-11-03 | —— | —— | **本文档成文**：把上列轮次合并成固定清单（72 条），并标出守卫缺口 |

「当天 fix 数」一列是 `git log` 实测值（`git log --since=<date> --until=<date>` 统计以 `fix` 开头的提交），
不是估算 —— 它同时也是「这轮复审的产出」的度量。
