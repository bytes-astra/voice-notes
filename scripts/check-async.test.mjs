// async 阻塞锁门禁自身的测试：`npm run check:async`（其中串了 `node --test` 这一步）
//
// 门禁的作用是「async 路径上出现阻塞式取引擎锁时红」。所以这里测的是门禁
// **能不能抓住那种写法** —— 光看门禁在没漂移时是绿的没用。
//
// 用 node 自带的 test runner（Node 18+），不引第三方依赖。
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, relative } from 'node:path'

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
