/* 结果区：转写结果 + 实时中间结果 */

import { els, gapMarkerTexts, isGapMarker, joinText, localizeText, renderResultMeta, state } from './core.js'
import { addHistory } from './history.js'
import { clearIcon, copyIcon, saveIcon } from './icons.js'

/// 结果卡片右上角的三个动作按钮：纯图标，文字放在 title / aria-label 里。
/// 图标是静态的，切语言不用重绘 —— title / aria-label 由 i18n.apply() 填。
export function renderResultActions() {
  els.copyBtn.innerHTML = copyIcon()
  els.saveBtn.innerHTML = saveIcon()
  els.clearBtn.innerHTML = clearIcon()
}

// —— 结果框里的实时句子 ——
//
// 一次会话内维护一个**段列表**（句子与缺口）：每段记后端原文、时间区间与
// 到达序号。段列表是结果框的事实源 —— 渲染顺序按本地时间轴，而不是事件
// 到达顺序（云端的缺口段可能先于它所属的句子到达）。
//
// 段列表放模块级，**不要**放进 `state.resultView` —— `setResultView` 每次
// 增量追加都会整体覆盖 `resultView`，放进去会被冲掉。

/// 一次会话内的段（句子 / 缺口）。
let liveSegments = []
/// 到达序号：排序的 tie-break（数组下标会随重排变，不能用）。
let liveOrder = 0
/// 用户是否动过结果框（**粘滞**）：动过之后一律走追加分支。
let liveEdited = false
/// 本会话是否出现过没有时间的段：出现后整个会话退化为按到达顺序渲染。
let liveDegraded = false
/// 已渲染的最大 start（秒）；`-Infinity` = 还没有带时间的段。
let liveMaxStart = -Infinity
/// 最大 start 处有没有缺口（同一 start 时缺口要排在句子前面）。
let liveGapAtMax = false
/// 段列表之前已有的文本（会话开始时结果框里已有的内容）。
let liveBaseText = ''

/// 清空段列表：新会话开始、或结果区被整体重写时。
///
/// 段列表在 `clearResult` / `showTranscript` 的**入口**清 —— `showTranscript`
/// 的空转写分支会提前返回，末尾清不到；而 `setResultView` 是增量追加的路径，
/// 不能在那里清。
///
/// 载入历史（`src/history.js`）**不**清段列表：它只在空闲时发生，而空闲时
/// 事件闸门挡着所有段；下一次录音 / 文件转写都会 `clearResult` 重置。
function resetLiveSegments() {
  liveSegments = []
  liveOrder = 0
  liveEdited = false
  liveDegraded = false
  liveMaxStart = -Infinity
  liveGapAtMax = false
  liveBaseText = ''
}

/// 段按**本地时间轴**排序：排序键只有 start；同一 start 时缺口在前，
/// 其余按到达顺序（与后端 `insert_gaps` 的「同一时刻缺口排在句子前面」一致）。
function sortedSegments() {
  return [...liveSegments].sort((a, b) => {
    if (a.start !== b.start) return a.start - b.start
    const aGap = isGapMarker(a.text)
    const bGap = isGapMarker(b.text)
    if (aGap !== bGap) return aGap ? -1 : 1
    return a.order - b.order
  })
}

/// 从段列表重建全文。
///
/// 拼句用 `joinText` 逐段拼 —— 规则必须和后端 `asr::join_segments` 一致
/// （`check:join` 只比对那两处，**查不到这里的调用点**，别手写拼接）。
function rebuildRawText() {
  const list = liveDegraded ? liveSegments : sortedSegments()
  let out = liveBaseText
  for (const seg of list) out = joinText(out, seg.text)
  return out
}

function setResultView(rawText, meta) {
  const rendered = localizeText(rawText)
  state.resultView = { rawText, rendered, meta, markers: gapMarkerTexts(rawText) }
  els.text.value = rendered
  renderResultMeta()
}

/// 把编辑后文本里的旧占位符换成新语言的占位符。
///
/// 找不到就跳过 —— 用户把那一段改了，就别动它。
function swapGapMarkers(text, oldMarkers, newMarkers) {
  let out = text
  let from = 0
  for (let i = 0; i < oldMarkers.length; i++) {
    const oldMark = oldMarkers[i]
    const newMark = newMarkers[i]
    if (!oldMark || !newMark || oldMark === newMark) continue
    const at = out.indexOf(oldMark, from)
    if (at < 0) continue
    out = out.slice(0, at) + newMark + out.slice(at + oldMark.length)
    from = at + newMark.length
  }
  return out
}

/// 切语言只重画缺口占位符和元信息，不覆盖用户其他编辑。
export function relocalizeResult() {
  const view = state.resultView
  if (!view) return
  const rendered = localizeText(view.rawText)
  const markers = gapMarkerTexts(view.rawText)
  if (els.text.value === view.rendered) {
    // 用户没动过：整段按新语言重画。
    els.text.value = rendered
  } else {
    // 用户动过：只换缺口占位符，其他字符一个都不动。
    els.text.value = swapGapMarkers(els.text.value, view.markers || [], markers)
  }
  // rendered 永远存「原文的本地化结果」，不能写成用户文本 ——
  // 否则第二次切语言时守卫会误判成「用户没动过」，把编辑盖回原文。
  view.rendered = rendered
  view.markers = markers
  renderResultMeta()
}

/// 清空结果区：文本框清空，元信息区只留一条提示。
export function clearResult(label = 'result.waiting') {
  resetLiveSegments()
  setResultView('', { kind: 'label', key: label })
}

/// 渲染一次转写结果。`record: false` 只重绘、不写历史。
///
/// `emptyKey` 是「什么都没识别出来」时结果区显示的那句话：
/// 录音说「请检查麦克风与录音环境」（麦克风的锅），文件说「没有识别到内容」
/// （文件的内容已经定下来了，别给建议）。
export function showTranscript(transcript, { record = true, emptyKey = 'record.empty' } = {}) {
  // 入口就清段列表：空转写会走下面的早退分支，末尾清不到。
  resetLiveSegments()
  const raw = (transcript.text || '').trim()
  if (!raw) {
    setResultView('', { kind: 'label', key: emptyKey })
    return
  }
  const audio = transcript.durationSeconds || 0
  const elapsed = (transcript.elapsedMs || 0) / 1000
  setResultView(raw, {
    kind: 'transcript',
    audio,
    elapsed,
    rtf: audio > 0 ? (elapsed / audio).toFixed(2) : '—',
  })
  // 历史存**原文**（带 [[gap:N]] 占位符），不是渲染后的文字 ——
  // 存渲染结果的话，历史里的缺口提示会永远停在存档时那一种语言。
  if (record) addHistory(transcript, raw)
}

/// 追加一句识别好的文本（事件是增量载荷：只带新句子）。
///
/// `start` / `end` 是句子在**本地录音时间轴**上的区间（秒）。缺 `start`
/// 的事件（文件转写的老事件）没有时间信息，排不了序。
export function appendSegment(text, start, end) {
  // 取消 / 收尾结束之后迟到的句子不再追加，否则结果框会冒出被取消的内容。
  if (state.phase !== 'recording' && state.phase !== 'transcribing') return
  const piece = (text || '').trim()
  if (!piece) return
  const view = state.resultView
  // 用户动过结果框：把新句接到他们改过的文本后面，别整段重画覆盖掉。
  // `rendered` 仍保持「原文的本地化结果」—— relocalizeResult 靠它判断
  // 文本有没有被编辑过，写成用户文本会让切语言时再次覆盖。
  //
  // 标志是**粘滞**的：文本比较只能看出「此刻有没有被改过」，用户改完又
  // 撤销回原样时它会变回false —— 那时再从段列表重拼，就把编辑期间追加的
  // 句子弄丢了。所以一旦发现被改过，本会话就一直走追加分支。
  if (liveEdited || (view && els.text.value !== view.rendered)) {
    liveEdited = true
    const rawText = joinText(view?.rawText || els.text.value, piece)
    // 缺口段要先本地化再上屏，否则用户看到的是内部标记 [[gap:N]]。
    // 缺口文案自带空格规则（gapText），别再让 joinText 补一个。
    const shown = localizeText(piece)
    els.text.value = isGapMarker(piece) ? `${els.text.value}${shown}` : joinText(els.text.value, shown)
    state.resultView = { ...view, rawText, markers: gapMarkerTexts(rawText) }
    renderResultMeta()
    return
  }
  // 段列表只收**后端原文**（`[[gap:N]]` 没本地化）：本地化在渲染那一层做，
  // 存本地化文本会让缺口判定（`isGapMarker`）和切语言都失效。
  // 用户编辑过的分支不进段列表 —— 那时它不再是事实源。
  // 第一次落段时把结果框里已有的文本记成前缀（会话开始时通常是空的；
  // 停止之后迟到的句子要接在最终转写后面）。
  if (liveSegments.length === 0) liveBaseText = view?.rawText || ''
  const seg = {
    // 缺时间戳的段（文件转写的老事件）排不了序 —— 整个会话退化为到达顺序。
    start: Number.isFinite(start) ? start : null,
    end: Number.isFinite(end) ? end : null,
    text: piece,
    order: liveOrder++,
  }
  liveSegments.push(seg)
  const gap = isGapMarker(piece)
  // 退化标记在**这一段之前**是什么样，判定快路径时要看旧值：
  // 已经退化了的话，渲染顺序就是到达顺序 —— 新段直接接在末尾即可（O(1)）。
  const wasDegraded = liveDegraded
  if (seg.start === null) liveDegraded = true
  // 能不能直接追加到末尾（顺序不会变）？
  // - 已经退化了：可以 —— 整个会话按到达顺序渲染，追加正是到达顺序。
  // - 这一段才让会话退化：不行 —— 要整段重排一次（切成到达顺序）。
  // - 起点比已渲染的终点还晚：可以。
  // - 起点相同、不是缺口、且那个时间点上没有缺口：可以（同时间点按到达顺序）。
  // 其余都要重排 —— 同一 start 的缺口若直接追加，会排到句子**后面**，
  // 正好是这条规则要防的反向顺序。
  const canAppend =
    wasDegraded ||
    (seg.start !== null &&
      (seg.start > liveMaxStart || (seg.start === liveMaxStart && !gap && !liveGapAtMax)))
  if (seg.start !== null) {
    if (seg.start > liveMaxStart) {
      liveMaxStart = seg.start
      liveGapAtMax = gap
    } else if (seg.start === liveMaxStart && gap) {
      liveGapAtMax = true
    }
  }
  if (canAppend) {
    // 快路径：直接接在后面（O(1)，不用整段重排重拼）。
    setResultView(joinText(view?.rawText || liveBaseText, piece), { kind: 'label', key: 'record.live' })
  } else {
    // 重排：按本地时间轴重建全文。
    setResultView(rebuildRawText(), { kind: 'label', key: 'record.live' })
  }
}

export function showPartial(partial) {
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
