// 前端纯函数的直接断言 —— `src/core.js` 的格式化与拼装。
//
// 与 `scripts/ui/check.mjs` 的分工：那层守**交互结果**（点下去界面上是什么），
// 这层守**纯函数的边界**（进位、跨字符、全角半角）—— 后者要摆一整套界面状态
// 才碰得到一条，恰恰是最容易在重构中被改坏的。
//
// 期望值一律是**手写字面量**，不从被测函数推导 —— 推导出来的期望恒真。
import { test } from 'node:test'
import assert from 'node:assert/strict'

import { importModule, setupSandbox } from './sandbox.mjs'

const sandbox = setupSandbox()
const core = await importModule('src/core.js')

test('时长：秒 / 分秒 / 时分秒', () => {
  assert.equal(core.fmtDuration(0), '0.0 秒')
  assert.equal(core.fmtDuration(12.3), '12.3 秒')
  assert.equal(core.fmtDuration(59.5), '59.5 秒')
  assert.equal(core.fmtDuration(60), '1 分 0 秒')
  assert.equal(core.fmtDuration(65), '1 分 5 秒')
  assert.equal(core.fmtDuration(3599), '59 分 59 秒')
  assert.equal(core.fmtDuration(3600), '1 小时 0 分 0 秒')
  assert.equal(core.fmtDuration(3661), '1 小时 1 分 1 秒')
})

test('时长：秒进位到分', () => {
  // 不足 60 秒走秒分支（一位小数），不进位。
  assert.equal(core.fmtDuration(59.6), '59.6 秒')
  // 119.6 秒：秒四舍五入成 60 → 进位到分 → 2 分 0 秒（不是「1 分 60 秒」）。
  assert.equal(core.fmtDuration(119.6), '2 分 0 秒')
})

test('时长：分进位到时', () => {
  // 59 分 59.6 秒 → 秒进位成 60 → 分进位成 60 → 时。
  assert.equal(core.fmtDuration(3599.6), '1 小时 0 分 0 秒')
})

test('时长：负数归零', () => {
  assert.equal(core.fmtDuration(-5), '0.0 秒')
})

test('计时器：补零与超过一小时', () => {
  assert.equal(core.fmtClock(0), '00:00')
  assert.equal(core.fmtClock(5), '00:05')
  assert.equal(core.fmtClock(65), '01:05')
  assert.equal(core.fmtClock(3599), '59:59')
  assert.equal(core.fmtClock(3600), '01:00:00')
  assert.equal(core.fmtClock(3661), '01:01:01')
})

test('计时器：负数归零', () => {
  assert.equal(core.fmtClock(-5), '00:00')
})

test('字节数：一位小数与整数的分界', () => {
  assert.equal(core.fmtBytes(0), '0 MB')
  assert.equal(core.fmtBytes(1024 * 1024), '1.0 MB')
  assert.equal(core.fmtBytes(99 * 1024 * 1024), '99.0 MB')
  // 100 MB 起不再显示小数。
  assert.equal(core.fmtBytes(100 * 1024 * 1024), '100 MB')
  assert.equal(core.fmtBytes(1024 * 1024 * 1024), '1024 MB')
})

test('拼句：中日韩之间直连，其余加空格', () => {
  assert.equal(core.joinText('你好', '世界'), '你好世界')
  assert.equal(core.joinText('hello', 'world'), 'hello world')
  assert.equal(core.joinText('你好', 'world'), '你好 world')
  assert.equal(core.joinText('hello', '世界'), 'hello 世界')
})

test('拼句：任一边为空时不加空格', () => {
  assert.equal(core.joinText('', 'world'), 'world')
  assert.equal(core.joinText('hello', ''), 'hello')
  assert.equal(core.joinText('', ''), '')
})

test('拼句：缺口占位符前后不加空格（右边是占位符）', () => {
  assert.equal(core.joinText('前面', '[[gap:2]]'), '前面[[gap:2]]')
  // 占位符后面接正文也不加空格 —— 占位符整体是一个单位。
  assert.equal(core.joinText('[[gap:2]]', '后面'), '[[gap:2]]后面')
})

test('拼句：缺口占位符前后不加空格（左边以占位符结尾）', () => {
  assert.equal(core.joinText('前面[[gap:2]]', '后面'), '前面[[gap:2]]后面')
})

test('CJK 判定：各区间与边界', () => {
  assert.equal(core.isCjk('中'), true)
  assert.equal(core.isCjk('日'), true)
  assert.equal(core.isCjk('한'), true)
  assert.equal(core.isCjk('，'), true)
  assert.equal(core.isCjk('Ａ'), true)
  assert.equal(core.isCjk('a'), false)
  assert.equal(core.isCjk('0'), false)
  assert.equal(core.isCjk(' '), false)
  assert.equal(core.isCjk('、'), true)
})

test('缺口占位符：形状判定', () => {
  assert.equal(core.isGapMarker('[[gap:2]]'), true)
  assert.equal(core.isGapMarker('[[gap:12]]'), true)
  assert.equal(core.isGapMarker('普通文本'), false)
  assert.equal(core.isGapMarker(''), false)
  assert.equal(core.isGapMarker(null), false)
})

test('缺口文案：中文全角不加空格', () => {
  sandbox.setLocale('zh-CN')
  const text = core.gapText(2)
  assert.ok(!text.startsWith(' '), '中文缺口文案前面不该有空格：' + JSON.stringify(text))
  assert.ok(!text.endsWith(' '), '中文缺口文案后面不该有空格：' + JSON.stringify(text))
  assert.ok(text.includes('2'), '缺口文案里该有秒数：' + JSON.stringify(text))
})

test('缺口文案：英文半角加空格', () => {
  sandbox.setLocale('en')
  const text = core.gapText(2)
  assert.ok(text.startsWith(' '), '英文缺口文案前面该有空格：' + JSON.stringify(text))
  assert.ok(text.endsWith(' '), '英文缺口文案后面该有空格：' + JSON.stringify(text))
  sandbox.setLocale('zh-CN')
})

test('本地化：把占位符换成当前语言的文案', () => {
  sandbox.setLocale('zh-CN')
  const zh = core.localizeText('前面[[gap:2]]后面')
  assert.ok(!zh.includes('[[gap:'), '占位符没被换掉：' + zh)
  assert.ok(zh.includes('2'), '秒数没保留：' + zh)

  sandbox.setLocale('en')
  const en = core.localizeText('before[[gap:2]]after')
  assert.ok(!en.includes('[[gap:'), '占位符没被换掉：' + en)

  sandbox.setLocale('zh-CN')
})

test('本地化：空文本与无占位符的文本原样返回', () => {
  assert.equal(core.localizeText(''), '')
  assert.equal(core.localizeText('普通文本'), '普通文本')
})

test('本地化：多处占位符按出现顺序取出', () => {
  sandbox.setLocale('zh-CN')
  const texts = core.gapMarkerTexts('甲[[gap:1]]乙[[gap:2]]丙')
  assert.equal(texts.length, 2, '应该取出两段：' + JSON.stringify(texts))
  assert.ok(texts[0].includes('1'), '第一段该是 1 秒：' + JSON.stringify(texts))
  assert.ok(texts[1].includes('2'), '第二段该是 2 秒：' + JSON.stringify(texts))
  assert.deepEqual(core.gapMarkerTexts('没有占位符'), [])
  assert.deepEqual(core.gapMarkerTexts(''), [])
})
