// i18n 体检：
//   1. 各语言字典的 key 必须完全对齐（漏翻 → 界面直接显示 key 名）
//   2. index.html 里引用的 data-i18n* key 必须存在于基础字典
//   3. src/*.js 里 t('key') 引用的 key 也必须存在（漏 key 同样会显示 key 名）
//   4. 后端用的错误码（code("…")）必须在字典里
//   5. t('key', { x }) 传的参数必须真有 {x} 占位符（否则参数被静默丢弃）
//   6. 同一个 key 在各语言里的占位符集合必须一致
// 用法：node scripts/check-i18n.mjs
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { dirname, join, relative } from 'node:path'
import { realpathSync } from 'node:fs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

// 后端错误码的调用形态：code("…") / code_detail("…")。
//
// 前置的 (?<![\w$]) 排除「前面是标识符字符」的情况（encode( / my_code( 不是错误码），
// 但**不能**排除冒号 —— `crate::code_detail("record.tooShort", …)` 这种写法很常见，
// 排掉冒号的话这些错误码就静默漏网了。
const RUST_CODE_RE = /(?<![\w$])code(?:_detail)?\(\s*"([^"]+)"/g

/** 扫出一份 Rust 源码里用到的错误码。 */
export function rustErrorCodes(source) {
  return [...source.matchAll(RUST_CODE_RE)].map((match) => match[1])
}

/** 递归列出目录下的 .rs 文件（子目录里也可能有生产代码）。 */
export function rustSourceFiles(dir) {
  const found = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) found.push(...rustSourceFiles(path))
    else if (entry.name.endsWith('.rs')) found.push(path)
  }
  return found
}

/** 跑一遍体检，返回问题清单和统计（problems 为空 = 通过）。 */
export function collectProblems(root) {
  const problems = []

  // i18n.js / locales/*.js 都是挂到 window 上的浏览器脚本，用假 window 在 Node 里加载
  const fakeWindow = {}
  const load = (file) => {
    const code = readFileSync(join(root, file), 'utf8')
    new Function('window', code)(fakeWindow)
  }

  // 加载顺序跟应用一致：字典文件先来（window.I18N_LOCALES），i18n.js 最后读它们。
  for (const file of readdirSync(join(root, 'src/locales')).sort()) {
    if (!file.endsWith('.js')) continue
    load(join('src/locales', file))
  }
  load('src/i18n.js')

  const locales = { ...fakeWindow.I18N_LOCALES }
  const base = 'zh-CN'
  if (!locales[base]) problems.push(`缺少基础字典：${base}`)
  const baseKeys = Object.keys(locales[base] || {})
  for (const [name, dict] of Object.entries(locales)) {
    if (name === base) continue
    const keys = new Set(Object.keys(dict))
    for (const key of baseKeys) {
      if (!keys.has(key)) problems.push(`${name} 缺少 key：${key}`)
    }
    for (const key of keys) {
      if (!baseKeys.includes(key)) problems.push(`${name} 多出 key（${base} 没有）：${key}`)
    }
  }

  // 字典文件必须在 index.html 里注册，且排在 i18n.js 之前 ——
  // 漏了这行，语言文件永远加载不到，切语言时部分文案不跟着变。
  const html = readFileSync(join(root, 'src/index.html'), 'utf8')
  const scriptTags = [...html.matchAll(/<script[^>]+src="([^"]+)"/g)].map((match) => ({
    src: match[1],
    at: match.index,
  }))
  const registered = scriptTags
    .filter((tag) => tag.src.startsWith('locales/') && tag.src.endsWith('.js'))
    .map((tag) => tag.src.slice('locales/'.length, -'.js'.length))
  for (const name of Object.keys(locales)) {
    if (!registered.includes(name)) {
      problems.push(`src/locales/${name}.js 没有在 index.html 注册（切语言会加载不到）`)
    }
  }
  const i18nTag = scriptTags.find((tag) => tag.src === 'i18n.js')
  if (i18nTag) {
    for (const tag of scriptTags) {
      if (tag.src.startsWith('locales/') && tag.at > i18nTag.at) {
        problems.push(`${tag.src} 必须排在 i18n.js 之前，否则 window.I18N_LOCALES 还读不到`)
      }
    }
  }

  // HTML 里用到的 key 必须在字典里。
  // 属性名必须落在已知后缀里：拼错（data-i18n-titel）不会被 I18N.apply() 认，
  // 界面会永远停在原文 —— 不能静默跳过。
  const ATTR_SUFFIXES = new Set(['', '-title', '-placeholder', '-aria'])
  for (const match of html.matchAll(/\bdata-i18n(-[a-z-]+)?="([^"]*)"/g)) {
    const suffix = match[1] || ''
    if (!ATTR_SUFFIXES.has(suffix)) {
      problems.push(`index.html 里的 data-i18n${suffix} 不是合法属性名（界面不会翻译它）`)
      continue
    }
    if (!baseKeys.includes(match[2])) {
      problems.push(`index.html 引用了不存在的 key：${match[2]}`)
    }
  }

  // JS 里 t('...') 用到的 key 也必须在字典里。
  // 前置的 (?<![\w$.]) 是为了跳过 createElement('li') 这类以 t( 结尾的调用；
  // 带 ${} 的动态模板（t(`lang.${code}`)）无法静态展开，跳过。
  const jsFiles = readdirSync(join(root, 'src')).filter((file) => file.endsWith('.js'))
  const callRe = /(?<![\w$.])t\(\s*(['"])((?:\\.|[^'"\\])*)\1/g
  const staticTemplateRe = /(?<![\w$.])t\(\s*`([^`$]*)`/g
  for (const file of jsFiles) {
    if (file === 'i18n.js') continue
    const code = readFileSync(join(root, 'src', file), 'utf8')
    for (const match of code.matchAll(callRe)) {
      if (!baseKeys.includes(match[2])) problems.push(`src/${file} 引用了不存在的 key：${match[2]}`)
    }
    for (const match of code.matchAll(staticTemplateRe)) {
      if (!baseKeys.includes(match[1])) problems.push(`src/${file} 引用了不存在的 key：${match[1]}`)
    }
  }

  // 后端错误码必须在字典里：后端只返回 key（code() / code_detail()），前端照着翻译。
  // 漏翻的话用户看到的就是一串 "record.micError" 而不是人话。
  const rsDir = join(root, 'src-tauri/src')
  for (const file of rustSourceFiles(rsDir)) {
    const code = readFileSync(file, 'utf8')
    for (const key of rustErrorCodes(code)) {
      if (!baseKeys.includes(key)) {
        problems.push(`${relative(root, file)} 用了不存在的错误码：${key}`)
      }
    }
  }

  // 字典文本里的占位符：{name}。
  const placeholders = (text) =>
    new Set([...String(text).matchAll(/\{(\w+)\}/g)].map((match) => match[1]))

  // t('key', { a, b }) 的顶层参数名。对象字面量里可能嵌套 {}，
  // 只有顶层逗号分出来的才是 t() 的参数，嵌套里的不算。
  const topLevelParams = (body) => {
    const parts = []
    let depth = 0
    let buffer = ''
    for (const ch of body) {
      if (ch === '{') depth++
      else if (ch === '}') depth--
      else if (ch === ',' && depth === 0) {
        parts.push(buffer)
        buffer = ''
        continue
      }
      buffer += ch
    }
    parts.push(buffer)
    return parts
      .map((part) => part.match(/^\s*([A-Za-z_$][\w$]*)\s*:/))
      .filter(Boolean)
      .map((match) => match[1])
  }

  // 5. 传给 t() 的参数必须在字典里有对应占位符。
  // 传了字典里没有的占位符不会报错，只是参数被静默丢掉 —— 界面上就少了一句话。
  const callWithParamsRe = /(?<![\w$.])t\(\s*(['"])((?:\\.|[^'"\\])*)\1\s*,\s*\{([^}]*)\}/g
  for (const file of jsFiles) {
    const code = readFileSync(join(root, 'src', file), 'utf8')
    for (const match of code.matchAll(callWithParamsRe)) {
      const [, , key, body] = match
      const text = locales[base]?.[key]
      if (text == null) continue // key 不存在的情况上面已经报过了
      for (const param of topLevelParams(body)) {
        if (!text.includes(`{${param}}`)) {
          problems.push(
            `src/${file} 传给 t('${key}') 的参数 ${param} 在字典里没有 {${param}} 占位符（会被静默丢弃）：「${text}」`
          )
        }
      }
    }
  }

  // 6. 同一个 key 在各语言里的占位符必须一致：少一个占位符 → 那一语言少一段内容。
  for (const [name, dict] of Object.entries(locales)) {
    if (name === base) continue
    for (const [key, text] of Object.entries(locales[base] || {})) {
      const other = dict[key]
      if (other == null) continue // 缺 key 已经报过了
      const mine = [...placeholders(text)].sort().join()
      const theirs = [...placeholders(other)].sort().join()
      if (mine !== theirs) {
        problems.push(
          `${name} 的 ${key} 占位符和 ${base} 不一致：${base}=「${text}」 ${name}=「${other}」`
        )
      }
    }
  }

  return {
    problems,
    summary: {
      localeCount: Object.keys(locales).length,
      keyCount: baseKeys.length,
    },
  }
}

function main() {
  const { problems, summary } = collectProblems(root)
  if (problems.length) {
    console.error('i18n 检查未通过：')
    for (const p of problems) console.error('  - ' + p)
    process.exit(1)
  }
  console.log(
    `i18n 检查通过：${summary.localeCount} 个语言，共 ${summary.keyCount} 个 key`
  )
}

// 直接运行时才检查（被 import 时不跑 —— 测试要能自己喂数据）。
//
// 两边都取 realpath 再比：`/tmp` 在 macOS 上是 `/private/tmp` 的软链，
// 只比 `process.argv[1]` 会在软链路径下判不出来 → 门禁静默放过（exit 0）。
function isEntryPoint() {
  if (!process.argv[1]) return false
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href
  } catch {
    return false
  }
}

if (isEntryPoint()) main()
