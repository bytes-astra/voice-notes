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
// CJK 码位区间（含端点开闭）、缺口占位符的完整形状、三条规则的表达式。
// 只比对子串会漏掉最自然的漂移形状（`..=` 改 `..`、`||` 改 `&&`、
// 少写半个条件、占位符少个 `]`），所以两边都归一化成同一套中间表示再比。
import { readFileSync, realpathSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

// 规则 1/2 的判定式：两个操作数 + 连接符。
//
// 操作数名字两边不一样（`isGapMarker(right)` vs `is_gap_marker(part)`），
// 抹掉名字只留**语义角色**；连接符必须留着 —— 把「或」改成「与」是最自然的
// 漂移，两边一起改也不能放过（行为虽然仍一致，但跟规则原本的语义不同了：
// 原本是「任一方向成立就不加空格」）。
const GAP_RULE_RIGHT = 'GAP_R'
const GAP_RULE_LEFT_END = 'GAP_L_END'
const GAP_RULE_EXPECTED_OPERANDS = [GAP_RULE_LEFT_END, GAP_RULE_RIGHT]
const GAP_RULE_EXPECTED_OP = '||'

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

function requireBody(source, name, side, problems) {
  const text = body(source, name)
  if (!text) problems.push(`${side}找不到 ${name} —— 规则实现被改名或删掉了`)
  return text || ''
}

// 归一化规则 1/2 的判定式 → { operands, op }。
//
// operands 里的名字换成语义角色，连接符原样保留。
function normalizeGapRule(condition, replacements) {
  if (!condition) return null
  let out = condition
  for (const [pattern, role] of replacements) out = out.replace(pattern, role)
  const op = out.includes('&&') ? '&&' : out.includes('||') ? '||' : null
  if (op === null) return null
  const operands = out
    .split(op)
    .map((part) => part.replace(/[()]/g, '').trim())
    .filter(Boolean)
    .sort()
  return { operands, op }
}

// 前端：`if (isGapMarker(right) || GAP_END_RE.test(left)) return ...`
function gapRuleFromJs(text) {
  const index = text.indexOf('isGapMarker(right)')
  if (index < 0) return null
  const ifStart = text.lastIndexOf('if (', index)
  if (ifStart < 0) return null
  const open = text.indexOf('(', ifStart)
  let depth = 0
  let end = -1
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === '(') depth += 1
    else if (text[i] === ')') {
      depth -= 1
      if (depth === 0) {
        end = i
        break
      }
    }
  }
  if (end < 0) return null
  return normalizeGapRule(text.slice(open + 1, end), [
    [/isGapMarker\(right\)/g, GAP_RULE_RIGHT],
    [/GAP_END_RE\.test\(left\)/g, GAP_RULE_LEFT_END],
  ])
}

// 后端：`if is_gap_marker(part) || ends_with_gap_marker(&out) { ... }`
function gapRuleFromRust(text) {
  const index = text.indexOf('is_gap_marker(part)')
  if (index < 0) return null
  const ifStart = text.lastIndexOf('if ', index)
  if (ifStart < 0) return null
  const brace = text.indexOf('{', index)
  if (brace < 0) return null
  return normalizeGapRule(text.slice(ifStart + 3, brace), [
    [/is_gap_marker\(part\)/g, GAP_RULE_RIGHT],
    [/ends_with_gap_marker\(&out\)/g, GAP_RULE_LEFT_END],
  ])
}

// 规则 1/2 的判定式：两个方向各要有一个判定，且必须用「或」连起来。
function checkGapRule(jsText, rustText, problems) {
  const js = gapRuleFromJs(jsText)
  const rust = gapRuleFromRust(rustText)
  if (!js || !rust) {
    problems.push(
      `规则「占位符前后不加空格」的判定式解析不出来${
        !js ? '（前端）' : '（后端）'
      } —— 实现被改写了？`,
    )
    return
  }
  const same = js.op === rust.op && js.operands.join() === rust.operands.join()
  if (!same) {
    problems.push(
      `规则「占位符前后不加空格」前后端不一致：前端 ${js.operands.join(
        ` ${js.op} `,
      )}，后端 ${rust.operands.join(` ${rust.op} `)}`,
    )
  }
  for (const [shape, side] of [
    [js, '前端'],
    [rust, '后端'],
  ]) {
    if (shape.op !== GAP_RULE_EXPECTED_OP) {
      problems.push(
        `规则「占位符前后不加空格」的${side}连接符是 ${shape.op}，应该是 ${GAP_RULE_EXPECTED_OP}`,
      )
    }
    if (shape.operands.join() !== GAP_RULE_EXPECTED_OPERANDS.join()) {
      problems.push(
        `规则「占位符前后不加空格」的${side}判定条件是 ${shape.operands.join(
          ` ${shape.op} `,
        )}，应该是 ${GAP_RULE_EXPECTED_OPERANDS.join(` ${GAP_RULE_EXPECTED_OP} `)}`,
      )
    }
  }
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

// ── 三条规则的表达式 ──────────────────────────────────────
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

/**
 * 检查前后端拼句规则是否一致。
 *
 * @param {string} core 前端 `src/core.js` 源码
 * @param {string} asr 后端 `src-tauri/src/asr.rs` 源码
 * @param {string} audio 后端 `src-tauri/src/audio.rs` 源码
 * @returns {string[]} 问题列表（空数组 = 一致）
 */
export function collectProblems(core, asr, audio) {
  const problems = []

  const jsRanges = jsCjkRanges(requireBody(core, 'export function isCjk', '前端', problems))
  const rustRanges = rustCjkRanges(requireBody(asr, 'fn is_cjk', '后端', problems))
  if (jsRanges.length === 0 || rustRanges.length === 0) {
    problems.push('CJK 区间解析不出来 —— 写法变了？两端都应至少解析出 1 个区间')
  } else if (
    jsRanges.length !== rustRanges.length ||
    jsRanges.some((v, i) => v.join() !== rustRanges[i].join())
  ) {
    const show = (list) =>
      list.map(([a, b, inc]) => `${a.toString(16)}..${inc ? '=' : ''}${b.toString(16)}`)
    problems.push(`CJK 码位区间不一致：前端 ${show(jsRanges)}，后端 ${show(rustRanges)}`)
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

  const jsJoin = requireBody(core, 'export function joinText', '前端', problems)
  const rustJoin = requireBody(asr, 'pub fn join_segments', '后端', problems)

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

  checkGapRule(jsJoin, rustJoin, problems)

  return problems
}

function main() {
  const core = readFileSync(join(process.cwd(), 'src/core.js'), 'utf8')
  const asr = readFileSync(join(process.cwd(), 'src-tauri/src/asr.rs'), 'utf8')
  const audio = readFileSync(join(process.cwd(), 'src-tauri/src/audio.rs'), 'utf8')

  const problems = collectProblems(core, asr, audio)
  if (problems.length) {
    console.error('拼句规则检查失败：')
    for (const problem of problems) console.error(`  - ${problem}`)
    process.exit(1)
  }

  const jsRanges = jsCjkRanges(body(core, 'export function isCjk'))
  const jsGap = jsGapShape(core)
  console.log(
    `拼句规则检查通过：${jsRanges.length} 个 CJK 区间，缺口占位符 ${jsGap}，3 条规则前后端一致`,
  )
}

// 直接运行时才检查（被 import 时不跑 —— 测试要能自己喂数据）。
//
// 两边都取 realpath 再比：`/tmp` 在 macOS 上是 `/private/tmp` 的软链，
// 只比 `process.argv[1]` 会在软链路径下判不出来 → 门禁静默放过（exit 0）。
function isEntryPoint() {
  if (!process.argv[1]) return false
  try {
    return (
      import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href
    )
  } catch {
    return false
  }
}

if (isEntryPoint()) main()
