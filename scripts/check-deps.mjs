// 前端依赖图检查：单向依赖不能反、也不能成环，外加经典脚本的规矩。
//
// 运行：npm run check:deps
//
// 链（AGENTS.md「核心架构与原则」）：
//   icons → core/tauri → theme/export → history → result → status → recorder → files/events → app
//
// 规则一：一条 import 边的目标层必须「不晚于」来源层 —— 同层可以（core ↔ tauri 是平级），
// 指向更早的层是正常的（谁都能引 core），指向更晚的层就是「反了」。
// 这一条同时排除了环：成环必然有一条边指向更晚的层。
//
// 规则二：不能成环（层规则已经能挡住大部分，这里兜底给出直观信息）。
//
// 规则三：经典脚本（i18n.js / select.js / locales/*.js）**不得被任何模块 import**。
// 它们挂的是 window 上的全局对象，被 import 会**再执行一遍**：
// window.I18N 被换成第二个对象，而 core.js 抓的是哪一个取决于求值顺序 ——
// 表现为「切语言后部分文案不跟着变」，且不报错，极难查。
//
// 规则四：src/ 下每个 .js 都必须在层表或经典脚本表里登记。
// 没登记的模块**完全豁免**判定 —— 以前这就是个洞：把模块名写错，
// 它的 import 边一条都不查了。
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { realpathSync } from 'node:fs'

// 层号越小越靠前（叶子）。
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

// 经典脚本：挂 window 的浏览器脚本，不是 ES module。
// 它们排在层表的最前面（没有 import 边），但**不许被 import**（规则三）。
const CLASSIC = ['i18n', 'select']

const layerOf = new Map()
LAYERS.forEach((names, index) => {
  for (const name of names) layerOf.set(name, index)
})
const classicOf = new Set(CLASSIC)

// 三种相对 import 都要认出来，缺一种就等于给违规开了后门：
//   import { a, b } from './x.js'   （可以跨多行写）
//   import './x.js'                 （副作用导入）
//   export { a } from './x.js'
//   import('./x.js')                （动态导入）
// 中间那段排除引号，免得副作用导入把下一条 import 的 from 一起吞掉。
const STATEMENT_RE = /(?:^|[\n;])\s*(?:import|export)\b[^;'"]*?\bfrom\s*(['"])\.\/([^'"]+)\.js\1/g
const BARE_RE = /(?:^|[\n;])\s*import\s*(['"])\.\/([^'"]+)\.js\1/g
const DYNAMIC_RE = /import\s*\(\s*(['"])\.\/([^'"]+)\.js\1\s*\)/g

/// src/ 下的所有 .js（递归）。locales/ 里的也是脚本，一样要查。
function jsFiles(dir) {
  const found = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) found.push(...jsFiles(path))
    else if (entry.name.endsWith('.js')) found.push(path)
  }
  return found
}

/// 模块名：src/xxx.js → xxx；src/locales/xx.js → locales/xx。
function moduleName(root, path) {
  const rel = relative(root, path).replace(/\\/g, '/')
  const inSrc = rel.startsWith('src/') ? rel.slice('src/'.length) : rel
  return inSrc.slice(0, -'.js'.length)
}

/// 收集一个文件里的 import 边。
function edgesOf(root, path) {
  const module = moduleName(root, path)
  const text = readFileSync(path, 'utf8')
  const lineOf = (index) => text.slice(0, index).split('\n').length
  const edges = []
  for (const re of [STATEMENT_RE, BARE_RE, DYNAMIC_RE]) {
    re.lastIndex = 0
    let match
    while ((match = re.exec(text)) !== null) {
      edges.push({ from: module, to: match[2], file: relative(root, path), line: lineOf(match.index) })
    }
  }
  return { module, text, edges }
}

/// 经典脚本自己不许有 import/export —— 有就不是经典脚本了，
/// 挂 window 的写法在 module 里语义不同（this / 顶层作用域都不一样）。
const CLASSIC_STATEMENT_RE = /(?:^|[\n;])\s*(?:import|export)\b/

export function collectProblems(root) {
  const srcDir = join(root, 'src')
  const problems = []
  const files = jsFiles(srcDir)
  const modules = new Map()
  let edgeCount = 0

  for (const path of files) {
    const { module, text, edges } = edgesOf(root, path)
    modules.set(module, { module, path, text, edges })
    edgeCount += edges.length
  }

  // 规则四：每个文件都要登记。没登记 = 它的 import 边一条都不查。
  for (const module of modules.keys()) {
    const base = module.split('/').pop()
    const registered = layerOf.has(base) || classicOf.has(base)
    const isLocale = module.startsWith('locales/')
    if (!registered && !isLocale) {
      problems.push(
        `${module}.js 没有登记在 check-deps 的层表里 —— 它的 import 边不会被判定（规则四）`
      )
    }
  }

  for (const { module, text, edges } of modules.values()) {
    // 规则三：经典脚本不得被 import。
    for (const edge of edges) {
      const target = edge.to.split('/').pop()
      if (classicOf.has(target) || edge.to.startsWith('locales/')) {
        problems.push(
          `${edge.file}:${edge.line} 不能 import 经典脚本 ${edge.to}.js —— ` +
            '它挂 window 上，被 import 会执行两遍（实测：i18n.js 执行 2 次，' +
            'window.I18N 被换成第二个实例）。当前代码恰好没因此坏掉，' +
            '但哪个实例生效取决于求值顺序 —— 这种隐患不值得留'
        )
      }
    }
    // 经典脚本自己不许有 import/export。
    const base = module.split('/').pop()
    if (classicOf.has(base) || module.startsWith('locales/')) {
      const match = CLASSIC_STATEMENT_RE.exec(text)
      if (match) {
        problems.push(
          `${module}.js 是经典脚本，不能有 import/export —— ` +
            '它必须保持浏览器脚本（挂在 window 上），加了就变成 module，行为不一样'
        )
      }
    }
  }

  // 规则一：不得指向更晚的层。
  for (const { edges } of modules.values()) {
    for (const edge of edges) {
      const from = layerOf.get(edge.from)
      const to = layerOf.get(edge.to)
      if (from === undefined || to === undefined) continue
      if (to > from) {
        problems.push(`${edge.file}:${edge.line} 依赖方向反了：${edge.from} -> ${edge.to}`)
      }
    }
  }

  // 规则二：不得成环（层规则已经能挡住大部分，这里兜底给出直观信息）。
  const adjacency = new Map()
  for (const { edges } of modules.values()) {
    for (const edge of edges) {
      if (!adjacency.has(edge.from)) adjacency.set(edge.from, [])
      adjacency.get(edge.from).push(edge.to)
    }
  }
  const state = new Map()
  const path = []
  function walk(node) {
    if (state.get(node) === 1) {
      const cycle = path.slice(path.indexOf(node))
      problems.push(`依赖成环：${cycle.join(' -> ')} -> ${node}`)
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
  for (const module of modules.keys()) {
    if (walk(module)) break
  }

  return {
    problems,
    summary: { moduleCount: modules.size, edgeCount },
  }
}

function main() {
  const root = join(fileURLToPath(import.meta.url), '..', '..')
  const { problems, summary } = collectProblems(root)
  if (problems.length) {
    console.error('前端依赖检查未通过：')
    for (const problem of problems) console.error(`  ${problem}`)
    process.exit(1)
  }
  console.log(
    `依赖检查通过：${summary.moduleCount} 个模块，${summary.edgeCount} 条 import，无环、无反向依赖`
  )
}

// 直接运行时才检查（被 import 时不跑 —— 测试要能自己喂数据）。
//
// 两边都取 realpath 再比：`/tmp` 在 macOS 上是 `/private/tmp` 的软链，
// 只比 `process.argv[1]` 会在软链路径下判不出来 → 门禁静默放过（exit 0）。
function isEntryPoint() {
  if (!process.argv[1]) return false
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href
  } catch {
    return false
  }
}

if (isEntryPoint()) main()
