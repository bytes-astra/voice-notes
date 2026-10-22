/* 历史记录：本地存储 + 列表渲染（A3 再补渲染部分） */

import { HISTORY_KEY, historyStorageGet, historyStorageSet, state } from './core.js'

export function loadHistory() {
  return historyStorageGet()
}

export function saveHistory() {
  historyStorageSet(state.history)
}

export { HISTORY_KEY }
