// 错误码的切分与翻译 —— `src/core.js` 的 `errorCode` / `fmtError`。
//
// 后端只返回字典 key（AGENTS.md「文本分层」），前端负责把它翻成用户看得懂的
// 一句话。**字典里没有的 key 不能把原文漏到界面上** —— 那是这层的核心不变量。
import { test } from 'node:test'
import assert from 'node:assert/strict'

import { importModule, setupSandbox } from './sandbox.mjs'

const sandbox = setupSandbox()
const core = await importModule('src/core.js')

test('错误码：从字符串里切出冒号前的一段', () => {
  assert.equal(core.errorCode('record.empty'), 'record.empty')
  assert.equal(core.errorCode('record.stopFailed:通道关闭'), 'record.stopFailed')
})

test('错误码：对象载荷取 code 字段', () => {
  assert.equal(core.errorCode({ code: 'record.empty' }), 'record.empty')
  assert.equal(core.errorCode({ code: 'record.micError', detail: '打开失败' }), 'record.micError')
})

test('错误码：剥掉 Error: 前缀', () => {
  assert.equal(core.errorCode('Error: record.empty'), 'record.empty')
})

test('错误文案：字典里有的 key 翻成译文', () => {
  sandbox.setLocale('zh-CN')
  const text = core.fmtError('record.empty')
  assert.notEqual(text, 'record.empty', '字典里有的 key 不该原样显示')
  assert.ok(text.length > 0, '译文不该是空串')
})

test('错误文案：带细节的错误也认得出是哪种错误', () => {
  sandbox.setLocale('zh-CN')
  const withDetail = core.fmtError('record.stopFailed:通道关闭')
  const withoutDetail = core.fmtError('record.stopFailed')
  assert.equal(withDetail, withoutDetail, '带细节与不带细节应该是同一句文案')
})

test('错误文案：对象载荷也能翻', () => {
  sandbox.setLocale('zh-CN')
  const text = core.fmtError({ code: 'record.empty' })
  assert.notEqual(text, 'record.empty', '字典里有的 key 不该原样显示')
})

test('错误文案：字典里没有的 key 显示「未知错误」', () => {
  sandbox.setLocale('zh-CN')
  const text = core.fmtError({ code: 'no.suchKey' })
  assert.equal(text, '发生未知错误')
})

test('错误文案：原文不漏到界面上', () => {
  sandbox.setLocale('zh-CN')
  // 原文里带着一个不该出现在界面上的技术细节。
  const text = core.fmtError('/Users/someone/very/private/path.dylib')
  assert.ok(!text.includes('/Users/'), '原文漏到界面上了：' + text)
  assert.ok(!text.includes('private'), '原文漏到界面上了：' + text)
})

test('错误文案：像错误码但字典里没有的 key 也显示「未知错误」', () => {
  sandbox.setLocale('zh-CN')
  // 带点的形态像错误码，但字典里没有 → 仍然不能把原文显示出来。
  const text = core.fmtError('some.unknownKey')
  assert.equal(text, '发生未知错误')
})

test('错误文案：英文界面也翻成英文', () => {
  sandbox.setLocale('en')
  const text = core.fmtError({ code: 'no.suchKey' })
  assert.notEqual(text, '发生未知错误', '切到英文后不该还显示中文')
  assert.notEqual(text, 'no.suchKey', '字典里没有的 key 不该原样显示')
  sandbox.setLocale('zh-CN')
})

// Error 实例**不在契约内**：后端 `Result<T, String>` 回来的是字符串，
// 事件载荷是 `{ code, detail }` 对象。实测 `errorCode(new Error('x'))` 返回
// `''` —— `errorCode` 的对象分支先命中，Error 实例没有 `code` 字段。
// 那条 `error?.message` 兜底因此不可达（现状如此，不是本 change 引入的）。
//
// 这里钉住「Error 实例也不泄露原文」这个**仍然要成立**的性质；
// 「Error 实例该不该被当成错误码」不在契约内，暂不断言。
test('错误文案：Error 实例也不泄露原文', () => {
  sandbox.setLocale('zh-CN')
  const text = core.fmtError(new Error('/Users/someone/private/path.dylib'))
  assert.ok(!text.includes('/Users/'), '原文漏到界面上了：' + text)
})
