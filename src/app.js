/* VoiceNotes 前端入口：初始化、事件绑定、全局协调（各功能在独立模块里） */

import {
  $, I18N, LOCALE_KEY, THEME_KEY,
  els, errorText, fmtError, state, t, toast,
} from './core.js'
import { getCurrentWindow, getCurrentWebview, invoke, logEvent } from './tauri.js'
import { renderHistory, saveHistory } from './history.js'
import { clearResult, showTranscript } from './result.js'
import {
  downloadModel, downloadStreamingModel, loadDevices,
  localeItems, pickModelDir, refreshSelectItems, refreshStatus, renderLang, renderStatus,
  setMode, setSelects, warmUp,
} from './status.js'
import {
  buildWave, cancelRecording, renderHint, renderMicLabel, startRecording, stopRecording,
} from './recorder.js'
import { openOutputDir, pickAndTranscribeFile, pickOutputDir, transcribeFilePath } from './files.js'
import { listenEvents } from './events.js'
import { copyText, saveText } from './export.js'
import { renderThemeIcon, syncWindowBackground, toggleTheme } from './theme.js'

let deviceSelect = null
let localeSelect = null
// 界面语言是否已经初始化过：第一次 applyLocale 是启动时的初始化，不算「切换」。
let localeReady = false

// 视图标题存 key、文案在渲染时取：模块顶层就 t() 的话切语言后标题不会更新
const VIEW_KEYS = { transcribe: 'view.transcribe', history: 'view.history', settings: 'view.settings' }

function viewTitle(view) {
  return t(VIEW_KEYS[view])
}

/* ── 异常捕获 ─────────────────────────────────────────── */

/// 装一次全局异常捕获：JS 异常和没被接住的 Promise 拒绝都送进日志文件。
/// 启动时立刻装（在 init() 之前），这样后面任何一处抛错都能记下来。
/// 这里只记日志，不弹窗、不改界面行为 —— 界面上的提示各有各的通路。
function installErrorLogging() {
  window.addEventListener('error', (event) => {
    try {
      if (event.error || event.message) {
        logEvent('ERROR', `JS 异常: ${errorText(event.error, event.message)}`)
        return
      }
      // 资源加载失败（script / img 等）：没有 error 对象，只有 target。
      const target = event.target
      logEvent('WARN', `资源加载失败: ${target?.src || target?.href || 'unknown'}`)
    } catch {
      /* 记日志失败也不能影响界面 */
    }
  })
  window.addEventListener('unhandledrejection', (event) => {
    try {
      logEvent('ERROR', `未处理的 Promise 拒绝: ${errorText(event.reason)}`)
    } catch {
      /* 忽略 */
    }
  })
}

installErrorLogging()

export function switchView(view) {
  if (!VIEW_KEYS[view]) return
  if (state.phase === 'recording' && view !== 'transcribe') {
    toast(t('toast.recordingBusy'))
    return
  }
  const changed = state.view !== view
  state.view = view
  els.app.dataset.view = view
  for (const panel of document.querySelectorAll('[data-view-panel]')) {
    panel.classList.toggle('hidden', panel.dataset.viewPanel !== view)
  }
  for (const item of els.navItems) {
    item.setAttribute('aria-current', item.dataset.view === view ? 'page' : 'false')
  }
  els.viewTitle.textContent = viewTitle(view)
  // 只记真的切换了视图的那一次（启动时的初始视图不算）。
  if (changed) logEvent('INFO', `视图切换: ${view}`)
}

/// 切换界面语言：存到 localStorage，然后重绘所有动态文案。
export function applyLocale(preference) {
  const stored = preference || localStorage.getItem(LOCALE_KEY) || 'system'
  const before = I18N.locale
  I18N.setLocale(stored)
  const after = I18N.locale
  if (preference) localStorage.setItem(LOCALE_KEY, preference)
  renderLang()
  renderHistory()
  renderStatus()
  renderThemeIcon()
  renderMicLabel()
  renderHint()
  refreshSelectItems(stored)
  els.viewTitle.textContent = viewTitle(state.view)
  // 原生窗口标题也跟着界面语言走（tauri.conf.json 里的只是启动默认值）。
  try {
    void getCurrentWindow().setTitle(t('app.title')).catch(() => {})
  } catch {
    /* 标题失败不能影响界面 */
  }
  // 只重绘，不写历史：否则每切一次语言就多一条重复记录。
  if (state.lastTranscript) showTranscript(state.lastTranscript, { record: false })
  // 启动时的初始化不算「切换」，只记用户真的改了语言的那一次。
  if (localeReady && after !== before) logEvent('INFO', `界面语言已切换: ${after}`)
  localeReady = true
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
    state.lastTranscript = null
    clearResult()
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

  for (const item of els.navItems) {
    item.addEventListener('click', () => switchView(item.dataset.view))
  }
  els.newBtn.addEventListener('click', () => {
    if (state.phase !== 'idle') {
      toast(t('toast.recordingBusy'))
      return
    }
    switchView('transcribe')
    state.lastTranscript = null
    clearResult()
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

  els.hotwordsSave.addEventListener('click', async () => {
    const hotwords = els.hotwordsInput.value
      .split('\n')
      .map((word) => word.trim())
      .filter(Boolean)
    try {
      state.status = await invoke('set_hotwords', { hotwords })
      renderStatus()
      toast(
        hotwords.length
          ? t('settings.hotwordsSaved', { count: hotwords.length })
          : t('settings.hotwordsCleared'),
      )
    } catch (error) {
      toast(fmtError(error), 'error')
    }
  })

  els.hotwordsExample.addEventListener('click', async () => {
    try {
      els.hotwordsInput.value = (await invoke('default_hotwords')).join('\n')
      toast(t('settings.hotwordsExampleFilled'))
    } catch (error) {
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
  els.pickFileBtn?.addEventListener('click', () => void pickAndTranscribeFile())

  document.addEventListener('keydown', (event) => {
    const target = event.target instanceof Element ? event.target : null
    const typing = !!target && (['INPUT', 'TEXTAREA'].includes(target.tagName) || target.isContentEditable)
    // 按钮 / 链接 / 下拉项上的按键归控件自己：否则按空格会变成开始录音。
    const onControl = !!target?.closest?.('button, a, [role="option"]')
    if (event.key === 'Escape' && (state.phase === 'recording' || state.phase === 'requesting')) {
      event.preventDefault()
      void cancelRecording()
      return
    }
    if (event.key === 'Escape' && state.view !== 'transcribe') {
      switchView('transcribe')
      return
    }
    if (typing || onControl) return
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'o') {
      event.preventDefault()
      void pickAndTranscribeFile()
      return
    }
    if (event.code === 'Space' && state.view === 'transcribe') {
      event.preventDefault()
      if (state.phase === 'recording') void stopRecording()
      else if (state.phase === 'idle') void startRecording()
    }
  })

  bindFileDrop()
}

/// 拖拽文件到窗口。
///
/// 用 Tauri 的拖放事件而不是 HTML5 的 drop：Tauri v2 默认拦截系统拖放
/// （`dragDropEnabled`），HTML5 的 drag/drop 事件在真实应用里根本不会触发。
function bindFileDrop() {
  const webview = getCurrentWebview()
  if (!webview?.onDragDropEvent) return
  webview.onDragDropEvent((event) => {
    const payload = event.payload || {}
    if (payload.type === 'over') {
      els.dropOverlay.classList.remove('hidden')
    } else if (payload.type === 'leave') {
      els.dropOverlay.classList.add('hidden')
    } else if (payload.type === 'drop') {
      els.dropOverlay.classList.add('hidden')
      const path = payload.paths?.[0]
      if (path) void transcribeFilePath(path)
    }
  })
}

/* ── 启动 ─────────────────────────────────────────────── */


async function init() {
  document.body.classList.toggle('macos', navigator.userAgent.includes('Mac'))
  const theme = localStorage.getItem(THEME_KEY)
  if (theme) document.documentElement.dataset.theme = theme
  syncWindowBackground()
  renderThemeIcon()
  // 没手动选过主题时，系统切换深浅色要跟着变（连窗口底色一起）
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if (!localStorage.getItem(THEME_KEY)) {
      renderThemeIcon()
      syncWindowBackground()
    }
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
  // 只预热当前模式要用的引擎：整段模式常驻 239 MB，流式用户用不到它。
  if (state.status?.mode === 'streaming') {
    if (state.status.streamingReady) void invoke('warm_up_streaming').catch(() => {})
  } else if (state.status?.modelReady) {
    void warmUp()
  }
}

init().catch((error) => {
  console.error(error)
  logEvent('ERROR', `启动失败: ${errorText(error)}`)
  toast(t('app.startFailed', { error: fmtError(error) }), 'error')
})
