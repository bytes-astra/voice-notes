/* 结果区：转写结果 + 实时中间结果 */

import { els, fmtDuration, joinText, state, t } from './core.js'
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

/// 追加一句识别好的文本（事件是增量载荷：只带新句子）。
export function appendSegment(text) {
  const piece = (text || '').trim()
  if (!piece) return
  els.text.value = joinText(els.text.value, piece)
  updateListening()
}

/// 「正在听…」指示：录音中但还没出字时显示。
///
/// 整段模式要等 VAD 断句（最长 30 秒）才出第一句，没有这个指示用户会以为卡住了。
export function updateListening() {
  const show = state.phase === 'recording' && !els.text.value.trim()
  els.listening.classList.toggle('hidden', !show)
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
