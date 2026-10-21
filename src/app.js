/* VoiceNotes 前端：录音、转写、历史、设置（无构建、无框架） */

const { invoke } = window.__TAURI__.core
const { listen } = window.__TAURI__.event
const { open: openDialog } = window.__TAURI__.dialog
const { writeText } = window.__TAURI__.clipboardManager
const { openPath } = window.__TAURI__.opener
const { t } = window.I18N

const LANGS = ['auto', 'zh', 'en', 'yue', 'ja', 'ko']
const HISTORY_KEY = 'voicenotes.history.v1'
const THEME_KEY = 'voicenotes.theme'
const LOCALE_KEY = 'voicenotes.locale'
const BARS = 48
// 空闲时的静态波形剪影，避免整片区空着
const IDLE_WAVE = [4, 6, 9, 13, 18, 24, 19, 14, 10, 7, 5, 8, 12, 17, 23, 28, 22, 16, 11, 7, 5, 6, 10, 15, 21, 26, 20, 15, 10, 7, 6, 9, 14, 19, 24, 18, 13, 9, 6, 5, 7, 11, 16, 21, 17, 12, 8, 5]
const MAX_SECONDS = 600
const MIB = 1024 * 1024

const $ = (id) => document.getElementById(id)

let deviceSelect = null
let localeSelect = null

const state = {
  status: null,
  phase: 'idle',
  levels: new Array(BARS).fill(0),
  startedAt: 0,
  timer: 0,
  history: loadHistory(),
  engine: 'idle',
  downloading: false,
  downloadingStreaming: false,
  downloadingPunct: false,
  view: 'transcribe',
  lastTranscript: null,
  toastTimer: 0,
}

const STREAM_ASSETS = new Set(['encoder.int8.onnx', 'decoder.int8.onnx', 'joiner.int8.onnx', 'tokens.txt'])
const STREAM_LANGS = ['auto', 'zh']

const els = {
  app: document.querySelector('.app'),
  badge: $('model-badge'),
  dot: $('engine-dot'),
  setup: $('setup'),
  setupProgress: $('setup-progress'),
  setupFill: $('setup-progress-fill'),
  setupText: $('setup-progress-text'),
  setupPct: $('setup-progress-pct'),
  downloadBtn: $('download-btn'),
  pickModelBtn: $('pick-model-btn'),
  cancelDownloadBtn: $('cancel-download-btn'),
  wave: $('wave'),
  mic: $('mic-btn'),
  timer: $('timer'),
  hint: $('hint'),
  liveLine: $('live-line'),
  modeRow: $('mode-row'),
  streamSetup: $('stream-setup'),
  streamSetupText: $('stream-setup-text'),
  streamProgress: $('stream-progress'),
  streamProgressFill: $('stream-progress-fill'),
  streamProgressText: $('stream-progress-text'),
  streamProgressPct: $('stream-progress-pct'),
  streamDownload: $('stream-download'),
  streamCancel: $('stream-cancel'),
  langRow: $('lang-row'),
  deviceSelect: $('device-select'),
  localeSelect: $('locale-select'),
  text: $('text'),
  meta: $('meta'),
  copyBtn: $('copy-btn'),
  saveBtn: $('save-btn'),
  clearBtn: $('clear-btn'),
  historyList: $('history-list'),
  historyEmpty: $('history-empty'),
  clearHistoryBtn: $('clear-history-btn'),
  settingsModel: $('settings-model'),
  settingsOutput: $('settings-output'),
  settingsVersion: $('settings-version'),
  settingsDownload: $('settings-download'),
  settingsPickModel: $('settings-pick-model'),
  settingsClearModel: $('settings-clear-model'),
  settingsPickOutput: $('settings-pick-output'),
  settingsOpenOutput: $('settings-open-output'),
  itnToggle: $('itn-toggle'),
  punctToggle: $('punct-toggle'),
  hybridToggle: $('hybrid-toggle'),
  hybridStatus: $('hybrid-status'),
  punctStatus: $('punct-status'),
  punctDownload: $('punct-download'),
  toast: $('toast'),
  dropOverlay: $('drop-overlay'),
  fileInput: $('file-input'),
  pickFileBtn: $('pick-file-btn'),
  viewTitle: $('view-title'),
  headHint: $('head-hint'),
  modelMode: $('model-mode'),
  historyCount: $('history-count'),
  newBtn: $('new-btn'),
  navItems: document.querySelectorAll('.nav-item'),
}

const VIEW_TITLES = { transcribe: t('view.transcribe'), history: t('view.history'), settings: t('view.settings') }

function switchView(view) {
  if (!VIEW_TITLES[view]) return
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
  els.viewTitle.textContent = VIEW_TITLES[view]
}

/// 模型徽标文案：流式与整段用同一套说法，避免两处提示对不上。
function modelBadgeText(phase) {
  const name = state.status?.mode === 'streaming' ? t('model.label.streaming') : t('model.label.offline')
  if (phase === 'error') return t('model.badge.error', { name })
  if (phase === 'loading') return t('model.badge.loading', { name })
  return t('model.badge.ready', { name })
}

/* ── 基础工具 ─────────────────────────────────────────── */

function fmtError(error) {
  const text = typeof error === 'string' ? error : error?.message || String(error)
  return text.replace(/^Error:\s*/, '')
}

function fmtBytes(bytes) {
  if (!bytes) return '0 MB'
  return `${(bytes / MIB).toFixed(bytes < 100 * MIB ? 1 : 0)} MB`
}

function fmtClock(seconds) {
  const total = Math.max(0, Math.floor(seconds))
  const mm = String(Math.floor(total / 60)).padStart(2, '0')
  const ss = String(total % 60).padStart(2, '0')
  return `${mm}:${ss}`
}

function fmtDuration(seconds) {
  if (seconds < 60) return t('duration.seconds', { seconds: seconds.toFixed(1) })
  return t('duration.minutes', { minutes: Math.floor(seconds / 60), seconds: Math.round(seconds % 60) })
}

function loadHistory() {
  try {
    const parsed = JSON.parse(localStorage.getItem(HISTORY_KEY))
    if (!Array.isArray(parsed)) return []
    return parsed.filter((item) => item && typeof item.text === 'string')
  } catch {
    return []
  }
}

function saveHistory() {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(state.history.slice(0, 100)))
  } catch {
    /* 存储满时忽略 */
  }
}

function toast(message, type = 'info') {
  clearTimeout(state.toastTimer)
  els.toast.textContent = message
  els.toast.classList.toggle('error', type === 'error')
  els.toast.classList.remove('hidden')
  requestAnimationFrame(() => els.toast.classList.add('show'))
  state.toastTimer = setTimeout(() => {
    els.toast.classList.remove('show')
    setTimeout(() => els.toast.classList.add('hidden'), 200)
  }, type === 'error' ? 4200 : 2400)
}

/* ── 录音界面 ─────────────────────────────────────────── */

const HINTS = {
  requesting: t('record.preparing'),
  recording: t('record.recording'),
  transcribing: t('record.transcribing'),
}

function idleHint() {
  return state.status?.mode === 'streaming' ? t('record.idleStreaming') : t('record.idle')
}

function setPhase(phase) {
  state.phase = phase
  els.app.dataset.phase = phase
  els.mic.setAttribute('aria-label', phase === 'recording' ? t('record.micStop') : t('record.micStart'))
  els.mic.querySelector('.mic-glyph').classList.toggle('hidden', phase === 'recording')
  els.mic.querySelector('.stop-glyph').classList.toggle('hidden', phase !== 'recording')
  els.mic.disabled = phase === 'requesting' || phase === 'transcribing'
  if (phase === 'idle') els.hint.textContent = idleHint()
  else if (HINTS[phase]) els.hint.textContent = HINTS[phase]
  if (phase !== 'recording') {
    els.liveLine.classList.add('hidden')
    els.liveLine.textContent = ''
  }
  if (phase === 'idle') resetWave()
  renderMode()
}

function buildWave() {
  for (let i = 0; i < BARS; i++) els.wave.appendChild(document.createElement('span'))
}

function renderWave() {
  const bars = els.wave.children
  const recording = state.phase === 'recording'
  for (let i = 0; i < bars.length; i++) {
    const height = recording
      ? 4 + Math.round((state.levels[i] || 0) * 62)
      : IDLE_WAVE[i % IDLE_WAVE.length]
    bars[i].style.height = `${height}px`
  }
}

function resetWave() {
  state.levels.fill(0)
  renderWave()
}

function pushLevel(level) {
  state.levels.push(Number(level) || 0)
  state.levels.shift()
  renderWave()
}

function startTimer() {
  stopTimer()
  state.timer = setInterval(() => {
    const elapsed = (performance.now() - state.startedAt) / 1000
    els.timer.textContent = fmtClock(elapsed)
    if (elapsed >= MAX_SECONDS && state.phase === 'recording') void stopRecording()
  }, 100)
}

function stopTimer() {
  if (state.timer) clearInterval(state.timer)
  state.timer = 0
}

async function startRecording() {
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

async function stopRecording() {
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

async function cancelRecording() {
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

function showTranscript(transcript) {
  state.lastTranscript = transcript
  const text = (transcript.text || '').trim()
  els.text.value = text
  if (!text) {
    els.meta.innerHTML = '<span>' + t('record.empty') + '</span>'
    return
  }
  const audio = transcript.durationSeconds || 0
  const elapsed = (transcript.elapsedMs || 0) / 1000
  const rtf = audio > 0 ? (elapsed / audio).toFixed(2) : '—'
  const chars = [...text].length
  els.meta.innerHTML = ''
  for (const part of [
    t('result.audio', { duration: fmtDuration(audio) }),
    t('result.elapsed', { seconds: elapsed.toFixed(1) }),
    t('result.rtf', { rtf }),
    t('result.chars', { count: chars }),
    transcript.corrected ? t('result.corrected') : null,
  ]) {
    if (!part) continue
    const span = document.createElement('span')
    span.textContent = part
    els.meta.appendChild(span)
  }
  addHistory(transcript, text)
}

/* ── 历史记录 ─────────────────────────────────────────── */

function addHistory(transcript, text) {
  state.history.unshift({
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    text,
    at: Date.now(),
    durationSeconds: transcript.durationSeconds || 0,
    elapsedMs: transcript.elapsedMs || 0,
    language: transcript.language || 'auto',
  })
  state.history = state.history.slice(0, 100)
  saveHistory()
  renderHistory()
}

function renderHistory() {
  els.historyCount.textContent = String(state.history.length)
  els.historyList.innerHTML = ''
  els.historyEmpty.classList.toggle('hidden', state.history.length > 0)
  for (const item of state.history) {
    const li = document.createElement('li')
    li.className = 'history-item'

    const main = document.createElement('button')
    main.type = 'button'
    main.className = 'history-main'
    main.title = t('history.load')
    const text = document.createElement('div')
    text.className = 'history-text'
    text.textContent = item.text
    const sub = document.createElement('div')
    sub.className = 'history-sub'
    const when = new Date(item.at).toLocaleString('zh-CN', {
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
    const parts = [when, fmtDuration(item.durationSeconds || 0), t('result.chars', { count: [...item.text].length })]
    for (const part of parts) {
      const span = document.createElement('span')
      span.textContent = part
      sub.appendChild(span)
    }
    main.append(text, sub)
    main.addEventListener('click', () => {
      els.text.value = item.text
      els.meta.innerHTML = ''
      const span = document.createElement('span')
      span.textContent = t('history.from', { when })
      els.meta.appendChild(span)
      els.text.focus()
    })

    const actions = document.createElement('div')
    actions.className = 'history-actions'
    actions.append(
      iconAction(t('history.copy'), copyIcon(), () => copyText(item.text)),
      iconAction(t('history.save'), saveIcon(), () => saveText(item.text)),
      iconAction(t('history.delete'), trashIcon(), () => {
        state.history = state.history.filter((entry) => entry.id !== item.id)
        saveHistory()
        renderHistory()
      }),
    )

    li.append(main, actions)
    els.historyList.appendChild(li)
  }
}

function iconAction(label, icon, onClick) {
  const button = document.createElement('button')
  button.type = 'button'
  button.className = 'icon-btn sm'
  button.title = label
  button.setAttribute('aria-label', label)
  button.innerHTML = icon
  button.addEventListener('click', onClick)
  return button
}

function copyIcon() {
  return '<svg viewBox="0 0 24 24" width="15" height="15"><rect x="9" y="9" width="11" height="11" rx="2.5" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M15 5.5A2.5 2.5 0 0 0 12.5 3H6.5A2.5 2.5 0 0 0 4 5.5v6A2.5 2.5 0 0 0 6.5 14" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>'
}

function saveIcon() {
  return '<svg viewBox="0 0 24 24" width="15" height="15"><path d="M12 4v10m0 0 3.5-3.5M12 14l-3.5-3.5M5 16v3h14v-3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}

function trashIcon() {
  return '<svg viewBox="0 0 24 24" width="15" height="15"><path d="M5 7h14M10 7V5h4v2m-7 0 .8 12h8.4L17 7" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>'
}

/* ── 复制 / 保存 ──────────────────────────────────────── */

async function copyText(text) {
  if (!text || !text.trim()) {
    toast(t('result.noCopy'))
    return
  }
  try {
    await writeText(text)
    toast(t('result.copied'))
  } catch {
    try {
      await navigator.clipboard.writeText(text)
      toast(t('result.copied'))
    } catch {
      toast(t('result.copyFailed'), 'error')
    }
  }
}

function buildFileName(text) {
  const head = text.replace(/\s+/g, ' ').trim().slice(0, 18).replace(/[\\/:*?"<>|]/g, '')
  const now = new Date()
  const pad = (n) => String(n).padStart(2, '0')
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  return t('history.filename', { head: head || t('view.transcribe'), stamp })
}

async function saveText(text) {
  if (!text || !text.trim()) {
    toast(t('result.noSave'))
    return
  }
  try {
    const saved = await invoke('save_text', { text, fileName: buildFileName(text) })
    toast(t('result.saved', { path: saved.path }))
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

/* ── 状态 / 模型 ──────────────────────────────────────── */

async function refreshStatus() {
  try {
    state.status = await invoke('status')
    renderStatus()
  } catch (error) {
    toast(t('result.readStatusFailed', { error: fmtError(error) }), 'error')
  }
}

function renderStatus() {
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
    : t('settings.punctMissing', { size: fmtBytes(state.punctTotalBytes || 0) })
  els.punctDownload.classList.toggle('hidden', status.punctReady)
  els.hybridToggle.checked = !!status.hybrid
  els.hybridStatus.textContent = status.modelReady
    ? t('settings.hybridReady')
    : t('settings.hybridNeedModel')
  els.modelMode.textContent = streamingMode ? t('model.name.streaming') : t('model.name.offline')

  for (const button of els.langRow.children) {
    button.setAttribute('aria-pressed', String(button.dataset.lang === status.language))
  }
  renderMode()
}

function renderMode() {
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

async function setMode(mode) {
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

function setStreamProgress(done, total) {
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0
  els.streamProgressFill.style.width = `${pct}%`
  els.streamProgressPct.textContent = `${pct}%`
}

async function downloadStreamingModel() {
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

function showPartial(partial) {
  if (state.phase !== 'recording') return
  const text = (partial || '').trim()
  if (!text) {
    els.liveLine.classList.add('hidden')
    els.liveLine.textContent = ''
    return
  }
  els.liveLine.textContent = text.length > 160 ? `…${text.slice(-160)}` : text
  els.liveLine.classList.remove('hidden')
}

function renderLang() {
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

/// 列出输入设备，第一项是「系统默认」。
async function loadDevices() {
  if (!deviceSelect) return
  let devices = []
  try {
    devices = (await invoke('list_input_devices')) || []
  } catch {
    devices = []
  }
  const items = [{ value: '', label: t('record.deviceDefault') }]
  for (const device of devices) {
    items.push({
      value: device.name,
      label: device.isDefault ? `${device.name} · ${t('record.deviceSystemDefault')}` : device.name,
    })
  }
  const current = state.status?.inputDevice || deviceSelect.value || ''
  deviceSelect.setItems(items, items.some((item) => item.value === current) ? current : '')
}

/// 界面语言下拉。
function localeItems() {
  return [
    { value: 'system', label: t('settings.localeSystem') },
    { value: 'zh-CN', label: t('settings.localeZh') },
    { value: 'en', label: t('settings.localeEn') },
  ]
}

/// 切换界面语言：存到 localStorage，然后重绘所有动态文案。
function applyLocale(preference) {
  const stored = preference || localStorage.getItem(LOCALE_KEY) || 'system'
  I18N.setLocale(stored)
  if (preference) localStorage.setItem(LOCALE_KEY, preference)
  renderLang()
  renderHistory()
  renderStatus()
  if (deviceSelect) void loadDevices()
  if (localeSelect) localeSelect.setItems(localeItems(), stored)
  if (state.lastTranscript) showTranscript(state.lastTranscript)
}

async function setLanguage(language) {
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

async function warmUp() {
  if (!state.status?.modelReady) return
  try {
    await invoke('warm_up')
  } catch (error) {
    console.warn('模型预热失败', error)
  }
}

function setProgress(done, total) {
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0
  els.setupFill.style.width = `${pct}%`
  els.setupPct.textContent = `${pct}%`
}

async function downloadModel() {
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

async function pickModelDir() {
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

/* ── 文件转写（浏览器解码 + 重采样） ──────────────────── */

function encodeWave(samples) {
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

function audioBase64(bytes) {
  let text = ''
  for (let i = 0; i < bytes.length; i += 8192) {
    text += String.fromCharCode(...bytes.subarray(i, i + 8192))
  }
  return btoa(text)
}

async function downloadPunctuationModel() {
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

async function transcribeFile(file) {
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

async function pickOutputDir() {
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

async function openOutputDir() {
  const dir = state.status?.outputDir
  if (!dir) return
  try {
    await openPath(dir)
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

/* ── 事件绑定 ─────────────────────────────────────────── */

function bindUI() {
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

function toggleTheme() {
  const current =
    document.documentElement.dataset.theme ||
    (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
  const next = current === 'dark' ? 'light' : 'dark'
  document.documentElement.dataset.theme = next
  localStorage.setItem(THEME_KEY, next)
}

/* ── 启动 ─────────────────────────────────────────────── */

async function listenEvents() {
  await listen('audio:level', (event) => {
    const payload = event.payload || {}
    pushLevel(payload.level || 0)
    if (state.phase === 'recording' && typeof payload.elapsed === 'number') {
      els.timer.textContent = fmtClock(payload.elapsed)
    }
  })

  await listen('audio:error', (event) => {
    toast(fmtError(event.payload), 'error')
    if (state.phase === 'recording') void cancelRecording()
  })

  await listen('model:progress', (event) => {
    const progress = event.payload || {}
    if (state.downloadingPunct) {
      const pct = progress.total > 0 ? Math.round(((progress.done || 0) / progress.total) * 100) : 0
      els.punctStatus.textContent = t('progress.punct', { pct, done: fmtBytes(progress.done || 0), total: fmtBytes(progress.total || 0) })
      return
    }
    if (state.downloadingStreaming) {
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
    if (progress.phase === 'error' && progress.message) toast(progress.message, 'error')
  })

  await listen('stream:partial', (event) => {
    const payload = event.payload || {}
    if (typeof payload.committed === 'string' && els.text.value !== payload.committed) {
      els.text.value = payload.committed
    }
    showPartial(payload.partial)
  })

  await listen('stream:segment', (event) => {
    const payload = event.payload || {}
    if (typeof payload.committed === 'string') els.text.value = payload.committed
    showPartial('')
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

async function init() {
  document.body.classList.toggle('macos', navigator.userAgent.includes('Mac'))
  const theme = localStorage.getItem(THEME_KEY)
  if (theme) document.documentElement.dataset.theme = theme

  applyLocale(localStorage.getItem(LOCALE_KEY) || 'system')
  buildWave()
  renderLang()
  renderHistory()
  bindUI()
  void loadDevices()
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
