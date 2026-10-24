// 真实 WebKit 冒烟：Node 静态服务器 + Playwright WebKit + 假 window.__TAURI__。
// 运行：npm run check:ui
// 浏览器不在默认位置时：PLAYWRIGHT_BROWSERS_PATH=/path/to/browsers npm run check:ui
import { webkit } from 'playwright'
import { startServer, SRC } from './server.mjs'
import { join } from 'node:path'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'

const SHOTS = await mkdtemp(join(tmpdir(), 'vn-ui-shots-'))
const { server, base } = await startServer(SRC)

const browser = await webkit.launch()
const context = await browser.newContext({ viewport: { width: 980, height: 760 }, locale: 'zh-CN' })
const page = await context.newPage()

const errors = []
page.on('console', (msg) => {
  if (msg.type() === 'error') errors.push(`console: ${msg.text()}`)
})
page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`))
page.on('requestfailed', (request) => errors.push(`requestfailed: ${request.url()} ${request.failure()?.errorText}`))

await context.addInitScript(() => {
  const base = {
    modelReady: true,
    modelSource: 'app_data',
    modelDir: '/models',
    modelDirCustom: false,
    vadReady: true,
    outputDir: '/out',
    language: 'auto',
    mode: 'offline',
    itn: true,
    downloading: false,
    installedBytes: 241_357_257,
    totalBytes: 241_357_257,
    streamingReady: true,
    streamingDir: '/models/streaming',
    streamingBytes: 167_217_442,
    streamingTotalBytes: 461_589_961,
    punct: true,
    punctReady: true,
    hotwords: [],
    inputDevice: null,
    punctBytes: 294_372_519,
    punctTotalBytes: 294_372_519,
    appVersion: '0.1.1',
  }
  window.__mockCalls = []
  window.__mockStatus = { ...base }
  const clone = (value) => JSON.parse(JSON.stringify(value))
  const result = { text: '浏览器结果', segments: [], durationSeconds: 2, elapsedMs: 100, language: 'zh' }
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        window.__mockCalls.push([cmd, args])
        switch (cmd) {
          case 'status':
            return clone(window.__mockStatus)
          case 'set_language':
            window.__mockStatus = { ...window.__mockStatus, language: args.language }
            return clone(window.__mockStatus)
          case 'set_mode':
            window.__mockStatus = { ...window.__mockStatus, mode: args.mode }
            return clone(window.__mockStatus)
          case 'set_punct':
            window.__mockStatus = { ...window.__mockStatus, punct: args.enabled }
            return clone(window.__mockStatus)
          case 'set_hotwords':
            window.__mockStatus = { ...window.__mockStatus, hotwords: args.hotwords }
            return clone(window.__mockStatus)
          case 'default_hotwords':
            return ['产品评审会', '玄戒芯片', '张三']
          case 'set_itn':
            window.__mockStatus = { ...window.__mockStatus, itn: args.enabled }
            return clone(window.__mockStatus)
          case 'set_input_device':
            window.__mockStatus = { ...window.__mockStatus, inputDevice: args.name }
            return clone(window.__mockStatus)
          case 'set_model_dir':
          case 'set_output_dir':
          case 'download_model':
          case 'download_streaming_model':
            // 挂起：模拟「正在下载」，这样界面上的 busy 状态能保持住
            return new Promise(() => {})
          case 'list_input_devices':
            return [{ name: 'Mock Mic', isDefault: true }]
          case 'stop_recording':
            return clone(result)
          case 'save_text':
            return { path: '/out/x.txt', name: 'x.txt' }
          default:
            return undefined
        }
      },
    },
    event: {
      listen: async (name, cb) => {
        ;(window.__listeners ||= {})[name] ||= []
        window.__listeners[name].push(cb)
        return () => {}
      },
    },
    dialog: { open: async () => null },
    clipboardManager: { writeText: async () => {} },
    opener: { openPath: async () => {}, openUrl: async () => {} },
    window: { getCurrentWindow: () => ({ setBackgroundColor() {}, startDragging() {} }) },
    webview: { getCurrentWebview: () => ({ onDragDropEvent() {} }) },
  }
})

// 页面内触发 Tauri 事件（模拟后端发进度）
await page.addInitScript(() => {
  window.__emit = (name, payload) => {
    for (const cb of (window.__listeners?.[name] || [])) cb({ payload })
  }
})

const results = []
const check = (name, ok, extra = '') => results.push({ name, ok: Boolean(ok), extra })
const text = (selector) => page.locator(selector).innerText()
const phase = () => page.locator('.app').getAttribute('data-phase')

await page.goto(`${base}/index.html`)
await page.waitForSelector('#lang-row .chip')
await page.waitForTimeout(1200) // 等 init 的 900ms 设备枚举走完

// 1. 首屏
check('标题已翻译', (await text('#view-title')) === '转写')
check('6 个语言芯片', (await page.locator('#lang-row .chip').count()) === 6)
check('波形 48 根', (await page.locator('#wave span').count()) === 48)
check('空闲剪影有高度', (await page.locator('#wave span').nth(4).evaluate((el) => el.style.height)) === '18px')
check('无横向溢出', await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth))
check('无未翻译 key', !(await page.evaluate(() => /\b(view|record|result|history|settings|mode|model|setup|progress|duration|drop|theme|nav)\.[a-z]+\b/.test(document.body.innerText))))
await page.screenshot({ path: `${SHOTS}/01-zh-light.png` })

// 2. 麦克风流程：开始 → 设备下拉 Esc 不取消 → Esc 取消清空
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
check('点麦克风进入录音', (await phase()) === 'recording')
await page.locator('#device-select .select-button').click()
await page.waitForTimeout(150)
check('设备下拉打开', (await page.locator('.select-popup').count()) === 1)
await page.keyboard.press('Escape')
await page.waitForTimeout(150)
check('下拉 Esc 关闭弹层', (await page.locator('.select-popup').count()) === 0)
check('下拉 Esc 不取消录音', (await phase()) === 'recording')
await page.locator('body').click({ position: { x: 940, y: 740 } })
await page.keyboard.press('Escape')
await page.waitForTimeout(400)
check('Esc 取消回到 idle', (await phase()) === 'idle')
check('取消清空结果区', (await page.locator('#text').inputValue()) === '')

// 3. 录音 → 停止 → 结果 + 历史
await page.locator('#mic-btn').click()
await page.waitForTimeout(150)
check('再次开始录音', (await phase()) === 'recording')
await page.locator('#mic-btn').click()
await page.waitForTimeout(500)
check('停止后回到 idle', (await phase()) === 'idle')
check('结果框有文本', (await page.locator('#text').inputValue()) === '浏览器结果')
const historyCount = () =>
  page.evaluate(() => JSON.parse(localStorage.getItem('voicenotes.history.v1') || '[]').length)
check('历史写了一条', (await historyCount()) === 1)
await page.screenshot({ path: `${SHOTS}/02-zh-result.png` })

// 4. 设置页切英文：结果不被覆盖、历史不重复
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(150)
check('设置页模型路径正常', !(await text('#settings-model')).includes('null'))
// 术语表：填入示例 → 保存 → 回填
check('术语表输入框存在', (await page.locator('#hotwords-input').count()) === 1)
await page.locator('#hotwords-example').click()
await page.waitForTimeout(150)
check('填入示例', (await page.locator('#hotwords-input').inputValue()).includes('产品评审会'))
await page.locator('#hotwords-save').click()
await page.waitForTimeout(250)
check(
  '保存术语表调用后端',
  await page.evaluate(() => {
    const call = window.__mockCalls.filter(([cmd]) => cmd === 'set_hotwords').pop()
    return call?.[1]?.hotwords?.length === 3
  }),
)
check('保存后回填', (await page.locator('#hotwords-input').inputValue()).includes('玄戒芯片'))

await page.locator('#locale-select .select-button').click()
await page.waitForTimeout(150)
await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
await page.waitForTimeout(300)
check('切英文标题', (await text('#view-title')) === 'Settings')
check('英文术语表标签', (await page.locator('h3', { hasText: 'Terminology' }).count()) >= 1)
check('切英文结果保留', (await page.locator('#text').inputValue()) === '浏览器结果')
check('切英文历史不重复', (await historyCount()) === 1)
check('切英文 meta 跟随', (await text('#meta')).includes('Audio 2.0s'))
check('无未翻译 key（英文）', !(await page.evaluate(() => /\b(view|record|result|history|settings|mode|model|setup|progress|duration|drop|theme|nav)\.[a-z]+\b/.test(document.body.innerText))))
await page.screenshot({ path: `${SHOTS}/03-en-settings.png` })

// 5. 清空后切回中文：旧内容不复原
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.waitForTimeout(100)
await page.locator('#clear-btn').click()
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(100)
await page.locator('#locale-select .select-button').click()
await page.waitForTimeout(150)
await page.locator('.select-popup .select-item', { hasText: '简体中文' }).first().click()
await page.waitForTimeout(300)
check('清空后切中文不复原', (await page.locator('#text').inputValue()) === '')

// 6. 深色主题 + 截图
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.waitForTimeout(100)
await page.locator('#theme-btn').click()
await page.waitForTimeout(200)
check('深色主题生效', (await page.evaluate(() => document.documentElement.dataset.theme)) === 'dark')
check('深色无横向溢出', await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth))
await page.screenshot({ path: `${SHOTS}/04-dark.png` })
await page.locator('#theme-btn').click()

// 7. 焦点在按钮上按空格不录音；空白处空格开始、Esc 取消
await page.locator('#theme-btn').focus()
const callsBefore = await page.evaluate(() => window.__mockCalls.filter(([cmd]) => cmd === 'warm_up_mic').length)
await page.keyboard.press('Space')
await page.waitForTimeout(300)
check(
  '按钮上的空格不抢去录音',
  (await phase()) === 'idle' &&
    (await page.evaluate(() => window.__mockCalls.filter(([cmd]) => cmd === 'warm_up_mic').length)) === callsBefore,
)
await page.locator('#view-title').click()
await page.keyboard.press('Space')
await page.waitForTimeout(250)
check('空白处空格开始录音', (await phase()) === 'recording')
await page.keyboard.press('Escape')
await page.waitForTimeout(400)
check('空格开始后 Esc 取消', (await phase()) === 'idle')

// 8. 模式切换
await page.locator('.mode-chip[data-mode="streaming"]').click()
await page.waitForTimeout(300)
check('切到流式', (await page.locator('.mode-chip[data-mode="streaming"]').getAttribute('aria-pressed')) === 'true')
check(
  '流式下粤语/日语/韩语禁用',
  (await page.locator('#lang-row .chip[data-lang="ja"]').isDisabled()) &&
    (await page.locator('#lang-row .chip[data-lang="ko"]').isDisabled()),
)
await page.locator('.mode-chip[data-mode="offline"]').click()
await page.waitForTimeout(300)
check('切回整段', (await page.locator('.mode-chip[data-mode="offline"]').getAttribute('aria-pressed')) === 'true')

// ── 下载进度分流：两个下载可以同时跑，进度事件同名，靠 kind 区分 ──
await page.evaluate(() =>
  window.__emit('model:progress', {
    kind: 'streaming',
    resource: 'encoder.onnx',
    done: 50,
    total: 100,
    phase: 'downloading',
  })
)
await page.waitForTimeout(100)
const streamText = await page.locator('#stream-progress-text').innerText()
const setupText = await page.locator('#setup-progress-text').innerText()
check('流式进度显示在流式面板', streamText.includes('encoder.onnx'), streamText.slice(0, 40))
check('流式进度不串到整段面板', !setupText.includes('encoder.onnx'), setupText.slice(0, 40))

// 整段那边用 verifying 阶段：它的分支在没下载时也会更新（下载前要显示校验中）
await page.evaluate(() =>
  window.__emit('model:progress', {
    kind: 'offline',
    resource: 'model.int8.onnx',
    done: 10,
    total: 20,
    phase: 'verifying',
  })
)
await page.waitForTimeout(100)
const setupText2 = await page.locator('#setup-progress-text').innerText()
const streamText2 = await page.locator('#stream-progress-text').innerText()
check('整段进度显示在整段面板', setupText2.includes('model.int8.onnx'), setupText2.slice(0, 40))
check('整段进度不串到流式面板', !streamText2.includes('model.int8.onnx'), streamText2.slice(0, 40))

// ── 结果 ────────────────────────────────────────────────────
const failed = results.filter((r) => !r.ok)
for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}${r.extra ? ' — ' + r.extra : ''}`)
if (errors.length) {
  console.log('\n浏览器错误：')
  for (const e of errors) console.log('  ' + e)
}
console.log(`\n${results.length - failed.length}/${results.length} 通过`)
await browser.close()
server.close()
process.exit(failed.length || errors.length ? 1 : 0)
