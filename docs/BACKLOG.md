# 工程待办（BACKLOG）

> 这份文档记**工程改进项** —— 门禁缺口、结构简化、中期升级。
> 与 [`REVIEW-CHECKLIST.md`](REVIEW-CHECKLIST.md) 分工不同：那份是**不变量 + 守卫**的固定作业面
> （审的是「有没有缺陷漏网」），这份是**「现状不够好」**的改进清单（审的是「能不能更好」）。
> 两份不要合并 —— 前者的条目是「必须守住」，后者的条目是「可以变好」，混在一起会稀释优先级。
>
> **当前状态**：8 条待办（第一档 1 条 / 第二档 1 条 / 第三档 6 条），全部**未开始**；
> 另有 4 条已完成（BL-2 格式门禁、BL-3 录音槽合并、BL-5 云端归一器、BL-12 经典脚本技术债的处置口径，均 2026-11-24）。
> 登记于 2026-11-23，来自一次全库分析（未改任何生产代码）。

## 怎么用

1. **每轮开工前**读一遍这份，挑一条做 —— 优先级就是下面的分档顺序。
2. **做之前**先立 OpenSpec change（见 [`../AGENTS.md`](../AGENTS.md)「OpenSpec 规划」）：
   这些改动多数动的是共享结构（`AppState`、错误码、门禁），先有 spec 再动代码。
3. **做完**回来把状态改成 ✅ 并写上「改了什么 / 在哪 / 怎么验的」，别删条目。
4. **发现新的**追加到对应档，编号续着往后（`BL-13`、`BL-14`……），别复用编号。

编号 `BL-*` 与 `RC-*` 两套，互不通用：`BL` 是改进项，`RC` 是不变量。

---

## 第一档：补防线（成本低，收益直接）

> BL-2 已完成（2026-11-24）；BL-1 暂不接 CI（理由见该条）。

### BL-1 · 把 `check:ui` 接进 CI ⬜

**现状**：`scripts/ui/check.mjs` 有 **239 项**界面断言（光标位置、撤销栈、云端列表行复用、
编辑区就地展开、默认窗口尺寸无纵向滚动条、焦点不落到 body……）。`check.yml` 的 frontend job
只跑六道纯 Node 门禁（i18n / deps / join / contract / bundle / async）—— **`check:ui` 不在里面**，
因为它要 Playwright WebKit。界面层的回归目前只靠本地手动跑。

**2026-11-24 决定：CI 暂不接。** 理由：接进 CI 的前置条件是**界面 mock 与真后端语义对齐**
（mock 更宽松 = 真实缺陷被挡住，CI 一旦变绿就给人「界面层已经守住」的错觉），而
RC-K25 / RC-K26 只对齐了三条语义（空 key = 不改 / 删活动项落点 / 未知 id 忽略），其余字段
**尚未用 mutation 逐条验过**。在验完之前接进 CI，只会把「mock 宽松」固化成「CI 绿」。
下一步要做的是那条 mutation 核验（见下面「做之前先立 change」的顺序），核验通过再谈接 CI。

**改法**（核验通过后）：frontend job 换 `runs-on: macos-latest`，加 `npx playwright install webkit` +
`npm run check:ui`；或单开一个 macOS 的 `ui` job 与前端门禁并行（推荐 —— 不拖慢纯 Node 那条）。
**注意**：浏览器二进制不在 npm 包里，CI 上必须显式装（`playwright` 只装驱动）。

**前置条件（重要）**：`scripts/ui/check.mjs` 的假 `__TAURI__` mock 必须先与真后端对齐。
RC-K25 已经做过一轮对齐（空 key = 不改 / 删活动项落点 / 未知 id 忽略），但那是**语义**对齐；
接进 CI 前要用 `mutation` 再验一遍 mock 与真后端在**其余**字段上是否一致（`check:contract`
已经查了 `Status` / `InputDevice` 的字段名 —— 实测两侧都是 34 个字段、无差集 —— 但查不到
**语义**漂移）。

**验证**：`npm run check:ui` 239/239；CI 上 WebKit 装好后跑通一次。

### BL-2 · 加 `cargo fmt --check` 门禁 ✅ 2026-11-24

**现状**：CI 有 clippy `-D warnings` 当门禁，**格式不检查**；`npm run check` 里也没有。

**改法**：
- CI（`check.yml` 的 rust job）：`cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
- `package.json` 加 `check:fmt`，并挂进 `npm run check` 的链里。

**注意**：`cargo fmt` 会重排整个仓库的 Rust 文件 —— 第一步是**先跑一遍 `--check` 看差多少**。
差得多就分两步走：先在一次专门的提交里格式化（不改逻辑），再挂门禁。

**做完了（2026-11-24）**：
- **格式化**：实测差异 **46 个文件 / 220 处** → 跑 `cargo fmt --all`，**单独成提交**
  （`3de5719`，只重排空白、换行与尾逗号，不改语义 —— 逐文件比对「去掉空白与注释后的
  token 流」确认差异只有尾逗号与 `{id}` → `{ id }` 这类 rustfmt 惯用形式）。
- **门禁**：`package.json` 加 `check:fmt`，挂进 `npm run check`（排在 `check:rust` 前 ——
  格式先过，秒级；再编译，分钟级）。**CI 没动**（与 BL-1 同批决定：CI 暂不接这两道）。
- **顺带修掉一个真缺陷**：`scripts/check-async.test.mjs` 里「把整段会话的构造挪回 async 路径
  会被抓住」那条自测用**逐字文本**匹配 `spawn_blocking` 块 —— `cargo fmt` 改掉缩进 / 换行后
  匹配不上，那条自测会**悄悄退化成「什么都没改」**（假绿）而不是报错。改成宽松正则 +
  一条断言确认替换真的发生了。
- **验证**：`npm run check` exit 0（八道：六道前端门禁 + `check:fmt` + clippy + 300 条 Rust 单测）；
  `npm run check:ui` **239/239**（格式化没影响界面行为）；`cargo fmt --check` 单独验过
  能变红（改一处缩进 → exit 1 并指出 `record.rs:163`）。

---

## 第二档：结构简化（收益明确，但要动共享结构）

### BL-3 · 四个录音槽合并成一个 ✅ 2026-11-24

**做完了**：四个槽（采集器 + 流式 / 整段 / 云端三个会话槽）→ **一个** `Session { recorder, run, mode }`。
- **核实后的关键事实**（立项时才发现）：三个会话槽装的**全是 `RecordingRun`**（停止信号 + 结果通道）——
  真正的识别会话对象在「开始录音」时就被移进识别线程的闭包里了，槽里并没有会话。
  而三个槽类型完全相同、`register_slots` 第二参数是泛型 → **写错槽编译不报错**，也没测试盯
  （守卫全是泛型 `u8` 测试）。后果是停止路径走错分支：超时秒数（20/20/30）、失败文案、
  云端那套「断连保句」全错。
- 合并后：登记**一次写**（「两槽一起登记」的双锁协议随之消失）；`finish` / `cancel` 收成 `Session` 的方法
  →「先停采集、再停线程」的顺序由调用方排不错；`Session` 的 `Drop` 兜底置 `stop`/`discard`
  （`Recorder` 的 `Drop` 只关麦、**不置 `stop`** → 不兜底会「麦克风关了、线程空转」）；
  `stop` / `cancel` 各拿一半的 split-brain 顺带消掉（单槽 take 后要么全拿到、要么全拿不到）。
- **顺带改名**：三个编排函数原本 `run_offline` / `run_streaming_core` / `run_cloud_streaming_core`
  —— **前两个都是本地跑的**，只有第三个不是，名字把「本地 vs 云端」藏起来了。改成
  `run_local_offline` / `run_local_streaming` / `run_cloud_streaming`（`SessionMode` 变体同步）。
  `asr.rs` 的 `OfflineSession` / `OfflineEvent` **不动** —— 那是 sherpa-onnx 的官方术语
  （offline ASR = 整段识别器），跟随上游才不错位。
- **守卫**：RC-J1 的五条改成守同一件事的新形态；另补两条 mutation 可验的 ——
  「护句尾的 stop 顺序」（真麦克风测试默认跳过，跑不到等于没守卫 → 用源码级守卫）与
  「每个 `start_*` 登记的 mode 与它跑的识别核心一致」。**四条 mutation 均已验证变红**。
- **验证**：`cargo test --lib` 300 → **303 passed**；`npm run check` 全绿；`npm run check:ui` **239/239**。
- **选型经两份独立复核**（Codex + Claude agent）：两份都推荐单槽；复核指出了原 design 的两处论证错误
  （「今天有类型保证」不成立；耦合点是 `Recorder`↔`SampleQueue`↔ASR 线程而非 run）与一条漏掉的耦合
  （`RecordingRun` 没有 `Drop`）。

**现状**：`engine.rs` 的 `AppState` 有四个录音相关槽 ——
`recorder`（采集器）、`stream`（流式会话）、`offline_session`（整段会话）、`cloud_session`（云端会话），
外加两套下载标志（`downloading` / `downloading_streaming`）。

**动机**：`record.rs` 的 `register_slots` 要求「采集器槽与会话槽**一起**登记」，
因为只写一个会让识别线程永远收不到 stop（线程泄漏、麦克风不关、语言/ITN 永久锁死）——
**这个坑本身就是多槽设计的成本**。RC-J1 靠「先拿齐两把锁再写」把它守住了，但代价是
每个 `start_*` 都要维护这个不变量。

**改法**：单个 `session: Option<Session>`，`Session` 带 `mode: enum { Offline, Streaming, Cloud }`，
登记变成一次原子操作。

**代价与风险**：`lib.rs` 的 `stop_recording` 现在是三分支（流式 / 云端 / 整段），
要重写；`finish_recording` 的超时语义**分模式不同**（流式 20 秒、整段 30 秒），合并时别抹平。
**收益与风险都中等，需要一整段不被打断的时间。**

**验证**：改完重跑 RC-J1 的五条守卫（`record/tests.rs`）+ `cargo test --lib` 300 条 +
`npm run check:ui` 239 项。

### BL-4 · 错误码从字符串改成类型 ⬜（**已立项**：OpenSpec change `2026-11-24-error-code-type`，2026-11-24）

> **已立 change，未动代码。** 规模比预想的大：**168 处调用点 / 75 个唯一 key**，
> 而且**门禁要跟着改** —— `check:i18n` 的后端错误码扫描靠字面量（`code("…")`），
> 改成枚举后扫不到，**门禁会静默失效**（拼错的 key 就没人拦了）。
> 新扫描方式见 change 的 design D4：改成扫**枚举定义**，反而更强。
> 待拍板项（枚举形状 / 比较逻辑的做法）写在 change 的 tasks 0.1–0.2。

**现状**：`crate::code("record.micError")` / `code_detail("record.micError", e)` 返回
`"record.micError"` / `"record.micError:细节"`，**靠 `:` 分隔文本**再 `split(':')` 判断类型
（`record.rs` 的 `is_stop_failed` 就是这么写的）。字符串拼接当错误码用，编译器拦不住拼错的 key。

**改法**：定义 `ErrorCode` 枚举 + `Display`（输出格式不变，对外契约不动），
`is_stop_failed` 改成枚举比较。

**为什么值得做**：`check:i18n` 能查 key 在不在字典里，**查不了比较逻辑用错了 key** ——
`is_stop_failed` 拼错成 `record.stopFaild` 时，守卫不会红，只在真机上超时文案换不上。

**验证**：改完 `cargo test --lib` 300 条（`record/tests.rs` 的 `stop_failed_is_recognised_with_and_without_detail`
等直接覆盖这条）+ `npm run check:i18n`（后端错误码扫描要跟着调整扫描方式）。

### BL-5 · 云端中间结果归一器：让火山也走，或收窄抽象 ✅ 2026-11-24

**现状**：`asr/cloud/partial.rs` 是通用的归一器（三种归一档 `Delta` / `Preview` / `Range`），
但**只有 Realtime 一个消费者** —— 火山与阿里都绕过它，映射器直接产出 `CloudEvent::Partial`。
当前状态是「抽象成本已付、收益未收」。

**两条路，二选一**：
- **A：让火山也走归一器** —— 火山是 `Preview` 语义（确认前缀为空、整句就是草稿），
  映射很直接。好处是三家统一走一条路，将来接讯飞（`Range`）/ 百度（`Preview`）有现成的路。
- **B：把归一器降级成 Realtime 内部实现** —— 少一层抽象。代价是将来接第四家时要重新抽。

**建议 A**（协议形态是硬事实，`CLOUD.md` 已经把三家的形态列成表了，归一器本来就是为这个设计的）。

**做完了（2026-11-24）· 选了 A 的「只做火山」**：
- 火山的映射器不再直接产 `CloudEvent::Partial`，改为产 `VolcengineEvent::Partial(PartialUpdate::Preview { confirmed: "", draft: 整句 })`
  —— 火山的协议形态本就是「确认前缀为空、整句都是草稿」；provider 层建 `PartialAccumulator`（火山无句 id → **单桶**）做归一，
  定稿时清桶。**对外行为逐字不变** —— 改的是「谁做归一」，不是「归一成什么」。
- **阿里不动**：它的 `stash` 属于**下一句**的 `sentence_id`，与火山的单桶形态结构上不同（要归一阿里得处理交错语义）。
- `PartialUpdate::Preview` 的 `#[allow(dead_code)]` 解除 —— 火山在非测试代码里真构造它了。
- **mutation 验证查出两条既有守卫是空测**：
  ① 「`Preview` 的 confirmed/draft 互换」守不住（归一器是 `format!("{confirmed}{draft}")`，两种拼法结果相同 —— **这条守不住是对的**，它不是缺陷）；
  ② 「映射器改用 `Delta` 档」只有 `indefinite_sentence_maps_to_partial` 变红，**核心守卫 `partial_is_a_whole_sentence_replacement` 反而全绿**
  （`Preview` 与 `Delta` 在「第二次比第一次短」的样例上表现一样）→ 补两条新守卫：
  `mapper_emits_the_declared_normalization_tier`（直接看映射器产出的**归一档**）与
  `finalized_sentence_clears_the_normalizer_bucket`（直接测归一器的**桶状态** —— 火山 `Preview` 档是整句替换，残留在文本上看不出来）。
  两条新守卫都经 mutation 验证承重（`Preview`→`Delta`、不清桶，各自变红）。
- **验证**：`cargo test --lib` 300 → **302 passed**；`npm run check` 全绿（含新增的 `check:fmt`）；`npm run check:ui` **239/239**。

---

## 第三档：中期升级

### BL-6 · 云端模式补质量基线 ⬜

**现状**：README「已知限制」明写：云端模式**没有质量门禁，也没有实测 CER 基线** —— 本地模式才有。
本地侧有 `src-tauri/tests/quality_gate.rs`（整段 CER ≤ 25%、流式 ≤ 55%、RTF ≤ 1.0）、
语料脚本（`fetch-quality-corpus.mjs` / `gen-tts-corpus.sh`）、HOW_TO_VERIFY 里的实测 CER 表；
云端侧**质量这一维完全空白**（协议层有 100+ 单测，但那是「帧解得对不对」，不是「认得准不准」）。

**为什么现在做**：服务端断句族已知「终点偏长」（火山补静音后差 **1.28 s**、阿里 **1.71 s**，
真机 3 轮 12 组数字逐位一致，见 [`CLOUD.md`](CLOUD.md)）。这些数字现在只是文档里的一张表，
**没变成断言** —— 下次改协议时没人会红。

**改法**：给 `quality_gate.rs` 加云端变体（或另起 `cloud_quality_gate.rs`）：同一批 TTS 语料
喂三家，对比 CER 与本地基线的差值，设一个宽松阈值。**真 key 不入库、不进日志**，
凭据走环境变量（`VOICE_NOTES_VOLCENGINE_KEY` / `VOICE_NOTES_ALIYUN_KEY`），
没设就跳过 —— 与 `live_smoke` 同一套口径。

**注意**：服务端识别结果**有随机性**（同一段音频两次运行文本可能不同），
CER 断言要给足余量，或者比「相对本地基线的差值」而不是绝对值。

### BL-7 · 评估提升 Rust MSRV ⬜

**现状**：`Cargo.toml` 声明 `rust-version = "1.78"`，**两个依赖被它卡住**：

| 依赖 | 现在 | 目标 | 目标版的要求 |
| --- | --- | --- | --- |
| `symphonia` | 0.5 | 0.6 | Rust **1.85** |
| `tokio-tungstenite` | 0.28 | 0.30 | Rust **1.85** |

`symphonia` 是后端解码器（mp3 / wav / flac / ogg / m4a / mp4 / mov），
`tungstenite` 是云端 WebSocket 客户端 —— 两个都是不能丢的核心路径。

**为什么现在评估**：1.85 已经是 2024 年的稳定版，两年过去还在 1.78 上意味着**主动放弃了**
所有需要新 MSRV 的 crate 版本。这不是「现在有问题」，是**未来的阻塞点**。

**风险**：`symphonia` 0.6 改过 API，`tungstenite` 0.30 动过 TLS 配置 ——
两个都要带真模型 / 真服务跑一遍，不是改个版本号就完事。

**建议**：单独排一次评估（升级 + 全量 `cargo test --lib` + `check:ui` + 三家 provider 的真机冒烟），
不要和别的改动混在一个提交里。

### BL-8 · 前端补纯函数单元测试层 ⬜

**现状**：前端只有门禁脚本与界面断言，**没有纯函数的单测层**。
`core.js` 的 `joinText` 被 `check:join` 守着（两份实现逐条比对），
但 `fmtDuration` / `fmtBytes` / `pctOf` / `localizeText` / `mask` 这类纯函数**没有直接断言**。

**改法**：用 Node 内置的 `node:test`（仓库已经在用 —— `scripts/check-*.test.mjs` 都是它），
把 `src/` 里的纯函数抽到一个可被 Node 直接 import 的位置（或加一个薄适配层），
逐个补断言。**不引入打包器** —— 这是 `FRONTEND.md` 的硬约束。

**优先级最低**：这些函数的输入域都很窄、界面断言间接覆盖了一部分。
只有当某个函数出过一次错、且当时没被任何守卫抓住时，才把它排上来。

### BL-9 · 拆分 `src/style.css` ⬜

**现状**：`src/style.css` **1358 行**，是前端最大的单文件（`status.js` 805、`app.js` 498、
`core.js` 369 次之）。

**改法**：按「布局 / 组件 / 主题变量 / macOS 原生层」拆成几份 `@import`。

**代价**：`@import` 会让 `check:ui` 的截图需要重新校一遍，`npm run shots` 的 10 张 README 截图
可能要重出。收益是可维护性 —— **优先级低，不急**。

### BL-10 · 日志轮转多留几份 ⬜

**现状**：`log.rs` 超过 1 MB 轮转一次，旧文件留成 `voice-notes.log.1` —— **只留一份**。

**动机**：复审台账里出现过一次「凭据进日志」的排查（RC-K6 的日志那半边补了两轮守卫）。
真要复现「某个时刻日志里写了什么」，**一份轮转文件往往不够**。

**改法**：轮转保留 3–5 份（`voice-notes.log.1` … `.5`），或加日期后缀。
注意体积上限 —— 用户反馈问题时会整份发出来，几十 MB 的日志没人看。

### BL-11 · 摸底 Linux 支持现状 ⬜

**现状**：CI 注释里明写「Rust 端在 Linux 上还没验证过（cpal 要 ALSA 头文件）」；
README 的 Linux 一节是「CI 不出 Linux 包，需要自己构建」。**Linux 是完全未验证的平台**。

**改法**：一次性摸底（`cargo check --target x86_64-unknown-linux-gnu` + ALSA 依赖 +
能否出包），把结论写进 README —— **哪怕结论是「不支持」**，也比现在「不知道」强。
不做就只是文档措辞，改了也不影响 macOS / Windows 用户。

### BL-12 · 经典脚本技术债的处置口径 ✅ 2026-11-24

**现状**：`i18n.js` / `select.js` 必须保持**经典脚本**（不能 import、不能加 `async`、
不能写成 `type="module"`），三条硬约束由 `check:i18n` 守着。`FRONTEND.md` 里写明了
每条约束背后的实测后果（界面直接挂 / 执行两遍 / `window.I18N` 被换实例）。

**判断**：**收益不足以支撑现在动它。** 改动会牵动「脚本加载顺序 = 决定成败」的历史坑，
而 `check:i18n` 已经把三条规则守住了。

**什么时候才做**：哪天真要上前端打包器 / TypeScript —— 那时整体重写前端模块系统更划算，
而不是在经典脚本上打补丁。

**这条的价值是**「记下判断依据，免得下次有人重新提一遍」。**2026-11-24 归档为「已核实不成立」**
—— 它不是待办，是一条**已经做完的决定**（决定 = 维持现状 + 写下依据）。

---

## 已核实不成立（别再提）

- **`mask_secret` 泄露比例** —— 已修（RC-K24，2026-11-20）：现在是头尾各露
  `min(4, 长度 / 4)`，守卫 `settings/cloud_tests.rs::mask_secret_never_reveals_more_than_half`
  覆盖 1/2/3/4/5/8/9/10/16/20/40 十一档。原「9 字符处从露 2 跳到露 8 = 89%」是修复前的现象。
- **`check:join` 没守住拼句规则** —— 守住了，且是逐条比对**判定结构**（不是子串），
  `check-join.test.mjs` 14 条变异测试。
- **界面检查的 mock 与真后端语义不一致** —— RC-K25 已对齐三条（空 key = 不改 /
  删活动项落点 / 未知 id 忽略），`check:contract` 另外查字段名。
  （**语义**漂移仍可能藏在别的字段上 —— 见 BL-1 的前置条件。）

---

## 相关文档

- 固定作业面（不变量 + 守卫，每轮复审从这里开始）：[REVIEW-CHECKLIST.md](REVIEW-CHECKLIST.md)
- 验证手段、基线、探针跑法：[HOW_TO_VERIFY.md](HOW_TO_VERIFY.md)
- 工程约定、模块边界、发布红线：[../AGENTS.md](../AGENTS.md)
