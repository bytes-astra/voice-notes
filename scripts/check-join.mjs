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
// 这里不重新实现规则，只把两边的**常量**抠出来比对：
// CJK 码位区间、缺口占位符的字面量、规则的三个分支。
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

// CJK 码位区间：JS 写 0x4e00，Rust 写 0x4E00 —— 大小写归一后比对。
function ranges(source, name) {
  const text = body(source, name)
  if (!text) return null
  return [...text.matchAll(/0x([0-9a-fA-F]{4,6})/g)].map((m) =>
    Number.parseInt(m[1], 16),
  )
}

const jsRanges = ranges(core, 'export function isCjk')
const rustRanges = ranges(asr, 'fn is_cjk')
if (!jsRanges || !rustRanges) {
  problems.push('找不到 isCjk / is_cjk —— 规则实现被改名或删掉了')
} else if (jsRanges.length !== rustRanges.length || jsRanges.some((v, i) => v !== rustRanges[i])) {
  problems.push(
    `CJK 码位区间不一致：前端 ${jsRanges.map((v) => v.toString(16))}，后端 ${rustRanges.map((v) => v.toString(16))}`,
  )
}

// 缺口占位符的字面量：前端正则与后端常量必须是同一个串。
// 前端写进正则（`\[\[gap:`），后端写进字符串常量（`"[[gap:"`）。
const jsGap = /\\\[\\\[gap:/.test(core)
const rustGap = /"\[\[gap:/.test(audio)
if (!jsGap || !rustGap) {
  problems.push('找不到缺口占位符 "[[gap:" —— 两端必须用同一个字面量')
}

// 规则 1/2/3：两边都要有对应的判定。
const jsJoin = body(core, 'export function joinText') || ''
const rustJoin = body(asr, 'pub fn join_segments') || ''
const checks = [
  ['右边是占位符', jsJoin.includes('isGapMarker(right)'), rustJoin.includes('is_gap_marker(part)')],
  ['左边以占位符结尾', jsJoin.includes('GAP_END_RE.test(left)'), rustJoin.includes('ends_with_gap_marker')],
  ['中日韩之间直连', jsJoin.includes('isCjk(left[left.length - 1])'), rustJoin.includes('!is_cjk(left)')],
]
for (const [name, js, rust] of checks) {
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
  `拼句规则检查通过：${jsRanges.length} 个 CJK 区间，缺口占位符 "[[gap:"，3 条规则前后端一致`,
)
