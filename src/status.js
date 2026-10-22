/* 模型状态、识别模式、语言、输入设备、模型下载、设置项 */

import { $, LANGS, STREAM_LANGS, els, fmtBytes, fmtError, idleHint, state, t, toast } from './core.js'
import { invoke, openDialog } from './tauri.js'

/* 下拉框实例由入口创建后交给这里（status 不该认识 applyLocale，否则会和入口互相依赖） */
let deviceSelect = null
let localeSelect = null

export function setSelects(device, locale) {
  deviceSelect = device
  localeSelect = locale
}

/// 模型徽标文案：流式与整段用同一套说法，避免两处提示对不上。
export function modelBadgeText(phase) {
  const name = state.status?.mode === 'streaming' ? t('model.label.streaming') : t('model.label.offline')
  if (phase === 'error') return t('model.badge.error', { name })
  if (phase === 'loading') return t('model.badge.loading', { name })
  return t('model.badge.ready', { name })
}

export async function refreshStatus() {
  try {
    state.status = await invoke('status')
    renderStatus()
  } catch (error) {
    toast(t('result.readStatusFailed', { error: fmtError(error) }), 'error')
  }
}

export function renderStatus() {
  const status = state.status
  if (!status) return
  const streamingMode = status.mode === 'streaming'
  const ready = streamingMode ? status.streamingReady : status.modelReady
  els.setup.classList.toggle('hidden', streamingMode || status.modelReady)

  if (streamingMode) {
    els.badge.className = ready ? 'badge' : 'badge warn'
    els.badge.textContent = ready
      ? modelBadgeText(state.engine)
      : t('model.badge.streamingMissing')
    els.dot.dataset.state = state.engine === 'error' ? 'error' : state.engine === 'loading' ? 'loading' : ready ? 'ready' : 'idle'
  } else if (!ready) {
    els.badge.textContent = t('model.badge.offlineMissing')
    els.badge.className = 'badge warn'
    els.dot.dataset.state = 'idle'
    const partial = status.installedBytes || 0
    if (partial > 0 && !state.downloading) {
      els.setupProgress.classList.remove('hidden')
      setProgress(partial, status.totalBytes)
      els.setupText.textContent = t('setup.cached', { done: fmtBytes(partial), total: fmtBytes(status.totalBytes) })
    }
  } else {
    els.badge.className = 'badge'
    els.badge.textContent = modelBadgeText(state.engine)
    els.dot.dataset.state = state.engine === 'error' ? 'error' : state.engine === 'loading' ? 'loading' : 'ready'
  }

  els.mic.classList.toggle('disabled', !ready)
  els.mic.title = ready ? '' : streamingMode ? t('model.needStreaming') : t('model.needOffline')

  els.settingsClearModel.classList.toggle('hidden', !status.modelDirCustom)
  els.settingsModel.textContent = ready
    ? t('settings.modelPath', {
        source: t(`model.source.${status.modelSource || 'app_data'}`),
        dir: status.modelDir,
      })
    : t('settings.modelNotReady', { size: fmtBytes(status.totalBytes) })
  els.settingsOutput.textContent = status.outputDir
  els.settingsVersion.textContent = t('settings.version', { version: status.appVersion })
  els.itnToggle.checked = !!status.itn
  els.punctToggle.checked = !!status.punct
  els.punctStatus.textContent = status.punctReady
    ? t('settings.punctReady')
    : t('settings.punctMissing', { size: fmtBytes(status.punctTotalBytes || 0) })
  els.punctDownload.classList.toggle('hidden', status.punctReady)
  els.modelMode.textContent = streamingMode ? t('model.name.streaming') : t('model.name.offline')

  for (const button of els.langRow.children) {
    button.setAttribute('aria-pressed', String(button.dataset.lang === status.language))
  }
  renderMode()
}

export function renderMode() {
  const mode = state.status?.mode || 'offline'
  for (const chip of els.modeRow.children) {
    chip.setAttribute('aria-pressed', String(chip.dataset.mode === mode))
    chip.disabled = state.phase !== 'idle'
  }
  if (state.phase === 'idle') els.hint.textContent = idleHint()
  if (state.status) {
    for (const chip of els.langRow.children) {
      chip.disabled = state.status.mode === 'streaming' && !STREAM_LANGS.includes(chip.dataset.lang)
      chip.title = chip.disabled ? t('mode.streamingOnlyZh') : ''
    }
  }
  const needStream = mode === 'streaming' && !state.status?.streamingReady
  els.streamSetup.classList.toggle('hidden', !needStream)
  if (needStream && state.status) {
    els.streamSetupText.textContent = t('mode.streamingMissing', { size: fmtBytes(state.status.streamingTotalBytes || 0) })
  }
}

export async function setMode(mode) {
  if (state.phase !== 'idle' || state.status?.mode === mode) return
  try {
    state.status = await invoke('set_mode', { mode })
    state.engine = 'idle'
    renderStatus()
    if (mode === 'streaming') {
      if (!STREAM_LANGS.includes(state.status.language)) {
        state.status = await invoke('set_language', { language: 'auto' })
        toast(t('mode.langSwitched'))
        renderStatus()
      }
      if (!state.status.streamingReady) {
        toast(t('mode.streamingDownloadFirst'))
      } else {
        void invoke('warm_up_streaming').catch(() => {})
      }
    }
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export async function setLanguage(language) {
  try {
    if (state.status?.mode === 'streaming' && !STREAM_LANGS.includes(language)) {
      toast(t('mode.switchedBack'))
      state.status = await invoke('set_mode', { mode: 'offline' })
    }
    state.status = await invoke('set_language', { language })
    renderStatus()
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export function renderLang() {
  els.langRow.innerHTML = ''
  for (const code of LANGS) {
    const button = document.createElement('button')
    button.type = 'button'
    button.className = 'chip'
    button.dataset.lang = code
    button.textContent = t(`lang.${code}`)
    button.setAttribute('aria-pressed', 'false')
    button.addEventListener('click', () => setLanguage(code))
    els.langRow.appendChild(button)
  }
}

/// 已列出的输入设备（列设备会触发 macOS 权限弹窗，所以只在用户打开下拉框时才去列）
let deviceCache = []

export function deviceItems() {
  const items = [{ value: '', label: t('record.deviceDefault') }]
  for (const device of deviceCache) {
    items.push({
      value: device.name,
      label: device.isDefault ? `${device.name} · ${t('record.deviceSystemDefault')}` : device.name,
    })
  }
  return items
}

export async function loadDevices() {
  if (!deviceSelect) return
  let devices = []
  try {
    devices = (await invoke('list_input_devices')) || []
  } catch {
    devices = []
  }
  deviceCache = devices
  const current = state.status?.inputDevice || deviceSelect.value || ''
  const items = deviceItems()
  deviceSelect.setItems(items, items.some((item) => item.value === current) ? current : '')
}

/// 界面语言下拉。
export function localeItems() {
  return [
    { value: 'system', label: t('settings.localeSystem') },
    { value: 'zh-CN', label: t('settings.localeZh') },
    { value: 'en', label: t('settings.localeEn') },
  ]
}

/// 切语言 / 重绘后同步两个下拉框的选项。
export function refreshSelectItems(localeValue) {
  if (deviceSelect) deviceSelect.setItems(deviceItems(), deviceSelect.value)
  if (localeSelect) localeSelect.setItems(localeItems(), localeValue)
}

export function setProgress(done, total) {
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0
  els.setupFill.style.width = `${pct}%`
  els.setupPct.textContent = `${pct}%`
}

export function setStreamProgress(done, total) {
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0
  els.streamProgressFill.style.width = `${pct}%`
  els.streamProgressPct.textContent = `${pct}%`
}

export async function downloadStreamingModel() {
  if (state.downloadingStreaming) return
  state.downloadingStreaming = true
  els.streamDownload.disabled = true
  els.streamCancel.classList.remove('hidden')
  els.streamProgress.classList.remove('hidden')
  setStreamProgress(0, state.status?.streamingTotalBytes || 0)
  els.streamProgressText.textContent = t('setup.preparing')
  try {
    state.status = await invoke('download_streaming_model')
    toast(t('model.badge.streamingReady'))
    renderStatus()
  } catch (error) {
    toast(fmtError(error), 'error')
    els.streamProgressText.textContent = t('setup.downloadFailed', { error: fmtError(error) })
  } finally {
    state.downloadingStreaming = false
    els.streamDownload.disabled = false
    els.streamCancel.classList.add('hidden')
  }
}

export async function downloadModel() {
  if (state.downloading) return
  state.downloading = true
  els.downloadBtn.disabled = true
  els.pickModelBtn.disabled = true
  els.cancelDownloadBtn.classList.remove('hidden')
  els.setupProgress.classList.remove('hidden')
  setProgress(0, state.status?.totalBytes || 0)
  els.setupText.textContent = t('setup.preparing')
  try {
    state.status = await invoke('download_model')
    state.downloading = false
    els.setupText.textContent = t('setup.done')
    toast(t('model.readyToast'))
    renderStatus()
    warmUp()
  } catch (error) {
    state.downloading = false
    toast(fmtError(error), 'error')
    els.setupText.textContent = t('setup.downloadFailed', { error: fmtError(error) })
  } finally {
    state.downloading = false
    els.downloadBtn.disabled = false
    els.pickModelBtn.disabled = false
    els.cancelDownloadBtn.classList.add('hidden')
  }
}

export async function downloadPunctuationModel() {
  if (state.downloadingPunct) return
  state.downloadingPunct = true
  els.punctDownload.disabled = true
  els.punctStatus.textContent = t('setup.preparing')
  try {
    state.status = await invoke('download_punctuation_model')
    state.status = await invoke('set_punct', { enabled: true })
    toast(t('settings.punctEnabled'))
    renderStatus()
  } catch (error) {
    toast(fmtError(error), 'error')
    els.punctStatus.textContent = t('setup.downloadFailed', { error: fmtError(error) })
  } finally {
    state.downloadingPunct = false
    els.punctDownload.disabled = false
  }
}

export async function pickModelDir() {
  const dir = await openDialog({ directory: true, multiple: false, title: t('setup.chooseDir') })
  if (!dir) return
  try {
    state.status = await invoke('set_model_dir', { dir })
    renderStatus()
    if (state.status.modelReady) {
      toast(t('model.ready'))
      warmUp()
    } else {
      toast(t('setup.noModel'), 'error')
    }
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export async function warmUp() {
  if (!state.status?.modelReady) return
  try {
    await invoke('warm_up')
  } catch (error) {
    console.warn('模型预热失败', error)
  }
}
