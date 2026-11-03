// check-deps 门禁自己的测试。
//
// 门禁本身也要有人守：正则是文本级的，漏一个写法就静默放过一个违规。
// 用法：node --test scripts/check-deps.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { collectProblems } from './check-deps.mjs'

test('依赖方向反了要报出来', () => {
  const dir = makeFixture({
    'src/app.js': "import { x } from './core.js'\n",
    'src/core.js': "import { y } from './app.js'\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('依赖方向反了')),
    `反向依赖被放过了：${JSON.stringify(problems)}`
  )
})

test('依赖成环要报出来', () => {
  const dir = makeFixture({
    'src/app.js': "import { x } from './core.js'\n",
    'src/core.js': "import { y } from './history.js'\n",
    'src/history.js': "import { z } from './app.js'\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('成环')),
    `成环被放过了：${JSON.stringify(problems)}`
  )
})

test('没登记在层表里的模块要报出来（规则四）', () => {
  const dir = makeFixture({
    'src/app.js': "import { x } from './core.js'\n",
    'src/core.js': "export const x = 1\n",
    'src/utils.js': "export const u = 1\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('utils') && problem.includes('层表')),
    `未登记模块被放过了：${JSON.stringify(problems)}`
  )
})

test('import 经典脚本要报出来（规则三）', () => {
  const dir = makeFixture({
    'src/app.js': "import './i18n.js'\n",
    'src/i18n.js': 'window.I18N = {}\n',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('不能 import 经典脚本')),
    `import 经典脚本被放过了：${JSON.stringify(problems)}`
  )
})

test('import locales 下的字典文件也要报出来', () => {
  const dir = makeFixture({
    'src/app.js': "import './locales/zh-CN.js'\n",
    'src/locales/zh-CN.js': "window.I18N_LOCALES = {}\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('locales')),
    `import 字典文件被放过了：${JSON.stringify(problems)}`
  )
})

test('经典脚本自己写 import 要报出来', () => {
  const dir = makeFixture({
    'src/i18n.js': "import { x } from './core.js'\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('经典脚本')),
    `经典脚本里的 import 被放过了：${JSON.stringify(problems)}`
  )
})

test('多行 import 也要抓到（不能只看单行）', () => {
  const dir = makeFixture({
    'src/app.js': "import {\n  x,\n  y,\n} from './core.js'\n",
    'src/core.js': "import { z } from './app.js'\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('依赖方向反了')),
    `多行 import 没被抓到：${JSON.stringify(problems)}`
  )
})

test('副作用 import 也要抓到', () => {
  const dir = makeFixture({
    'src/app.js': "import './core.js'\n",
    'src/core.js': "import './app.js'\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('依赖方向反了')),
    `副作用 import 没被抓到：${JSON.stringify(problems)}`
  )
})

test('正常依赖图不该报错', () => {
  const dir = makeFixture({
    'src/app.js': "import { x } from './core.js'\n",
    'src/core.js': "import { y } from './tauri.js'\n",
    'src/tauri.js': 'export const y = 1\n',
    'src/i18n.js': 'window.I18N = {}\n',
    'src/locales/zh-CN.js': "window.I18N_LOCALES = {}\n",
  })
  const { problems } = collectProblems(dir)
  assert.deepEqual(problems, [])
})

/// 造一个最小的仓库骨架，让 collectProblems 能跑起来。
function makeFixture(files) {
  const dir = mkdtempSync(join(tmpdir(), 'check-deps-'))
  for (const [file, content] of Object.entries(files)) {
    const path = join(dir, file)
    mkdirSync(join(path, '..'), { recursive: true })
    writeFileSync(path, content)
  }
  return dir
}
