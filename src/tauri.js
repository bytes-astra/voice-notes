/* Tauri 插件的唯一解构处：各模块从这里 import，不要各自去摸 window.__TAURI__ */

export const { invoke } = window.__TAURI__.core
export const { listen } = window.__TAURI__.event
export const { open: openDialog } = window.__TAURI__.dialog
export const { writeText } = window.__TAURI__.clipboardManager
export const { openPath } = window.__TAURI__.opener
