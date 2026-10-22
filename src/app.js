/* VoiceNotes 前端入口：初始化、事件绑定、全局协调（各功能在独立模块里） */

import {
  $, I18N, LOCALE_KEY, THEME_KEY,
  els, fmtError, state, t, toast,
} from './core.js'
import { invoke } from './tauri.js'
import { renderHistory, saveHistory } from './history.js'
import { showTranscript } from './result.js'
import {
  downloadModel, downloadPunctuationModel, downloadStreamingModel, loadDevices,
  localeItems, pickModelDir, refreshSelectItems, refreshStatus, renderLang, renderStatus,
  setMode, setSelects, warmUp,
} from './status.js'
import {
  buildWave, cancelRecording, renderMicLabel, startRecording, stopRecording,
} from './recorder.js'
import { openOutputDir, pickOutputDir, transcribeFile } from './files.js'
import { listenEvents } from './events.js'
import { copyText, saveText } from './export.js'
import { renderThemeIcon, syncWindowBackground, toggleTheme } from './theme.js'

let deviceSelect = null
let localeSelect = null

// 视图标题存 key、文案在渲染时取：模块顶层就 t() 的话切语言后标题不会更新
const VIEW_KEYS = { transcribe: 'view.transcribe', history: 'view.history', settings: 'view.settings' }

function viewTitle(view) {
  return t(VIEW_KEYS[view])
}

export function switchView(view) {
  if (!VIEW_KEYS[view]) return
  if (state.phase === 'recording' && view !== 'transcribe') {
    toast(t('toast.recordingBusy'))
    return
  }
  state.view = view
  els.app.dataset.view = view
  for (const panel of document.querySelectorAll('[data-view-panel]')) {
    panel.classList.toggle('hidden', panel.dataset.viewPanel !== view)
  }
  for (const item of els.navItems) {
    item.setAttribute('aria-current', item.dataset.view === view ? 'page' : 'false')
  }
  els.viewTitle.textContent = viewTitle(view)
}

/// 切换界面语言：存到 localStorage，然后重绘所有动态文案。
export function applyLocale(preference) {
  const stored = preference || localStorage.getItem(LOCALE_KEY) || 'system'
  I18N.setLocale(stored)
  if (preference) localStorage.setItem(LOCALE_KEY, preference)
  renderLang()
  renderHistory()
  renderStatus()
  renderThemeIcon()
  renderMicLabel()
  refreshSelectItems(stored)
  els.viewTitle.textContent = viewTitle(state.view)
  if (state.lastTranscript) showTranscript(state.lastTranscript)
}

/* ── 事件绑定 ─────────────────────────────────────────── */

export function bindUI() {
  els.mic.addEventListener('click', () => {
    if (state.phase === 'recording') void stopRecording()
    else if (state.phase === 'idle') void startRecording()
  })

  els.copyBtn.addEventListener('click', () => copyText(els.text.value))
  els.saveBtn.addEventListener('click', () => saveText(els.text.value))
  els.clearBtn.addEventListener('click', () => {
    els.text.value = ''
    els.meta.innerHTML = '<span>' + t('result.waiting') + '</span>'
    els.text.focus()
  })

  for (const chip of els.modeRow.children) {
    chip.addEventListener('click', () => void setMode(chip.dataset.mode))
  }
  els.streamDownload.addEventListener('click', () => void downloadStreamingModel())
  els.streamCancel.addEventListener('click', async () => {
    try {
      await invoke('cancel_download')
    } catch {
      /* 忽略 */
    }
  })

  els.downloadBtn.addEventListener('click', () => void downloadModel())
  els.cancelDownloadBtn.addEventListener('click', async () => {
    try {
      await invoke('cancel_download')
    } catch {
      /* 忽略 */
    }
  })
  els.pickModelBtn.addEventListener('click', () => void pickModelDir())

  for (const item of els.navItems) {
    item.addEventListener('click', () => switchView(item.dataset.view))
  }
  els.newBtn.addEventListener('click', () => {
    if (state.phase !== 'idle') {
      toast(t('toast.recordingBusy'))
      return
    }
    switchView('transcribe')
    els.text.value = ''
    els.meta.innerHTML = '<span>' + t('result.waiting') + '</span>'
    els.timer.textContent = '00:00'
    els.text.focus()
  })
  $('theme-btn').addEventListener('click', toggleTheme)
  els.settingsDownload.addEventListener('click', () => {
    switchView('transcribe')
    void downloadModel()
  })
  els.settingsPickModel.addEventListener('click', () => {
    switchView('transcribe')
    void pickModelDir()
  })
  els.settingsClearModel.addEventListener('click', async () => {
    try {
      state.status = await invoke('set_model_dir', { dir: null })
      renderStatus()
      if (state.status.modelReady) {
        toast(t('settings.switchedAppModel'))
        void warmUp()
      } else {
        toast(t('settings.switchedAppModelNeedDownload'))
      }
    } catch (error) {
      toast(fmtError(error), 'error')
    }
  })
  els.settingsPickOutput.addEventListener('click', () => void pickOutputDir())
  els.settingsOpenOutput.addEventListener('click', () => void openOutputDir())
  els.punctDownload.addEventListener('click', () => void downloadPunctuationModel())
  els.punctToggle.addEventListener('change', async () => {
    const wants = els.punctToggle.checked
    if (wants && !state.status?.punctReady) {
      els.punctToggle.checked = false
      toast(t('settings.punctNeedDownload'))
      return
    }
    try {
      state.status = await invoke('set_punct', { enabled: wants })
      renderStatus()
      toast(wants ? t('settings.punctAutoOn') : t('settings.punctAutoOff'))
    } catch (error) {
      els.punctToggle.checked = !wants
      toast(fmtError(error), 'error')
    }
  })

  els.hybridToggle.addEventListener('change', async () => {
    const wants = els.hybridToggle.checked
    try {
      state.status = await invoke('set_hybrid', { enabled: wants })
      renderStatus()
      toast(wants ? t('settings.hybridOn') : t('settings.hybridOff'))
    } catch (error) {
      els.hybridToggle.checked = !wants
      toast(fmtError(error), 'error')
    }
  })

  els.itnToggle.addEventListener('change', async () => {
    try {
      state.status = await invoke('set_itn', { enabled: els.itnToggle.checked })
      renderStatus()
    } catch (error) {
      toast(fmtError(error), 'error')
    }
  })

  els.clearHistoryBtn.addEventListener('click', () => {
    if (!state.history.length) return
    state.history = []
    saveHistory()
    renderHistory()
  })

  localeSelect = UISelect.create(els.localeSelect, {
    onChange: (value) => applyLocale(value),
  })
  localeSelect.setItems(localeItems(), localStorage.getItem(LOCALE_KEY) || 'system')
  deviceSelect = UISelect.create(els.deviceSelect, {
    // 列设备会触发 macOS 的麦克风权限弹窗，所以只在用户打开下拉框时才列
    onBeforeOpen: loadDevices,
    onChange: async (value) => {
      const name = value || null
      try {
        state.status = await invoke('set_input_device', { name })
        toast(name ? t('record.deviceChanged', { name }) : t('record.deviceChangedDefault'))
      } catch (error) {
        toast(fmtError(error), 'error')
      }
    },
  })
  // 先放上已保存的设备名（不枚举设备，避免启动就触发权限弹窗）
  const savedDevice = state.status?.inputDevice || ''
  deviceSelect.setItems(
    [{ value: savedDevice, label: savedDevice || t('record.deviceDefault') }],
    savedDevice,
  )
  setSelects(deviceSelect, localeSelect)
  els.pickFileBtn?.addEventListener('click', () => els.fileInput.click())
  els.fileInput.addEventListener('change', () => {
    const file = els.fileInput.files?.[0]
    els.fileInput.value = ''
    if (file) void transcribeFile(file)
  })

  document.addEventListener('keydown', (event) => {
    const typing = ['INPUT', 'TEXTAREA'].includes(event.target.tagName) || event.target.isContentEditable
    if (event.key === 'Escape' && (state.phase === 'recording' || state.phase === 'requesting')) {
      event.preventDefault()
      void cancelRecording()
      return
    }
    if (event.key === 'Escape' && state.view !== 'transcribe') {
      switchView('transcribe')
      return
    }
    if (typing) return
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'o') {
      event.preventDefault()
      els.fileInput.click()
      return
    }
    if (event.code === 'Space' && state.view === 'transcribe') {
      event.preventDefault()
      if (state.phase === 'recording') void stopRecording()
      else if (state.phase === 'idle') void startRecording()
    }
  })

  let dragDepth = 0
  const hasFiles = (event) => Array.from(event.dataTransfer?.types || []).includes('Files')
  window.addEventListener('dragenter', (event) => {
    if (!hasFiles(event)) return
    event.preventDefault()
    dragDepth += 1
    els.dropOverlay.classList.remove('hidden')
  })
  window.addEventListener('dragover', (event) => {
    if (hasFiles(event)) event.preventDefault()
  })
  window.addEventListener('dragleave', () => {
    dragDepth = Math.max(0, dragDepth - 1)
    if (dragDepth === 0) els.dropOverlay.classList.add('hidden')
  })
  window.addEventListener('drop', (event) => {
    event.preventDefault()
    dragDepth = 0
    els.dropOverlay.classList.add('hidden')
    const file = event.dataTransfer?.files?.[0]
    if (file) void transcribeFile(file)
  })
}

/* ── 启动 ─────────────────────────────────────────────── */


async function init() {
  document.body.classList.toggle('macos', navigator.userAgent.includes('Mac'))
  const theme = localStorage.getItem(THEME_KEY)
  if (theme) document.documentElement.dataset.theme = theme
  syncWindowBackground()
  renderThemeIcon()
  // 没手动选过主题时，系统切换深浅色要跟着变
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if (!localStorage.getItem(THEME_KEY)) renderThemeIcon()
  })

  applyLocale(localStorage.getItem(LOCALE_KEY) || 'system')
  buildWave()
  renderLang()
  renderHistory()
  bindUI()
  // 界面画完再去列设备：列设备会触发 macOS 的麦克风权限弹窗，
  // 放在渲染之后弹，用户看到的是已经画好的界面。
  setTimeout(() => {
    void loadDevices()
  }, 900)
  await listenEvents()
  await refreshStatus()
  switchView('transcribe')
  if (state.status?.modelReady) void warmUp()
  if (state.status?.mode === 'streaming' && state.status.streamingReady) {
    void invoke('warm_up_streaming').catch(() => {})
  }
}

init().catch((error) => {
  console.error(error)
  toast(t('app.startFailed', { error: fmtError(error) }), 'error')
})
