// 真实 WebKit 冒烟：Node 静态服务器 + Playwright WebKit + 假 window.__TAURI__。
// 运行：npm run check:ui
// 浏览器不在默认位置时：PLAYWRIGHT_BROWSERS_PATH=/path/to/browsers npm run check:ui
import { webkit } from 'playwright'
import { startServer, SRC, REPO } from './server.mjs'
import { join } from 'node:path'
import { mkdtemp } from 'node:fs/promises'
import { readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'

// 界面检查用「应用真实的默认窗口尺寸」：尺寸写在 src-tauri/tauri.conf.json 里，
// 这里读出来当 viewport —— 改了窗口大小，界面断言自动跟着，不会跟实际默认值脱节。
const tauriConf = JSON.parse(readFileSync(join(REPO, 'src-tauri/tauri.conf.json'), 'utf8'))
const mainWindow = tauriConf.app.windows.find((w) => w.label === 'main')
const viewport = { width: mainWindow.width, height: mainWindow.height }

const SHOTS = await mkdtemp(join(tmpdir(), 'vn-ui-shots-'))
const { server, base } = await startServer(SRC)

const browser = await webkit.launch()
const context = await browser.newContext({ viewport, locale: 'zh-CN' })
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
    downloadingStreaming: false,
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
  window.__mockTranscript = ''
  window.__mockStopError = null
  window.__mockStopWarning = null
  window.__mockStopGate = null
  window.__mockStartGate = null
  window.__mockReserveError = null
  window.__mockSessionId = 0
  window.__mockFileSessionId = 0
  window.__mockCancelGate = null
  window.__mockError = null
  window.__mockStatusError = null
  window.__mockStatus = { ...base }
  const clone = (value) => JSON.parse(JSON.stringify(value))
  const result = { text: '浏览器结果', segments: [], durationSeconds: 2, elapsedMs: 100, language: 'zh' }
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        window.__mockCalls.push([cmd, args])
        if (window.__mockError) throw window.__mockError
        switch (cmd) {
          case 'status':
            if (window.__mockStatusError) throw window.__mockStatusError
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
          case 'reserve_session':
            if (window.__mockReserveError) throw window.__mockReserveError
            return ++window.__mockSessionId
          case 'transcribe_file':
            return { ...clone(result), text: '文件结果' }
          case 'cancel_recording':
            if (window.__mockCancelGate) await window.__mockCancelGate
            return undefined
          case 'start_offline_recording':
          case 'start_streaming_recording':
            if (window.__mockStartGate) await window.__mockStartGate
            window.__mockTranscript = ''
            return args.sessionId
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
            // 第二个设备名字是空的：宿主偶尔会返回无名设备，界面要有回退标签。
            return [{ name: 'Mock Mic', isDefault: true }, { name: '', isDefault: false }]
          case 'stop_recording':
            if (window.__mockStopGate) await window.__mockStopGate
            if (window.__mockStopError) throw window.__mockStopError
            // 跟真后端一致：返回这次会话累积出来的全文
            return { ...clone(result), text: window.__mockTranscript || result.text, warning: window.__mockStopWarning || null }
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
check(
  '默认窗口尺寸下没有纵向滚动条',
  await page.evaluate(() => {
    const scroll = document.querySelector('.main-scroll')
    return scroll.scrollHeight <= scroll.clientHeight
  }),
  await page.evaluate(() => {
    const scroll = document.querySelector('.main-scroll')
    return `内容 ${scroll.scrollHeight} / 可视 ${scroll.clientHeight}`
  }),
)
check(
  '默认窗口尺寸下转写结果卡片完整可见',
  await page.evaluate(() => {
    const card = document.querySelector('.result').getBoundingClientRect()
    return card.top >= 0 && card.bottom <= window.innerHeight
  }),
  await page.evaluate(() => {
    const card = document.querySelector('.result').getBoundingClientRect()
    return `卡片 ${Math.round(card.top)}..${Math.round(card.bottom)} / 窗口 ${window.innerHeight}`
  }),
)
check('macOS 根背景保持透明', await page.evaluate(() => getComputedStyle(document.documentElement).backgroundColor === 'rgba(0, 0, 0, 0)'))
check('无未翻译 key', !(await page.evaluate(() => /\b(view|record|result|history|settings|mode|model|setup|progress|duration|drop|theme|nav)\.[a-z]+\b/.test(document.body.innerText))))
check(
  '结果区按钮是纯图标且有可访问名称',
  await page.evaluate(() => {
    const want = { 'copy-btn': '复制', 'save-btn': '保存', 'clear-btn': '清空' }
    return Object.entries(want).every(([id, label]) => {
      const btn = document.getElementById(id)
      if (!btn) return false
      return (
        Boolean(btn.querySelector('svg')) &&
        btn.getAttribute('aria-label') === label &&
        btn.title === label &&
        !btn.textContent.trim()
      )
    })
  }),
  await page.evaluate(() => document.getElementById('copy-btn')?.outerHTML.slice(0, 140) || '(没有 copy-btn)'),
)
check(
  '结果区复制/保存是主操作，清空不是',
  await page.evaluate(() => {
    const cls = (id) => document.getElementById(id)?.className || ''
    return cls('copy-btn').includes('outlined') && cls('save-btn').includes('outlined') && !cls('clear-btn').includes('outlined')
  }),
  await page.evaluate(() => ['copy-btn', 'save-btn', 'clear-btn'].map((id) => `${id}:${document.getElementById(id)?.className}`).join(' | ')),
)
await page.screenshot({ path: `${SHOTS}/01-zh-light.png` })

// 2. 麦克风流程：开始 → 设备下拉 Esc 不取消 → Esc 取消清空
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
check('点麦克风进入录音', (await phase()) === 'recording', await text('#toast'))
await page.locator('#device-select .select-button').click()
await page.waitForTimeout(150)
check('设备下拉打开', (await page.locator('.select-popup').count()) === 1)
// 断言设备项本身带后缀：下拉首项本来就叫「系统默认」，
// 断言整段文本里有没有「系统默认」是恒真的，等于没测。
check('默认设备标记使用真实后端字段', (await page.locator('.select-popup').innerText()).includes('Mock Mic · 系统默认'))
check('空设备名有回退标签', (await page.locator('.select-popup').innerText()).includes('未命名设备'))
await page.evaluate(async () => {
  const { toast } = await import('./core.js')
  let now = 0
  const timers = new Map()
  let next = 1
  window.__realSetTimeout = window.setTimeout
  window.__realClearTimeout = window.clearTimeout
  window.setTimeout = (fn, delay) => { const id = next++; timers.set(id, { fn, at: now + delay }); return id }
  window.clearTimeout = (id) => timers.delete(id)
  toast('旧提示')
  now = 2400
  for (const [id, timer] of timers) if (timer.at <= now) { timers.delete(id); timer.fn() }
  toast('新提示')
  now = 2450
  for (const [id, timer] of timers) if (timer.at <= now) { timers.delete(id); timer.fn() }
  window.__toastHiddenByOldTimer = document.getElementById('toast').classList.contains('hidden')
  window.setTimeout = window.__realSetTimeout
  window.clearTimeout = window.__realClearTimeout
})
check('旧 toast 隐藏定时器不隐藏新提示', !(await page.evaluate(() => window.__toastHiddenByOldTimer)))
await page.evaluate(() => { document.getElementById('toast').classList.add('hidden') })
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
check(
  '历史行内三个图标按钮都在',
  await page.evaluate(() => {
    const actions = document.querySelector('.history-item .history-actions')
    if (!actions) return false
    const buttons = [...actions.querySelectorAll('button.icon-btn')]
    return buttons.length === 3 && buttons.every((b) => b.querySelector('svg') && b.getAttribute('aria-label'))
  }),
  await page.evaluate(() => document.querySelector('.history-item .history-actions')?.innerHTML.slice(0, 120) || '(没有历史行)'),
)
check(
  '结果区清空按钮不是垃圾桶（与历史删除按钮不同图标）',
  await page.evaluate(() => {
    const clearSvg = document.getElementById('clear-btn')?.querySelector('svg')?.outerHTML || ''
    const deleteSvg = document.querySelector('.history-item .history-actions button:nth-child(3) svg')?.outerHTML || ''
    if (!clearSvg || !deleteSvg) return false
    return clearSvg !== deleteSvg
  }),
  await page.evaluate(() => document.getElementById('clear-btn')?.querySelector('svg')?.outerHTML.slice(0, 120) || '(没有清空按钮图标)'),
)
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
check('原生菜单同步英文', await page.evaluate(() => {
  const call = window.__mockCalls.filter(([cmd]) => cmd === 'set_menu_texts').pop()
  return call?.[1]?.edit === 'Edit' && call?.[1]?.view === 'View'
}))
check('切英文 meta 跟随', (await text('#meta')).includes('Audio 2.0s'))
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.locator('#text').fill('用户手改的内容')
await page.locator('.nav-item[data-view="settings"]').click()
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: '简体中文' }).first().click()
await page.waitForTimeout(200)
check('切语言不覆盖用户编辑', (await page.locator('#text').inputValue()) === '用户手改的内容')
check('切语言刷新字数', (await text('#meta')).includes('7 字'))
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.locator('#clear-btn').click()
await page.locator('.nav-item[data-view="settings"]').click()
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
await page.waitForTimeout(200)
check('空结果切语言刷新提示', (await text('#meta')).includes('Waiting'))
await page.locator('.nav-item[data-view="settings"]').click()
check(
  '切英文后结果区按钮跟随',
  await page.evaluate(() => {
    const want = { 'copy-btn': 'Copy', 'save-btn': 'Save', 'clear-btn': 'Clear' }
    return Object.entries(want).every(([id, label]) => {
      const btn = document.getElementById(id)
      return Boolean(btn) && btn.getAttribute('aria-label') === label && btn.title === label
    })
  }),
  await page.evaluate(() => document.getElementById('copy-btn')?.getAttribute('aria-label') || '(没有 copy-btn)'),
)
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
check('原生菜单同步中文', await page.evaluate(() => {
  const call = window.__mockCalls.filter(([cmd]) => cmd === 'set_menu_texts').pop()
  return call?.[1]?.edit === '编辑' && call?.[1]?.view === '视图'
}))

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

// ── 整段录音：一段话 → 静音 → 再说，句子不能丢 ──
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
check('录音中', (await page.locator('.app').getAttribute('data-phase')) === 'recording')
check('录音中语言与 ITN 锁定', await page.evaluate(() =>
  [...document.querySelectorAll('#lang-row .chip')].every((chip) => chip.disabled) &&
  document.getElementById('itn-toggle').disabled
), await page.evaluate(() => JSON.stringify({ phase: document.querySelector('.app').dataset.phase, itn: document.getElementById('itn-toggle').disabled, langs: [...document.querySelectorAll('#lang-row .chip')].map((chip) => chip.disabled) })))

const say = async (text) => {
  await page.evaluate((t) => {
    // 模拟后端：先累积到 parts，再发事件
    window.__mockTranscript = window.__mockTranscript ? window.__mockTranscript + t : t
    window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: t, start: 0, end: 1 })
  }, text)
}
await say('第一句')
await page.waitForTimeout(100)
await say('第二句')
await page.waitForTimeout(100)
// 静音一段（前端这段时间什么都不该做）
await page.waitForTimeout(600)
await say('第三句')
await page.waitForTimeout(200)

const live = await page.locator('#text').inputValue()
check(
  '说话→静音→再说话：三句都在',
  ['第一句', '第二句', '第三句'].every((t) => live.includes(t)),
  live.slice(0, 60)
)

// 停止录音：后端返回全文，前端要用它替换结果框
await page.locator('#mic-btn').click()
await page.waitForTimeout(400)
const final = await page.locator('#text').inputValue()
check(
  '停止后结果框是后端返回的全文',
  ['第一句', '第二句', '第三句'].every((t) => final.includes(t)),
  final.slice(0, 60)
)
check('停止后回到空闲', (await page.locator('.app').getAttribute('data-phase')) === 'idle')
check('停止后语言与 ITN 解锁', await page.evaluate(() =>
  [...document.querySelectorAll('#lang-row .chip')].every((chip) => !chip.disabled) &&
  !document.getElementById('itn-toggle').disabled
))
check('按模式调用录音命令', await page.evaluate(() =>
  window.__mockCalls.some(([cmd]) => cmd === 'start_offline_recording') &&
  window.__mockCalls.every(([cmd]) => cmd !== 'start_recording')
))

// 流式模式必须调 start_streaming_recording：mock 里两个启动命令走同一分支，
// 不显式断言就测不出「流式误调成整段」。
await page.locator('.mode-chip[data-mode="streaming"]').click()
await page.waitForTimeout(200)
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
check('流式调用流式启动命令', await page.evaluate(() =>
  window.__mockCalls.some(([cmd]) => cmd === 'start_streaming_recording')
))
await page.locator('#mic-btn').click()
await page.waitForTimeout(400)
check('流式停止后回到空闲', (await phase()) === 'idle')
await page.locator('.mode-chip[data-mode="offline"]').click()
await page.waitForTimeout(200)

// ── 麦克风异常：保存已有转写，不能像 Esc 一样清空 ──
const historyBeforeMicError = await historyCount()
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
await say('故障前的一句')
await page.evaluate(() => window.__emit('audio:error', { code: 'record.micError', fatal: true }))
await page.waitForTimeout(400)
check('麦克风异常后回到空闲', (await phase()) === 'idle')
check('麦克风异常保留部分结果', (await page.locator('#text').inputValue()) === '故障前的一句')
check('麦克风异常写入历史', (await historyCount()) === historyBeforeMicError + 1)
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
await say('带警告的部分结果')
await page.evaluate(() => {
  window.__mockStopWarning = 'record.micError'
  window.__emit('audio:error', { code: 'record.micError', fatal: false })
})
await page.locator('#mic-btn').click()
await page.waitForTimeout(400)
check('部分结果警告不被完成提示覆盖', (await text('#toast')) === '麦克风发生异常')
await page.evaluate(() => { window.__mockStopWarning = null })

// 启动尚未完成时的致命采集错误：不得继续进入 failed recording。
await page.evaluate(() => {
  window.__mockStartGate = new Promise((resolve) => { window.__releaseStart = resolve })
})
await page.locator('#mic-btn').click()
await page.waitForTimeout(100)
await page.evaluate(() => {
  window.__emit('audio:error', { code: 'record.micError', fatal: true })
  window.__releaseStart()
})
await page.waitForTimeout(400)
check('启动期麦克风异常不进入录音', (await phase()) === 'idle')
await page.evaluate(() => { window.__mockStartGate = null })

// Esc 取消与致命事件竞态：迟到的停止结果不能复活已取消的录音。
const historyBeforeCancelRace = await historyCount()
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
await say('取消时不能保存')
await page.evaluate(() => {
  let cancelReady
  let stopReady
  window.__mockCancelGate = new Promise((resolve) => { cancelReady = resolve })
  window.__mockStopGate = new Promise((resolve) => { stopReady = resolve })
  window.__releaseCancelRace = () => { cancelReady(); stopReady() }
})
await page.locator('#view-title').click()
await page.keyboard.press('Escape')
await page.waitForTimeout(50)
await page.evaluate(() => {
  window.__emit('audio:error', { code: 'record.micError', fatal: true })
  window.__releaseCancelRace()
})
await page.waitForTimeout(400)
check('取消竞态不写历史', (await historyCount()) === historyBeforeCancelRace)
check('取消竞态结果仍为空', (await page.locator('#text').inputValue()) === '')
await page.evaluate(() => window.__emit('stream:segment', { sessionId: 0, segment: '旧会话不能入新结果' }))
check('旧会话分段被忽略', !(await page.locator('#text').inputValue()).includes('旧会话'))
await page.evaluate(() => window.__emit('stream:partial', { sessionId: 0, partial: '旧中间结果' }))
check('旧会话中间结果被忽略', (await page.locator('#live-line').textContent()) === '')

// 停止彻底失败：保留并保存已经上屏的文本，空文本不写历史。
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
await say('停止失败前的一句')
await page.evaluate(() => { window.__mockStopError = 'record.streamTimeout' })
await page.locator('#mic-btn').click()
await page.waitForTimeout(400)
check('停止失败保留已有文本', (await page.locator('#text').inputValue()) === '停止失败前的一句')
check('停止失败保留已有历史', (await historyCount()) === historyBeforeMicError + 3)
check('停止失败仍显示错误', (await text('#toast')).includes('超时'))

// 停止彻底失败且编辑区没有文本：不写历史、不留结果（__mockStopError 还是上一条留下的）。
const historyBeforeEmptyStop = await historyCount()
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
await page.locator('#mic-btn').click()
await page.waitForTimeout(400)
check('停止失败且无文本时不写历史', (await historyCount()) === historyBeforeEmptyStop)
check('停止失败且无文本时结果区为空', (await page.locator('#text').inputValue()) === '')
await page.evaluate(() => { window.__mockStopError = null })

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
check('流式下载空来源不显示缓存', !streamText.includes('本地缓存'), streamText)
await page.evaluate(async () => {
  const { loadDevices } = await import('./status.js')
  const { state } = await import('./core.js')
  state.status.inputDevice = '已经拔掉的麦克风'
  await loadDevices()
})
check('失效设备显示系统默认', (await page.locator('#device-select .select-label').innerText()) === '系统默认')
check('失效设备同步后端设置', await page.evaluate(() =>
  window.__mockCalls.some(([cmd, args]) => cmd === 'set_input_device' && args.name === null)
))

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

// 正常文件转写：后端 ID 用于事件过滤，不能误当本地代号导致卡死。
await page.evaluate(async () => {
  const { transcribeFilePath } = await import('./files.js')
  const task = transcribeFilePath('/tmp/demo.mp3')
  await new Promise((resolve) => setTimeout(resolve, 50))
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '文件句' })
  await task
})
await page.waitForTimeout(200)
check('文件转写成功恢复空闲', (await phase()) === 'idle')
check('文件转写保存结果', (await page.locator('#text').inputValue()) === '文件结果')

// 文件会话预留失败也必须复位，不能卡住之后的工作。
await page.evaluate(async () => {
  window.__mockReserveError = 'record.stopFailed'
  const { transcribeFilePath } = await import('./files.js')
  await transcribeFilePath('/tmp/demo.mp3')
})
await page.waitForTimeout(200)
check('文件预留失败恢复空闲', (await phase()) === 'idle')
await page.locator('.nav-item[data-view="settings"]').click()
await page.locator('#hotwords-input').focus()
await page.keyboard.press('Escape')
check('输入框 Esc 不跳转视图', (await page.locator('.app').getAttribute('data-view')) === 'settings')
await page.evaluate(async () => {
  const { state } = await import('./core.js')
  const { renderHistory, saveHistory } = await import('./history.js')
  state.history = [{ text: '旧一', at: 1 }, { text: '旧二', at: 2 }]
  saveHistory()
  renderHistory()
})
await page.locator('.nav-item[data-view="history"]').click()
await page.locator('.history-item').first().locator('.history-actions button').nth(2).click()
check('无 id 旧历史只删除当前项', await page.evaluate(() =>
  JSON.parse(localStorage.getItem('voicenotes.history.v1')).length === 1
))
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.evaluate(() => { window.__mockReserveError = null })
await page.locator('#mic-btn').click()
await page.waitForTimeout(250)
check('文件预留失败后可录音', (await phase()) === 'recording')
await page.keyboard.press('Escape')
await page.waitForTimeout(300)

// ── 错误提示：只能显示字典文案，不能把错误码 / 本地路径原样弹给用户 ──
await page.evaluate(() =>
  window.__emit('engine:state', {
    phase: 'error',
    message: 'model.loadFailed:/Users/someone/Library/Application Support/models/model.int8.onnx',
  }),
)
await page.waitForTimeout(200)
const engineErrorToast = await page.locator('#toast').innerText()
check('模型加载失败显示字典文案', engineErrorToast === '模型加载失败', engineErrorToast)
check(
  '错误提示不泄露错误码和本地路径',
  !engineErrorToast.includes('model.loadFailed') && !engineErrorToast.includes('/Users/'),
  engineErrorToast,
)

// ── 下载失败：不能把镜像地址 / HTTP 状态原样弹给用户 ──
// 下载按钮在「模型已就绪」时是隐藏的，直接在页面里点它的 click 事件。
await page.evaluate(() => document.getElementById('download-btn').click())
await page.waitForTimeout(200)
await page.evaluate(() =>
  window.__emit('model:progress', {
    kind: 'offline',
    resource: 'model.int8.onnx',
    done: 0,
    total: 1,
    phase: 'error',
    message: 'https://hf-mirror.com: HTTP 500',
  }),
)
await page.waitForTimeout(200)
const downloadErrorToast = await page.locator('#toast').innerText()
check('下载失败不弹原始错误串', !downloadErrorToast.includes('hf-mirror'), downloadErrorToast)

// 非应用错误码只能进日志，不能把系统路径或异常原文显示出来。
await page.evaluate(async () => {
  const { refreshStatus } = await import('./status.js')
  window.__mockStatusError = { detail: '/tmp/private-model' }
  await refreshStatus()
})
await page.waitForTimeout(200)
check('未知对象错误不泄露细节', (await text('#toast')).includes('发生未知错误') && !(await text('#toast')).includes('/tmp/'))
check('未知对象细节写入日志', await page.evaluate(() =>
  window.__mockCalls.some(([cmd, args]) => cmd === 'client_log' && args.message.includes('/tmp/private-model'))
))
await page.evaluate(async () => {
  const { refreshStatus } = await import('./status.js')
  window.__mockStatusError = 'permission denied /tmp/private-model'
  await refreshStatus()
})
await page.waitForTimeout(200)
check('未知字符串错误不泄露细节', (await text('#toast')).includes('发生未知错误') && !(await text('#toast')).includes('/tmp/'))
await page.evaluate(async () => {
  const { refreshStatus } = await import('./status.js')
  window.__mockStatusError = new Error('boom /tmp/private-model')
  await refreshStatus()
})
await page.waitForTimeout(200)
check('Error 对象不泄露原文', (await text('#toast')).includes('发生未知错误') && !(await text('#toast')).includes('/tmp/'))
check('Error 对象细节写入日志', await page.evaluate(() =>
  window.__mockCalls.some(([cmd, args]) => cmd === 'client_log' && args.message.includes('/tmp/private-model'))
))
await page.evaluate(() => { window.__mockStatusError = null })

// ── 切语言不覆盖用户编辑（连续两次切换）────────────────────
// 结果区里放一段带缺口占位符的文本：切语言时占位符要跟着变，
// 用户手改的正文一个字符都不能动 —— 而且连续切两次也不能被原文盖回去。
await page.evaluate(async () => {
  const { showTranscript } = await import('./result.js')
  showTranscript(
    { text: '原始文本 [[gap:3]]', segments: [], durationSeconds: 2, elapsedMs: 100, language: 'zh' },
    { record: false },
  )
})
await page.waitForTimeout(150)
check('缺口占位符初始为中文', (await page.locator('#text').inputValue()).includes('（约 3 秒未识别）'))

// 用户手改正文，占位符留着不动。
await page.locator('#text').fill('用户手改的内容 （约 3 秒未识别）')
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(150)
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
await page.waitForTimeout(250)
check('切英文占位符跟着变', (await page.locator('#text').inputValue()).includes('(3s not recognised)'))
check('切英文不覆盖用户编辑', (await page.locator('#text').inputValue()).includes('用户手改的内容'))

// 再切一次中文：第二次切换也不能覆盖编辑。
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: '简体中文' }).first().click()
await page.waitForTimeout(250)
check('连续切两次语言不覆盖用户编辑', (await page.locator('#text').inputValue()).includes('用户手改的内容'))
check('连续切两次占位符回到中文', (await page.locator('#text').inputValue()).includes('（约 3 秒未识别）'))

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
