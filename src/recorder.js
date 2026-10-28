/* 录音流程：阶段、波形、计时、开始 / 停止 / 取消 */

import { $, BARS, IDLE_WAVE, els, errorText, fmtClock, fmtError, idleHint, state, t, toast } from './core.js'
import { invoke, logEvent } from './tauri.js'
import { renderMode } from './status.js'
import { clearResult, showTranscript } from './result.js'

const HINTS = {
  requesting: 'record.preparing',
  recording: 'record.recording',
  transcribing: 'record.transcribing',
}

export function renderMicLabel() {
  els.mic.setAttribute('aria-label', state.phase === 'recording' ? t('record.micStop') : t('record.micStart'))
}

/// 按当前阶段重绘麦克风下方的提示语（切语言时要跟着重调）。
export function renderHint() {
  if (state.phase === 'idle') els.hint.textContent = idleHint()
  else if (HINTS[state.phase]) els.hint.textContent = t(HINTS[state.phase])
  if (state.transcribingFile) {
    els.hint.textContent = t('result.fileProgress', {
      count: state.fileSegments,
      elapsed: fmtClock(state.fileStartedAt ? (performance.now() - state.fileStartedAt) / 1000 : 0),
    })
  }
}

export function setPhase(phase) {
  state.phase = phase
  els.app.dataset.phase = phase
  renderMicLabel()
  els.mic.querySelector('.mic-glyph').classList.toggle('hidden', phase === 'recording')
  els.mic.querySelector('.stop-glyph').classList.toggle('hidden', phase !== 'recording')
  els.mic.disabled = phase === 'requesting' || phase === 'transcribing'
  renderHint()
  if (phase !== 'recording') {
    els.liveLine.classList.add('hidden')
    els.liveLine.textContent = ''
  }
  if (phase === 'idle') resetWave()
  renderMode()
}

export function buildWave() {
  for (let i = 0; i < BARS; i++) els.wave.appendChild(document.createElement('span'))
  // 首帧就把空闲剪影画出来，否则要等到第一次阶段变化才不显示。
  renderWave()
}

export function renderWave() {
  const bars = els.wave.children
  const recording = state.phase === 'recording'
  for (let i = 0; i < bars.length; i++) {
    const height = recording
      ? 4 + Math.round((state.levels[i] || 0) * 62)
      : IDLE_WAVE[i % IDLE_WAVE.length]
    bars[i].style.height = `${height}px`
  }
}

export function resetWave() {
  state.levels.fill(0)
  renderWave()
}

export function pushLevel(level) {
  state.levels.push(Number(level) || 0)
  state.levels.shift()
  renderWave()
}

export function startTimer() {
  stopTimer()
  state.timer = setInterval(() => {
    const elapsed = (performance.now() - state.startedAt) / 1000
    els.timer.textContent = fmtClock(elapsed)
  }, 100)
}

export function stopTimer() {
  if (state.timer) clearInterval(state.timer)
  state.timer = 0
}

/// 正在进行的「启动录音」流程；取消时要等它落定，否则它可能在取消之后才把会话建起来。
let startTask = null

export async function startRecording() {
  const streaming = state.status?.mode === 'streaming'
  if (streaming && !state.status.streamingReady) {
    toast(t('model.streamingMissing'), 'error')
    els.streamSetup.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
    return
  }
  if (!streaming && !state.status?.modelReady) {
    toast(t('model.offlineMissing'), 'error')
    els.setup.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
    return
  }
  if (state.phase !== 'idle' || startTask) return
  setPhase('requesting')
  const requestId = ++state.requestId
  startTask = runStart(requestId, streaming)
  try {
    await startTask
  } catch (error) {
    if (requestId === state.requestId) {
      setPhase('idle')
      logEvent('ERROR', `启动录音失败: ${errorText(error)}`)
      toast(fmtError(error), 'error')
    }
  } finally {
    startTask = null
  }
}

/// 真正启动一次录音。requestId 变了说明这次请求已经被取消（Esc）。
async function runStart(requestId, streaming) {
  try {
    // 先单独触发一次麦克风权限（macOS 在第一次打开输入流时才弹框），
    // 用户同意后再开始录音；不同意就停在这里。
    await invoke('warm_up_mic')
  } catch (error) {
    if (requestId !== state.requestId) return
    setPhase('idle')
    logEvent('ERROR', `麦克风不可用: ${errorText(error)}`)
    toast(t('record.micDenied', { error: fmtError(error) }), 'error')
    return
  }
  // 等权限的过程中取消了：不再开始。
  if (requestId !== state.requestId) return
  try {
    const sessionId = await invoke('reserve_session')
    if (requestId !== state.requestId) return
    state.recordingSessionId = sessionId
    await invoke(streaming ? 'start_streaming_recording' : 'start_offline_recording', { sessionId })
    // 启动后端会话的过程中被取消：把已经起来的会话也停掉，别留下一个看不见的录音。
    if (requestId !== state.requestId) {
      await invoke('cancel_recording').catch(() => {})
      return
    }
    state.startedAt = performance.now()
    els.timer.textContent = '00:00'
    // 两种模式都是边录边出字：先把结果区清空
    state.lastTranscript = null
    clearResult('record.live')
    startTimer()
    setPhase('recording')
    logEvent('INFO', `开始录音 mode=${state.status?.mode || 'offline'}`)
  } catch (error) {
    // 过期的请求失败了不能影响新请求的阶段和提示。
    if (requestId !== state.requestId) return
    setPhase('idle')
    logEvent('ERROR', `启动录音失败: ${errorText(error)}`)
    toast(fmtError(error), 'error')
  }
}

export async function stopRecording() {
  if (state.phase !== 'recording') return
  const requestId = state.requestId
  const elapsed = (performance.now() - state.startedAt) / 1000
  stopTimer()
  els.timer.textContent = fmtClock(elapsed)
  setPhase('transcribing')
  els.hint.textContent = t('record.transcribingFirst')
  try {
    const transcript = await invoke('stop_recording')
    if (requestId !== state.requestId) return
    // 录音的最终结果：时长 + 句数，和文件转写一样记一条。
    logEvent('INFO', `停止录音，转写完成 时长=${(transcript.durationSeconds || 0).toFixed(1)}s 句数=${transcript.segments?.length || 0}`)
    showTranscript(transcript)
    toast(transcript.warning ? fmtError(transcript.warning) : transcript.text ? t('result.done') : t('result.none'), transcript.warning ? 'error' : 'info')
  } catch (error) {
    if (requestId !== state.requestId) return
    logEvent('ERROR', `停止录音失败: ${errorText(error)}`)
    const text = els.text.value.trim()
    if (text) {
      const partial = {
        text,
        segments: [],
        durationSeconds: elapsed,
        elapsedMs: 0,
        language: state.status?.language || 'auto',
      }
      state.resultView = {
        rawText: state.resultView?.rawText || text,
        rendered: text,
        meta: { kind: 'transcript', audio: elapsed, elapsed: 0, rtf: '—' },
      }
      showTranscript({ ...partial, text: state.resultView.rawText })
    }
    toast(fmtError(error), 'error')
  } finally {
    if (requestId === state.requestId) setPhase('idle')
  }
}

export async function cancelRecording() {
  if (state.phase !== 'recording' && state.phase !== 'requesting') return
  const wasRecording = state.phase === 'recording'
  logEvent('INFO', `取消录音 phase=${state.phase}`)
  // 先让本会话及迟到的停止结果失效，再等待后端取消。
  state.requestId += 1
  const requestId = state.requestId
  setPhase('cancelling')
  stopTimer()
  // 等它落定再取消：否则它可能在这之后才把后端会话建起来，留下一个看不见的录音。
  if (startTask) await startTask.catch(() => {})
  try {
    await invoke('cancel_recording')
  } catch {
    /* 忽略 */
  }
  if (requestId !== state.requestId) return
  els.timer.textContent = '00:00'
  if (wasRecording) {
    // 取消就是不要这段：清掉已经实时上屏的文字。
    clearResult()
  }
  setPhase('idle')
}
