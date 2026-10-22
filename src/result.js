/* 结果区：转写结果 + 实时中间结果 */

import { els, fmtDuration, joinText, state, t } from './core.js'
import { addHistory } from './history.js'

/// 渲染一次转写结果。`record: false` 只重绘、不写历史（切语言时用）。
export function showTranscript(transcript, { record = true } = {}) {
  state.lastTranscript = transcript
  const text = (transcript.text || '').trim()
  els.text.value = text
  if (!text) {
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
    t('result.elapsed', { seconds: elapsed.toFixed(1) }),
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
  const piece = (text || '').trim()
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
