/* 复制 / 保存转写结果（单独成模块：history 和 files 都要用，且不能反向依赖它们） */

import { fmtError, t, toast } from './core.js'
import { invoke, writeText } from './tauri.js'

export async function copyText(text) {
  if (!text || !text.trim()) {
    toast(t('result.noCopy'))
    return
  }
  try {
    await writeText(text)
    toast(t('result.copied'))
  } catch {
    try {
      await navigator.clipboard.writeText(text)
      toast(t('result.copied'))
    } catch {
      toast(t('result.copyFailed'), 'error')
    }
  }
}

export function buildFileName(text) {
  const head = text.replace(/\s+/g, ' ').trim().slice(0, 18).replace(/[\\/:*?"<>|]/g, '')
  const now = new Date()
  const pad = (n) => String(n).padStart(2, '0')
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  return t('history.filename', { head: head || t('view.transcribe'), stamp })
}

export async function saveText(text) {
  if (!text || !text.trim()) {
    toast(t('result.noSave'))
    return
  }
  try {
    const saved = await invoke('save_text', { text, fileName: buildFileName(text) })
    toast(t('result.saved', { path: saved.path }))
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}
