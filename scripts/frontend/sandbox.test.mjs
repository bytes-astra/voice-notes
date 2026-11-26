// 沙箱自身的守卫。
//
// 沙箱是这层测试的基础设施 —— **沙箱坏了，测试会静默地测错东西**：
// 比如 `t` 返回 undefined，而断言写成 `assert.equal(x, undefined)` 就恒绿；
// 比如切语言没生效，而断言只测了中文那一半。
//
// 所以这里钉住的是「沙箱本身可信」，不是被测函数的行为。
import { test } from 'node:test'
import assert from 'node:assert/strict'

import { importModule, setupSandbox } from './sandbox.mjs'

test('沙箱：字典真的加载上了', () => {
  const sandbox = setupSandbox()
  const dict = sandbox.i18n.locales['zh-CN']
  assert.ok(dict && typeof dict === 'object', '基础字典没加载上')
  assert.ok(Object.keys(dict).length > 100, '字典条目太少：' + Object.keys(dict).length)
})

test('沙箱：t 是函数，且查得到字典里的 key', () => {
  const sandbox = setupSandbox()
  assert.equal(typeof sandbox.i18n.t, 'function')
  const text = sandbox.i18n.t('duration.minutes', { minutes: 1, seconds: 5 })
  assert.notEqual(text, 'duration.minutes', '查不到字典 —— 返回了 key 本身')
  assert.ok(text.includes('1'), '译文里该有分钟数：' + text)
  assert.ok(text.includes('5'), '译文里该有秒数：' + text)
})

test('沙箱：英文字典也在', () => {
  const sandbox = setupSandbox()
  assert.ok(sandbox.i18n.locales['en'], '英文字典没加载上')
  assert.equal(
    Object.keys(sandbox.i18n.locales['en']).length,
    Object.keys(sandbox.i18n.locales['zh-CN']).length,
    '两种语言的 key 数不该不一样',
  )
})

test('沙箱：切语言对已经 import 的模块生效', async () => {
  // core.js 的 `t` 是顶层解构的引用（`export const t = window.I18N.t`）——
  // 这条守的是「切语言对已有引用有效」，不是靠推理，是实测。
  const sandbox = setupSandbox()
  const core = await importModule('src/core.js')
  sandbox.setLocale('zh-CN')
  const zh = core.fmtDuration(65)
  sandbox.setLocale('en')
  const en = core.fmtDuration(65)
  assert.notEqual(zh, en, '切语言后输出没变 —— 沙箱的切语言没生效')
  assert.ok(!/[一-鿿]/.test(en), '英文界面不该出中文：' + en)
  sandbox.setLocale('zh-CN')
})

test('沙箱：每次 import 拿到不同的模块实例', async () => {
  // Node 的 ES module 缓存按 URL 走 —— 不绕开的话，第二次 import 还是同一个
  // 实例，测试之间没法重新初始化。
  setupSandbox()
  const first = await importModule('src/core.js')
  const second = await importModule('src/core.js')
  assert.notEqual(first, second, '两次 import 拿到同一个实例 —— 缓存没绕开')
})

test('沙箱：document 桩够用（core.js 顶层建 els 不抛异常）', async () => {
  setupSandbox()
  const core = await importModule('src/core.js')
  assert.ok(core.els, 'els 该建出来')
  assert.equal(core.els.app, null, '桩的 querySelector 该返回 null')
})

test('沙箱：localStorage 桩能存能取', async () => {
  setupSandbox()
  const core = await importModule('src/core.js')
  core.historyStorageSet([{ id: 'a', text: '你好', at: 1 }])
  const items = core.historyStorageGet()
  assert.equal(items.length, 1)
  assert.equal(items[0].text, '你好')
})
