/* 历史记录：本地存储 + 列表渲染 */

import { HISTORY_KEY, els, fmtDuration, historyStorageGet, historyStorageSet, state, t } from './core.js'
import { copyText, saveText } from './export.js'

export function loadHistory() {
  return historyStorageGet()
}

export function saveHistory() {
  historyStorageSet(state.history)
}

export function addHistory(transcript, text) {
  state.history.unshift({
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    text,
    at: Date.now(),
    durationSeconds: transcript.durationSeconds || 0,
    elapsedMs: transcript.elapsedMs || 0,
    language: transcript.language || 'auto',
  })
  state.history = state.history.slice(0, 100)
  saveHistory()
  renderHistory()
}

export function renderHistory() {
  els.historyCount.textContent = String(state.history.length)
  els.historyList.innerHTML = ''
  els.historyEmpty.classList.toggle('hidden', state.history.length > 0)
  for (const item of state.history) {
    const li = document.createElement('li')
    li.className = 'history-item'

    const main = document.createElement('button')
    main.type = 'button'
    main.className = 'history-main'
    main.title = t('history.load')
    const text = document.createElement('div')
    text.className = 'history-text'
    text.textContent = item.text
    const sub = document.createElement('div')
    sub.className = 'history-sub'
    const when = new Date(item.at).toLocaleString('zh-CN', {
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
    const parts = [when, fmtDuration(item.durationSeconds || 0), t('result.chars', { count: [...item.text].length })]
    for (const part of parts) {
      const span = document.createElement('span')
      span.textContent = part
      sub.appendChild(span)
    }
    main.append(text, sub)
    main.addEventListener('click', () => {
      els.text.value = item.text
      els.meta.innerHTML = ''
      const span = document.createElement('span')
      span.textContent = t('history.from', { when })
      els.meta.appendChild(span)
      els.text.focus()
    })

    const actions = document.createElement('div')
    actions.className = 'history-actions'
    actions.append(
      iconAction(t('history.copy'), copyIcon(), () => copyText(item.text)),
      iconAction(t('history.save'), saveIcon(), () => saveText(item.text)),
      iconAction(t('history.delete'), trashIcon(), () => {
        state.history = state.history.filter((entry) => entry.id !== item.id)
        saveHistory()
        renderHistory()
      }),
    )

    li.append(main, actions)
    els.historyList.appendChild(li)
  }
}

export function iconAction(label, icon, onClick) {
  const button = document.createElement('button')
  button.type = 'button'
  button.className = 'icon-btn sm'
  button.title = label
  button.setAttribute('aria-label', label)
  button.innerHTML = icon
  button.addEventListener('click', onClick)
  return button
}

export function copyIcon() {
  return '<svg viewBox="0 0 24 24" width="15" height="15"><rect x="9" y="9" width="11" height="11" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M15 5.5A2.5 2.5 0 0 0 12.5 3H6.5A2.5 2.5 0 0 0 4 5.5v6A2.5 2.5 0 0 0 6.5 14" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>'
}

export function saveIcon() {
  return '<svg viewBox="0 0 24 24" width="15" height="15"><path d="M12 4v10m0 0 3.5-3.5M12 14l-3.5-3.5M5 16v3h14v-3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}

export function trashIcon() {
  return '<svg viewBox="0 0 24 24" width="15" height="15"><path d="M5 7h14M10 7V5h4v2m-7 0 .8 12h8.4L17 7" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}

export { HISTORY_KEY }
