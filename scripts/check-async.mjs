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
//
// ── 第二条规则：阻塞的会话构造器 ──
//
// 建识别会话要加载 onnx、建 ORT session（几十到几百毫秒）。这些构造器
// **不许建在 async 执行器线程上** —— 那段时间里别的命令全排队，
// 用户看着像界面冻住（实测建 VAD ~70ms）。要建就放进 `spawn_blocking` 闭包。
//
// 怎么算「在 async 路径上」：跟第一条规则**同一个调用图** —— 从 async 函数
// 出发沿普通调用能走到的函数。`spawn_blocking` 闭包与专用 `std::thread`
// 闭包里的调用都不算 —— 那两种线程不是 async 执行器，在上面阻塞不占执行器。
//
// **执行器的 spawn 不豁免**：`tauri::async_runtime::spawn(async move {…})`
// 与 `tokio::spawn(…)` 跑的是**执行器上的异步任务**（tauri 的
// `async_runtime::spawn` 就是 `tokio::spawn` 到全局 tokio runtime）——
// 在里面阻塞同样占执行器。判据是参数形态：闭包（`move || {…}`）只可能是
// OS 线程（`std::thread::spawn` 要求 `FnOnce() -> T + Send + 'static`，
// 装不下 future）；`async` 块 / future 是执行器任务。
//
// 构造器**调用点**用限定名匹配（`Type::method`）—— 方法名会撞车
// （到处都是 `new`），光看方法名分不出是哪个类型的。
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
 * 找出所有 `impl <Type> { … }` 块的类型名与范围。
 *
 * 用来给方法限定名（`OfflineSession::new`）—— 光看 `fn new` 分不清是
 * 哪个类型的 `new`（本仓库有 18 个 `fn new`）。
 */
export function findImplTypes(text) {
  const blocks = []
  const re = /\bimpl(?:\s*<[^>]*>)?\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*\{/g
  let match
  while ((match = re.exec(text))) {
    const open = text.indexOf('{', match.index)
    if (open < 0) continue
    const close = matchBrace(text, open)
    if (close < 0) continue
    blocks.push({ type: match[1], bodyStart: open, bodyEnd: close })
  }
  return blocks
}

/**
 * 找出所有函数定义：名字、是否 async、函数体的大括号范围、
 * 以及它属于哪个 impl（限定名 `Type::name`）。
 *
 * 没有函数体的声明（trait 里的方法签名 `fn foo(&self);`）跳过 ——
 * 它们的 `;` 在 `{` 前面，不跳过的话会把下一个函数的体算成自己的。
 */
export function findFunctions(text, implTypes = []) {
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
  // 给每个函数标上它所属的 impl 类型（最内层的那个）。
  for (const fn of functions) {
    let owner = null
    for (const block of implTypes) {
      if (block.bodyStart <= fn.bodyStart && fn.bodyEnd <= block.bodyEnd) {
        if (!owner || block.bodyStart > owner.bodyStart) owner = block
      }
    }
    fn.implType = owner ? owner.type : null
    fn.qualifiedName = owner ? `${owner.type}::${fn.name}` : fn.name
  }
  return functions
}

/** 从 open（一个 `(`）开始配平圆括号，返回闭合位置；找不到返回 -1。 */
function matchParen(text, open) {
  let depth = 0
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === '(') depth += 1
    else if (text[i] === ')') {
      depth -= 1
      if (depth === 0) return i
    }
  }
  return -1
}

/**
 * 找出 `spawn_blocking` 闭包的体范围 —— 那些代码跑在阻塞线程上。
 *
 * 按**整次调用的括号**配平，不按大括号：`spawn_blocking(move || expr)`
 * 这种没有大括号的写法（models.rs / lib.rs 里都有）如果按 `{` 找，
 * 会找到**后面**某个 `{`（实测错圈 93 / 34 个字符，落在紧邻的下一段代码上），
 * 那段范围里的检查会被跳过 —— 范围越滚越大就越危险。
 */
export function findBlockingThreadRanges(text) {
  const ranges = []
  const re = /spawn_blocking\s*\(/g
  let match
  while ((match = re.exec(text))) {
    const open = text.indexOf('(', match.index)
    if (open < 0) continue
    const close = matchParen(text, open)
    if (close < 0) continue
    ranges.push([open, close])
  }
  return ranges
}

/**
 * 找出**专用线程**闭包的体范围 —— `std::thread::Builder::new()...spawn(move || {…})`
 * 与 `std::thread::spawn(move || {…})`。
 *
 * 那些线程不是 async 执行器 —— 在里面阻塞不占执行器。
 *
 * **执行器的 spawn 不豁免**（`tauri::async_runtime::spawn(async move {…})` /
 * `tokio::spawn(…)`）：那是执行器上的异步任务（tauri 的 `async_runtime::spawn`
 * 就是 `tokio::spawn` 到全局 tokio runtime）—— 在里面阻塞同样占执行器，
 * 必须按 async 路径查。
 *
 * 判据是**参数形态**：
 *   · 闭包（`move || {…}` / `|| {…}` / `move |x| {…}`）→ 只可能是 OS 线程
 *     （`std::thread::spawn` 要求 `FnOnce() -> T + Send + 'static`，装不下
 *     future）；参数名不固定，不能只匹配 `||`；
 *   · `async` 块 / future → 执行器任务，**不豁免**。
 * 外加**线程路径白名单**（`std::thread::spawn`）：那种写法传的是函数项
 * （`std::thread::spawn(some_fn)`），参数不是闭包，按形态判不出来 ——
 * 仓库里目前没用，但豁免规则不该留这种洞。
 *
 * 按**整次调用的括号**配平，不按大括号：`spawn(move || expr)` 这种没有
 * 大括号的写法如果按 `{` 找，会找到**后面**某个 `{`（实测错圈到下一个
 * 函数，664 个字符），那段里的检查会被静默跳过。
 */
export function findDedicatedThreadRanges(text) {
  const ranges = []
  // 闭包形态：`(?:move )?|` 开头。`async move {` 不匹配（`{` 不是 `|`）。
  const closure = /(?:\.|::)\s*spawn\s*\(\s*(?:move\s+)?\|/g
  // 线程路径白名单：参数是函数项时闭包形态判不出来。
  const threadPath = /(?:std::thread|\bthread)::spawn\s*\(/g
  for (const re of [closure, threadPath]) {
    let match
    while ((match = re.exec(text))) {
      const open = text.indexOf('(', match.index)
      if (open < 0) continue
      const close = matchParen(text, open)
      if (close < 0) continue
      ranges.push([open, close])
    }
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
 * 阻塞的识别资源构造器 —— 建它们要加载 onnx / 建 ORT session，
 * 实测几十到几百毫秒。
 *
 * 跑在 async 执行器上 = 那段时间里别的命令全排队，用户看着像界面冻住。
 */
const BLOCKING_CONSTRUCTORS = new Map([
  ['Engine::new', '整段模型（239 MB）'],
  ['StreamingEngine::new', '流式模型（161 MB）'],
  ['PunctuationEngine::new', '标点模型（281 MB）'],
  ['OfflineSession::new', 'VAD detector（载 onnx）'],
  ['VoiceActivityDetector::create', 'VAD detector（载 onnx）'],
])

/**
 * 阻塞构造器的**调用点**。
 *
 * 用限定名匹配（`Type::method`）—— 方法名会撞车（到处都是 `new`），
 * 光看方法名分不出是哪个类型。前缀允许 `asr::` / `sherpa_onnx::` 这种路径。
 */
const BLOCKING_CALL_RE =
  /(^|[^A-Za-z0-9_])(?:[A-Za-z_][A-Za-z0-9_]*::)*(Engine|StreamingEngine|PunctuationEngine|OfflineSession|VoiceActivityDetector)::(new|create)\s*\(/g

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
    threadRanges: [],
  }))

  for (const file of files) {
    file.implTypes = findImplTypes(file.code)
    file.functions = findFunctions(file.code, file.implTypes)
    file.ranges = findBlockingThreadRanges(file.code)
    file.threadRanges = findDedicatedThreadRanges(file.code)
  }

  // 函数名 → 定义（同名取第一个）。
  //
  // **同名会撞车**：本仓库有 18 个 `fn new` —— 光看名字分不清是哪个类型的。
  // 所以另外登记一张**限定名表**（`OfflineSession::new`），调用点写成
  // `Type::method(...)` 时优先按限定名连边；连不上才退回普通名字。
  const byName = new Map()
  const byQualifiedName = new Map()
  for (const file of files) {
    for (const fn of file.functions) {
      if (!byName.has(fn.name)) byName.set(fn.name, fn)
      if (fn.implType && !byQualifiedName.has(fn.qualifiedName)) {
        byQualifiedName.set(fn.qualifiedName, fn)
      }
    }
  }

  // 调用图：只算「普通调用」—— 两种线程闭包里的调用都不在 async 执行器上：
  //   · `spawn_blocking` 闭包 → 阻塞线程；
  //   · 专用 `std::thread` 闭包 → 专用 OS 线程。
  // 两种都不该算「async 路径」，所以边都不连。
  const edges = new Map()
  for (const file of files) {
    for (const fn of file.functions) {
      const callees = new Set()
      const body = file.code.slice(fn.bodyStart, fn.bodyEnd + 1)
      // 限定名调用：`Type::method(`（`Type` 前可能有路径前缀，如 `asr::OfflineSession::new`）
      const qualified = /([A-Za-z_][A-Za-z0-9_]*)\s*::\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(/g
      let match
      while ((match = qualified.exec(body))) {
        const absolute = fn.bodyStart + match.index
        if (inRanges(absolute, file.ranges)) continue
        if (inRanges(absolute, file.threadRanges)) continue
        const key = `${match[1]}::${match[2]}`
        if (byQualifiedName.has(key)) {
          callees.add(key)
          continue
        }
        // 限定名连不上（如 `crate::code_detail` 这种自由函数）→ 退回普通名字。
        callees.add(match[2])
      }
      // 普通调用：`name(`（前面没有 `::`）
      const plain = /(?<![A-Za-z0-9_:])([A-Za-z_][A-Za-z0-9_]*)\s*\(/g
      while ((match = plain.exec(body))) {
        const absolute = fn.bodyStart + match.index
        if (inRanges(absolute, file.ranges)) continue
        if (inRanges(absolute, file.threadRanges)) continue
        callees.add(match[1])
      }
      const resolved = []
      for (const name of callees) {
        const target = byQualifiedName.get(name) || byName.get(name)
        if (target && !resolved.includes(target)) resolved.push(target)
      }
      edges.set(fn, resolved)
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

  // ── 第二条规则：阻塞的构造器不许建在 async 路径上 ──
  //
  // 判定按**调用点**算：构造器写在 async 函数体里（且不在任何线程闭包里）
  // 就是建在 async 执行器线程上 —— 那段时间里别的命令全排队，用户看着像界面冻住。
  //
  // 两种线程闭包里的调用不算：
  //   · `spawn_blocking` —— 阻塞线程，不是执行器；
  //   · `std::thread::Builder::new()…spawn(move || {…})` —— 专用 OS 线程，
  //     也不是执行器（record.rs 的 run_offline / run_streaming_core 在那上面跑）。
  let constructorChecked = 0
  for (const file of files) {
    BLOCKING_CALL_RE.lastIndex = 0
    let match
    while ((match = BLOCKING_CALL_RE.exec(file.code))) {
      const key = `${match[2]}::${match[3]}`
      const what = BLOCKING_CONSTRUCTORS.get(key)
      if (!what) continue
      constructorChecked += 1
      const position = match.index
      if (inRanges(position, file.ranges)) continue
      if (inRanges(position, file.threadRanges)) continue
      const fn = enclosingFunction(file.functions, position)
      if (!fn || !asyncReachable.has(fn)) continue
      const line = file.code.slice(0, position).split('\n').length
      problems.push(
        `${file.path}:${line} 在 async 路径上建${what}（${key}）—— ` +
          `那要加载 onnx、建 ORT session（几十到几百毫秒），` +
          `async 执行器线程被占住时别的命令全排队，用户看着像界面冻住；` +
          `要建就放进 spawn_blocking 闭包`,
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
    constructorCount: constructorChecked,
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
      `${summary.lockCount} 处引擎槽取锁都在阻塞线程上，` +
      `${summary.constructorCount} 处阻塞构造器都不在 async 路径上`,
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
