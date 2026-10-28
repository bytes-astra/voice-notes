/* 模型状态、识别模式、语言、输入设备、模型下载、设置项 */

import { $, LANGS, STREAM_LANGS, els, errorText, fmtBytes, fmtError, idleHint, state, t, toast } from './core.js'
import { invoke, logEvent, openDialog } from './tauri.js'

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
  // 设置页这块只讲整段模型：流式模式下不能用 streamingReady 冒充它已就绪。
  els.settingsModel.textContent = status.modelReady
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
  // 回填术语表；用户正在编辑时不要覆盖输入
  if (document.activeElement !== els.hotwordsInput) {
    els.hotwordsInput.value = (status.hotwords || []).join('\n')
  }
  els.modelMode.textContent = streamingMode ? t('model.name.streaming') : t('model.name.offline')

  for (const button of els.langRow.children) {
    button.setAttribute('aria-pressed', String(button.dataset.lang === status.language))
  }
  renderMode()
}

export function renderMode() {
  const mode = state.status?.mode || 'offline'
  els.itnToggle.disabled = state.phase !== 'idle'
  for (const chip of els.modeRow.children) {
    chip.setAttribute('aria-pressed', String(chip.dataset.mode === mode))
    chip.disabled = state.phase !== 'idle'
  }
  if (state.phase === 'idle') els.hint.textContent = idleHint()
  if (state.status) {
    for (const chip of els.langRow.children) {
      chip.disabled = state.phase !== 'idle' || (state.status.mode === 'streaming' && !STREAM_LANGS.includes(chip.dataset.lang))
      chip.title = state.phase !== 'idle'
        ? t('record.configLocked')
        : chip.disabled ? t('mode.streamingOnlyZh') : ''
    }
  }
  const needStream = mode === 'streaming' && !state.status?.streamingReady
  els.streamSetup.classList.toggle('hidden', !needStream)
  if (needStream && state.status) {
    els.streamSetupText.textContent = t('mode.streamingMissing', {
      size: fmtBytes(state.status.streamingTotalBytes || 0),
    })
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
    } else if (state.status.modelReady) {
      // 切回整段前先把 239 MB 引擎加载好（后端切模式时已经释放了另一份）。
      void warmUp()
    }
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export async function setLanguage(language) {
  if (state.phase !== 'idle') return
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
  const selected = items.some((item) => item.value === current) ? current : ''
  deviceSelect.setItems(items, selected)
  if (current && !selected) {
    // 设备已拔掉：界面回到默认，后端也必须同步，否则下次仍使用旧设备名。
    try {
      state.status = await invoke('set_input_device', { name: null })
    } catch (error) {
      logEvent('ERROR', `失效输入设备回退失败: ${errorText(error)}`)
    }
  }
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
  if (deviceSelect) {
    const items = deviceItems()
    const current = deviceSelect.value || state.status?.inputDevice || ''
    deviceSelect.setItems(items, items.some((item) => item.value === current) ? current : '')
  }
  if (localeSelect) localeSelect.setItems(localeItems(), localeValue)
}

/// 百分比：两个进度条共用一份算法。
const pctOf = (done, total) => (total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0)

export function setProgress(done, total) {
  const pct = pctOf(done, total)
  els.setupFill.style.width = `${pct}%`
  els.setupPct.textContent = `${pct}%`
}

export function setStreamProgress(done, total) {
  const pct = pctOf(done, total)
  els.streamProgressFill.style.width = `${pct}%`
  els.streamProgressPct.textContent = `${pct}%`
}

/// 下载模型：整段与流式的流程完全一样，只有命令名、字节数、
/// 文案、额外步骤和预热方式不同，统一走这里。
async function runDownload({
  isBusy, setBusy, button, cancelButton, progressBox, progressText, setProgress,
  command, totalBytes, afterDownload, readyToast, warm, label,
}) {
  if (isBusy()) return
  logEvent('INFO', `开始下载模型: ${label}`)
  setBusy(true)
  button.disabled = true
  cancelButton.classList.remove('hidden')
  progressBox.classList.remove('hidden')
  setProgress(0, totalBytes)
  progressText.textContent = t('setup.preparing')
  try {
    state.status = await invoke(command)
    if (afterDownload) await afterDownload()
    progressText.textContent = t('setup.done')
    toast(t(readyToast))
    renderStatus()
    // 和整段模型一样：下载完先把引擎加载起来，第一次录音不用等
    warm()
  } catch (error) {
    logEvent('ERROR', `模型下载失败: ${errorText(error)}`)
    toast(fmtError(error), 'error')
    progressText.textContent = t('setup.downloadFailed', { error: fmtError(error) })
  } finally {
    setBusy(false)
    button.disabled = false
    cancelButton.classList.add('hidden')
  }
}

export async function downloadStreamingModel() {
  await runDownload({
    isBusy: () => state.downloadingStreaming,
    setBusy: (value) => {
      state.downloadingStreaming = value
    },
    button: els.streamDownload,
    cancelButton: els.streamCancel,
    progressBox: els.streamProgress,
    progressText: els.streamProgressText,
    setProgress: setStreamProgress,
    command: 'download_streaming_model',
    totalBytes: state.status?.streamingTotalBytes || 0,
    // 标点模型和流式模型一起下载：标点下失败只是降级为无标点，
    // 所以只有它确实就绪时才自动开启，否则开关别装作打开。
    afterDownload: async () => {
      if (state.status.punctReady) {
        state.status = await invoke('set_punct', { enabled: true })
      }
    },
    readyToast: 'model.badge.streamingReady',
    label: '流式模型',
    warm: () => {
      void invoke('warm_up_streaming').catch(() => {})
    },
  })
}

export async function downloadModel() {
  await runDownload({
    isBusy: () => state.downloading,
    setBusy: (value) => {
      state.downloading = value
    },
    button: els.downloadBtn,
    cancelButton: els.cancelDownloadBtn,
    progressBox: els.setupProgress,
    progressText: els.setupText,
    setProgress,
    command: 'download_model',
    totalBytes: state.status?.totalBytes || 0,
    afterDownload: null,
    readyToast: 'model.readyToast',
    label: '整段模型',
    warm: () => {
      warmUp()
    },
  })
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
  // 流式模式用不到整段引擎：下载 / 选目录后不要把它常驻在内存里。
  if (state.status?.mode === 'streaming') return
  try {
    await invoke('warm_up')
  } catch (error) {
    console.warn('模型预热失败', error)
  }
}
