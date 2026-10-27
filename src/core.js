/* 全局状态、DOM 引用、通用工具：所有模块的底座，不依赖任何业务模块 */

import { logEvent } from './tauri.js'

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
  view: 'transcribe',
  lastTranscript: null,
  recordingSessionId: 0,
  fileSessionId: 0,
  fileAttemptId: 0,
  /// 录音/启动请求的代号：取消时 +1，让还在 await 的启动流程自己放弃。
  requestId: 0,
  toastTimer: 0,
  /// 正在转写文件（用于显示「第 N 句」进度）
  transcribingFile: false,
  fileSegments: 0,
  /// 文件转写的开始时刻（performance.now()）：用来显示「已用时」
  fileStartedAt: 0,
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
  punctStatus: $('punct-status'),
  hotwordsInput: $('hotwords-input'),
  hotwordsSave: $('hotwords-save'),
  hotwordsExample: $('hotwords-example'),
  toast: $('toast'),
  dropOverlay: $('drop-overlay'),
  pickFileBtn: $('pick-file-btn'),
  viewTitle: $('view-title'),
  modelMode: $('model-mode'),
  historyCount: $('history-count'),
  newBtn: $('new-btn'),
  navItems: document.querySelectorAll('.nav-item'),
}

/* ── 基础工具 ─────────────────────────────────────────── */

/// 后端错误 → 界面文案。
///
/// 后端只返回字典 key，不返回展示文案（AGENTS.md）：
///   字符串 "record.empty" / "record.micError:打开失败"
///   事件载荷对象 { code, detail?, seconds?, fatal? }
///
/// **界面只显示字典里的干净文案**：技术细节（系统错误文本、路径、格式名）
/// 由后端写进日志，不往界面上贴。字典里能填的只有用户看得懂的参数：
/// `{value}`（一个具体值）和 `{seconds}`（秒数）。
/// 字典里没有这个 key → 显示「未知错误」，同样不把原文漏给用户。
export function fmtError(error) {
  const unknown = () => {
    const detail = error && typeof error === 'object'
      ? error.detail || error.message || JSON.stringify(error)
      : errorText(error)
    logEvent('ERROR', `未翻译的异常: ${detail}`)
    return t('error.unknown')
  }
  if (error && typeof error === 'object' && !Array.isArray(error)) {
    const vars = {}
    if (error.seconds != null) vars.seconds = error.seconds
    const text = translateCode(error.code, vars)
    if (text) return text
    if (isErrorCode(error.code)) return t('error.unknown')
    return unknown()
  }
  const raw = (typeof error === 'string' ? error : error?.message || String(error)).replace(/^Error:\s*/, '')
  const cut = raw.indexOf(':')
  const key = cut > 0 ? raw.slice(0, cut) : raw
  const detail = cut > 0 ? raw.slice(cut + 1) : ''
  // 细节只填 {value} / {seconds} 这类「用户看得懂的参数」。
  const vars = detail ? { value: detail, seconds: detail } : undefined
  const text = translateCode(key, vars)
  if (text) return text
  // 字典里没有这个 key → 未知错误，原文和细节只进日志，不往界面上贴。
  return isErrorCode(key) ? t('error.unknown') : unknown()
}

/// 像不像我们自己的错误码（`record.micError` 这种带点的）。
const ERROR_CODE_RE = /^[a-z][A-Za-z0-9]*(\.[A-Za-z0-9]+)+$/

function isErrorCode(key) {
  return typeof key === 'string' && ERROR_CODE_RE.test(key)
}

/// 字典里有这个 key 就返回译文，没有就返回 null（让调用方回退到原文）。
function translateCode(key, vars) {
  if (!key) return null
  const dict = I18N.locales[I18N.locale] || {}
  if (!(key in dict)) return null
  return t(key, vars)
}

export function fmtBytes(bytes) {
  if (!bytes) return '0 MB'
  return `${(bytes / MIB).toFixed(bytes < 100 * MIB ? 1 : 0)} MB`
}

/// 计时器：不足 1 小时显示 mm:ss，超过 1 小时显示 hh:mm:ss（录音不限时长，会录很久）
export function fmtClock(seconds) {
  const total = Math.max(0, Math.floor(seconds))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const pad = (n) => String(n).padStart(2, '0')
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`
}

/// 时长文案：秒 / 分秒 / 时分秒
export function fmtDuration(seconds) {
  const total = Math.max(0, seconds)
  if (total < 60) return t('duration.seconds', { seconds: total.toFixed(1) })
  let h = Math.floor(total / 3600)
  let m = Math.floor((total % 3600) / 60)
  let s = Math.round(total % 60)
  if (s === 60) {
    s = 0
    m += 1
  }
  if (m === 60) {
    m = 0
    h += 1
  }
  if (h > 0) return t('duration.hours', { hours: h, minutes: m, seconds: s })
  return t('duration.minutes', { minutes: m, seconds: s })
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

/// 把异常对象变成日志里的一行文字：message + stack。
/// stack 是诊断信息，只进日志文件，永远不到用户眼前。
export function errorText(error, fallback) {
  if (!error) return fallback || '(无详情)'
  if (typeof error === 'string') return error
  const message = error.message || fallback || String(error)
  const stack = error.stack
  if (!stack) return message
  // stack 开头通常已经带着 message，别重复两遍。
  return stack.includes(message) ? stack : `${message}\n${stack}`
}

/// 判断是不是 CJK 字符（和后端 asr::is_cjk 同一套范围）。
export function isCjk(ch) {
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

/// 缺口占位符：后端丢掉音频时只给 `[[gap:秒数]]`，界面语言在渲染时才决定。
const GAP_RE = /\[\[gap:(\d+)\]\]/g
const GAP_END_RE = /\[\[gap:\d+\]\]$/

/// 是不是缺口占位符（和后端 audio::is_gap_marker 同一套规则）。
export function isGapMarker(text) {
  return typeof text === 'string' && text.startsWith('[[gap:')
}

/// 缺口标记的文案：全角括号（中文）本身是 CJK，前后不用空格；
/// 半角括号（英文）要补空格，否则会粘在句子中间。
export function gapText(seconds) {
  const label = t('record.gap', { seconds })
  if (!label) return label
  const cjk = isCjk(label[0]) && isCjk(label[label.length - 1])
  return cjk ? label : ` ${label} `
}

/// 把后端文本里的缺口占位符换成当前语言的标记。
export function localizeText(text) {
  if (!text) return text || ''
  return text.replace(GAP_RE, (_, seconds) => gapText(Number(seconds)))
}

/// 拼接两段文本：中日韩之间直接连，其余用空格。
///
/// 规则必须和后端 asr::join_segments 一致 —— 实时出字是逐句追加的，
/// 前端拼出来的全文要和最终结果一模一样。缺口占位符前后不加空格。
export function joinText(left, right) {
  if (!left) return right
  if (!right) return left
  if (isGapMarker(right) || GAP_END_RE.test(left)) return `${left}${right}`
  const needSpace = !isCjk(left[left.length - 1]) || !isCjk(right[0])
  return needSpace ? `${left} ${right}` : `${left}${right}`
}

/// 空闲时的提示语：整段/流式统一，模式差异由「整段 / 流式」芯片说明。
export function idleHint() {
  return t('record.idle')
}
