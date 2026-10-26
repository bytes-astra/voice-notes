/* 文件转写（后端解码 + 逐句出字）与输出目录设置 */

import { els, errorText, fmtError, state, t, toast } from './core.js'
import { invoke, logEvent, openDialog } from './tauri.js'
import { clearResult, showTranscript } from './result.js'
import { renderStatus } from './status.js'
import { setPhase } from './recorder.js'

/// 支持的文件格式（symphonia 0.5 能解出来的；视频轨会被忽略）。
/// 别把 wma / opus / webm 加回来：0.5 没有对应的解码器，选了必然报错。
const FILE_EXTENSIONS = ['mp3', 'wav', 'flac', 'ogg', 'oga', 'm4a', 'aac', 'mp4', 'mov', 'm4v']

/// 从路径里取文件名（只用来显示）。
function fileName(path) {
  const parts = String(path || '').split(/[\\/]/)
  return parts[parts.length - 1] || path
}

/// 转写一个音频 / 视频文件（路径由文件对话框或拖拽得到）。
///
/// 解码在 Rust 那边做（symphonia），逐块喂给识别会话：内存恒定，
/// 1 小时的文件也不会整条进内存，而且 mp4/mov 在 Windows 上也能转。
export async function transcribeFilePath(path) {
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
  const name = fileName(path)
  // 新的一次转写：先清掉上一次的结果，否则分句会追加在旧文本后面。
  state.lastTranscript = null
  clearResult()
  els.hint.textContent = t('result.fileDecoding', { name })
  state.fileSegments = 0
  state.transcribingFile = true
  const attemptId = (state.fileAttemptId || 0) + 1
  state.fileAttemptId = attemptId
  // 记下开始时间：进度提示要显示「已用时」。
  state.fileStartedAt = performance.now()
  try {
    const reserved = await invoke('reserve_session')
    if (attemptId !== state.fileAttemptId) return
    state.fileSessionId = reserved
    const transcript = await invoke('transcribe_file', { path, sessionId: reserved })
    if (state.fileSessionId !== reserved) return
    // 文件转写完成：时长 + 句数（和录音的最终结果同一套口径）。
    logEvent('INFO', `文件转写完成 文件=${name} 时长=${(transcript.durationSeconds || 0).toFixed(1)}s 句数=${transcript.segments?.length || 0}`)
    showTranscript(transcript)
    toast(transcript.text ? t('result.fileDone', { name }) : t('result.none'))
  } catch (error) {
    if (attemptId !== state.fileAttemptId) return
    logEvent('ERROR', `文件转写失败: ${errorText(error)}`)
    toast(t('result.fileFailed', { error: fmtError(error) }), 'error')
  } finally {
    if (attemptId !== state.fileAttemptId) return
    state.transcribingFile = false
    state.fileSessionId = 0
    state.fileSegments = 0
    state.fileStartedAt = 0
    setPhase('idle')
  }
}

/// 弹出文件选择框，然后转写选中的文件。
export async function pickAndTranscribeFile() {
  const picked = await openDialog({
    multiple: false,
    directory: false,
    title: t('result.pickFile'),
    filters: [{ name: t('result.fileFilter'), extensions: FILE_EXTENSIONS }],
  })
  const path = Array.isArray(picked) ? picked[0] : picked
  if (path) await transcribeFilePath(path)
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
  if (!state.status?.outputDir) return
  try {
    // 用后端命令而不是 opener 插件：插件要静态 scope，而保存目录用户可以改
    await invoke('open_output_dir')
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}
