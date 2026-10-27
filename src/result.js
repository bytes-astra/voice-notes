/* 结果区：转写结果 + 实时中间结果 */

import { els, fmtDuration, joinText, localizeText, state, t } from './core.js'
import { addHistory } from './history.js'
import { copyIcon, saveIcon, trashIcon } from './icons.js'

/// 结果卡片右上角的三个动作按钮：纯图标，文字放在 title / aria-label 里。
/// 图标是静态的，切语言不用重绘 —— title / aria-label 由 i18n.apply() 填。
export function renderResultActions() {
  els.copyBtn.innerHTML = copyIcon()
  els.saveBtn.innerHTML = saveIcon()
  els.clearBtn.innerHTML = trashIcon()
}

/// 清空结果区：文本框清空，元信息区只留一条提示。
export function clearResult(label = 'result.waiting') {
  els.text.value = ''
  els.meta.innerHTML = ''
  const span = document.createElement('span')
  span.textContent = t(label)
  els.meta.appendChild(span)
}

/// 渲染一次转写结果。`record: false` 只重绘、不写历史（切语言时用）。
export function showTranscript(transcript, { record = true } = {}) {
  // 存原文：切语言时要按新语言重新本地化缺口占位符。
  state.lastTranscript = transcript
  const raw = (transcript.text || '').trim()
  // 缺口占位符（[[gap:N]]）在这里换成本地化文案：历史与存盘里也是本地化的。
  const text = localizeText(raw)
  els.text.value = text
  if (!raw) {
    els.meta.innerHTML = '<span>' + t('record.empty') + '</span>'
    return
  }
  const audio = transcript.durationSeconds || 0
  const elapsed = (transcript.elapsedMs || 0) / 1000
  const rtf = audio > 0 ? (elapsed / audio).toFixed(2) : '—'
  const chars = [...text].length
  els.meta.innerHTML = ''
  for (const part of [
    t('result.audio', { duration: fmtDuration(audio) }),
    t('result.elapsed', { duration: fmtDuration(elapsed) }),
    t('result.rtf', { rtf }),
    t('result.chars', { count: chars }),
  ]) {
    if (!part) continue
    const span = document.createElement('span')
    span.textContent = part
    els.meta.appendChild(span)
  }
  if (record) addHistory(transcript, text)
}

/// 追加一句识别好的文本（事件是增量载荷：只带新句子）。
export function appendSegment(text) {
  // 取消 / 收尾结束之后迟到的句子不再追加，否则结果框会冒出被取消的内容。
  if (state.phase !== 'recording' && state.phase !== 'transcribing') return
  const piece = localizeText(text || '').trim()
  if (!piece) return
  els.text.value = joinText(els.text.value, piece)
}

export function showPartial(partial) {
  if (state.phase !== 'recording') return
  const text = (partial || '').trim()
  if (!text) {
    els.liveLine.classList.add('hidden')
    els.liveLine.textContent = ''
    return
  }
  els.liveLine.textContent = text.length > 160 ? `…${text.slice(-160)}` : text
  els.liveLine.classList.remove('hidden')
}
