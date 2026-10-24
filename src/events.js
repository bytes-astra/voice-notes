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
    const payload = event.payload
    toast(fmtError(payload), 'error')
    // 载荷是对象 { code, detail?, seconds?, fatal }：只有致命错误才中断录音。
    // 缺口提示（fatal:false）只提示、录音继续；没有 fatal 字段时按致命处理。
    const fatal = payload && typeof payload === 'object' ? payload.fatal !== false : true
    if (fatal && state.phase === 'recording') void cancelRecording()
  })

  await listen('model:progress', (event) => {
    const progress = event.payload || {}
    // 两个下载可以同时跑，事件同名 —— 按载荷里的 kind 分流，
    // 别看当前状态（那样会把整段的进度显示到流式面板里，反之亦然）。
    if (progress.kind === 'streaming') {
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
    // 下载失败的提示也是错误码：查字典翻译，别把 "error.downloadFailed" 显示给用户
    if (progress.phase === 'error' && progress.message) toast(fmtError(progress.message), 'error')
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
      // 进度里带上已用时：第 N 句 / 已用时 mm:ss
      const elapsed = state.fileStartedAt ? (performance.now() - state.fileStartedAt) / 1000 : 0
      els.hint.textContent = t('result.fileProgress', {
        count: state.fileSegments,
        elapsed: fmtClock(elapsed),
      })
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
