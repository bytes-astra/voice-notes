/* 结果区：转写结果 + 实时中间结果 */

import { els, fmtDuration, I18N, joinText, localizeText, state, t } from './core.js'
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
  state.resultView = { rawText, rendered, meta }
  els.text.value = rendered
  renderResultMeta()
}

/// 动态内容也要保存字典 key 和参数：I18N.apply 只处理静态 HTML。
export function renderResultMeta() {
  const view = state.resultView
  els.meta.innerHTML = ''
  if (!view) return
  const parts = view.meta.kind === 'transcript'
    ? [
        t('result.audio', { duration: fmtDuration(view.meta.audio) }),
        t('result.elapsed', { duration: fmtDuration(view.meta.elapsed) }),
        t('result.rtf', { rtf: view.meta.rtf }),
        t('result.chars', { count: [...els.text.value].length }),
      ]
    : [t(view.meta.key, {
        ...view.meta.vars,
        ...(view.meta.at ? { when: new Date(view.meta.at).toLocaleString(I18N.locale, {
          month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit',
        }) } : {}),
      })]
  for (const part of parts) {
    if (!part) continue
    const span = document.createElement('span')
    span.textContent = part
    els.meta.appendChild(span)
  }
}

/// 切语言只重画缺口占位符和元信息，不覆盖用户其他编辑。
export function relocalizeResult() {
  const view = state.resultView
  if (!view) return
  if (els.text.value === view.rendered) {
    els.text.value = localizeText(view.rawText)
  } else if (view.previousRaw) {
    // 用户编辑后的文本仍保留原文快照，只替换缺口占位符，不动其他字符。
    let edited = els.text.value
    let from = 0
    for (const marker of view.rawText.matchAll(/\[\[gap:\d+\]\]/g)) {
      const rawBefore = view.rawText.slice(from, marker.index)
      const renderedBefore = localizeText(rawBefore)
      const at = edited.indexOf(renderedBefore)
      if (at >= 0) {
        const start = at + renderedBefore.length
        const end = start + localizeText(marker[0]).length
        edited = edited.slice(0, start) + localizeText(marker[0]) + edited.slice(end)
        from = marker.index + marker[0].length
      }
    }
    els.text.value = edited
  }
  view.rendered = els.text.value
  view.previousRaw = view.rawText
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
