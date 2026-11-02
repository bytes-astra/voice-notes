/* 历史记录：本地存储 + 列表渲染 */

import { I18N, els, fmtDuration, gapMarkerTexts, historyStorageSet, localizeText, renderResultMeta, state, t } from './core.js'
import { copyText, saveText } from './export.js'
import { copyIcon, saveIcon, trashIcon } from './icons.js'

/// 载入历史后要切回结果页 —— 但 history.js 不能 import app.js（依赖方向）。
/// 由 app.js 在绑定界面时注入。
let loadHandler = null
export function setHistoryLoadHandler(fn) {
  loadHandler = fn
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

    // 历史存的是原文，缺口占位符在渲染时才翻成界面语言。
    const shown = localizeText(item.text)

    const main = document.createElement('button')
    main.type = 'button'
    main.className = 'history-main'
    main.title = t('history.load')
    const text = document.createElement('div')
    text.className = 'history-text'
    text.textContent = shown
    const sub = document.createElement('div')
    sub.className = 'history-sub'
    // 日期跟着界面语言走：写死 zh-CN 的话英文界面还是 24 小时制中文格式。
    const when = new Date(item.at).toLocaleString(I18N.locale, {
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
    const parts = [when, fmtDuration(item.durationSeconds || 0), t('result.chars', { count: [...shown].length })]
    for (const part of parts) {
      const span = document.createElement('span')
      span.textContent = part
      sub.appendChild(span)
    }
    main.append(text, sub)
    main.addEventListener('click', () => {
      // 载入的是一条历史，不是「上次转写」：切语言时不能再把它重绘回去。
      const rendered = localizeText(item.text)
      // 先切视图再写文本：结果框在别的面板里，隐藏元素上的 focus() 不生效。
      loadHandler?.()
      els.text.value = rendered
      // markers 交给 relocalizeResult 用：用户改过之后切语言，
      // 靠它把旧语言的缺口占位符换成新语言的。
      state.resultView = {
        rawText: item.text,
        rendered,
        markers: gapMarkerTexts(item.text),
        meta: { kind: 'label', key: 'history.from', at: item.at },
      }
      renderResultMeta()
      els.text.focus()
    })

    const actions = document.createElement('div')
    actions.className = 'history-actions'
    actions.append(
      iconAction(t('history.copy'), copyIcon(), () => copyText(shown)),
      iconAction(t('history.save'), saveIcon(), () => saveText(shown)),
      iconAction(t('history.delete'), trashIcon(), () => {
        state.history = state.history.filter((entry) => entry !== item)
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

