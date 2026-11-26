// 前端纯函数单元测试的沙箱：让 `src/` 的浏览器模块能在 Node 里跑起来。
//
// 为什么需要：`src/core.js` 顶层就取 `window.I18N.t`、`src/tauri.js` 顶层就
// 解构 `window.__TAURI__.core`、`src/core.js` 顶层还建了一份 `els`（摸
// `document`）—— 这些模块**一 import 就会执行顶层代码**，没有 `window` 直接炸。
//
// 为什么字典是真的：`fmtDuration` / `gapText` / `fmtError` 的输出都是查字典
// 得来的。用假字典的话，测的就不是生产行为。
//
// 为什么经典脚本用 `new Function`：`src/i18n.js` 是 IIFE、`src/locales/*.js`
// 是往 `window` 上挂对象的**浏览器脚本**，不是 ES module —— `import()` 会因为
// 没有 export 而报语法错。`scripts/check-i18n.mjs` 早就用这个手法加载它们了
// （同一个仓库里的先例）。
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

const ROOT = join(import.meta.dirname, '..', '..')

/// 装一个够纯函数跑的最小浏览器环境。
///
/// 只补「顶层会摸到的东西」，不做浏览器模拟 —— 纯函数不碰 DOM。
export function setupSandbox({ locale = 'zh-CN' } = {}) {
  // document：core.js 顶层建 els 时要摸，i18n.js 的 setLocale 要写
  // documentElement.lang。
  const doc = {
    documentElement: { lang: locale },
    getElementById: () => null,
    querySelector: () => null,
    querySelectorAll: () => [],
    createElement: () => ({
      className: '',
      textContent: '',
      style: {},
      appendChild() {},
      remove() {},
    }),
  }
  globalThis.document = doc

  // navigator：i18n.js 的 resolveLocale 要 navigator.language。
  //
  // Node 21+ 自带一个**只读**的 navigator（只有 getter），直接赋值会抛
  // TypeError —— 必须 defineProperty。
  Object.defineProperty(globalThis, 'navigator', {
    value: { language: locale },
    configurable: true,
    writable: true,
  })

  // localStorage：core.js 的历史读写要用。
  const store = new Map()
  globalThis.localStorage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  }

  // window：先只有 __TAURI__，字典加载完再补 I18N。
  const win = {
    __TAURI__: {
      core: { invoke: async () => undefined },
      event: { listen: async () => undefined },
      dialog: { open: async () => null, confirm: async () => false },
      clipboardManager: { writeText: async () => undefined },
      window: { getCurrentWindow: () => ({}) },
      webview: { getCurrentWebview: () => ({}) },
    },
  }
  globalThis.window = win

  // 经典脚本按浏览器的顺序加载：字典文件先来（往 window.I18N_LOCALES 注册），
  // i18n.js 最后读它们建 window.I18N。**顺序反了 i18n.js 拿不到字典。**
  const load = (file) => {
    const code = readFileSync(join(ROOT, file), 'utf8')
    new Function('window', code)(win)
  }
  for (const file of ['src/locales/en.js', 'src/locales/zh-CN.js']) {
    load(file)
  }
  load('src/i18n.js')

  win.I18N.setLocale(locale)

  return {
    window: win,
    i18n: win.I18N,
    document: doc,
    /// 切语言。core.js 的 `t` 是顶层解构的引用，但 i18n.js 的 t 闭包读的是
    /// 可变的 `current` —— 所以切语言对**已经 import 的模块**同样生效。
    setLocale: (next) => win.I18N.setLocale(next),
  }
}

/// import `src/` 下的模块。
///
/// 用**带 query 的 URL** 绕开 Node 的 ES module 缓存 —— 不然第二次 import
/// 拿到的还是同一个实例，测试之间没法重新初始化。
export function importModule(relPath, { fresh = true } = {}) {
  const url = pathToFileURL(join(ROOT, relPath)).href
  return import(fresh ? `${url}?v=${Date.now()}-${Math.random()}` : url)
}
