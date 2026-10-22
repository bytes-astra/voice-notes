/* 文件转写（后端解码 + 逐句出字）与输出目录设置 */

import { els, fmtError, state, t, toast } from './core.js'
import { invoke, openDialog, openPath } from './tauri.js'
import { showTranscript } from './result.js'
import { renderStatus } from './status.js'
import { setPhase } from './recorder.js'

/// 支持的文件格式（symphonia 解码；视频轨会被忽略）。
const FILE_EXTENSIONS = [
  'mp3', 'wav', 'flac', 'ogg', 'oga', 'opus', 'm4a', 'aac', 'mp4', 'mov', 'm4v', 'webm', 'wma',
]

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
  els.hint.textContent = t('result.fileDecoding', { name })
  state.fileSegments = 0
  state.transcribingFile = true
  try {
    const transcript = await invoke('transcribe_file', { path })
    showTranscript(transcript)
    toast(transcript.text ? t('result.fileDone', { name }) : t('result.none'))
  } catch (error) {
    toast(t('result.fileFailed', { error: fmtError(error) }), 'error')
  } finally {
    state.transcribingFile = false
    state.fileSegments = 0
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
  const dir = state.status?.outputDir
  if (!dir) return
  try {
    await openPath(dir)
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}
