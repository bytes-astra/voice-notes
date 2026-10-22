/* 结果区：转写结果 + 实时中间结果 */

import { els, fmtDuration, state, t } from './core.js'
import { addHistory } from './history.js'

export function showTranscript(transcript) {
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
    transcript.corrected ? t('result.corrected') : null,
  ]) {
    if (!part) continue
    const span = document.createElement('span')
    span.textContent = part
    els.meta.appendChild(span)
  }
  addHistory(transcript, text)
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
