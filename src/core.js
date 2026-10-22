/* 全局状态、DOM 引用、通用工具：所有模块的底座，不依赖任何业务模块 */

export const t = window.I18N.t
export const I18N = window.I18N

export const LANGS = ['auto', 'zh', 'en', 'yue', 'ja', 'ko']
export const HISTORY_KEY = 'voicenotes.history.v1'
export const THEME_KEY = 'voicenotes.theme'
export const LOCALE_KEY = 'voicenotes.locale'
export const BARS = 48
// 空闲时的静态波形剪影，避免整片区空着
export const IDLE_WAVE = [4, 6, 9, 13, 18, 24, 19, 14, 10, 7, 5, 8, 12, 17, 23, 28, 22, 16, 11, 7, 5, 6, 10, 15, 21, 26, 20, 15, 10, 7, 6, 9, 14, 19, 24, 18, 13, 9, 6, 5, 7, 11, 16, 21, 17, 12, 8, 5]
export const MIB = 1024 * 1024

export const STREAM_LANGS = ['auto', 'zh']

export const $ = (id) => document.getElementById(id)

/* 历史记录的原始存储读写：core 的 state 初始化要用，所以放在最底层 */
export function historyStorageGet() {
  try {
    const parsed = JSON.parse(localStorage.getItem(HISTORY_KEY))
    if (!Array.isArray(parsed)) return []
    return parsed.filter((item) => item && typeof item.text === 'string')
  } catch {
    return []
  }
}

export function historyStorageSet(items) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(items.slice(0, 100)))
  } catch {
    /* 存储满时忽略 */
  }
}

export const state = {
  status: null,
  phase: 'idle',
  levels: new Array(BARS).fill(0),
  startedAt: 0,
  timer: 0,
  history: historyStorageGet(),
  engine: 'idle',
  downloading: false,
  downloadingStreaming: false,
  downloadingPunct: false,
  view: 'transcribe',
  lastTranscript: null,
  toastTimer: 0,
  /// 正在转写文件（用于显示「第 N 句」进度）
  transcribingFile: false,
  fileSegments: 0,
}

export const els = {
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
  listening: $('listening'),
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

/* ── 基础工具 ─────────────────────────────────────────── */

export function fmtError(error) {
  const text = typeof error === 'string' ? error : error?.message || String(error)
  return text.replace(/^Error:\s*/, '')
}

export function fmtBytes(bytes) {
  if (!bytes) return '0 MB'
  return `${(bytes / MIB).toFixed(bytes < 100 * MIB ? 1 : 0)} MB`
}

export function fmtClock(seconds) {
  const total = Math.max(0, Math.floor(seconds))
  const mm = String(Math.floor(total / 60)).padStart(2, '0')
  const ss = String(total % 60).padStart(2, '0')
  return `${mm}:${ss}`
}

export function fmtDuration(seconds) {
  if (seconds < 60) return t('duration.seconds', { seconds: seconds.toFixed(1) })
  return t('duration.minutes', { minutes: Math.floor(seconds / 60), seconds: Math.round(seconds % 60) })
}

export function toast(message, type = 'info') {
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

/// 判断是不是 CJK 字符（和后端 asr::is_cjk 同一套范围）。
function isCjk(ch) {
  const code = ch.codePointAt(0);
  return (
    (code >= 0x3000 && code <= 0x303f) ||
    (code >= 0x3040 && code <= 0x30ff) ||
    (code >= 0x3400 && code <= 0x4dbf) ||
    (code >= 0x4e00 && code <= 0x9fff) ||
    (code >= 0xac00 && code <= 0xd7af) ||
    (code >= 0xf900 && code <= 0xfaff) ||
    (code >= 0xff00 && code <= 0xffef)
  )
}

/// 拼接两段文本：中日韩之间直接连，其余用空格。
///
/// 规则必须和后端 asr::join_segments 一致 —— 实时出字是逐句追加的，
/// 前端拼出来的全文要和最终结果一模一样。
export function joinText(left, right) {
  if (!left) return right
  if (!right) return left
  const needSpace = !isCjk(left[left.length - 1]) || !isCjk(right[0])
  return needSpace ? `${left} ${right}` : `${left}${right}`
}

/// 空闲时的提示语：整段/流式统一，模式差异由「整段 / 流式」芯片说明。
export function idleHint() {
  return t('record.idle')
}
