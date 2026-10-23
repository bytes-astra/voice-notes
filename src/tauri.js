/* Tauri 插件的唯一解构处：各模块从这里 import，不要各自去摸 window.__TAURI__ */

export const { invoke } = window.__TAURI__.core
export const { listen } = window.__TAURI__.event
export const { open: openDialog } = window.__TAURI__.dialog
export const { writeText } = window.__TAURI__.clipboardManager
export const { openPath } = window.__TAURI__.opener
export const { getCurrentWindow } = window.__TAURI__.window
export const { getCurrentWebview } = window.__TAURI__.webview

/// 记一条日志到应用日志文件（后端已有同名命令）。
/// 界面上的异常和关键操作都走这里，日志文件里就是完整的一份记录。
/// 一律 fire-and-forget：调用方不 await，失败也不影响界面。
export function logEvent(level, message) {
  void invoke('client_log', { level, message }).catch(() => {})
}
