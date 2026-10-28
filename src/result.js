/* 结果区：转写结果 + 实时中间结果 */

import { els, gapMarkerTexts, joinText, localizeText, renderResultMeta, state } from './core.js'
import { addHistory } from './history.js'
import { clearIcon, copyIcon, saveIcon } from './icons.js'

/// 结果卡片右上角的三个动作按钮：纯图标，文字放在 title / aria-label 里。
/// 图标是静态的，切语言不用重绘 —— title / aria-label 由 i18n.apply() 填。
export function renderResultActions() {
  els.copyBtn.innerHTML = copyIcon()
  els.saveBtn.innerHTML = saveIcon()
  els.clearBtn.innerHTML = clearIcon()
}

function setResultView(rawText, meta) {
  const rendered = localizeText(rawText)
  state.resultView = { rawText, rendered, meta, markers: gapMarkerTexts(rawText) }
  els.text.value = rendered
  renderResultMeta()
}

/// 把编辑后文本里的旧占位符换成新语言的占位符。
///
/// 找不到就跳过 —— 用户把那一段改了，就别动它。
function swapGapMarkers(text, oldMarkers, newMarkers) {
  let out = text
  let from = 0
  for (let i = 0; i < oldMarkers.length; i++) {
    const oldMark = oldMarkers[i]
    const newMark = newMarkers[i]
    if (!oldMark || !newMark || oldMark === newMark) continue
    const at = out.indexOf(oldMark, from)
    if (at < 0) continue
    out = out.slice(0, at) + newMark + out.slice(at + oldMark.length)
    from = at + newMark.length
  }
  return out
}

/// 切语言只重画缺口占位符和元信息，不覆盖用户其他编辑。
export function relocalizeResult() {
  const view = state.resultView
  if (!view) return
  const rendered = localizeText(view.rawText)
  const markers = gapMarkerTexts(view.rawText)
  if (els.text.value === view.rendered) {
    // 用户没动过：整段按新语言重画。
    els.text.value = rendered
  } else {
    // 用户动过：只换缺口占位符，其他字符一个都不动。
    els.text.value = swapGapMarkers(els.text.value, view.markers || [], markers)
  }
  // rendered 永远存「原文的本地化结果」，不能写成用户文本 ——
  // 否则第二次切语言时守卫会误判成「用户没动过」，把编辑盖回原文。
  view.rendered = rendered
  view.markers = markers
  renderResultMeta()
}

/// 清空结果区：文本框清空，元信息区只留一条提示。
export function clearResult(label = 'result.waiting') {
  setResultView('', { kind: 'label', key: label })
}

/// 渲染一次转写结果。`record: false` 只重绘、不写历史。
export function showTranscript(transcript, { record = true } = {}) {
  state.lastTranscript = transcript
  const raw = (transcript.text || '').trim()
  if (!raw) {
    setResultView('', { kind: 'label', key: 'record.empty' })
    return
  }
  const audio = transcript.durationSeconds || 0
  const elapsed = (transcript.elapsedMs || 0) / 1000
  setResultView(raw, {
    kind: 'transcript',
    audio,
    elapsed,
    rtf: audio > 0 ? (elapsed / audio).toFixed(2) : '—',
  })
  if (record) addHistory(transcript, els.text.value)
}

/// 追加一句识别好的文本（事件是增量载荷：只带新句子）。
export function appendSegment(text) {
  // 取消 / 收尾结束之后迟到的句子不再追加，否则结果框会冒出被取消的内容。
  if (state.phase !== 'recording' && state.phase !== 'transcribing') return
  const piece = (text || '').trim()
  if (!piece) return
  const rawText = joinText(state.resultView?.rawText || els.text.value, piece)
  setResultView(rawText, { kind: 'label', key: 'record.live' })
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
