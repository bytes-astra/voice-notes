// 前后端拼句规则一致性检查。
//
// 运行：npm run check:join
//
// 前端 `src/core.js` 的 `joinText()` 与后端 `src-tauri/src/asr.rs` 的
// `join_segments()` 是同一套规则的两份实现（实时出字在前端拼，
// 最终文本在后端拼 —— 两边不一致时界面上的字和保存下来的字会不一样）。
//
// 规则只有三条：
//   1. 右边是缺口占位符 → 不加空格
//   2. 左边（已拼好的文本）以缺口占位符结尾 → 不加空格
//   3. 否则中日韩之间直接连，其余加空格
//
// 这里不重新实现规则，只把两边的**判定结构**抠出来规范化后比对：
// CJK 码位区间（含端点开闭）、缺口占位符的完整形状、三条规则的表达式结构。
// 只比对子串会漏掉最自然的漂移形状（`..=` 改 `..`、`||` 改 `&&`、
// 少写半个条件、占位符少个 `]`），所以两边都归一化成同一套中间表示再比。
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const core = readFileSync(join(process.cwd(), 'src/core.js'), 'utf8')
const asr = readFileSync(join(process.cwd(), 'src-tauri/src/asr.rs'), 'utf8')
const audio = readFileSync(join(process.cwd(), 'src-tauri/src/audio.rs'), 'utf8')

const problems = []

// 取一段函数体（按大括号配平）。
function body(source, name) {
  const start = source.indexOf(name)
  if (start < 0) return null
  const open = source.indexOf('{', start)
  if (open < 0) return null
  let depth = 0
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === '{') depth += 1
    else if (source[i] === '}') {
      depth -= 1
      if (depth === 0) return source.slice(open + 1, i)
    }
  }
  return null
}

function requireBody(source, name, side) {
  const text = body(source, name)
  if (!text) problems.push(`${side}找不到 ${name} —— 规则实现被改名或删掉了`)
  return text || ''
}

// ── CJK 码位区间 ──────────────────────────────────────────────
// 归一化成 `[起点, 终点, 终点是否含]` 三元组列表。
// JS 写 `code >= 0x4e00 && code <= 0x9fff`，Rust 写 `0x4E00..=0x9FFF` ——
// 两边都要归一到同一个形状，端点开闭也要带上（`..` 与 `..=` 差一个码位）。

function jsCjkRanges(text) {
  const out = []
  // 形如 `(code >= 0x3000 && code <= 0x303f)` 或跨行的写法
  const re = /code\s*(>=|>)\s*0x([0-9a-fA-F]+)\s*&&\s*code\s*(<=|<)\s*0x([0-9a-fA-F]+)/g
  for (const m of text.matchAll(re)) {
    out.push([
      Number.parseInt(m[2], 16),
      Number.parseInt(m[4], 16),
      m[1] === '>=' && m[3] === '<=',
    ])
  }
  return out
}

function rustCjkRanges(text) {
  const out = []
  const re = /0x([0-9a-fA-F]+)\s*\.\.(=)?\s*0x([0-9a-fA-F]+)/g
  for (const m of text.matchAll(re)) {
    out.push([
      Number.parseInt(m[1], 16),
      Number.parseInt(m[3], 16),
      m[2] === '=',
    ])
  }
  return out
}

const jsRanges = jsCjkRanges(requireBody(core, 'export function isCjk', '前端'))
const rustRanges = rustCjkRanges(requireBody(asr, 'fn is_cjk', '后端'))
if (jsRanges.length === 0 || rustRanges.length === 0) {
  problems.push('CJK 区间解析不出来 —— 写法变了？两端都应至少解析出 1 个区间')
} else if (
  jsRanges.length !== rustRanges.length ||
  jsRanges.some((v, i) => v.join() !== rustRanges[i].join())
) {
  const show = (list) => list.map(([a, b, inc]) => `${a.toString(16)}..${inc ? '=' : ''}${b.toString(16)}`)
  problems.push(`CJK 码位区间不一致：前端 ${show(jsRanges)}，后端 ${show(rustRanges)}`)
}

// ── 缺口占位符的完整形状 ──────────────────────────────────────
// 归一化成 `[[gap:<数字>]]`。
// 前端是正则 `/\[\[gap:(\d+)\]\]/g`，后端是前缀常量 `[[gap:` + 格式串里的 `]]`。
// 只比对前缀会漏掉「少个 `]`」「数字形状变了」这类漂移。

function jsGapShape(text) {
  // 前端正则字面量：`/\[\[gap:(\d+)\]\]/g`
  const m = text.match(/\/\s*\\\[\\\[gap:(\(\\d\+\))\\\]\\\]/)
  if (!m) return null
  const digits = m[1] === '(\\d+)' ? '<digits>' : '<?>'
  return `[[gap:${digits}]]`
}

function rustGapShape(text) {
  // 后端：前缀常量 `[[gap:` + 格式串 `"{GAP_MARKER_PREFIX}{}]]"`
  const prefix = text.match(/GAP_MARKER_PREFIX:\s*&str\s*=\s*"([^"]*)"/)
  const fmt = text.match(/"\{GAP_MARKER_PREFIX\}([^"]*)"/)
  if (!prefix || !fmt) return null
  const suffix = fmt[1].replace(/\{\}/g, '')
  const digits = /is_ascii_digit/.test(text) ? '<digits>' : '<?>'
  return `${prefix[1]}${digits}${suffix}`
}

const jsGap = jsGapShape(core)
const rustGap = rustGapShape(audio)
if (!jsGap || !rustGap) {
  problems.push('缺口占位符的形状解析不出来 —— 前端正则或后端常量写法变了')
} else {
  // 前端捕获组是数字 → 归一化成 <digits> 再比。
  const normalize = (shape) => shape.replace(/\(\\d\+\)|<digits>/, '<digits>')
  if (normalize(jsGap) !== normalize(rustGap)) {
    problems.push(`缺口占位符形状不一致：前端 ${jsGap}，后端 ${rustGap}`)
  }
}

// ── 三条规则的表达式结构 ──────────────────────────────────────
// 归一化成 `!CJK_L || !CJK_R` 这样的布尔表达式再比 —— 只查子串会漏掉
// `||` 改 `&&`、少写半个条件、把否定挪到错误的一侧。

function normalizeJoin(text) {
  return text
    .replace(/isCjk\(left\[left\.length\s*-\s*1\]\)/g, 'CJK_L')
    .replace(/isCjk\(right\[0\]\)/g, 'CJK_R')
    .replace(/is_cjk\(left\)/g, 'CJK_L')
    .replace(/is_cjk\(right\)/g, 'CJK_R')
    .replace(/\s+/g, ' ')
    .trim()
}

// 规则 3 的判定式：前端是 `needSpace` 的计算，后端是 `need_space` 的 match 臂。
function ruleThreeJs(text) {
  const m = text.match(/const needSpace\s*=\s*([^\n]+)/)
  return m ? m[1] : null
}

function ruleThreeRust(text) {
  const m = text.match(/!is_cjk\(left\)\s*\|\|\s*!is_cjk\(right\)/)
  return m ? m[0] : null
}

const jsJoin = requireBody(core, 'export function joinText', '前端')
const rustJoin = requireBody(asr, 'pub fn join_segments', '后端')

const jsRule3 = ruleThreeJs(jsJoin)
const rustRule3 = ruleThreeRust(rustJoin)
if (!jsRule3 || !rustRule3) {
  problems.push('规则「中日韩之间直连」的判定式解析不出来')
} else {
  const a = normalizeJoin(jsRule3)
  const b = normalizeJoin(rustRule3)
  if (a !== b) {
    problems.push(`规则「中日韩之间直连」不一致：前端 ${a}，后端 ${b}`)
  }
}

// 规则 1/2：占位符的两个方向各要有一个判定，且不能只判一个方向。
const rules = [
  ['右边是占位符', jsJoin.includes('isGapMarker(right)'), rustJoin.includes('is_gap_marker(part)')],
  ['左边以占位符结尾', jsJoin.includes('GAP_END_RE.test(left)'), rustJoin.includes('ends_with_gap_marker')],
]
for (const [name, js, rust] of rules) {
  if (!js || !rust) {
    problems.push(`规则「${name}」在${!js ? '前端' : '后端'}缺失`)
  }
}

if (problems.length) {
  console.error('拼句规则检查失败：')
  for (const problem of problems) console.error(`  - ${problem}`)
  process.exit(1)
}

console.log(
  `拼句规则检查通过：${jsRanges.length} 个 CJK 区间，缺口占位符 ${jsGap}，3 条规则前后端一致`,
)
