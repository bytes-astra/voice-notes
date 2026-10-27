// 重新生成 README 用截图：9 张，@2x。
// 视口用「应用真实的默认窗口尺寸」（src-tauri/tauri.conf.json），README 里的图
// 就是用户启动后看到的样子。
// 运行：npm run shots
import { webkit } from 'playwright'
import { startServer, SRC, REPO } from './server.mjs'
import { join } from 'node:path'
import { mkdir } from 'node:fs/promises'
import { readFileSync } from 'node:fs'

const tauriConf = JSON.parse(readFileSync(join(REPO, 'src-tauri/tauri.conf.json'), 'utf8'))
const mainWindow = tauriConf.app.windows.find((w) => w.label === 'main')
const viewport = { width: mainWindow.width, height: mainWindow.height }

const OUT = join(REPO, 'docs/screenshots')
await mkdir(OUT, { recursive: true })
const { server, base } = await startServer(SRC)

const browser = await webkit.launch()

const BASE_STATUS = {
  modelReady: true,
  modelSource: 'app_data',
  modelDir: '/Users/hugh/Library/Application Support/com.github.bytes-astra.voicenotes/models',
  modelDirCustom: false,
  vadReady: true,
  outputDir: '/Users/hugh/Documents/VoiceNotes',
  language: 'auto',
  mode: 'offline',
  itn: true,
  downloading: false,
  downloadingStreaming: false,
  installedBytes: 241_357_257,
  totalBytes: 241_357_257,
  streamingReady: true,
  streamingDir:
    '/Users/hugh/Library/Application Support/com.github.bytes-astra.voicenotes/models/streaming',
  streamingBytes: 167_217_442,
  streamingTotalBytes: 461_589_961,
  punct: true,
  punctReady: true,
  hotwords: [],
  inputDevice: null,
  punctBytes: 294_372_519,
  punctTotalBytes: 294_372_519,
  appVersion: '1.0.0',
}

const HISTORY = [
  {
    id: 'h1',
    text: '今天的评审会主要讨论了三个问题：第一是接口文档的版本约定，第二是回归测试的覆盖范围，第三是灰度发布之后监控指标的阈值。',
    at: Date.now() - 1000 * 60 * 42,
    durationSeconds: 38.4,
    elapsedMs: 38_400,
    language: 'zh',
  },
  {
    id: 'h2',
    text: '记得明天下班前把取件码和公积金的发票拍给我，顺便把物业费也一起交掉。',
    at: Date.now() - 1000 * 60 * 60 * 5,
    durationSeconds: 12.8,
    elapsedMs: 12_800,
    language: 'zh',
  },
  {
    id: 'h3',
    text: '好的没问题，我这边确认一下再回复你。',
    at: Date.now() - 1000 * 60 * 60 * 26,
    durationSeconds: 5.1,
    elapsedMs: 5_100,
    language: 'zh',
  },
]

const RESULT = {
  text: '这次流式识别的结果就是这样，中间结果会单独显示一行，定稿之后会补上标点。',
  segments: [],
  durationSeconds: 18.6,
  elapsedMs: 18_600,
  language: 'zh',
}

/// macOS 上侧栏是透明的：原生材质（NSVisualEffectView）在 webview 后面。
/// 浏览器里没有那一层材质，透明侧栏会直接透出白色画布 —— 深色主题下就成了
/// 「白侧栏 + 深色主区」，连品牌字都看不见。给侧栏垫一层不透明底，
/// 还原原生材质的观感（深色比主区亮一档，浅色接近白）。
const SIDEBAR_MATERIAL_FALLBACK = 'body.macos .sidebar { background: var(--panel-2); }'

async function makePage({ status = {}, locale = 'zh-CN', theme = null, history = null } = {}) {
  const context = await browser.newContext({
    viewport,
    deviceScaleFactor: 2,
    locale,
    colorScheme: theme === 'dark' ? 'dark' : 'light',
  })
  await context.addInitScript(
    ({ base: baseStatus, historyEntries }) => {
      window.__mockStatus = { ...baseStatus }
      window.__handlers = {}
      window.__mockSessionId = null
      window.__mockSessionSeq = 0
      window.__emit = (name, payload) => {
        for (const cb of window.__handlers[name] || []) cb({ payload })
      }
      window.__TAURI__ = {
        core: {
          invoke: async (cmd, args) => {
            switch (cmd) {
              case 'status':
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'set_language':
                window.__mockStatus.language = args.language
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'set_mode':
                window.__mockStatus.mode = args.mode
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'set_itn':
                window.__mockStatus.itn = args.enabled
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'set_punct':
                window.__mockStatus.punct = args.enabled
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'set_hotwords':
                window.__mockStatus.hotwords = args.hotwords
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'default_hotwords':
                return ['帮我拿一下', '好的没问题', '取件码', '公积金', '老张']
              case 'set_input_device':
                window.__mockStatus.inputDevice = args.name
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'list_input_devices':
                return [{ name: 'MacBook Pro 麦克风', isDefault: true }]
              case 'warm_up':
              case 'warm_up_mic':
              case 'warm_up_streaming':
                return undefined
              // 会话 ID：前端用 reserve_session 拿到它，之后的流式事件必须带同一个
              // sessionId，不然会被前端当成旧会话丢掉（结果区一片空白）。
              case 'reserve_session':
                window.__mockSessionId = `sess-${++window.__mockSessionSeq}`
                return window.__mockSessionId
              case 'start_offline_recording':
              case 'start_streaming_recording':
              case 'start_recording':
                return JSON.parse(JSON.stringify(window.__mockStatus))
              case 'stop_recording':
                return JSON.parse(JSON.stringify(RESULT))
              case 'cancel_recording':
                return undefined
              case 'save_text':
                return { path: '/out/x.txt', name: 'x.txt' }
              default:
                return JSON.parse(JSON.stringify(window.__mockStatus))
            }
          },
        },
        event: {
          listen: async (name, cb) => {
            ;(window.__handlers[name] ||= []).push(cb)
            return () => {}
          },
        },
        dialog: { open: async () => null },
        clipboardManager: { writeText: async () => {} },
        window: { getCurrentWindow: () => ({ setBackgroundColor() {}, startDragging() {} }) },
        webview: { getCurrentWebview: () => ({ onDragDropEvent() {} }) },
      }
      if (historyEntries) {
        localStorage.setItem('voicenotes.history.v1', JSON.stringify(historyEntries))
      }
    },
    { base: { ...BASE_STATUS, ...status }, historyEntries: history },
  )
  const page = await context.newPage()
  await page.goto(`${base}/index.html`)
  await page.addStyleTag({ content: SIDEBAR_MATERIAL_FALLBACK })
  await page.waitForSelector('#lang-row .chip')
  await page.waitForTimeout(1400)
  return { context, page }
}

async function shot(page, name) {
  await page.screenshot({ path: `${OUT}/${name}.png` })
  console.log('写出', `${OUT}/${name}.png`)
}

// 1. 主界面（就绪）
{
  const { context, page } = await makePage()
  await shot(page, 'ready')
  await context.close()
}

// 2. 录音中（带波形）
{
  const { context, page } = await makePage()
  await page.locator('#mic-btn').click()
  await page.waitForTimeout(400)
  for (let i = 0; i < 40; i++) {
    const level = 0.15 + Math.random() * 0.75
    await page.evaluate((lv) => window.__emit('audio:level', { level: lv }), level)
    await page.waitForTimeout(25)
  }
  await shot(page, 'recording')
  await context.close()
}

// 3. 历史记录
{
  const { context, page } = await makePage({ history: HISTORY })
  await page.locator('.nav-item[data-view="history"]').click()
  await page.waitForTimeout(300)
  await shot(page, 'history')
  await context.close()
}

// 4. 设置页
{
  const { context, page } = await makePage({ history: HISTORY })
  await page.locator('.nav-item[data-view="settings"]').click()
  await page.waitForTimeout(300)
  await shot(page, 'settings')
  await context.close()
}

// 5. 流式模式（带结果）
{
  const { context, page } = await makePage()
  await page.locator('.mode-chip[data-mode="streaming"]').click()
  await page.waitForTimeout(400)
  await page.locator('#mic-btn').click()
  await page.waitForTimeout(600)
  // 流式事件必须带当前会话的 sessionId：前端只认当前会话，缺了就当成旧会话丢掉。
  await page.evaluate(() =>
    window.__emit('stream:segment', {
      segment: '今天的评审会主要讨论了三个问题。',
      sessionId: window.__mockSessionId,
    }),
  )
  await page.waitForTimeout(200)
  await page.evaluate(() =>
    window.__emit('stream:partial', {
      partial: '接口文档的版本约定，还有回归测试的覆盖范围',
      sessionId: window.__mockSessionId,
    }),
  )
  await page.waitForTimeout(200)
  await shot(page, 'streaming')
  await context.close()
}

// 6. 流式模型未下载
{
  const { context, page } = await makePage({ status: { streamingReady: false, streamingBytes: 0 } })
  await page.locator('.mode-chip[data-mode="streaming"]').click()
  await page.waitForTimeout(500)
  // 等提示条自己退掉：这张图要看的是「下载流式模型」卡片，不是压在上面的 toast。
  await page.waitForSelector('#toast.hidden', { timeout: 6000 }).catch(() => {})
  await shot(page, 'stream-setup')
  await context.close()
}

// 7. 首次运行（模型未下载）
{
  const { context, page } = await makePage({
    status: { modelReady: false, vadReady: false, installedBytes: 0 },
  })
  await shot(page, 'first-run')
  await context.close()
}

// 8. 深色主题（用应用自己的切换按钮，WebKit 里 prefers-color-scheme 不可靠）
{
  const { context, page } = await makePage()
  await page.locator('#theme-btn').click()
  await page.waitForTimeout(500)
  const theme = await page.evaluate(() => document.documentElement.dataset.theme)
  if (theme !== 'dark') throw new Error(`深色主题没生效（theme=${theme}）`)
  await shot(page, 'dark')
  await context.close()
}

// 9. 英文界面
{
  const { context, page } = await makePage({ locale: 'en-US' })
  await page.locator('.nav-item[data-view="settings"]').click()
  await page.waitForTimeout(200)
  await page.locator('#locale-select .select-button').click()
  await page.waitForTimeout(250)
  await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
  await page.waitForTimeout(500)
  await page.locator('.nav-item[data-view="transcribe"]').click()
  await page.waitForTimeout(400)
  await shot(page, 'english')
  await context.close()
}

await browser.close()
server.close()
