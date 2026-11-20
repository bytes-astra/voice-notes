/* 主题：跟随系统 / 浅色 / 深色（一个按钮循环三态）、窗口底色、按钮图标 */

import { $, THEME_KEY, t } from './core.js'
import { getCurrentWindow, logEvent } from './tauri.js'

/// 窗口底色跟着主题走：否则 webview 重载（系统权限弹窗、应用重启）时会闪白。
///
/// 用十六进制字符串：Tauri 2 的 `set_background_color` 收的是 `InnerColor`
/// 枚举，只认 hex 字符串或 {r,g,b,a} 且分量是 u8 —— 传浮点对象会被拒
/// （"data did not match any variant of untagged enum InnerColor"）。
const WINDOW_BG = {
  light: '#f3f4f7',
  dark: '#0b0d10',
}

/// 主题的三种取值，**按这个顺序循环**：跟随系统 → 浅色 → 深色 → 跟随系统。
export const THEME_MODES = ['system', 'light', 'dark']

/// 系统当前是深色还是浅色（只有「跟随系统」这一档用得上）。
///
/// 不用网页里的 `matchMedia('(prefers-color-scheme: dark)')` 当权威来源：
/// WKWebView 那个信号在系统切深浅色时**不一定会变**（窗口没聚焦、
/// webview 被隐藏时尤其不靠谱）。窗口自己的 `tauri://theme-changed` 才准。
let systemTheme = null

/// 用户选的是哪一种（存 localStorage）。默认「跟随系统」。
export function themeMode() {
  const stored = localStorage.getItem(THEME_KEY)
  return THEME_MODES.includes(stored) ? stored : 'system'
}

/// 当前**实际生效**的主题：「跟随系统」时看系统当前是什么。
export function currentTheme() {
  const mode = themeMode()
  if (mode === 'light' || mode === 'dark') return mode
  if (systemTheme) return systemTheme
  return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

/// 把主题偏好落到 `data-theme` 上。
///
/// 三档都**显式写** `data-theme`（含「跟随系统」那一档）：
/// 样式表里 `:root[data-theme="dark"]` 与
/// `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) }`
/// 两条规则都以它为准 —— 写清楚就不受 WKWebView 那个不可靠的媒体查询摆布。
///
/// 「跟随系统」且**还没读到**系统主题时先不写：让样式表的媒体查询接管，
/// 首帧不会闪（那一档本来就是系统的样子）。
export function applyStoredTheme() {
  const mode = themeMode()
  if (mode === 'system' && !systemTheme) {
    delete document.documentElement.dataset.theme
    return
  }
  document.documentElement.dataset.theme = mode === 'system' ? systemTheme : mode
}
export function syncWindowBackground() {
  // macOS 开了窗口透明 + 原生材质：再设不透明底色会把材质整个盖住。
  if (navigator.userAgent.includes('Mac')) return
  try {
    getCurrentWindow()?.setBackgroundColor?.(WINDOW_BG[currentTheme()])?.catch?.(() => {})
  } catch {
    /* 忽略 */
  }
}

/// 窗口外观（NSWindow.appearance）跟着应用主题走。
///
/// macOS 的原生材质（NSVisualEffectView）明暗跟的是**窗口外观**，不是网页的
/// `data-theme` —— 不同步的话，系统浅色 + 应用暗色 = 浅色材质配深色字，直接看不清。
///
/// **「跟随系统」这一档必须传 `null`**（Tauri 的约定：`setTheme(null)` = 交给
/// 系统决定）。传具体的 `'Light'` / `'Dark'` 会把 **整个 App** 的外观钉死
/// （macOS 上 `setAppearance` 是 App 级的，不是窗口级的）—— 钉死之后系统再切
/// 深浅色就传不进来，界面永远停在启动时那档，「跟随系统」名存实亡。
export function syncWindowTheme() {
  try {
    const mode = themeMode()
    const theme = mode === 'system' ? null : mode === 'dark' ? 'Dark' : 'Light'
    // 失败要记日志：命令被 ACL 拒掉时不会有任何界面提示，静默吞掉等于没调。
    getCurrentWindow()
      ?.setTheme?.(theme)
      ?.catch?.((e) => logEvent('ERROR', `设置窗口外观失败: ${e}`))
  } catch (e) {
    logEvent('ERROR', `设置窗口外观失败: ${e}`)
  }
}

/// 三态各自的图标：**表示当前是哪一档**。
///
/// - 跟随系统：一半浅一半深的圆（浅色 / 深色由系统定）；
/// - 浅色：太阳；
/// - 深色：月亮。
const THEME_ICONS = {
  system:
    '<svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><circle cx="12" cy="12" r="9.6" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M12 6.8a5.2 5.2 0 0 1 0 10.4z" fill="currentColor"/></svg>',
  light:
    '<svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><path d="M12 3v2m0 14v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M3 12h2m14 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/><circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>',
  dark: '<svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><path d="M20.5 14.3A8.5 8.5 0 1 1 9.7 3.5a6.6 6.6 0 0 0 10.8 10.8z" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/></svg>',
}

/// 三态各自的名字（提示文字用）。
const THEME_LABELS = {
  system: 'theme.system',
  light: 'theme.light',
  dark: 'theme.dark',
}

/// 画主题按钮：图标 = 当前档位，提示 = 当前档位的名字。
export function renderThemeIcon() {
  const button = $('theme-btn')
  if (!button) return
  const mode = themeMode()
  button.innerHTML = THEME_ICONS[mode]
  const label = t(THEME_LABELS[mode])
  button.setAttribute('aria-label', label)
  button.title = label
}

/// 换主题偏好：存盘 → 落样式 → 同步窗口外观 → 重画按钮。
export function setThemeMode(mode) {
  if (!THEME_MODES.includes(mode)) return
  localStorage.setItem(THEME_KEY, mode)
  applyStoredTheme()
  syncWindowBackground()
  syncWindowTheme()
  renderThemeIcon()
  // 切回「跟随系统」时重新问一次系统现在是什么：
  // macOS 只在**用户改系统外观**时发通知，我们自己刚解除的外观锁定不会触发它 ——
  // 不主动问一次的话，界面会停在切走前的那一档。
  if (mode === 'system') void refreshSystemTheme()
}

/// 主动问一次窗口现在是什么主题。
async function refreshSystemTheme() {
  try {
    const theme = await getCurrentWindow()?.theme?.()
    if (theme) setSystemTheme(theme)
  } catch (e) {
    logEvent('ERROR', `读取窗口主题失败: ${e}`)
  }
}

/// 主题按钮：按 **跟随系统 → 浅色 → 深色** 的顺序循环。
export function cycleTheme() {
  const index = THEME_MODES.indexOf(themeMode())
  setThemeMode(THEME_MODES[(index + 1) % THEME_MODES.length])
}

/// 记下系统当前的深浅，并按需重画。
///
/// 显式选了浅色 / 深色的用户**不跟**系统走 —— 那是他自己钉死的
/// （但系统主题照样记着：用户切回「跟随系统」时立刻能用上）。
function setSystemTheme(value) {
  systemTheme = String(value || '').toLowerCase() === 'dark' ? 'dark' : 'light'
  if (themeMode() !== 'system') return
  applyStoredTheme()
  syncWindowBackground()
  renderThemeIcon()
}

/// 监听**窗口**的主题变化（`tauri://theme-changed`）。
///
/// 不用网页里的 `matchMedia('(prefers-color-scheme: dark)')`：
/// WKWebView 那个信号在系统切深浅色时**不一定会触发**（窗口没聚焦、
/// webview 被隐藏时尤其不靠谱）—— 窗口自己的事件才准。
///
/// 顺带把**初始值**也读回来（`window.theme()`）：第一次启动时系统主题
/// 还没通过事件来过，得主动问一次。
export async function watchWindowTheme() {
  try {
    const win = getCurrentWindow()
    if (!win) return
    // 初始值：窗口当前是什么主题。
    const initial = await win.theme?.()
    if (initial) setSystemTheme(initial)
    await win.onThemeChanged?.((event) => setSystemTheme(event?.payload))
  } catch (e) {
    // 读不到 / 监听失败只影响「跟随系统」的自动切换 —— 记一笔，不影响别的。
    logEvent('ERROR', `监听窗口主题变化失败: ${e}`)
  }
}
