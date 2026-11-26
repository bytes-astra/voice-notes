// 转写结果的文件名生成 —— `src/export.js` 的 `buildFileName`。
//
// 关键不变量：**按字符切，不能劈开 emoji** —— 半个代理项进到文件名里，
// 后端 JSON 解析会失败。文件名还要去掉 Windows / macOS 的非法字符。
import { test } from 'node:test'
import assert from 'node:assert/strict'

import { importModule, setupSandbox } from './sandbox.mjs'

setupSandbox()
const exporter = await importModule('src/export.js')

/// 文件名 = 「正文 + 时间戳」。断言只关心正文部分 ——
/// 时间戳每次跑都不一样，整条断言没法写死。
///
/// 时间戳自己的形状是 `-20261126-161354`（含一个横杠），所以**不能**用
/// `lastIndexOf('-')` 切 —— 那会把日期和时刻切开。
function headOf(fileName) {
  const cut = fileName.search(/-\d{8}-\d{6}$/)
  assert.notEqual(cut, -1, '文件名末尾该是时间戳：' + fileName)
  return fileName.slice(0, cut)
}

test('文件名：正文里的空白压成单空格并去掉首尾空白', () => {
  const head = headOf(exporter.buildFileName('  你好   世界  '))
  assert.equal(head, '你好 世界')
})

test('文件名：去掉非法字符', () => {
  // Windows 不许的：\ / : * ? " < > |
  //
  // 注意顺序：实现是**先切 18 个字符、再去非法字符** —— 所以带非法字符的
  // 名字会短于 18（这里 19 个字符切到 18，去掉 9 个非法字符后剩 9 个）。
  const head = headOf(exporter.buildFileName('a/b\\c:d*e?f"g<h>i|j'))
  assert.equal(head, 'abcdefghi')
  assert.ok(!/[\/\\:*?"<>|]/.test(head), '还有非法字符没去掉：' + head)
})

test('文件名：正文最长 18 个字符', () => {
  const head = headOf(exporter.buildFileName('一二三四五六七八九十一二三四五六七八九十'))
  assert.equal([...head].length, 18, '正文该是 18 个字符：' + head)
})

test('文件名：按字符切，不劈开 emoji', () => {
  // 18 个 emoji：按字符切是 18 个字符 = 36 个 UTF-16 码元。
  // 用 slice 切就会在第 9 个 emoji 处劈成半个代理项。
  const head = headOf(exporter.buildFileName('😀'.repeat(18)))
  assert.equal([...head].length, 18, '该是 18 个字符：' + head)
  assert.equal(head.length, 36, '该是 18 个代理项（36 个码元）：' + head)
  assert.ok(!/[\uD800-\uDBFF]$/.test(head), '结尾落单了半个代理项：' + head)
  assert.equal([...head].every((ch) => ch === '😀'), true, '不该出现半个 emoji')
})

test('文件名：emoji 与普通字混排也不劈开', () => {
  const head = headOf(exporter.buildFileName('今天 😀 天气 😀 很好 😀 真的 😀 不错 😀 啊'))
  // 逐字符数一遍，确认没有落单的代理项。
  const chars = [...head]
  for (let i = 0; i < chars.length; i += 1) {
    const code = chars[i].codePointAt(0)
    if (code >= 0xd800 && code <= 0xdbff) {
      assert.ok(chars[i + 1] !== undefined, '代理项被劈开了：' + head)
      i += 1
    }
  }
  assert.equal(chars.length, 18, '该是 18 个字符：' + head)
})

test('文件名：正文为空时用兜底名', () => {
  const head = headOf(exporter.buildFileName(''))
  assert.equal(head, '转写')
})

test('文件名：正文全是空白时也用兜底名', () => {
  const head = headOf(exporter.buildFileName('   \n\t  '))
  assert.equal(head, '转写')
})
