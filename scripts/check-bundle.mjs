// 打包与原生层配置检查：签名、麦克风权限、macOS 专属代码的 cfg。
//
// 运行：npm run check:bundle
//
// 这一档守的都是「改了配置不会报错、真机上却坏掉」的那一类问题：
//
//   1. macOS 打包必须 ad-hoc 签名（bundle.macOS.signingIdentity = "-"）。
//      留空时 Tauri 不做 codesign，链接器留下的是 linker-signed
//      （Sealed Resources=none、Info.plist 未绑定），`codesign --verify` 失败，
//      别的 Mac 从 dmg 装完 Gatekeeper 直接报「已损坏」。
//   2. 必须挂麦克风 entitlement（com.apple.security.device.audio-input）。
//      打包签名带 hardened runtime 时，缺这条 tccd 根本不弹授权框，
//      直接拒绝麦克风请求 —— 表现是「能录音、有时长，就是没字」。
//   3. Info.plist 必须有 NSMicrophoneUsageDescription（授权框里的用途说明）。
//   4. macOS 专属的原生代码必须整体带 #[cfg(target_os = "macos")]。
//      原生菜单的 import 漏了 cfg，Windows 上就编译不过 —— 而 CI 平时不编译
//      Windows，等到打 tag 才发现就晚了。
//
// 这些都不是「代码风格」，是「改了没人拦、真机才炸」的配置漂移。
import { readFileSync, realpathSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

/// ad-hoc 签名在 tauri.conf.json 里的写法。
export const ADHOC_IDENTITY = '-'

/// 打包版必须有这条 entitlement，否则 tccd 连授权框都不弹。
export const MIC_ENTITLEMENT = 'com.apple.security.device.audio-input'

/// macOS 授权框里显示的用途说明。
export const MIC_USAGE_KEY = 'NSMicrophoneUsageDescription'

/// 找某个行号往上最近的一个属性行（`#[...]`）。
///
/// 属性行与目标行之间只允许空行和注释 —— 中间夹了别的代码就当「没有属性」。
function attrAbove(lines, index) {
  for (let i = index - 1; i >= 0 && i >= index - 5; i--) {
    const trimmed = lines[i].trim()
    if (trimmed.startsWith('#[')) return trimmed
    if (trimmed && !trimmed.startsWith('//')) return ''
  }
  return ''
}

/// 整个文件都是 macOS 专属时的检查（menu.rs）：
/// 顶层不能有 use（import 必须在 cfg 掉的函数体内），每个函数都要带 cfg。
export function menuModuleProblems(source, file = 'src-tauri/src/menu.rs') {
  const problems = []
  const lines = source.split('\n')
  lines.forEach((line, index) => {
    if (/^use\s+/.test(line)) {
      problems.push(
        `${file} 顶层有 use（${line.trim()}）—— ` +
          'macOS 专属的 import 必须放在 #[cfg(target_os = "macos")] 的函数体内',
      )
    }
    if (/^(pub\s+)?fn\s+/.test(line)) {
      if (!/cfg\(target_os\s*=\s*"macos"\)/.test(attrAbove(lines, index))) {
        problems.push(
          `${file} 的函数「${line.trim()}」没有 #[cfg(target_os = "macos")] —— ` +
            'Windows 上编译不过',
        )
      }
    }
  })
  return problems
}

/// 其它文件里引用 macOS 专属模块的 import 必须带 cfg。
///
/// 只看项目自己的 macOS 专属模块（`menu::`）—— 别的 `use` 在 Windows 上都要用。
export function macOnlyImportProblems(source, file = 'src-tauri/src/lib.rs') {
  const problems = []
  const lines = source.split('\n')
  lines.forEach((line, index) => {
    if (!/^use\s+menu::/.test(line)) return
    if (!/cfg\(target_os\s*=\s*"macos"\)/.test(attrAbove(lines, index))) {
      problems.push(
        `${file} 的「${line.trim()}」没有 #[cfg(target_os = "macos")] —— ` +
          'Windows 上编译不过（菜单只在 macOS 存在）',
      )
    }
  })
  return problems
}

/// 打包配置本身：签名身份、entitlement、Info.plist。
export function bundleConfigProblems({ config, entitlements, infoPlist }) {
  const problems = []
  const mac = config?.bundle?.macOS || {}
  if (mac.signingIdentity !== ADHOC_IDENTITY) {
    problems.push(
      `bundle.macOS.signingIdentity 必须是 "${ADHOC_IDENTITY}"（ad-hoc 签名），` +
        `现在是 ${JSON.stringify(mac.signingIdentity ?? null)} —— ` +
        '留空时打包版不弹标准授权框，别的 Mac 装完还报「已损坏」',
    )
  }
  if (!mac.entitlements) {
    problems.push('bundle.macOS.entitlements 没配 —— 打包版没有麦克风权限，tccd 直接拒绝')
  } else if (!entitlements.includes(MIC_ENTITLEMENT)) {
    problems.push(
      `${mac.entitlements} 里没有 ${MIC_ENTITLEMENT} —— ` +
        '缺这条时 tccd 根本不弹授权框，录音拿到的是精确零流',
    )
  }
  if (!mac.infoPlist) {
    problems.push('bundle.macOS.infoPlist 没配 —— 授权框里没有用途说明')
  } else if (!infoPlist.includes(MIC_USAGE_KEY)) {
    problems.push(`${mac.infoPlist} 里没有 ${MIC_USAGE_KEY} —— 授权框里没有用途说明`)
  }
  return problems
}

function readOptional(path) {
  try {
    return readFileSync(path, 'utf8')
  } catch {
    return ''
  }
}

/// 把仓库里各处文件读出来，跑完所有检查。
export function collectProblems(root) {
  const problems = []
  const tauriDir = join(root, 'src-tauri')
  const config = JSON.parse(readFileSync(join(tauriDir, 'tauri.conf.json'), 'utf8'))
  const mac = config?.bundle?.macOS || {}
  // conf 里的路径相对 src-tauri/（Tauri 就是这么解析的）。
  const entitlements = mac.entitlements ? readOptional(join(tauriDir, mac.entitlements)) : ''
  const infoPlist = mac.infoPlist ? readOptional(join(tauriDir, mac.infoPlist)) : ''

  problems.push(...bundleConfigProblems({ config, entitlements, infoPlist }))
  problems.push(...menuModuleProblems(readOptional(join(tauriDir, 'src/menu.rs'))))
  problems.push(...macOnlyImportProblems(readOptional(join(tauriDir, 'src/lib.rs'))))
  return { problems, summary: { entitlements: mac.entitlements || '(没配)', infoPlist: mac.infoPlist || '(没配)' } }
}

function main() {
  const root = join(new URL('.', import.meta.url).pathname, '..')
  const { problems, summary } = collectProblems(root)
  if (problems.length) {
    console.error('打包 / 原生层配置检查未通过：')
    for (const problem of problems) console.error(`  ${problem}`)
    process.exit(1)
  }
  console.log(
    `打包配置检查通过：ad-hoc 签名、麦克风 entitlement（${summary.entitlements}）、` +
      `用途说明（${summary.infoPlist}）、macOS 专属代码的 cfg`,
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
