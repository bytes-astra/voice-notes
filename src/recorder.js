/* 录音流程：阶段、波形、计时、开始 / 停止 / 取消 */

import { $, BARS, IDLE_WAVE, MAX_SECONDS, els, fmtClock, fmtError, idleHint, state, t, toast } from './core.js'
import { invoke } from './tauri.js'
import { renderMode } from './status.js'
import { showTranscript } from './result.js'

const HINTS = {
  requesting: 'record.preparing',
  recording: 'record.recording',
  transcribing: 'record.transcribing',
}

export function renderMicLabel() {
  els.mic.setAttribute('aria-label', state.phase === 'recording' ? t('record.micStop') : t('record.micStart'))
}

export function setPhase(phase) {
  state.phase = phase
  els.app.dataset.phase = phase
  renderMicLabel()
  els.mic.querySelector('.mic-glyph').classList.toggle('hidden', phase === 'recording')
  els.mic.querySelector('.stop-glyph').classList.toggle('hidden', phase !== 'recording')
  els.mic.disabled = phase === 'requesting' || phase === 'transcribing'
  if (phase === 'idle') els.hint.textContent = idleHint()
  else if (HINTS[phase]) els.hint.textContent = t(HINTS[phase])
  if (phase !== 'recording') {
    els.liveLine.classList.add('hidden')
    els.liveLine.textContent = ''
  }
  if (phase === 'idle') resetWave()
  renderMode()
}

export function buildWave() {
  for (let i = 0; i < BARS; i++) els.wave.appendChild(document.createElement('span'))
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
    if (elapsed >= MAX_SECONDS && state.phase === 'recording') void stopRecording()
  }, 100)
}

export function stopTimer() {
  if (state.timer) clearInterval(state.timer)
  state.timer = 0
}

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
  if (state.phase !== 'idle') return
  setPhase('requesting')
  try {
    // 先单独触发一次麦克风权限（macOS 在第一次打开输入流时才弹框），
    // 用户同意后再开始录音；不同意就停在这里。
    await invoke('warm_up_mic')
  } catch (error) {
    setPhase('idle')
    toast(t('record.micDenied', { error: fmtError(error) }), 'error')
    return
  }
  try {
    await invoke('start_recording')
    state.startedAt = performance.now()
    els.timer.textContent = '00:00'
    if (state.status?.mode === 'streaming') {
      els.text.value = ''
      els.meta.innerHTML = '<span>' + t('record.live') + '</span>'
    }
    startTimer()
    setPhase('recording')
  } catch (error) {
    setPhase('idle')
    toast(fmtError(error), 'error')
  }
}

export async function stopRecording() {
  if (state.phase !== 'recording') return
  const elapsed = (performance.now() - state.startedAt) / 1000
  stopTimer()
  els.timer.textContent = fmtClock(elapsed)
  setPhase('transcribing')
  els.hint.textContent = t('record.transcribingFirst')
  try {
    const transcript = await invoke('stop_recording')
    showTranscript(transcript)
    toast(transcript.text ? t('result.done') : t('result.none'))
  } catch (error) {
    toast(fmtError(error), 'error')
  } finally {
    setPhase('idle')
  }
}

export async function cancelRecording() {
  if (state.phase !== 'recording' && state.phase !== 'requesting') return
  stopTimer()
  try {
    await invoke('cancel_recording')
  } catch {
    /* 忽略 */
  }
  els.timer.textContent = '00:00'
  setPhase('idle')
}
