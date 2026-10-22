/* 后端事件 → 前端动作：事件名只在这里出现 */

import { els, fmtBytes, fmtClock, fmtError, state, t, toast } from './core.js'
import { listen } from './tauri.js'
import { modelBadgeText, renderStatus, setProgress, setStreamProgress } from './status.js'
import { cancelRecording, pushLevel } from './recorder.js'
import { appendSegment, showPartial } from './result.js'

export async function listenEvents() {
  await listen('audio:level', (event) => {
    const payload = event.payload || {}
    pushLevel(payload.level || 0)
    if (state.phase === 'recording' && typeof payload.elapsed === 'number') {
      els.timer.textContent = fmtClock(payload.elapsed)
    }
  })

  await listen('audio:error', (event) => {
    toast(fmtError(event.payload), 'error')
    if (state.phase === 'recording') void cancelRecording()
  })

  await listen('model:progress', (event) => {
    const progress = event.payload || {}
    if (state.downloadingPunct) {
      const pct = progress.total > 0 ? Math.round(((progress.done || 0) / progress.total) * 100) : 0
      els.punctStatus.textContent = t('progress.punct', { pct, done: fmtBytes(progress.done || 0), total: fmtBytes(progress.total || 0) })
      return
    }
    if (state.downloadingStreaming) {
      els.streamProgress.classList.remove('hidden')
      setStreamProgress(progress.done || 0, progress.total || 0)
      const label =
        { downloading: t('progress.downloading'), verifying: t('progress.verifying'), connecting: t('progress.connecting'), complete: t('progress.complete') }[progress.phase] || t('progress.downloading')
      const host = progress.source?.startsWith('http') ? new URL(progress.source).host : t('progress.cache')
      els.streamProgressText.textContent = `${label} ${progress.resource} · ${fmtBytes(progress.done || 0)} / ${fmtBytes(progress.total || 0)}${host ? ` · ${host}` : ''}`
      return
    }
    if (!state.downloading && progress.phase !== 'verifying') return
    els.setupProgress.classList.remove('hidden')
    setProgress(progress.done || 0, progress.total || 0)
    const label = { downloading: t('progress.downloading'), verifying: t('progress.verifying'), connecting: t('progress.connecting'), complete: t('progress.complete') }[progress.phase] || t('progress.downloading')
    const host = progress.source?.startsWith('http') ? new URL(progress.source).host : progress.source === 'cache' ? t('progress.cache') : ''
    els.setupText.textContent = `${label} ${progress.resource}${progress.phase === 'downloading' ? ` · ${fmtBytes(progress.done)} / ${fmtBytes(progress.total)}` : ''}${host ? ` · ${host}` : ''}`
    if (progress.phase === 'error' && progress.message) toast(progress.message, 'error')
  })

  // 事件是增量载荷：只带新句子，前端自己追加（累计全文在长时间会话下是 O(n²)）
  await listen('stream:partial', (event) => {
    const payload = event.payload || {}
    showPartial(payload.partial)
  })

  await listen('stream:segment', (event) => {
    const payload = event.payload || {}
    appendSegment(payload.segment)
    showPartial('')
    if (state.transcribingFile) {
      state.fileSegments += 1
      els.hint.textContent = t('result.fileProgress', { count: state.fileSegments })
    }
  })

  await listen('streaming:state', (event) => {
    const payload = event.payload || {}
    if (payload.phase === 'loading') {
      state.engine = 'loading'
      if (state.phase === 'requesting') els.hint.textContent = t('record.loadingStreaming')
      renderStatus()
    } else if (payload.phase === 'ready') {
      state.engine = 'ready'
      renderStatus()
    } else if (payload.phase === 'error') {
      state.engine = 'error'
      renderStatus()
      toast(payload.message || modelBadgeText('error'), 'error')
    }
  })

  await listen('engine:state', (event) => {
    const phase = event.payload?.phase
    state.engine = phase
    if (phase === 'error') {
      els.dot.dataset.state = 'error'
      els.badge.className = 'badge danger'
      els.badge.textContent = modelBadgeText('error')
      toast(event.payload?.message || modelBadgeText('error'), 'error')
    } else if (phase === 'loading') {
      els.dot.dataset.state = 'loading'
      els.badge.className = 'badge'
      els.badge.textContent = modelBadgeText('loading')
    } else if (phase === 'ready') {
      els.dot.dataset.state = 'ready'
      els.badge.className = 'badge'
      els.badge.textContent = modelBadgeText('ready')
    }
  })
}
