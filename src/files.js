/* 文件转写（浏览器解码 + 重采样）与输出目录设置 */

import { MAX_SECONDS, els, fmtError, state, t, toast } from './core.js'
import { invoke, openDialog, openPath } from './tauri.js'
import { showTranscript } from './result.js'
import { renderStatus } from './status.js'
import { setPhase } from './recorder.js'

export function encodeWave(samples) {
  const bytes = new Uint8Array(44 + samples.length * 2)
  const view = new DataView(bytes.buffer)
  const text = (at, value) => {
    for (let i = 0; i < value.length; i++) bytes[at + i] = value.charCodeAt(i)
  }
  text(0, 'RIFF')
  view.setUint32(4, bytes.length - 8, true)
  text(8, 'WAVE')
  text(12, 'fmt ')
  view.setUint32(16, 16, true)
  view.setUint16(20, 1, true)
  view.setUint16(22, 1, true)
  view.setUint32(24, 16000, true)
  view.setUint32(28, 32000, true)
  view.setUint16(32, 2, true)
  view.setUint16(34, 16, true)
  text(36, 'data')
  view.setUint32(40, samples.length * 2, true)
  for (let i = 0; i < samples.length; i++) {
    const value = Math.max(-1, Math.min(1, samples[i]))
    view.setInt16(44 + i * 2, Math.round(value * (value < 0 ? 32768 : 32767)), true)
  }
  return bytes
}

export function audioBase64(bytes) {
  let text = ''
  for (let i = 0; i < bytes.length; i += 8192) {
    text += String.fromCharCode(...bytes.subarray(i, i + 8192))
  }
  return btoa(text)
}

export async function transcribeFile(file) {
  if (!state.status?.modelReady) {
    toast(t('result.fileNeedModel'), 'error')
    return
  }
  if (state.phase === 'recording') {
    toast(t('toast.recordingBusy'))
    return
  }
  if (state.phase !== 'idle') return
  setPhase('transcribing')
  els.hint.textContent = t('result.fileDecode', { name: file.name })
  let context
  try {
    context = new AudioContext()
    const buffer = await context.decodeAudioData(await file.arrayBuffer())
    const seconds = Math.min(buffer.duration, MAX_SECONDS)
    const frames = Math.max(1, Math.floor(seconds * 16000))
    const offline = new OfflineAudioContext(1, frames, 16000)
    const source = offline.createBufferSource()
    source.buffer = buffer
    source.connect(offline.destination)
    source.start()
    const rendered = await offline.startRendering()
    els.hint.textContent = t('result.fileTranscribe', { name: file.name })
    const audio = audioBase64(encodeWave(rendered.getChannelData(0)))
    const transcript = await invoke('transcribe_wav', {
      audioBase64: audio,
      language: state.status.language,
    })
    showTranscript(transcript)
    toast(transcript.text ? t('result.fileDone', { name: file.name }) : t('result.none'))
  } catch (error) {
    console.error(error)
    toast(t('result.fileFailed', { error: fmtError(error) }), 'error')
  } finally {
    if (context) await context.close().catch(() => {})
    setPhase('idle')
  }
}

export async function pickOutputDir() {
  const dir = await openDialog({ directory: true, multiple: false, title: t('settings.chooseOutputTitle') })
  if (!dir) return
  try {
    state.status = await invoke('set_output_dir', { dir })
    renderStatus()
    toast(t('settings.outputChanged'))
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export async function openOutputDir() {
  const dir = state.status?.outputDir
  if (!dir) return
  try {
    await openPath(dir)
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}
