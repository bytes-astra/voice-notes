/* 主题：浅色/深色、窗口底色、按钮图标 */

import { $, THEME_KEY, t } from './core.js'
import { getCurrentWindow } from './tauri.js'

/// 窗口底色跟着主题走：否则 webview 重载（系统权限弹窗、应用重启）时会闪白。
///
/// 用十六进制字符串：Tauri 2 的 `set_background_color` 收的是 `InnerColor`
/// 枚举，只认 hex 字符串或 {r,g,b,a} 且分量是 u8 —— 传浮点对象会被拒
/// （"data did not match any variant of untagged enum InnerColor"）。
const WINDOW_BG = {
  light: '#f3f4f7',
  dark: '#0b0d10',
}

export function currentTheme() {
  const stored = document.documentElement.dataset.theme
  if (stored === 'dark' || stored === 'light') return stored
  return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
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
export function syncWindowTheme() {
  try {
    const theme = currentTheme() === 'dark' ? 'Dark' : 'Light'
    getCurrentWindow()?.setTheme?.(theme)?.catch?.(() => {})
  } catch {
    /* 忽略 */
  }
}

/// 主题按钮图标跟着当前主题走：浅色显示太阳，深色显示月亮。
const THEME_ICONS = {
  light:
    '<svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><path d="M12 3v2m0 14v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M3 12h2m14 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/><circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>',
  dark: '<svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><path d="M20.5 14.3A8.5 8.5 0 1 1 9.7 3.5a6.6 6.6 0 0 0 10.8 10.8z" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/></svg>',
}

export function renderThemeIcon() {
  const button = $('theme-btn')
  if (!button) return
  const dark = currentTheme() === 'dark'
  button.innerHTML = THEME_ICONS[dark ? 'dark' : 'light']
  const label = dark ? t('theme.toLight') : t('theme.toDark')
  button.setAttribute('aria-label', label)
  button.title = label
}

export function toggleTheme() {
  const next = currentTheme() === 'dark' ? 'light' : 'dark'
  document.documentElement.dataset.theme = next
  localStorage.setItem(THEME_KEY, next)
  syncWindowBackground()
  syncWindowTheme()
  renderThemeIcon()
}
