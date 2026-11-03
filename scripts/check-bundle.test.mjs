// check-bundle 门禁自己的测试。
//
// 门禁本身也要有人守：正则是文本级的，漏一个写法就静默放过一个配置漂移。
// 用法：node --test scripts/check-bundle.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'

import {
  bundleConfigProblems,
  macOnlyImportProblems,
  menuModuleProblems,
} from './check-bundle.mjs'

const OK_CONFIG = {
  bundle: {
    macOS: {
      signingIdentity: '-',
      entitlements: 'Entitlements.plist',
      infoPlist: 'Info.plist',
    },
  },
}
const OK_ENTITLEMENTS = '<key>com.apple.security.device.audio-input</key><true/>'
const OK_INFO_PLIST = '<key>NSMicrophoneUsageDescription</key><string>录音</string>'

test('配置齐全时不该报错', () => {
  const problems = bundleConfigProblems({
    config: OK_CONFIG,
    entitlements: OK_ENTITLEMENTS,
    infoPlist: OK_INFO_PLIST,
  })
  assert.deepEqual(problems, [])
})

test('签名身份不是 ad-hoc 要报出来', () => {
  const problems = bundleConfigProblems({
    config: { bundle: { macOS: { ...OK_CONFIG.bundle.macOS, signingIdentity: '' } } },
    entitlements: OK_ENTITLEMENTS,
    infoPlist: OK_INFO_PLIST,
  })
  assert.ok(
    problems.some((p) => p.includes('signingIdentity')),
    `没 ad-hoc 签名被放过了：${JSON.stringify(problems)}`,
  )
})

test('缺麦克风 entitlement 要报出来', () => {
  const problems = bundleConfigProblems({
    config: OK_CONFIG,
    entitlements: '<plist></plist>',
    infoPlist: OK_INFO_PLIST,
  })
  assert.ok(
    problems.some((p) => p.includes('com.apple.security.device.audio-input')),
    `缺麦克风权限被放过了：${JSON.stringify(problems)}`,
  )
})

test('Info.plist 缺用途说明要报出来', () => {
  const problems = bundleConfigProblems({
    config: OK_CONFIG,
    entitlements: OK_ENTITLEMENTS,
    infoPlist: '<plist></plist>',
  })
  assert.ok(
    problems.some((p) => p.includes('NSMicrophoneUsageDescription')),
    `缺用途说明被放过了：${JSON.stringify(problems)}`,
  )
})

test('menu.rs 的顶层 import 要报出来', () => {
  const problems = menuModuleProblems(`
use tauri::menu::MenuBuilder;

#[cfg(target_os = "macos")]
pub fn install_menu() {}
`)
  assert.ok(
    problems.some((p) => p.includes('顶层有 use')),
    `顶层 import 被放过了：${JSON.stringify(problems)}`,
  )
})

test('menu.rs 里没带 cfg 的函数要报出来', () => {
  const problems = menuModuleProblems(`
#[cfg(target_os = "macos")]
pub fn install_menu() {
    use tauri::menu::MenuBuilder;
}

pub fn set_texts() {}
`)
  assert.ok(
    problems.some((p) => p.includes('set_texts')),
    `没带 cfg 的函数被放过了：${JSON.stringify(problems)}`,
  )
})

test('menu.rs 规矩写对了不该报错', () => {
  const problems = menuModuleProblems(`
#[cfg(target_os = "macos")]
pub fn install_menu() {
    use tauri::menu::MenuBuilder;
    let _ = MenuBuilder;
}

/// 更新标题。
#[cfg(target_os = "macos")]
pub fn set_texts() {
    use tauri::menu::MenuItemKind;
    let _ = MenuItemKind;
}
`)
  assert.deepEqual(problems, [])
})

test('lib.rs 里 menu 模块的 import 漏 cfg 要报出来', () => {
  const problems = macOnlyImportProblems(`
use menu::install_menu;
`)
  assert.ok(
    problems.some((p) => p.includes('install_menu')),
    `漏 cfg 的 import 被放过了：${JSON.stringify(problems)}`,
  )
})

test('lib.rs 里 menu 模块的 import 带 cfg 不该报错', () => {
  const problems = macOnlyImportProblems(`
#[cfg(target_os = "macos")]
use menu::install_menu;
use tauri::Manager;
`)
  assert.deepEqual(problems, [])
})
