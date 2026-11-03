// 前后端契约检查：命令名与载荷字段名必须对得上。
//
// 运行：npm run check:contract
//
// 前端 `invoke('set_input_device', { name })` 与后端
// `#[tauri::command] fn set_input_device(..., name: Option<String>)` 是两份手写的清单，
// 改一边不会让另一边变红 —— 运行时才发现，界面直接不响应。
//
// 查三件事：
//   1. 前端调的命令在后端存在吗（后端改名/删命令 → 界面报「未知命令」）
//   2. 前端传的载荷字段是后端参数吗（Tauri v2 用 camelCase 收参数；
//      后端改参数名 → 前端传的字段被静默忽略，值永远是默认值）
//   3. 界面检查的假后端（scripts/ui/check.mjs）返回的字段名与真实后端一致吗
//      （历史上就出过：mock 写 is_default、真后端是 isDefault，
//        界面检查全绿、真机上「· 系统默认」标记不显示）
import { readFileSync, readdirSync, realpathSync } from 'node:fs'
import { join, relative } from 'node:path'
import { pathToFileURL } from 'node:url'

/// snake_case → camelCase（Tauri v2 的参数转换规则）。
function camel(name) {
  return name.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase())
}

/// 从一段 Rust 源码里抽出 `#[tauri::command]` 的命令名与参数。
///
/// 参数分两类：AppHandle / State 是 Tauri 注入的，前端不传；
/// 其余是前端要传的。Option<> 的可以不传，其余必传。
export function rustCommands(source) {
  const commands = new Map()
  // #[tauri::command] 与 fn 之间可能夹着文档注释（save_text 就有一行）。
  const re = /#\[tauri::command\][\s\S]{0,200}?(?:async\s+)?fn\s+([a-z_0-9]+)\s*\(/g
  let match
  while ((match = re.exec(source)) !== null) {
    const name = match[1]
    // 参数表可能跨行：按括号配平截出来。
    let depth = 1
    let i = re.lastIndex
    while (i < source.length && depth > 0) {
      if (source[i] === '(') depth++
      else if (source[i] === ')') depth--
      i++
    }
    const params = source.slice(re.lastIndex, i - 1)
    const args = []
    for (const part of splitTopLevel(params)) {
      const m = /^\s*([a-z_0-9]+)\s*:\s*(.+)$/.exec(part)
      if (!m) continue
      const [, argName, argType] = m
      if (/AppHandle|State\s*</.test(argType)) continue
      args.push({ name: argName, required: !argType.trim().startsWith('Option<') })
    }
    commands.set(name, args)
    re.lastIndex = i
  }
  return commands
}

/// 按顶层逗号切分（跳过 <>、()、[]、{} 里的逗号）。
///
/// 花括号也要算：JS 的对象字面量 `{ a: 1, b: 2 }` 里那个逗号是顶层分隔符，
/// 不算进去就会把对象切成两半。
function splitTopLevel(text) {
  const parts = []
  let depth = 0
  let current = ''
  for (const ch of text) {
    if (ch === '<' || ch === '(' || ch === '[' || ch === '{') depth++
    else if (ch === '>' || ch === ')' || ch === ']' || ch === '}') depth--
    if (ch === ',' && depth === 0) {
      parts.push(current)
      current = ''
    } else {
      current += ch
    }
  }
  if (current.trim()) parts.push(current)
  return parts
}

/// 从一段 Rust 源码里抽出 `#[derive(..., Serialize)]` 结构体的字段名。
///
/// 带 `#[serde(rename_all = "camelCase")]` 的按 camelCase 转换 —— 前端看到的就是它。
export function rustStructFields(source, structName) {
  const re = new RegExp(
    `(?:#\\[[^\\]]*\\]\\s*)*pub struct ${structName}\\s*\\{([\\s\\S]*?)\\n\\s*\\}`,
    'm'
  )
  const match = re.exec(source)
  if (!match) return null
  const body = match[1]
  // rename_all 属性可能贴在 struct 前面（#[serde(rename_all = "camelCase")]），
  // 也可能贴在更早的 derive 旁边 —— 两处都找。
  const camelCase = /rename_all\s*=\s*"camelCase"/.test(
    match[0] + source.slice(Math.max(0, match.index - 200), match.index)
  )
  const fields = []
  for (const part of splitTopLevel(body)) {
    // 字段前面可能有文档注释（`/// 整段模型正在下载`），先剥掉。
    const line = part.split('\n').filter((l) => !/^\s*\/\//.test(l)).join('\n')
    const m = /^\s*pub\s+([a-z_0-9]+)\s*:/.exec(line)
    if (m) fields.push(camelCase ? camel(m[1]) : m[1])
  }
  return fields
}

/// 从前端源码里抽出 invoke 调用：命令名 + 载荷字段。
///
/// 三种写法都算：
///   invoke('status')
///   invoke('set_input_device', { name })
///   invoke(streaming ? 'a' : 'b', { sessionId })   —— 两条命令共用载荷
export function frontendInvokes(source) {
  const calls = []
  const re = /invoke\(/g
  let match
  while ((match = re.exec(source)) !== null) {
    const [first, second, end] = readCall(source, re.lastIndex)
    if (!first) {
      re.lastIndex = end
      continue
    }
    const names = first.includes('?')
      ? [...first.matchAll(/'([a-z_0-9]+)'/g)].map((m) => m[1])
      : /^'([a-z_0-9]+)'$/.test(first.trim())
        ? [first.trim().slice(1, -1)]
        : []
    const keys = second ? objectKeys(second) : []
    for (const name of names) calls.push({ name, keys })
    re.lastIndex = end
  }
  // runDownload 的 command 选项：invoke(command) 里的命令名在这里。
  for (const m of source.matchAll(/\bcommand:\s*'([a-z_0-9]+)'/g)) {
    calls.push({ name: m[1], keys: [] })
  }
  return calls
}

/// 从 `invoke(` 的括号开始，读出第一、第二个实参（按顶层逗号切）。
function readCall(source, start) {
  let depth = 1
  let i = start
  while (i < source.length && depth > 0) {
    if (source[i] === '(' || source[i] === '{' || source[i] === '[') depth++
    else if (source[i] === ')' || source[i] === '}' || source[i] === ']') depth--
    i++
  }
  const inner = source.slice(start, i - 1)
  const parts = splitTopLevel(inner)
  return [parts[0] || '', parts[1] || '', i]
}

/// 对象字面量的键名。
///
/// 两种写法都要认：`{ name }`（简写）和 `{ name: value }`。
function objectKeys(text) {
  const keys = []
  const body = text.trim().replace(/^\{/, '').replace(/\}$/, '')
  // 先剥掉行注释：注释里也有 `:` 和 `,`，不剥会串到下一个字段上。
  const clean = body
    .split('\n')
    .filter((line) => !/^\s*\/\//.test(line))
    .join('\n')
  for (const part of splitTopLevel(clean)) {
    // 简写 `{ name }` 后面没有 `:`，值就是变量名本身。
    const m = /^\s*([a-zA-Z_$][\w$]*)\s*(?::|=|$)/.exec(part)
    if (m) keys.push(m[1])
  }
  return keys
}

/// 从假后端（scripts/ui/check.mjs）里抽出它返回的载荷字段。
///
/// 认两种：状态对象（`const base = { … }`）和设备列表（`return [{ … }]`）。
export function mockPayloads(source) {
  const payloads = []
  const base = /const base = \{([\s\S]*?)\n\s*\}/.exec(source)
  if (base) payloads.push({ name: 'status', keys: objectKeys(`{${base[1]}}`) })
  const devices = /case 'list_input_devices':\s*\n\s*return \[\{([\s\S]*?)\}\]/.exec(source)
  if (devices) payloads.push({ name: 'list_input_devices', keys: objectKeys(`{${devices[1]}}`) })
  return payloads
}

/// 文件不存在时返回空串（测试的最小骨架里可能没有这些文件）。
function readOptional(path) {
  try {
    return readFileSync(path, 'utf8')
  } catch {
    return ''
  }
}

export function collectProblems(root) {
  const problems = []

  // 后端：命令表 + 载荷结构体。
  const libSource = readFileSync(join(root, 'src-tauri/src/lib.rs'), 'utf8')
  const engineSource = readOptional(join(root, 'src-tauri/src/engine.rs'))
  const audioSource = readOptional(join(root, 'src-tauri/src/audio.rs'))
  const commands = rustCommands(libSource)

  // 前端：所有 invoke。
  const srcDir = join(root, 'src')
  const calls = []
  for (const file of readdirSync(srcDir).filter((f) => f.endsWith('.js'))) {
    const source = readFileSync(join(srcDir, file), 'utf8')
    for (const call of frontendInvokes(source)) {
      calls.push({ ...call, file: `src/${file}` })
    }
  }

  // 1 + 2：命令存在吗？载荷字段对吗？
  const invoked = new Set()
  for (const call of calls) {
    invoked.add(call.name)
    const args = commands.get(call.name)
    if (!args) {
      problems.push(
        `${call.file} 调用了后端没有的命令 ${call.name} —— ` +
          '后端改名或删了命令，这里不会报错，运行时才炸'
      )
      continue
    }
    const accepted = new Set(args.map((arg) => camel(arg.name)))
    for (const key of call.keys) {
      if (!accepted.has(key)) {
        problems.push(
          `${call.file} 给 ${call.name} 传的字段 ${key} 后端不认 —— ` +
            `后端只收：${[...accepted].join('、') || '（无）'}。` +
            '字段名对不上时 Tauri 会静默忽略，值永远是默认值'
        )
      }
    }
    const required = args.filter((arg) => arg.required).map((arg) => camel(arg.name))
    for (const name of required) {
      if (!call.keys.includes(name)) {
        problems.push(`${call.file} 调用 ${call.name} 时没传必填参数 ${name}`)
      }
    }
  }

  // 反向：后端有、前端从不调的命令 = 死代码（前端改调用方式时最容易漏）。
  for (const name of commands.keys()) {
    if (!invoked.has(name)) {
      problems.push(`后端命令 ${name} 前端从来没调用 —— 可能是死代码`)
    }
  }

  // 3：假后端的载荷字段与真后端一致吗？
  const mockSource = readOptional(join(root, 'scripts/ui/check.mjs'))
  const statusFields = rustStructFields(engineSource, 'Status')
  const deviceFields = rustStructFields(audioSource, 'InputDevice')
  for (const payload of mockPayloads(mockSource)) {
    const real = payload.name === 'status' ? statusFields : deviceFields
    if (!real) continue
    for (const key of payload.keys) {
      if (!real.includes(key)) {
        problems.push(
          `scripts/ui/check.mjs 的假后端返回了 ${key}，真后端没有这个字段 —— ` +
            `真后端是：${real.join('、')}。界面检查会全绿，真机上却是坏的`
        )
      }
    }
    for (const field of real) {
      if (!payload.keys.includes(field)) {
        problems.push(
          `scripts/ui/check.mjs 的假后端漏了真后端的字段 ${field} —— ` +
            '界面检查测不到这个字段的行为'
        )
      }
    }
  }

  return {
    problems,
    summary: { commandCount: commands.size, callCount: calls.length },
  }
}

function main() {
  const root = join(new URL('.', import.meta.url).pathname, '..')
  const { problems, summary } = collectProblems(root)
  if (problems.length) {
    console.error('前后端契约检查未通过：')
    for (const problem of problems) console.error(`  ${problem}`)
    process.exit(1)
  }
  console.log(
    `契约检查通过：${summary.commandCount} 个命令，${summary.callCount} 处调用，字段名一致`
  )
}

// 直接运行时才检查（被 import 时不跑 —— 测试要能自己喂数据）。
function isEntryPoint() {
  if (!process.argv[1]) return false
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href
  } catch {
    return false
  }
}

if (isEntryPoint()) main()
