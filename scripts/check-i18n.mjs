// i18n 体检：
//   1. 各语言字典的 key 必须完全对齐（漏翻 → 界面直接显示 key 名）
//   2. index.html 里引用的 data-i18n* key 必须存在于中文字典
//   3. src/*.js 里 t('key') 引用的 key 也必须存在（漏 key 同样会显示 key 名）
// 用法：node scripts/check-i18n.mjs
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const problems = []

// i18n.js / locales/*.js 都是挂到 window 上的浏览器脚本，用假 window 在 Node 里加载
const fakeWindow = {}
const load = (file) => {
  const code = readFileSync(join(root, file), 'utf8')
  new Function('window', code)(fakeWindow)
}

load('src/i18n.js')
const locales = { 'zh-CN': fakeWindow.I18N.locales['zh-CN'], ...fakeWindow.I18N_LOCALES }
for (const file of readdirSync(join(root, 'src/locales'))) {
  if (!file.endsWith('.js')) continue
  load(join('src/locales', file))
  Object.assign(locales, fakeWindow.I18N_LOCALES)
}

const base = 'zh-CN'
const baseKeys = Object.keys(locales[base])
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

// HTML 里用到的 key 必须在字典里
const html = readFileSync(join(root, 'src/index.html'), 'utf8')
for (const match of html.matchAll(/data-i18n(?:-title|-placeholder|-aria)?="([^"]+)"/g)) {
  if (!baseKeys.includes(match[1])) problems.push(`index.html 引用了不存在的 key：${match[1]}`)
}

// JS 里 t('...') 用到的 key 也必须在字典里。
// 前置的 (?<![\w$.]) 是为了跳过 createElement('li') 这类以 t( 结尾的调用；
// 带 ${} 的动态模板（t(`lang.${code}`)）无法静态展开，跳过。
const jsFiles = readdirSync(join(root, 'src')).filter((file) => file.endsWith('.js'))
const callRe = /(?<![\w$.])t\(\s*'([^'\\]+)'/g
const staticTemplateRe = /(?<![\w$.])t\(\s*`([^`$]*)`/g
for (const file of jsFiles) {
  if (file === 'i18n.js') continue
  const code = readFileSync(join(root, 'src', file), 'utf8')
  for (const match of code.matchAll(callRe)) {
    if (!baseKeys.includes(match[1])) problems.push(`src/${file} 引用了不存在的 key：${match[1]}`)
  }
  for (const match of code.matchAll(staticTemplateRe)) {
    if (!baseKeys.includes(match[1])) problems.push(`src/${file} 引用了不存在的 key：${match[1]}`)
  }
}

if (problems.length) {
  console.error('i18n 检查未通过：')
  for (const p of problems) console.error('  - ' + p)
  process.exit(1)
}
console.log(`i18n 检查通过：${Object.keys(locales).length} 个语言，共 ${baseKeys.length} 个 key`)
