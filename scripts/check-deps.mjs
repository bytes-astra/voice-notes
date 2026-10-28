// 前端依赖图检查：单向依赖不能反、也不能成环。
//
// 运行：npm run check:deps
//
// 链（AGENTS.md「核心架构与原则」）：
//   icons → core/tauri → theme/export → history → result → status → recorder → files/events → app
//
// 规则：一条 import 边的目标层必须「不晚于」来源层 —— 同层可以（core ↔ tauri 是平级），
// 指向更早的层是正常的（谁都能引 core），指向更晚的层就是「反了」。
// 这一条同时排除了环：成环必然有一条边指向更晚的层。
import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'

// 层号越小越靠前（叶子）。没列进来的模块按「任意层」处理，不参与判定。
const LAYERS = [
  ['icons'],
  ['core', 'tauri'],
  ['theme', 'export'],
  ['history'],
  ['result'],
  ['status'],
  ['recorder'],
  ['files', 'events'],
  ['app'],
]

const layerOf = new Map()
LAYERS.forEach((names, index) => {
  for (const name of names) layerOf.set(name, index)
})

const SRC = join(process.cwd(), 'src')

// 三种相对 import 都要认出来，缺一种就等于给违规开了后门：
//   import { a, b } from './x.js'   （可以跨多行写）
//   import './x.js'                 （副作用导入）
//   export { a } from './x.js'
//   import('./x.js')                （动态导入）
// 中间那段排除引号，免得副作用导入把下一条 import 的 from 一起吞掉。
const STATEMENT_RE = /(?:^|[\n;])\s*(?:import|export)\b[^;'"]*?\bfrom\s*(['"])\.\/([^'"]+)\.js\1/g
const BARE_RE = /(?:^|[\n;])\s*import\s*(['"])\.\/([^'"]+)\.js\1/g
const DYNAMIC_RE = /import\s*\(\s*(['"])\.\/([^'"]+)\.js\1\s*\)/g

const files = readdirSync(SRC).filter((name) => name.endsWith('.js'))
const edges = []

for (const name of files) {
  const module = name.slice(0, -3)
  const text = readFileSync(join(SRC, name), 'utf8')
  const lineOf = (index) => text.slice(0, index).split('\n').length
  for (const re of [STATEMENT_RE, BARE_RE, DYNAMIC_RE]) {
    re.lastIndex = 0
    let match
    while ((match = re.exec(text)) !== null) {
      edges.push({ from: module, to: match[2], file: name, line: lineOf(match.index) })
    }
  }
}

const violations = []

// 规则一：不得指向更晚的层。
for (const edge of edges) {
  const from = layerOf.get(edge.from)
  const to = layerOf.get(edge.to)
  if (from === undefined || to === undefined) continue
  if (to > from) {
    violations.push(`${edge.file}:${edge.line} 依赖方向反了：${edge.from} -> ${edge.to}`)
  }
}

// 规则二：不得成环（层规则已经能挡住大部分，这里兜底给出直观信息）。
const adjacency = new Map()
for (const edge of edges) {
  if (!adjacency.has(edge.from)) adjacency.set(edge.from, [])
  adjacency.get(edge.from).push(edge.to)
}
const state = new Map()
const path = []
function walk(node) {
  if (state.get(node) === 1) {
    const cycle = path.slice(path.indexOf(node))
    violations.push(`依赖成环：${cycle.join(' -> ')} -> ${node}`)
    return true
  }
  if (state.get(node) === 2) return false
  state.set(node, 1)
  path.push(node)
  for (const next of adjacency.get(node) || []) {
    if (walk(next)) return true
  }
  path.pop()
  state.set(node, 2)
  return false
}
for (const module of files.map((name) => name.slice(0, -3))) {
  if (walk(module)) break
}

if (violations.length) {
  console.error('前端依赖检查未通过：')
  for (const violation of violations) console.error(`  ${violation}`)
  process.exit(1)
}

console.log(`依赖检查通过：${files.length} 个模块，${edges.length} 条 import，无环、无反向依赖`)
