// check-i18n 门禁自己的测试。
//
// 门禁本身也要有人守：正则是文本级的，漏一个写法就静默放过一个错误码。
// 用法：node --test scripts/check-i18n.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, relative } from 'node:path'

import {
  collectProblems,
  rawErrorLiterals,
  rustErrorCodeEnumKeys,
  rustErrorCodes,
  rustSourceFiles,
} from './check-i18n.mjs'

test('crate:: 前缀的错误码也要扫到', () => {
  const codes = rustErrorCodes('return Err(crate::code_detail("record.tooShort", 0.2));')
  assert.deepEqual(codes, ['record.tooShort'])
})

test('错误码枚举的 key 表要扫到', () => {
  // 错误码是类型了（`ErrorCode::RecordEmpty`），调用点没有字面量 ——
  // 门禁扫的是枚举的「变体 → key」映射。
  const source = `
    impl ErrorCode {
        pub fn key(self) -> &'static str {
            match self {
                ErrorCode::RecordEmpty => "record.empty",
                ErrorCode::RecordMicError => "record.micError",
            }
        }
    }
  `
  assert.deepEqual(rustErrorCodeEnumKeys(source), ['record.empty', 'record.micError'])
})

test('枚举里拼错的 key 门禁要抓住', () => {
  // 枚举的 key 拼错 → 字典里查不到 → 用户看到一串 "record.emptyTYPO"。
  // 这里测**扫描层**（门禁的判定在 collectProblems 里，靠这个函数喂进去）。
  const source = 'impl ErrorCode { fn key(self) { match self { ErrorCode::RecordEmpty => "record.emptyTYPO", } } }'
  const keys = rustErrorCodeEnumKeys(source)
  assert.deepEqual(keys, ['record.emptyTYPO'])
  // 门禁对这些 key 逐个查字典 —— 查不到就报。
  const baseKeys = ['record.empty']
  const missing = keys.filter((key) => !baseKeys.includes(key))
  assert.deepEqual(missing, ['record.emptyTYPO'], '拼错的 key 应当被判定为「字典里没有」')
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

test('i18n.js 写成 type="module" 要报出来', () => {
  const dir = makeFixture({
    'src/index.html':
      '<script src="locales/zh-CN.js"></script>' +
      '<script type="module" src="i18n.js"></script>',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('i18n.js') && problem.includes('type="module"')),
    `写成 module 的 i18n.js 被放过了：${JSON.stringify(problems)}`,
  )
})

test('经典脚本加 async 要报出来（实测会让界面直接挂）', () => {
  const dir = makeFixture({
    'src/index.html':
      '<script src="locales/zh-CN.js"></script>' +
      '<script async src="i18n.js"></script>',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('i18n.js') && problem.includes('async')),
    `加了 async 的 i18n.js 被放过了：${JSON.stringify(problems)}`,
  )
})

test('app.js 丢掉 type="module" 要报出来', () => {
  const dir = makeFixture({
    'src/index.html':
      '<script src="locales/zh-CN.js"></script><script src="i18n.js"></script>' +
      '<script src="app.js"></script>',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('app.js')),
    `丢掉 type="module" 的 app.js 被放过了：${JSON.stringify(problems)}`,
  )
})

test('i18n.js 排在 app.js 之后要报出来', () => {
  const dir = makeFixture({
    'src/index.html':
      '<script src="locales/zh-CN.js"></script>' +
      '<script type="module" src="app.js"></script>' +
      '<script src="i18n.js"></script>',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('i18n.js') && problem.includes('app.js')),
    `排到 app.js 之后的 i18n.js 被放过了：${JSON.stringify(problems)}`,
  )
})

test('经典脚本不用 type="module" 不该报错', () => {
  const dir = makeFixture({
    'src/index.html':
      '<script src="locales/zh-CN.js"></script>' +
      '<script src="select.js"></script><script src="i18n.js"></script>' +
      '<script type="module" src="app.js"></script>',
  })
  const { problems } = collectProblems(dir)
  assert.deepEqual(problems, [])
})

test('走 code()/code_detail() 的不算裸字符串错误', () => {
  assert.deepEqual(rawErrorLiterals('crate::code("record.empty")'), [])
  assert.deepEqual(rawErrorLiterals('crate::code_detail("record.micError", e)'), [])
})

test('裸字符串错误要扫出来', () => {
  assert.deepEqual(rawErrorLiterals('Err("加载流式模型失败".into())'), ['加载流式模型失败'])
  assert.deepEqual(rawErrorLiterals('.ok_or("下载失败")'), ['下载失败'])
  assert.deepEqual(rawErrorLiterals('.ok_or_else(|| "没有音频轨道")'), ['没有音频轨道'])
  assert.deepEqual(rawErrorLiterals('.map_err(|_| "内部错误")'), ['内部错误'])
})

test('后端返回了不在字典里的裸字符串错误要报出来', () => {
  const dir = makeFixture({
    'src-tauri/src/lib.rs': 'fn f() -> Result<(), String> { Err("加载流式模型失败".into()) }',
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('裸字符串错误')),
    `裸字符串错误被放过了：${JSON.stringify(problems)}`,
  )
})

test('字典里有的裸字符串错误不该报出来', () => {
  const dir = makeFixture({
    'src-tauri/src/lib.rs': 'fn f() -> Result<(), String> { Err("record.empty".into()) }',
  })
  const { problems } = collectProblems(dir)
  assert.deepEqual(problems, [])
})

test('文案里有内部黑话要报出来（技术细节只进日志）', () => {
  const dir = makeFixture({
    'src/locales/zh-CN.js':
      "window.I18N_LOCALES = window.I18N_LOCALES || {}\n" +
      "window.I18N_LOCALES['zh-CN'] = { 'app.title': '语音转文字', 'record.empty': '识别线程已停止' }\n",
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('线程')),
    `文案里的黑话被放过了：${JSON.stringify(problems)}`,
  )
})

test('key 里有黑话不算（只查文案的值）', () => {
  const dir = makeFixture({
    'src/locales/zh-CN.js':
      "window.I18N_LOCALES = window.I18N_LOCALES || {}\n" +
      "window.I18N_LOCALES['zh-CN'] = { 'app.title': '语音转文字', 'record.threadExited': '识别已停止' }\n",
  })
  const { problems } = collectProblems(dir)
  assert.deepEqual(problems, [])
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
