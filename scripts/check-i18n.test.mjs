// check-i18n 门禁自己的测试。
//
// 门禁本身也要有人守：正则是文本级的，漏一个写法就静默放过一个错误码。
// 用法：node --test scripts/check-i18n.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, relative } from 'node:path'

import { collectProblems, rustErrorCodes, rustSourceFiles } from './check-i18n.mjs'

test('crate:: 前缀的错误码也要扫到', () => {
  const codes = rustErrorCodes('return Err(crate::code_detail("record.tooShort", 0.2));')
  assert.deepEqual(codes, ['record.tooShort'])
})

test('别把不是错误码的调用算进来', () => {
  // encode( / decode( 这种以 code 结尾的函数名不能被误判。
  assert.deepEqual(rustErrorCodes('let x = encode("abc");'), [])
  assert.deepEqual(rustErrorCodes('let x = my_code("abc");'), [])
})

test('子目录里的 .rs 也要算进扫描范围', () => {
  const dir = mkdtempSync(join(tmpdir(), 'check-i18n-'))
  mkdirSync(join(dir, 'record'))
  writeFileSync(join(dir, 'lib.rs'), '')
  writeFileSync(join(dir, 'record', 'tests.rs'), '')
  const files = rustSourceFiles(dir).map((file) => relative(dir, file)).sort()
  assert.deepEqual(files, ['lib.rs', join('record', 'tests.rs')])
})

test('后端用了字典里没有的错误码要报出来', () => {
  const dir = makeFixture({
    'src-tauri/src/lib.rs': 'fn main() { let _ = crate::code("record.notInDict"); }',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('record.notInDict')),
    `门禁没报出缺失的错误码：${JSON.stringify(problems)}`
  )
})

test('字典里有的错误码不该报出来', () => {
  const dir = makeFixture({
    'src-tauri/src/lib.rs': 'fn main() { let _ = crate::code("record.empty"); }',
  })
  const { problems } = collectProblems(dir)
  assert.deepEqual(problems, [])
})

test('字典文件没在 index.html 注册要报出来', () => {
  const dir = makeFixture({
    'src/locales/fr.js':
      "window.I18N_LOCALES = window.I18N_LOCALES || {}\n" +
      "window.I18N_LOCALES['fr'] = { 'app.title': 'V', 'record.empty': 'X' }\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('fr')),
    `没注册的 fr.js 被放过了：${JSON.stringify(problems)}`,
  )
})

test('双引号写的 t() 也要检查', () => {
  const dir = makeFixture({ 'src/app.js': 'const x = t("record.notInDict")' })
  const { problems } = collectProblems(dir)
  assert.ok(problems.some((problem) => problem.includes('record.notInDict')))
})

test('data-i18n 属性名拼错要报出来', () => {
  const dir = makeFixture({
    'src/index.html':
      '<script src="locales/zh-CN.js"></script><script src="i18n.js"></script>' +
      '<i data-i18n-titel="record.empty"></i>',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('data-i18n-titel')),
    `拼错的属性名被放过了：${JSON.stringify(problems)}`,
  )
})

/// 造一个最小的仓库骨架，让 collectProblems 能跑起来。
function makeFixture(files) {
  const dir = mkdtempSync(join(tmpdir(), 'check-i18n-'))
  const defaults = {
    'src/locales/zh-CN.js':
      "window.I18N_LOCALES = window.I18N_LOCALES || {}\n" +
      "window.I18N_LOCALES['zh-CN'] = { 'app.title': '语音转文字', 'record.empty': '没有识别到内容' }\n",
    'src/i18n.js':
      "window.I18N = { t: (key) => key, apply() {}, setLocale() {}, resolveLocale: () => 'zh-CN' }\n",
    'src/index.html': '<script src="locales/zh-CN.js"></script><script src="i18n.js"></script>',
    'src-tauri/src/lib.rs': '',
  }
  for (const [file, content] of Object.entries({ ...defaults, ...files })) {
    const path = join(dir, file)
    mkdirSync(join(path, '..'), { recursive: true })
    writeFileSync(path, content)
  }
  return dir
}
