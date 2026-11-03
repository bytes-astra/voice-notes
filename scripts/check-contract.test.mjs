// check-contract 门禁自己的测试。
//
// 门禁本身也要有人守：正则是文本级的，漏一个写法就静默放过一个契约破裂。
// 用法：node --test scripts/check-contract.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import {
  collectProblems,
  frontendInvokes,
  mockPayloads,
  rustCommands,
  rustStructFields,
} from './check-contract.mjs'

test('命令名与参数要扫出来', () => {
  const commands = rustCommands(`
    #[tauri::command]
    fn set_input_device(app: AppHandle, name: Option<String>) -> Result<(), String> {}

    #[tauri::command]
    async fn transcribe_file(path: String, session_id: u64) -> Result<(), String> {}
  `)
  assert.deepEqual([...commands.keys()], ['set_input_device', 'transcribe_file'])
  assert.deepEqual(commands.get('set_input_device'), [{ name: 'name', required: false }])
  assert.deepEqual(commands.get('transcribe_file'), [
    { name: 'path', required: true },
    { name: 'session_id', required: true },
  ])
})

test('#[tauri::command] 与 fn 之间夹着文档注释也要认', () => {
  const commands = rustCommands(`
    #[tauri::command]
    /// 把文本写进目录。
    fn save_text(text: String) -> Result<(), String> {}
  `)
  assert.ok(commands.has('save_text'), '文档注释把命令挡掉了')
})

test('前端 invoke 的命令名与载荷字段要扫出来', () => {
  const calls = frontendInvokes(`
    invoke('status')
    invoke('set_input_device', { name })
    invoke('save_text', { text, fileName: buildFileName(text) })
    invoke(streaming ? 'start_streaming_recording' : 'start_offline_recording', { sessionId })
  `)
  const byName = Object.fromEntries(calls.map((c) => [c.name, c.keys]))
  assert.deepEqual(byName.status, [])
  assert.deepEqual(byName.set_input_device, ['name'])
  assert.deepEqual(byName.save_text, ['text', 'fileName'])
  assert.deepEqual(byName.start_streaming_recording, ['sessionId'])
  assert.deepEqual(byName.start_offline_recording, ['sessionId'])
})

test('结构体字段要按 camelCase 抽出来', () => {
  const source = `
    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct InputDevice {
        pub name: String,
        /// 默认设备
        pub is_default: bool,
    }
  `
  assert.deepEqual(rustStructFields(source, 'InputDevice'), ['name', 'isDefault'])
})

test('假后端的载荷字段要扫出来', () => {
  const payloads = mockPayloads(`
    const base = {
      modelReady: true,
      hotwords: [],
    }
    switch (cmd) {
      case 'list_input_devices':
        return [{ name: 'Mock Mic', isDefault: true }]
    }
  `)
  const byName = Object.fromEntries(payloads.map((p) => [p.name, p.keys]))
  assert.deepEqual(byName.status, ['modelReady', 'hotwords'])
  assert.deepEqual(byName.list_input_devices, ['name', 'isDefault'])
})

test('前端调了后端没有的命令要报出来', () => {
  const dir = makeFixture({
    'src/app.js': "invoke('set_langauge', { language })\n",
    'src-tauri/src/lib.rs': `
      #[tauri::command]
      fn set_language(language: String) -> Result<(), String> {}
    `,
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('set_langauge')),
    `拼错的后端命令被放过了：${JSON.stringify(problems)}`,
  )
})

test('载荷字段名对不上要报出来（历史 bug：is_default vs isDefault）', () => {
  const dir = makeFixture({
    'src/status.js': "invoke('set_input_device', { isDefault })\n",
    'src-tauri/src/lib.rs': `
      #[tauri::command]
      fn set_input_device(name: Option<String>) -> Result<(), String> {}
    `,
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('isDefault')),
    `对不上的字段名被放过了：${JSON.stringify(problems)}`,
  )
})

test('假后端的字段名与真后端不一致要报出来', () => {
  const dir = makeFixture({
    'src/app.js': "invoke('status')\n",
    'src-tauri/src/lib.rs': `
      #[tauri::command]
      fn status() -> u64 { 0 }
    `,
    'src-tauri/src/engine.rs': `
      #[derive(Clone, Serialize)]
      #[serde(rename_all = "camelCase")]
      pub struct Status {
          pub model_ready: bool,
          pub hotwords_ignored: usize,
      }
    `,
    'scripts/ui/check.mjs': `
      const base = {
        modelReady: true,
        is_default: false,
      }
    `,
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('is_default')),
    `假后端的错误字段名被放过了：${JSON.stringify(problems)}`,
  )
  assert.ok(
    problems.some((problem) => problem.includes('hotwordsIgnored')),
    `假后端漏的字段被放过了：${JSON.stringify(problems)}`,
  )
})

test('没传必填参数要报出来', () => {
  const dir = makeFixture({
    'src/app.js': "invoke('transcribe_file', { path })\n",
    'src-tauri/src/lib.rs': `
      #[tauri::command]
      fn transcribe_file(path: String, session_id: u64) -> Result<(), String> {}
    `,
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('sessionId')),
    `漏传的必填参数被放过了：${JSON.stringify(problems)}`,
  )
})

test('后端有、前端不调的命令要报出来（死代码）', () => {
  const dir = makeFixture({
    'src/app.js': "invoke('status')\n",
    'src-tauri/src/lib.rs': `
      #[tauri::command]
      fn status() -> u64 { 0 }

      #[tauri::command]
      fn never_called() -> u64 { 0 }
    `,
  })
  const { problems } = collectProblems(dir)
  assert.ok(
    problems.some((problem) => problem.includes('never_called')),
    `死命令被放过了：${JSON.stringify(problems)}`,
  )
})

test('动态命令名要能解析（三元 + command 变量）', () => {
  const calls = frontendInvokes(`
    invoke(streaming ? 'start_streaming_recording' : 'start_offline_recording', { sessionId })
    invoke(command)
  `)
  const names = calls.map((c) => c.name)
  assert.ok(names.includes('start_streaming_recording'), '三元里的第一个命令没解析出来')
  assert.ok(names.includes('start_offline_recording'), '三元里的第二个命令没解析出来')
  // invoke(command) 的命令名在 command: '...' 选项里 —— 也要捞出来，
  // 否则这两个命令会被误报成「后端有、前端没调」。
  const withOption = frontendInvokes(`
    invoke(command)
  ` + "command: 'download_model'")
  assert.ok(
    withOption.some((c) => c.name === 'download_model'),
    `command 选项里的命令名没解析出来：${JSON.stringify(withOption)}`,
  )
})

test('动态命令名不会造成「死命令」假阳性', () => {
  const dir = makeFixture({
    'src/recorder.js':
      "invoke(streaming ? 'start_streaming_recording' : 'start_offline_recording', { sessionId })\n",
    'src-tauri/src/lib.rs': `
      #[tauri::command]
      fn start_streaming_recording(session_id: u64) -> Result<(), String> {}

      #[tauri::command]
      fn start_offline_recording(session_id: u64) -> Result<(), String> {}
    `,
  })
  const { problems } = collectProblems(dir)
  assert.deepEqual(problems, [])
})

test('一致的契约不该报错', () => {
  const dir = makeFixture({
    'src/app.js': "invoke('set_input_device', { name })\n",
    'src-tauri/src/lib.rs': `
      #[tauri::command]
      fn set_input_device(name: Option<String>) -> Result<(), String> {}
    `,
  })
  const { problems } = collectProblems(dir)
  assert.deepEqual(problems, [])
})

/// 造一个最小的仓库骨架，让 collectProblems 能跑起来。
function makeFixture(files) {
  const dir = mkdtempSync(join(tmpdir(), 'check-contract-'))
  for (const [file, content] of Object.entries(files)) {
    const path = join(dir, file)
    mkdirSync(join(path, '..'), { recursive: true })
    writeFileSync(path, content)
  }
  return dir
}
