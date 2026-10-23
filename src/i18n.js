// 国际化机制：怎么查字典、怎么填 HTML、怎么切语言。
//
// 文案本身不在这里 —— 每种语言一个字典文件（src/locales/*.js），
// 自己在 window.I18N_LOCALES 上注册；这些文件必须在 index.html 里排在
// 本文件之前加载（`<script src="locales/zh-CN.js"></script>`）。
// 加语言 = 加一个字典文件 + 加一行 script，不用改这个文件。
//
// 用法：
//   JS  ：t('toast.copied')  /  t('result.chars', { count })
//   HTML：<span data-i18n="toast.copied"></span>，启动时 apply() 自动填充
//         data-i18n-title / data-i18n-placeholder / data-i18n-aria 同理
;(function () {
  /// 基础语言（兜底）：字典没加载上时用它，也决定「跟随系统」的判定。
  const BASE = 'zh-CN'

  // 全部已加载的字典。没有合并特例：每种语言都是独立文件，key 冲突要在
  // check:i18n 里查出来，不在这里悄悄盖掉。
  const LOCALES = { ...(window.I18N_LOCALES || {}) }

  if (!LOCALES[BASE]) {
    // 基础字典没加载成功 = 界面会显示 key 而不是文案。明确报出来，
    // 别让人以为是逻辑坏了（本地静态服务器偶发加载失败，见 AGENTS.md）。
    console.error(`[i18n] 缺少基础字典 ${BASE}，界面会显示 key 而不是文案`)
  }

  /// 当前语言：一定是字典已加载的那种（否则满屏 key）。
  let current = LOCALES[BASE] ? BASE : Object.keys(LOCALES)[0] || BASE

  function t(key, vars) {
    const dict = LOCALES[current] || {}
    let text = dict[key]
    if (text === undefined) return key
    if (vars) {
      for (const [name, value] of Object.entries(vars)) {
        text = text.split(`{${name}}`).join(String(value))
      }
    }
    return text
  }

  function apply(root) {
    const scope = root || document
    for (const el of scope.querySelectorAll('[data-i18n]')) {
      el.textContent = t(el.dataset.i18n)
    }
    for (const el of scope.querySelectorAll('[data-i18n-title]')) {
      el.title = t(el.dataset.i18nTitle)
    }
    for (const el of scope.querySelectorAll('[data-i18n-placeholder]')) {
      el.placeholder = t(el.dataset.i18nPlaceholder)
    }
    for (const el of scope.querySelectorAll('[data-i18n-aria]')) {
      el.setAttribute('aria-label', t(el.dataset.i18nAria))
    }
  }

  /// 'system' 表示跟随系统语言。
  function resolveLocale(preference) {
    if (preference && LOCALES[preference]) return preference
    const wanted = (navigator.language || BASE).toLowerCase()
    if (wanted.startsWith('zh') && LOCALES[BASE]) return BASE
    // 系统语言没有对应字典：用第一个能用的（英文通常就是它）。
    return Object.keys(LOCALES).find((locale) => locale !== BASE) || current
  }

  function setLocale(locale) {
    const resolved = resolveLocale(locale)
    if (!LOCALES[resolved]) return false
    current = resolved
    document.documentElement.lang = resolved
    apply()
    return true
  }

  window.I18N = {
    t,
    apply,
    setLocale,
    locales: LOCALES,
    get locale() {
      return current
    },
  }
})()
