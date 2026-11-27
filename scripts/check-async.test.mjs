// async 阻塞锁门禁自身的测试：`npm run check:async`（其中串了 `node --test` 这一步）
//
// 门禁的作用是「async 路径上出现阻塞式取引擎锁时红」。所以这里测的是门禁
// **能不能抓住那种写法** —— 光看门禁在没漂移时是绿的没用。
//
// 用 node 自带的 test runner（Node 18+），不引第三方依赖。
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { tmpdir } from 'node:os'

import { collectProblems, rustSources } from './check-async.mjs'

const root = join(process.cwd(), 'src-tauri', 'src')
const sources = rustSources(root).map((path) => ({
  path: relative(process.cwd(), path),
  text: readFileSync(path, 'utf8'),
}))

test('没漂移时门禁放行', () => {
  const { problems, summary } = collectProblems(sources)
  assert.deepEqual(problems, [])
  assert.ok(summary.asyncCount > 0, '摘要里应有 async 函数数')
})

test('async 命令里直接阻塞取引擎锁会被抓住', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(state: &State) {
            let guard = state.engine.lock().unwrap();
            let _ = guard.clone();
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, 'async 里直接 state.engine.lock() 仍然通过 —— 门禁漏检')
})

test('async 命令调用同步取锁助手会被抓住', () => {
  // 这正是 shared_engine 那类写法：助手自己不是 async，但被 async 命令调用。
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        fn shared_engine(state: &State) -> Arc<Engine> {
            let guard = state.engine.lock().unwrap();
            guard.clone()
        }

        async fn cmd(state: &State) {
            let _engine = shared_engine(state);
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, 'async 命令调的同步取锁助手仍然通过 —— 门禁漏检')
})

test('助手被 async 命令间接调到也会被抓住', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        fn shared_engine(state: &State) -> Arc<Engine> {
            let guard = state.engine.lock().unwrap();
            guard.clone()
        }

        fn middle(state: &State) -> Arc<Engine> {
            shared_engine(state)
        }

        async fn cmd(state: &State) {
            let _engine = middle(state);
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, '隔一层的同步取锁助手仍然通过 —— 门禁漏检')
})

test('spawn_blocking 里的取锁不会被误报', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(app: &AppHandle) {
            let slot = state.engine.clone();
            let _ = spawn_blocking(move || {
                let mut guard = slot.lock().unwrap();
                *guard = None;
            })
            .await;
        }
      `,
    },
  ])
  assert.deepEqual(problems, [], '阻塞线程里的取锁被误报了')
})

test('try_lock 不算阻塞取锁', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(state: &State) {
            let _cached = state.engine.try_lock().ok().and_then(|g| g.clone());
        }
      `,
    },
  ])
  assert.deepEqual(problems, [], 'try_lock 被当成了阻塞取锁')
})

test('注释和字符串里的内容不会被当成代码', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(state: &State) {
            // 以前这里写的是 state.engine.lock()，别再写回去了
            let note = "state.engine.lock()";
            let _ = note;
        }
      `,
    },
  ])
  assert.deepEqual(problems, [], '注释 / 字符串里的写法被当成了真代码')
})

test('采集器槽与会话槽的锁不受这条规则管', () => {
  // 那两个槽的临界区只有几行，不存在「等模型加载」的长等待 —— 不查它们。
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(state: &State) {
            let _slot = state.recorder.lock().unwrap();
            let _run = state.stream.lock().unwrap();
        }
      `,
    },
  ])
  assert.deepEqual(problems, [], '采集器 / 会话槽被这条规则误报了')
})

test('把旧的 shared_engine 加回真源码会被抓住', () => {
  // 这就是 F4 原来的样子：async 命令调的同步取锁助手。
  const broken = sources.map((source) => {
    if (source.path.endsWith('engine.rs')) {
      return {
        ...source,
        text:
          source.text +
          `
          pub fn shared_engine(state: &AppState) -> Result<Arc<Engine>, String> {
              let guard = state.engine.lock().map_err(|_| code("error.engineState"))?;
              guard.as_ref().cloned().ok_or_else(|| code("error.engineState"))
          }
          `,
      }
    }
    if (source.path.endsWith('lib.rs')) {
      return {
        ...source,
        text:
          source.text +
          `
          async fn legacy_probe(state: &AppState) {
              let _engine = shared_engine(state);
          }
          `,
      }
    }
    return source
  })
  const { problems } = collectProblems(broken)
  assert.ok(
    problems.length > 0,
    '把旧的 shared_engine 加回真源码仍然通过 —— 门禁没抓住 F4 的老写法',
  )
})

// ── 第二条规则：阻塞的构造器不许建在 async 路径上 ──

test('async 命令里直接建 VAD detector 会被抓住', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(vad: VadModelConfig) {
            let _detector = VoiceActivityDetector::create(&vad, 60.0);
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, 'async 里直接建 detector 仍然通过 —— 门禁漏检')
})

test('async 命令里直接建识别会话会被抓住', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(engine: Arc<Engine>) {
            let _session = asr::OfflineSession::new(engine, vad)?;
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, 'async 里直接建会话仍然通过 —— 门禁漏检')
})

test('spawn_blocking 里建会话不会被误报', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(engine: Arc<Engine>) {
            let session = spawn_blocking(move || {
                asr::OfflineSession::new(engine, vad)
            })
            .await
            .map_err(|e| code_detail("error.recordState", e))??;
            let _ = session;
        }
      `,
    },
  ])
  assert.deepEqual(problems, [], '阻塞线程里建会话被误报了')
})

test('专用线程里建 detector 不会被误报', () => {
  // record.rs 的 run_streaming_core 在专用 std::thread 上跑，
  // 那里的 VoiceActivityDetector::create 是允许的。
  //
  // **外层必须是 async**：否则规则对非 async 函数体本来就不报，
  // 这条自测压根触达不到「专用线程豁免」那段代码 —— 成了空测。
  // 真正的形状是：async 命令起一个专用线程，线程里建 detector。
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(config: VadModelConfig) {
            std::thread::Builder::new()
                .name("worker".into())
                .spawn(move || {
                    let detector = sherpa_onnx::VoiceActivityDetector::create(&config, 60.0);
                    let _ = detector;
                });
        }
      `,
    },
  ])
  assert.deepEqual(problems, [], '专用线程里的建 detector 被误报了')
})

test('专用线程的豁免真的在起作用', async () => {
  // 上一条如果把「专用线程」那段豁免去掉，就该开始报错 ——
  // 这才证明那条自测不是空测。
  const source = {
    path: 'fake.rs',
    text: `
      async fn cmd(config: VadModelConfig) {
          std::thread::Builder::new()
              .name("worker".into())
              .spawn(move || {
                  let detector = sherpa_onnx::VoiceActivityDetector::create(&config, 60.0);
                  let _ = detector;
              });
      }
    `,
  }
  // 直接在源码里去掉豁免：把 threadRanges 的判断改成永假。
  const original = readFileSync(new URL('./check-async.mjs', import.meta.url), 'utf8')
  const broken = original.replace(
    'if (inRanges(position, file.threadRanges)) continue',
    'if (false && inRanges(position, file.threadRanges)) continue',
  )
  assert.notEqual(original, broken, '没找到「专用线程豁免」那行 —— 门禁结构变了，自测得跟着改')
  // 把改过的门禁加载进来跑一遍（临时文件，跑完删）。
  const tmp = join(tmpdir(), `check-async-mut-${process.pid}.mjs`)
  writeFileSync(tmp, broken)
  try {
    const mutated = await import(`file://${tmp}`)
    const { problems } = mutated.collectProblems([source])
    assert.ok(
      problems.length > 0,
      '去掉专用线程豁免后仍然全绿 —— 那条自测是空测，测不到东西',
    )
  } finally {
    rmSync(tmp, { force: true })
  }
})

test('async 调同步助手、助手里建会话也会被抓住', () => {
  // 间接写法：构造器不在 async 函数体里，但那个助手是 async 调的 ——
  // 助手就在执行器线程上跑，一样占着执行器。
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        fn helper_session(engine: Arc<Engine>, vad: VadModelConfig) {
            let _session = asr::OfflineSession::new(engine, vad);
        }

        async fn cmd(engine: Arc<Engine>, vad: VadModelConfig) {
            helper_session(engine, vad);
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, 'async 调同步助手建会话仍然通过 —— 门禁漏检')
})

test('同名方法不会把调用点连错定义', () => {
  // 本仓库有 18 个 `fn new` —— 只按名字连边的话，`Type::new(...)` 会连到
  // 某个不相干的 `fn new`（按名字取第一个），门禁就看穿了构造器内部。
  // 这条守住「按限定名连边」：内层的构造点也要能被抓到。
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        impl Alpha {
            fn new() -> Alpha { Alpha }
        }
        impl Beta {
            fn new() -> Beta { Beta }
        }
        impl Wrapper {
            pub fn new(inner: Alpha) -> Wrapper {
                let _detector = VoiceActivityDetector::create(&vad, 60.0);
                Wrapper
            }
        }
        async fn cmd() {
            let _w = Wrapper::new(Alpha::new());
        }
      `,
    },
  ])
  assert.ok(
    problems.length > 0,
    '同名方法下门禁看不穿构造器内部 —— 调用图把 Type::new 连到了别的定义',
  )
})

test('把整段会话的构造挪回 async 路径会被抓住', () => {
  // 这就是 RC-J4 的老样子：会话在 async 命令里直接建，
  // 建 VAD 的那几十毫秒全占着执行器。
  //
  // 改的是**真实调用点**（不是另加一个合成函数）—— 代理测试容易测偏。
  //
  // 匹配用**正则**而不是逐字文本：`cargo fmt` 一改缩进 / 换行，
  // 逐字匹配就匹配不上（测试悄悄退化成「什么都没改」→ 假绿）。
  const broken = sources.map((source) => {
    if (source.path.endsWith('lib.rs')) {
      // `spawn_blocking(move || { asr::OfflineSession::new(engine, vad) })`
      // 或 `spawn_blocking(move || asr::OfflineSession::new(engine, vad))`
      // —— 大括号有没有、缩进多少都随 rustfmt 变，用宽松模式匹配两种形态。
      const pattern =
        /tauri::async_runtime::spawn_blocking\s*\(\s*move\s*\|\|\s*(?:\{[\s\S]{0,120}?)?asr::OfflineSession::new\(engine,\s*vad\)\s*(?:\})?\s*\)/
      if (!pattern.test(source.text)) return source
      return {
        ...source,
        text: source.text.replace(pattern, 'asr::OfflineSession::new(engine, vad)'),
      }
    }
    return source
  })
  // 先确认替换真的发生了 —— 否则下面那句断言测的是「没改坏也抓不到」。
  const replaced = broken.some((source, index) => source.text !== sources[index].text)
  assert.ok(replaced, '没在 lib.rs 里找到那处 spawn_blocking —— 自测本身失效了')
  const { problems } = collectProblems(broken)
  assert.ok(
    problems.length > 0,
    '把会话构造挪回 async 路径仍然通过 —— 门禁没抓住 RC-J4 的老写法',
  )
})

// ── 执行器的 spawn 不是专用线程 ──
//
// `tauri::async_runtime::spawn(async move {…})` / `tokio::spawn(…)` 跑的是
// **执行器上的异步任务**（tauri 的 `async_runtime::spawn` 就是 `tokio::spawn`
// 到全局 tokio runtime）—— 在里面阻塞同样占执行器，**不能豁免**。
//
// 这条是踩过的坑：早期实现用 `(?:\.|::)\s*spawn\s*\(` 找专用线程，
// `::spawn(` 一并命中 → 执行器任务被当成专用线程豁免掉 →
// ① 规则 2 漏报（构造器建在执行器任务里没人管）；
// ② **规则 1 相对更早的版本回退**（引擎槽取锁也不再被报）。
// 仓库里 `asr/cloud/{volcengine,realtime,aliyun}.rs` 就有三处
// `tauri::async_runtime::spawn`。

test('async_runtime::spawn 里建 detector 会被抓住', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(vad: VadModelConfig) {
            tauri::async_runtime::spawn(async move {
                let _d = sherpa_onnx::VoiceActivityDetector::create(&vad, 60.0);
            });
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, '执行器任务里建 detector 仍然通过 —— 门禁漏检')
})

test('tokio::spawn 里建 detector 会被抓住', () => {
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(vad: VadModelConfig) {
            tokio::spawn(async move {
                let _d = sherpa_onnx::VoiceActivityDetector::create(&vad, 60.0);
            });
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, 'tokio 执行器任务里建 detector 仍然通过 —— 门禁漏检')
})

test('async_runtime::spawn 闭包里的引擎槽取锁会被抓住', () => {
  // 规则 1 也要守住：执行器任务里的阻塞取锁跟 async 命令体里的一样糟。
  const { problems } = collectProblems([
    {
      path: 'fake.rs',
      text: `
        async fn cmd(vad: VadModelConfig) {
            tauri::async_runtime::spawn(async move {
                helper(&vad);
            });
        }
        fn helper(vad: &VadModelConfig) {
            let _e = state.engine.lock().unwrap();
        }
      `,
    },
  ])
  assert.ok(problems.length > 0, '执行器任务里的引擎槽取锁仍然通过 —— 门禁漏检')
})

test('专用线程的几种写法都仍然豁免', () => {
  // 豁免侧不能被收得太紧 —— 专用 OS 线程上阻塞是允许的。
  // 四种写法：闭包 / Builder / 函数项 / 无大括号的表达式。
  const shapes = [
    `
      async fn cmd(vad: VadModelConfig) {
          std::thread::spawn(move || {
              let _d = sherpa_onnx::VoiceActivityDetector::create(&vad, 60.0);
          });
      }
    `,
    `
      async fn cmd(vad: VadModelConfig) {
          std::thread::Builder::new().name("w".into()).spawn(move || {
              let _d = sherpa_onnx::VoiceActivityDetector::create(&vad, 60.0);
          });
      }
    `,
    `
      async fn cmd(vad: VadModelConfig) {
          std::thread::spawn(worker);
      }
      fn worker() {
          let _d = sherpa_onnx::VoiceActivityDetector::create(&vad, 60.0);
      }
    `,
    `
      async fn cmd(vad: VadModelConfig) {
          std::thread::spawn(move || make_detector(&vad));
      }
      fn make_detector(vad: &VadModelConfig) -> VoiceActivityDetector {
          sherpa_onnx::VoiceActivityDetector::create(vad, 60.0)
      }
    `,
  ]
  for (const text of shapes) {
    const { problems } = collectProblems([{ path: 'fake.rs', text }])
    assert.deepEqual(problems, [], '专用线程被误报了')
  }
})

test('执行器 spawn 的豁免真的不在（mutation）', async () => {
  // 上面几条如果把「闭包形态」判据放宽回 `spawn(`（执行器任务又被豁免），
  // 就该开始报错 —— 这才证明那几条自测不是空测。
  const original = readFileSync(new URL('./check-async.mjs', import.meta.url), 'utf8')
  const broken = original.replace(
    '/(?:\\.|::)\\s*spawn\\s*\\(\\s*(?:move\\s+)?\\|/g',
    '/(?:\\.|::)\\s*spawn\\s*\\(/g',
  )
  assert.notEqual(original, broken, '没找到「闭包形态」那行正则 —— 门禁结构变了，自测得跟着改')
  const tmp = join(tmpdir(), `check-async-mut-exec-${process.pid}.mjs`)
  writeFileSync(tmp, broken)
  try {
    const mutated = await import(`file://${tmp}`)
    const source = {
      path: 'fake.rs',
      text: `
        async fn cmd(vad: VadModelConfig) {
            tauri::async_runtime::spawn(async move {
                let _d = sherpa_onnx::VoiceActivityDetector::create(&vad, 60.0);
            });
        }
      `,
    }
    const { problems } = mutated.collectProblems([source])
    assert.ok(
      problems.length === 0,
      '把执行器 spawn 重新豁免后仍然报错 —— mutation 没打上，自测测不出东西',
    )
  } finally {
    rmSync(tmp, { force: true })
  }
})
