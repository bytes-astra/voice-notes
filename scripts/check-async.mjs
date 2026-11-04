// async 路径上的阻塞锁检查。
//
// 运行：npm run check:async
//
// 规则：**引擎槽**（engine / streaming_engine / punct_engine）的阻塞式
// `.lock()` 只能出现在阻塞线程（`spawn_blocking` 闭包）里。
//
// 为什么只查引擎槽：引擎加载要好几秒（239 MB / 161 MB / 294 MB），
// 那把锁会被占住。async 命令在执行器线程上死等这把锁，整个执行器就卡住 ——
// 命令的响应被拖到模型加载完，用户看着像界面冻住。
// `try_lock()` 不算：拿不到就往下走加载路径（见 engine.rs 的 cached_engine）。
//
// 采集器槽 / 会话槽的锁**故意**在 async 命令里直接取：那些临界区只有几行，
// 不存在「等模型加载」这种长等待。不查它们是避免误报，不是放过风险。
//
// 怎么算「在 async 路径上」：从 async 函数出发，沿**普通调用**走到的函数
// 都算 async 路径；走 `spawn_blocking` 闭包进去的算阻塞线程，不算。
// 所以 `shared_engine` 这种「async 命令调的同步取锁助手」会被抓住，
// 而 `install_engine`（只在 spawn_blocking 里调）不会。
import { readFileSync, readdirSync, realpathSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { pathToFileURL } from 'node:url'

// 这些名字的变量是引擎槽（`AppState` 里的槽，或它的 Arc 克隆）。
const ENGINE_SLOT_NAMES = new Set([
  'engine',
  'streaming_engine',
  'punct_engine',
  'engine_slot',
  'streaming',
  'punct',
])

/**
 * 把注释和字符串抹掉（替换成等长空格，保持偏移不变）。
 *
 * 不抹的话：注释里写一句「别用 state.engine.lock()」就会被当成真代码；
 * 字符串里的 `{` 会把大括号配平算歪。
 */
export function stripNoise(text) {
  let out = ''
  let i = 0
  const n = text.length
  const blank = (count) => ' '.repeat(count)
  while (i < n) {
    const ch = text[i]
    // 行注释
    if (ch === '/' && text[i + 1] === '/') {
      let end = i
      while (end < n && text[end] !== '\n') end += 1
      out += blank(end - i)
      i = end
      continue
    }
    // 块注释
    if (ch === '/' && text[i + 1] === '*') {
      let end = i + 2
      while (end < n && !(text[end] === '*' && text[end + 1] === '/')) end += 1
      end = Math.min(end + 2, n)
      out += text.slice(i, end).replace(/[^\n]/g, ' ')
      i = end
      continue
    }
    // 原始字符串：r"..." / r#"..."# / r##"..."##
    if ((ch === 'r' || ch === 'b') && /["#]/.test(text[i + 1] ?? '')) {
      const hashes = (text.slice(i + 1).match(/^#*/) || [''])[0].length
      const quote = text[i + 1 + hashes]
      if (quote === '"') {
        const close = `"${'#'.repeat(hashes)}`
        const end = text.indexOf(close, i + 2 + hashes)
        const stop = end < 0 ? n : end + close.length
        out += text.slice(i, stop).replace(/[^\n]/g, ' ')
        i = stop
        continue
      }
    }
    // 普通字符串
    if (ch === '"') {
      let end = i + 1
      while (end < n && text[end] !== '"') {
        if (text[end] === '\\') end += 1
        end += 1
      }
      end = Math.min(end + 1, n)
      out += text.slice(i, end).replace(/[^\n]/g, ' ')
      i = end
      continue
    }
    // 字符字面量：'x' / '\n'（生命周期 'a 不算 —— 后面没有紧跟的引号）
    if (ch === "'") {
      const m = /^'(\\.|[^\\'])'/.exec(text.slice(i))
      if (m) {
        out += blank(m[0].length)
        i += m[0].length
        continue
      }
    }
    out += ch
    i += 1
  }
  return out
}

/** 从 open（一个 `{`）开始配平大括号，返回闭合位置；找不到返回 -1。 */
function matchBrace(text, open) {
  let depth = 0
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === '{') depth += 1
    else if (text[i] === '}') {
      depth -= 1
      if (depth === 0) return i
    }
  }
  return -1
}

/**
 * 找出所有函数定义：名字、是否 async、函数体的大括号范围。
 *
 * 没有函数体的声明（trait 里的方法签名 `fn foo(&self);`）跳过 ——
 * 它们的 `;` 在 `{` 前面，不跳过的话会把下一个函数的体算成自己的。
 */
export function findFunctions(text) {
  const functions = []
  const re = /\b(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/g
  let match
  while ((match = re.exec(text))) {
    const name = match[1]
    const isAsync = /\basync\s+(?:unsafe\s+)?fn\s/.test(match[0])
    const open = text.indexOf('{', match.index + match[0].length)
    if (open < 0) continue
    const semi = text.indexOf(';', match.index + match[0].length)
    if (semi >= 0 && semi < open) continue
    const close = matchBrace(text, open)
    if (close < 0) continue
    functions.push({ name, isAsync, bodyStart: open, bodyEnd: close })
  }
  return functions
}

/**
 * 找出 `spawn_blocking` 闭包的体范围 —— 那些代码跑在阻塞线程上。
 */
export function findBlockingThreadRanges(text) {
  const ranges = []
  const re = /spawn_blocking/g
  let match
  while ((match = re.exec(text))) {
    const open = text.indexOf('{', match.index)
    if (open < 0) continue
    const close = matchBrace(text, open)
    if (close < 0) continue
    ranges.push([open, close])
  }
  return ranges
}

function inRanges(position, ranges) {
  return ranges.some(([start, end]) => position >= start && position <= end)
}

/** 包住某个位置的**最内层**函数（没有就返回 null）。 */
function enclosingFunction(functions, position) {
  let found = null
  for (const fn of functions) {
    if (fn.bodyStart <= position && position <= fn.bodyEnd) {
      if (!found || fn.bodyStart > found.bodyStart) found = fn
    }
  }
  return found
}

/**
 * 检查一批 Rust 源文件。
 *
 * @param {{path: string, text: string}[]} sources
 * @returns {{problems: string[], summary: object}}
 */
export function collectProblems(sources) {
  const problems = []

  const files = sources.map((source) => ({
    path: source.path,
    code: stripNoise(source.text),
    functions: [],
    ranges: [],
  }))

  for (const file of files) {
    file.functions = findFunctions(file.code)
    file.ranges = findBlockingThreadRanges(file.code)
  }

  // 函数名 → 定义（同名取第一个；方法名撞车在 Rust 里本来就不该出现）
  const byName = new Map()
  for (const file of files) {
    for (const fn of file.functions) {
      if (!byName.has(fn.name)) byName.set(fn.name, fn)
    }
  }

  // 调用图：只算「普通调用」——spawn_blocking 闭包里的调用跑在阻塞线程上。
  const edges = new Map()
  for (const file of files) {
    for (const fn of file.functions) {
      const callees = new Set()
      const body = file.code.slice(fn.bodyStart, fn.bodyEnd + 1)
      const re = /([A-Za-z_][A-Za-z0-9_]*)\s*\(/g
      let match
      while ((match = re.exec(body))) {
        const absolute = fn.bodyStart + match.index
        if (inRanges(absolute, file.ranges)) continue
        callees.add(match[1])
      }
      edges.set(
        fn,
        [...callees]
          .filter((name) => byName.has(name))
          .map((name) => byName.get(name)),
      )
    }
  }

  // async 可达：从 async 函数出发沿普通调用走能走到的函数。
  const asyncReachable = new Set()
  const queue = []
  for (const file of files) {
    for (const fn of file.functions) {
      if (fn.isAsync) {
        asyncReachable.add(fn)
        queue.push(fn)
      }
    }
  }
  while (queue.length) {
    const fn = queue.shift()
    for (const callee of edges.get(fn) || []) {
      if (asyncReachable.has(callee)) continue
      asyncReachable.add(callee)
      queue.push(callee)
    }
  }

  let checked = 0
  for (const file of files) {
    const re = /([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*lock\s*\(/g
    let match
    while ((match = re.exec(file.code))) {
      const receiver = match[1]
      if (!ENGINE_SLOT_NAMES.has(receiver)) continue
      checked += 1
      const position = match.index
      // 阻塞线程里取的锁：没问题（那把锁本来就是给阻塞线程用的）。
      if (inRanges(position, file.ranges)) continue
      const fn = enclosingFunction(file.functions, position)
      if (!fn) continue
      if (!asyncReachable.has(fn)) continue
      const line = file.code.slice(0, position).split('\n').length
      problems.push(
        `${file.path}:${line} 在 async 路径上阻塞式取引擎锁（${receiver}.lock()）—— ` +
          `引擎加载会占住这把锁好几秒，async 命令在执行器线程上死等，整个执行器就卡住`,
      )
    }
  }

  const summary = {
    fileCount: files.length,
    asyncCount: files.reduce(
      (sum, file) => sum + file.functions.filter((fn) => fn.isAsync).length,
      0,
    ),
    lockCount: checked,
  }
  return { problems, summary }
}

/** 递归列出 src-tauri/src 下的 .rs 文件。 */
export function rustSources(root) {
  const out = []
  const walk = (dir) => {
    for (const entry of readdirSync(dir)) {
      const full = join(dir, entry)
      if (statSync(full).isDirectory()) walk(full)
      else if (full.endsWith('.rs')) out.push(full)
    }
  }
  walk(root)
  return out
}

function main() {
  const root = join(process.cwd(), 'src-tauri', 'src')
  const sources = rustSources(root).map((path) => ({
    path: relative(process.cwd(), path),
    text: readFileSync(path, 'utf8'),
  }))

  const { problems, summary } = collectProblems(sources)
  if (problems.length) {
    console.error('async 阻塞锁检查失败：')
    for (const problem of problems) console.error(`  - ${problem}`)
    process.exit(1)
  }
  console.log(
    `async 阻塞锁检查通过：${summary.fileCount} 个文件，${summary.asyncCount} 个 async 函数，` +
      `${summary.lockCount} 处引擎槽取锁都在阻塞线程上`,
  )
}

// 直接运行时才检查（被 import 时不跑 —— 测试要能自己喂数据）。
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
