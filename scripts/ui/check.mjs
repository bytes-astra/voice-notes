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
const collectErrors = (target) => {
  target.on('console', (msg) => {
    if (msg.type() === 'error') errors.push(`console: ${msg.text()}`)
  })
  target.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`))
  target.on('requestfailed', (request) => errors.push(`requestfailed: ${request.url()} ${request.failure()?.errorText}`))
}
collectErrors(page)

/// 假后端：主页面和「Windows 场景」页面装同一套。
const installMock = () => {
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
    // 术语表里有几条含模型不认识的字（真后端：Status.hotwordsIgnored）。
    // 少了它，界面检查就测不到「其中 N 个不会生效」这条提示。
    hotwordsIgnored: 0,
    // 云端 provider 在**协议层**丢掉的术语条数（真后端：Status.cloudHotwordsIgnored）。
    cloudHotwordsIgnored: 0,
    inputDevice: null,
    punctBytes: 294_372_519,
    punctTotalBytes: 294_372_519,
    appVersion: '0.1.1',
    // 云端：只回传布尔值与地址/模型名，凭据明文不回来（真后端也是这个结构）。
    cloudConfigured: false,
    cloudBaseUrl: '',
    cloudProvider: 'realtime',
    cloudSupportsKeywords: true,
    cloudResourceId: '',
    cloudModel: '',
    // 连接列表 + 正在使用的那条（凭据只回传掩码，明文不回来）。
    cloudProfiles: [],
    cloudActiveProfile: '',
  }
  window.__mockCalls = []
  window.__mockTranscript = ''
  window.__mockStopError = null
  // 保存术语表后，后端会回报「有几条含模型不认识的字」（Status.hotwordsIgnored）。
  window.__mockHotwordsIgnored = 0
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
  // 造一条连接（界面检查用）：凭据只留掩码，跟真后端一个口径。
// 真后端的语义：apiKey 为 undefined = **不改**（老掩码留着），'' = 清空。
// 不能拿 `||` 伪造一把默认 key —— 伪造的 key 会把「换 provider 不沿用凭据」
// 这条缺陷整个挡在界面断言之外（RC-K25）。
function mockKeyMask(key) {
  return key.length > 8 ? `${key.slice(0, 4)}…${key.slice(-4)}` : `${key.slice(0, 2)}…`
}

function mockProfile(args, index, previous) {
  let keyMask = previous?.keyMask || ''
  if (typeof args.apiKey === 'string') {
    keyMask = args.apiKey ? mockKeyMask(args.apiKey) : ''
  }
  return {
    id: previous?.id || args.id || `p-${index}`,
    label: args.label || '测试连接',
    provider: args.provider || 'realtime',
    baseUrl: args.baseUrl || '',
    model: args.model || '',
    keyMask,
  }
}

// 新增或改一条连接。
function upsertMockProfile(profiles, args) {
  const list = [...(profiles || [])]
  const found = list.findIndex((profile) => profile.id === args.id)
  if (found >= 0) list[found] = mockProfile(args, found + 1, list[found])
  else list.push(mockProfile(args, list.length + 1))
  return list
}

// 活动项变了 → 把「使用中」那条的状态同步到 Status 的顶层字段
// （真后端也是从活动连接取 provider / 地址 / 模型的）。
function syncMockActiveProfile() {
  const active = (window.__mockStatus.cloudProfiles || []).find(
    (profile) => profile.id === window.__mockStatus.cloudActiveProfile,
  )
  window.__mockStatus = {
    ...window.__mockStatus,
    cloudProvider: active?.provider ?? '',
    cloudBaseUrl: active?.baseUrl ?? '',
    cloudModel: active?.model ?? '',
    cloudResourceId: active?.provider === 'volcengine' ? active?.model ?? '' : '',
    cloudConfigured: !!active?.keyMask,
  }
}

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
            window.__mockStatus = {
              ...window.__mockStatus,
              hotwords: args.hotwords,
              hotwordsIgnored: window.__mockHotwordsIgnored,
              cloudHotwordsIgnored: window.__mockCloudHotwordsIgnored,
            }
            return clone(window.__mockStatus)
          case 'save_cloud_profile': {
            // 云端配置现在是**连接列表**：每条一整套配置。凭据只回传掩码
            // （明文不留 —— 跟真后端一个口径）。
            const profiles = upsertMockProfile(window.__mockStatus.cloudProfiles, args)
            const added = profiles.find((profile) => profile.id === args.id) || profiles[profiles.length - 1]
            window.__mockStatus = {
              ...window.__mockStatus,
              cloudProfiles: profiles,
              // 新增的那条自动成为「使用中」。
              cloudActiveProfile: window.__mockStatus.cloudActiveProfile || added?.id || '',
            }
            syncMockActiveProfile()
            return clone(window.__mockStatus)
          }
          case 'delete_cloud_profile': {
            // 与真后端同语义：删掉「使用中」那条 → 落到**下一条**
            // （删的是最后一条则落到前一条），不是留在已删的 id 上。
            const profiles = window.__mockStatus.cloudProfiles || []
            const index = profiles.findIndex((profile) => profile.id === args.id)
            const wasActive = window.__mockStatus.cloudActiveProfile === args.id
            const next = profiles.filter((profile) => profile.id !== args.id)
            let active = window.__mockStatus.cloudActiveProfile
            if (wasActive) active = next[index]?.id || next[index - 1]?.id || ''
            window.__mockStatus = {
              ...window.__mockStatus,
              cloudProfiles: next,
              cloudActiveProfile: active,
            }
            syncMockActiveProfile()
            return clone(window.__mockStatus)
          }
          case 'clear_cloud_profile_key':
            // 只清凭据：地址 / 模型 / 备注一个字都不动（跟真后端一个口径）。
            window.__mockStatus = {
              ...window.__mockStatus,
              cloudProfiles: (window.__mockStatus.cloudProfiles || []).map((profile) =>
                profile.id === args.id ? { ...profile, keyMask: '' } : profile,
              ),
            }
            syncMockActiveProfile()
            return clone(window.__mockStatus)
          case 'set_active_cloud_profile': {
            // 与真后端同语义：不存在的 id 静默忽略，不能把活动项指到幽灵 id。
            const exists = (window.__mockStatus.cloudProfiles || []).some(
              (profile) => profile.id === args.id,
            )
            if (exists) {
              window.__mockStatus = { ...window.__mockStatus, cloudActiveProfile: args.id }
            }
            syncMockActiveProfile()
            return clone(window.__mockStatus)
          }
          case 'default_hotwords':
            return ['产品评审会', '玄戒芯片', '张三']
          case 'reserve_session':
            if (window.__mockReserveError) throw window.__mockReserveError
            return ++window.__mockSessionId
          case 'transcribe_file':
            // 挂起 / 失败：用来测「转写进行中」和「过期任务退出」这两条路径
            if (window.__mockFileGate) await window.__mockFileGate
            if (window.__mockFileError) throw window.__mockFileError
            return { ...clone(result), text: window.__mockFileText ?? '文件结果' }
          case 'cancel_recording':
            if (window.__mockCancelGate) await window.__mockCancelGate
            return undefined
          case 'start_offline_recording':
          case 'start_streaming_recording':
          case 'start_cloud_recording':
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
            return clone(window.__mockStatus)
          case 'download_model':
          case 'download_streaming_model':
            // 默认挂起：模拟「正在下载」，busy 状态能保持住。
            // __mockDownloadOk 打开时正常返回，用来测「下载完成后预热失败」。
            if (!window.__mockDownloadOk) {
              // 挂起时把拒收句柄留着：取消下载要能把它以 setup.cancelled 拒掉
              //（真后端 models.rs 取消后返回的就是这个字典 key）。
              return new Promise((resolve, reject) => { window.__rejectDownload = reject })
            }
            return clone(window.__mockStatus)
          case 'cancel_download':
            // 真后端：取消会让在飞的下载以字典 key setup.cancelled 失败。
            if (window.__rejectDownload) {
              const reject = window.__rejectDownload
              window.__rejectDownload = null
              reject('setup.cancelled')
            }
            return undefined
          case 'warm_up_streaming':
            if (window.__mockWarmError) throw window.__mockWarmError
            return null
          case 'warm_up':
            if (window.__mockWarmUpError) throw window.__mockWarmUpError
            return null
          case 'list_input_devices':
            // 列设备会触发 macOS 的麦克风权限弹框 —— 记下这一刻界面画完了没有。
            window.__mockDevicesListedAfterRender = document.querySelectorAll('#lang-row .chip').length > 0
            return [{ name: 'Mock Mic', isDefault: true }]
          case 'stop_recording':
            if (window.__mockStopGate) await window.__mockStopGate
            if (window.__mockStopError) throw window.__mockStopError
            // 停止瞬间迟到的句子：前端这时还处在「转写中」，会把它追加到结果框，
            // 但最终转写以**后端返回的全文**为准 —— 迟到的这句不该混进去。
            window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '迟到的一句', start: 30, end: 31 })
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
    dialog: {
      open: async () => null,
      // 隐私确认框：记下参数（断言提示确实出现过），返回值由 __mockConfirmAnswer 控制。
      confirm: async (message, options) => {
        window.__mockConfirmCalls = window.__mockConfirmCalls || []
        window.__mockConfirmCalls.push({ message, options })
        return window.__mockConfirmAnswer !== false
      },
    },
    clipboardManager: { writeText: async () => {} },
    // 窗口 API 要跟真后端对得上：少了 setTheme / setTitle，
    // 窗口外观（NSWindow.appearance）和窗口标题在 harness 里就静默变成 no-op ——
    // 真机上材质明暗不跟主题、标题不跟界面语言，这两条断言会恒真。
    window: {
      getCurrentWindow: () => ({
        setBackgroundColor() {},
        startDragging() {},
        setTheme(theme) {
          window.__mockWindowTheme = theme
          return Promise.resolve()
        },
        setTitle(title) {
          window.__mockWindowTitle = title
          return Promise.resolve()
        },
        // 窗口当前的主题（跟随系统时用它当初值）。
        theme() {
          return Promise.resolve(window.__mockSystemTheme || 'Light')
        },
        // 窗口主题变化事件：模拟系统切深浅色。
        onThemeChanged(callback) {
          window.__mockThemeChanged = callback
          return Promise.resolve(() => {})
        },
      }),
    },
    webview: { getCurrentWebview: () => ({ onDragDropEvent() {} }) },
  }
}
await context.addInitScript(installMock)

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
// 麦克风权限不能一启动就申请：点麦克风时才申请（warm_up_mic）。
// 列设备（会触发 macOS 的权限弹框）也要等界面画完 —— 否则弹框时窗口是空白的。
check(
  '启动时不申请麦克风权限',
  (await page.evaluate(() => window.__mockCalls.filter(([cmd]) => cmd === 'warm_up_mic').length)) === 0,
  await page.evaluate(
    () => `warm_up_mic 调了 ${window.__mockCalls.filter(([cmd]) => cmd === 'warm_up_mic').length} 次`,
  ),
)
check(
  '列设备时界面已经画完',
  (await page.evaluate(() => window.__mockDevicesListedAfterRender)) === true,
  await page.evaluate(() => String(window.__mockDevicesListedAfterRender)),
)
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
// 旧 toast 的隐藏定时器不能把后来的新提示藏掉：真等 2.4s 让旧定时器到期。
await page.evaluate(async () => {
  const { toast } = await import('./core.js')
  toast('旧提示')
})
await page.waitForTimeout(2450)
await page.evaluate(async () => {
  const { toast } = await import('./core.js')
  toast('新提示')
})
await page.waitForTimeout(300)
check('旧 toast 隐藏定时器不隐藏新提示', !(await page.evaluate(() => document.getElementById('toast').classList.contains('hidden'))))
await page.evaluate(() => { document.getElementById('toast').classList.add('hidden') })
await page.keyboard.press('Escape')
await page.waitForTimeout(150)
check('下拉 Esc 关闭弹层', (await page.locator('.select-popup').count()) === 0)
check('下拉 Esc 不取消录音', (await phase()) === 'recording')

// 选中后焦点必须回到按钮：掉到 body 的话，紧接着按空格会触发全局的「开始录音」。
await page.locator('#device-select .select-button').click()
await page.waitForTimeout(150)
await page.locator('.select-popup .select-item').first().click()
await page.waitForTimeout(120)
check(
  '选中后焦点回到下拉按钮',
  await page.evaluate(() => document.activeElement?.closest('#device-select') != null),
)
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

// ── RC-A10：历史写不进去（配额满 / 无痕模式）不能静默 ──
// 用户以为存上了，下次打开发现历史没了，没有任何痕迹 —— 只能靠日志查。
// 守的是「失败要留痕」：日志里有 ERROR，界面上的提示是另一回事。
const historyErrorLogged = await page.evaluate(async () => {
  const real = Storage.prototype.setItem
  Storage.prototype.setItem = function (key, value) {
    if (String(key).includes('history')) throw new DOMException('quota', 'QuotaExceededError')
    return real.call(this, key, value)
  }
  const before = window.__mockCalls.length
  try {
    // 触发一次历史写入：删掉现有历史会重新写一份。
    document.querySelector('.history-item .history-actions button:nth-child(3)')?.click()
    await new Promise((resolve) => setTimeout(resolve, 200))
  } finally {
    Storage.prototype.setItem = real
  }
  return window.__mockCalls
    .slice(before)
    .filter(([cmd, args]) => cmd === 'client_log' && /ERROR/.test(JSON.stringify(args)))
    .length
})
check('历史写入失败会记日志', historyErrorLogged > 0, `client_log ERROR 条数=${historyErrorLogged}`)

await page.screenshot({ path: `${SHOTS}/02-zh-result.png` })

// 4. 设置页切英文：结果不被覆盖、历史不重复
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(150)
check('设置页模型路径正常', !(await text('#settings-model')).includes('null'))
// 主题偏好：一枚按钮循环三态（跟随系统 → 浅色 → 深色 → 跟随系统）。
// 默认「跟随系统」。
check('主题按钮存在', (await page.locator('#theme-btn').count()) === 1)
check(
  '主题默认跟随系统',
  (await page.evaluate(() => localStorage.getItem('voicenotes.theme'))) === null,
  await page.evaluate(() => localStorage.getItem('voicenotes.theme')),
)
// **关键**：跟随系统时给窗口外观传的是 **null**（不钉死）。
// 钉死（'Light'/'Dark'）会把整个 App 的 macOS 外观写死，系统再切深浅色
// 就传不进来 —— 「跟随系统」会变成「跟随启动时那档」。
check(
  '跟随系统时不钉死窗口外观（setTheme 收到 null）',
  (await page.evaluate(() => window.__mockWindowTheme)) === null,
  JSON.stringify(await page.evaluate(() => window.__mockWindowTheme)),
)
check(
  '跟随系统时按钮提示是「跟随系统」',
  (await page.locator('#theme-btn').getAttribute('aria-label')) === '跟随系统',
  await page.locator('#theme-btn').getAttribute('aria-label'),
)
// 点一下 → 浅色；再点 → 深色；再点 → 回到跟随系统。
await page.locator('#theme-btn').click()
await page.waitForTimeout(200)
check(
  '第一次点击切到浅色',
  (await page.evaluate(() => localStorage.getItem('voicenotes.theme'))) === 'light' &&
    (await page.evaluate(() => document.documentElement.dataset.theme)) === 'light' &&
    (await page.locator('#theme-btn').getAttribute('aria-label')) === '浅色',
  await page.evaluate(() => `${localStorage.getItem('voicenotes.theme')} / ${document.documentElement.dataset.theme}`),
)
check(
  '显式浅色时钉死窗口外观',
  (await page.evaluate(() => window.__mockWindowTheme)) === 'Light',
  JSON.stringify(await page.evaluate(() => window.__mockWindowTheme)),
)
await page.locator('#theme-btn').click()
await page.waitForTimeout(200)
check(
  '第二次点击切到深色',
  (await page.evaluate(() => localStorage.getItem('voicenotes.theme'))) === 'dark' &&
    (await page.evaluate(() => document.documentElement.dataset.theme)) === 'dark' &&
    (await page.locator('#theme-btn').getAttribute('aria-label')) === '深色',
  await page.evaluate(() => `${localStorage.getItem('voicenotes.theme')} / ${document.documentElement.dataset.theme}`),
)
await page.locator('#theme-btn').click()
await page.waitForTimeout(200)
check(
  '第三次点击回到跟随系统',
  (await page.evaluate(() => localStorage.getItem('voicenotes.theme'))) === 'system',
  await page.evaluate(() => localStorage.getItem('voicenotes.theme')),
)
check(
  '回到跟随系统后不再钉死窗口外观',
  (await page.evaluate(() => window.__mockWindowTheme)) === null,
  JSON.stringify(await page.evaluate(() => window.__mockWindowTheme)),
)
// 跟随系统时，系统切深浅色 → 界面跟着变（靠窗口的主题变化事件）。
await page.evaluate(() => {
  window.__mockSystemTheme = 'Dark'
  window.__mockThemeChanged?.({ payload: 'Dark' })
})
await page.waitForTimeout(200)
check(
  '跟随系统时系统切深色 → 界面跟着变深色',
  (await page.evaluate(() => document.documentElement.dataset.theme)) === 'dark',
  await page.evaluate(() => document.documentElement.dataset.theme),
)
await page.evaluate(() => {
  window.__mockSystemTheme = 'Light'
  window.__mockThemeChanged?.({ payload: 'Light' })
})
await page.waitForTimeout(200)
check(
  '跟随系统时系统切浅色 → 界面跟着变浅色',
  (await page.evaluate(() => document.documentElement.dataset.theme)) === 'light',
  await page.evaluate(() => document.documentElement.dataset.theme),
)
// 术语表：填入示例 → 保存 → 回填
check('术语表输入框存在', (await page.locator('#hotwords-input').count()) === 1)
await page.locator('#hotwords-example').click()
await page.waitForTimeout(150)
check('填入示例', (await page.locator('#hotwords-input').inputValue()).includes('产品评审会'))
// 后端会回报「有几条含模型不认识的字」（Status.hotwordsIgnored）：
// 流式模型是单字词表，表外的字会让整条术语作废 —— 界面必须告诉用户有几条不生效。
await page.evaluate(() => { window.__mockHotwordsIgnored = 2 })
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
const hotwordsToast = await text('#toast')
check(
  '术语表提示词表外字有几条',
  hotwordsToast.includes('2') && hotwordsToast.includes('词表之外') && !hotwordsToast.includes('{ignored}'),
  hotwordsToast,
)
await page.evaluate(() => { window.__mockHotwordsIgnored = 0 })

// 云端 provider 的**协议层**丢弃（Status.cloudHotwordsIgnored）：各家限制不同
//（阿里的 15 字规范、火山的 100 条上限）—— 用户在云端模式下必须知道哪些术语
// 没送出去，不能只说「已保存」。
await page.evaluate(() => { window.__mockCloudHotwordsIgnored = 1 })
await page.locator('#hotwords-save').click()
await page.waitForTimeout(250)
const cloudHotwordsToast = await text('#toast')
check(
  '术语表提示云端协议层丢弃几条',
  cloudHotwordsToast.includes('1') &&
    cloudHotwordsToast.includes('云端') &&
    !cloudHotwordsToast.includes('{ignored}'),
  cloudHotwordsToast,
)
await page.evaluate(() => { window.__mockCloudHotwordsIgnored = 0 })

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
await page.waitForTimeout(150)
// 界面语言（locale）只有「跟随系统 + 两种」，识别语言是上面那排 6 个芯片 ——
// 两者不是一回事，界面语言的下拉里不该出现识别语言的选项。
const localeItemTexts = await page.locator('.select-popup .select-item').allInnerTexts()
check('界面语言下拉只有三个选项', localeItemTexts.length === 3, localeItemTexts.join(' | '))
check(
  '界面语言下拉不混进识别语言',
  !localeItemTexts.some((label) => ['粤语', '日本語', '한국어', '中文'].includes(label.trim())),
  localeItemTexts.join(' | '),
)
const recognitionLangBefore = await page.evaluate(
  () => document.querySelector('#lang-row .chip[aria-pressed="true"]')?.dataset.lang || '(没有选中的)',
)
await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
await page.waitForTimeout(200)
check('空结果切语言刷新提示', (await text('#meta')).includes('Waiting'))
// 切界面语言不能改识别语言：两者混了，用户说的话就用另一种语言去认了。
check(
  '切界面语言不改识别语言',
  (await page.evaluate(() => document.querySelector('#lang-row .chip[aria-pressed="true"]')?.dataset.lang || '(没有选中的)')) ===
    recognitionLangBefore,
  `切换前 ${recognitionLangBefore}`,
)
// 原生窗口标题跟着界面语言走（tauri.conf.json 里的只是启动默认值）。
check(
  '切语言同步原生窗口标题',
  (await page.evaluate(() => window.__mockWindowTitle)) === 'Voice to Text',
  await page.evaluate(() => window.__mockWindowTitle || '(没调用 setTitle)'),
)
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
check('英文设置页无横向溢出', await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth))
await page.screenshot({ path: `${SHOTS}/03-en-settings.png` })

// 英文的副标题比中文长（"uploaded to a cloud service"）：模式行放不下时会
// 撑出卡片 —— 中文看不出来，英文界面直接坏掉。断言它不许越过卡片内边距。
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.waitForTimeout(150)
check(
  '英文模式行不溢出卡片',
  await page.evaluate(() => {
    const row = document.querySelector('.mode-row')
    const card = row?.closest('.card')
    if (!row || !card) return false
    const rect = row.getBoundingClientRect()
    const box = card.getBoundingClientRect()
    const style = getComputedStyle(card)
    const padLeft = parseFloat(style.paddingLeft) || 0
    const padRight = parseFloat(style.paddingRight) || 0
    return rect.left >= box.left + padLeft - 1 && rect.right <= box.right - padRight + 1
  }),
  await page.evaluate(() => {
    const row = document.querySelector('.mode-row')
    const card = row?.closest('.card')
    if (!row || !card) return '(找不到模式行)'
    const rect = row.getBoundingClientRect()
    const box = card.getBoundingClientRect()
    return `模式行 ${Math.round(rect.left)}~${Math.round(rect.right)} / 卡片 ${Math.round(box.left)}~${Math.round(box.right)}`
  }),
)
check('英文转写页无横向溢出', await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth))
// 英文副标题改短后三档应该排在一行（换行说明还是太长）。
check(
  '英文模式行排在一行',
  await page.evaluate(() => {
    const row = document.querySelector('.mode-row')
    const chip = row?.querySelector('.mode-chip')
    if (!row || !chip) return false
    // 行内所有芯片的顶部对齐且只有一行：芯片数 == 第一行的芯片数。
    const tops = [...row.querySelectorAll('.mode-chip')].map((el) => Math.round(el.getBoundingClientRect().top))
    return new Set(tops).size === 1
  }),
  await page.evaluate(() => {
    const row = document.querySelector('.mode-row')
    if (!row) return '(找不到模式行)'
    const tops = [...row.querySelectorAll('.mode-chip')].map((el) => Math.round(el.getBoundingClientRect().top))
    return `芯片顶部：${tops.join(' / ')}`
  }),
)
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(100)

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
// 主题按钮循环三态：跟随系统 → 浅色 → 深色 → 跟随系统。
// 点到「深色」为止（前面的用例结束时停在「跟随系统」，这里最多点两次）。
const clickThemeUntil = async (mode) => {
  for (let i = 0; i < 3; i++) {
    if ((await page.evaluate(() => localStorage.getItem('voicenotes.theme'))) === mode) return
    await page.locator('#theme-btn').click()
    await page.waitForTimeout(200)
  }
}
await clickThemeUntil('dark')
check('深色主题生效', (await page.evaluate(() => document.documentElement.dataset.theme)) === 'dark')
check('深色无横向溢出', await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth))
// 主题按钮的图标表示**当前是哪一档**：浅色=太阳、深色=月亮、跟随系统=一半浅一半深的圆。
const themeIconKind = () =>
  page.locator('#theme-btn').evaluate((el) => {
    if (el.querySelector('path[d^="M20.5 14.3"]')) return 'moon'
    if (el.querySelector('circle[r="4"]')) return 'sun'
    if (el.querySelector('circle[r="9.6"]')) return 'auto'
    return 'unknown'
  })
check(
  '深色下主题按钮给月亮图标',
  (await themeIconKind()) === 'moon' &&
    (await page.locator('#theme-btn').getAttribute('aria-label')) === '深色',
  await themeIconKind(),
)
// 窗口外观（NSWindow.appearance）跟着主题走：macOS 的原生材质明暗跟的是它，
// 不同步就是「浅色材质配深色字」。
check(
  '切主题同步窗口外观',
  (await page.evaluate(() => window.__mockWindowTheme)) === 'Dark',
  await page.evaluate(() => window.__mockWindowTheme || '(没调用 setTheme)'),
)
await page.screenshot({ path: `${SHOTS}/04-dark.png` })
await clickThemeUntil('system')
check(
  '切回跟随系统：窗口外观交回系统（null）',
  (await page.evaluate(() => window.__mockWindowTheme)) === null,
  JSON.stringify(await page.evaluate(() => window.__mockWindowTheme)),
)
check(
  '跟随系统时按钮给一半浅一半深的圆',
  (await themeIconKind()) === 'auto' &&
    (await page.locator('#theme-btn').getAttribute('aria-label')) === '跟随系统',
  await themeIconKind(),
)

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

// ── 结果框里的实时句子按**本地时间轴**排序 ──
//
// 三条断言都在**非编辑态**下跑：用户一旦改过结果框，后面的段一律追加到末尾
// （粘滞标志），排序断言就测不到东西了。所以要放在下面「用户手改」那几行之前。
const emitSegment = async (text, start, end) => {
  await page.evaluate(([t, s, e]) => {
    window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: t, start: s, end: e })
  }, [text, start, end])
}

// ① 缺口段先到、它所属的句子后到（句子起点更早）：句子要排在缺口前面。
//    这正是云端丢包时的真实顺序（缺口锚在丢包起点，句子更早开始）。
await emitSegment('[[gap:7]]', 7, 7)
await page.waitForTimeout(60)
await emitSegment('迟到的一句', 3, 4)
await page.waitForTimeout(120)
const reordered = await page.locator('#text').inputValue()
const atLate = reordered.indexOf('迟到的一句')
const atGap7 = reordered.indexOf('约 7 秒未识别')
check(
  '缺口先到时句子仍排在缺口前面',
  atLate >= 0 && atGap7 >= 0 && atLate < atGap7,
  reordered.slice(-60)
)

// ② 同一 start：缺口在前（与后端 insert_gaps 的「同一时刻缺口在前」一致）。
//    句子和缺口都取零长区间 —— 这样「按 (start, end) 排序」的实现会退回按
//    到达顺序，只有显式的「缺口在前」规则才能通过。
await emitSegment('同刻的一句', 9, 9)
await page.waitForTimeout(60)
await emitSegment('[[gap:9]]', 9, 9)
await page.waitForTimeout(120)
const sameStart = await page.locator('#text').inputValue()
const atSame = sameStart.indexOf('同刻的一句')
const atGap9 = sameStart.indexOf('约 9 秒未识别')
check(
  '同一时刻缺口排在句子前面',
  atSame >= 0 && atGap9 >= 0 && atGap9 < atSame,
  sameStart.slice(-60)
)

// ②' 同一 start、缺口**先**到（新 max 处）：缺口仍然排在句子前面。
//    它抓的是「同 start 时把句子排到缺口前」的反向实现（把 tie-break 反过来
//    就会红）；「完全不排序」它抓不到 —— 那种实现恰好也是缺口在前。
await emitSegment('[[gap:22]]', 22, 22)
await page.waitForTimeout(60)
await emitSegment('同刻先缺口', 22, 22)
await page.waitForTimeout(120)
const gapFirst = await page.locator('#text').inputValue()
const atGap22 = gapFirst.indexOf('约 22 秒未识别')
const atSameFirst = gapFirst.indexOf('同刻先缺口')
check(
  '同一时刻缺口先到时也在句子前面',
  atSameFirst >= 0 && atGap22 >= 0 && atGap22 < atSameFirst,
  gapFirst.slice(-60)
)

// ②'' 普通句子乱序到达（与缺口无关）：起点早的排前面。
await emitSegment('乱序在后', 21, 22)
await page.waitForTimeout(60)
await emitSegment('乱序在前', 20, 21)
await page.waitForTimeout(120)
const swapped = await page.locator('#text').inputValue()
const atBefore = swapped.indexOf('乱序在前')
const atAfter = swapped.indexOf('乱序在后')
check(
  '乱序到达的句子按起点排',
  atBefore >= 0 && atAfter >= 0 && atBefore < atAfter,
  swapped.slice(-60)
)

// ③ 本地句子按时间递增到达：结果框的完整文本与改动前逐字相同。
const before = await page.locator('#text').inputValue()
// 起点要大于前面用过的最大起点（22），否则会走重排路径而不是追加路径。
await emitSegment('递增甲', 30, 31)
await page.waitForTimeout(60)
await emitSegment('递增乙', 31, 32)
await page.waitForTimeout(120)
const after = await page.locator('#text').inputValue()
check('时间递增到达时文本逐字不变', after === `${before}递增甲递增乙`, after.slice(-60))

// 录音中手改的内容，不能被下一句识别结果覆盖。
await page.locator('#text').fill('我手改的一行')
await page.evaluate(() => {
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '下一句', start: 0, end: 1 })
})
await page.waitForTimeout(120)
check('实时追加不覆盖用户编辑', (await page.locator('#text').inputValue()).startsWith('我手改的一行'))
check('实时追加接在用户编辑之后', (await page.locator('#text').inputValue()).includes('下一句'))

// ── RC-E8：实时追加不能夺走用户的光标，也不能清空撤销栈 ──
// 用户在中间改字，下一句识别结果一到，光标跳到末尾 = 接着敲的字跑错地方；
// 撤销栈被清空 = 改过的字再也撤不回来。
//
// 两条守卫对应两条不同的写路径：
//   · 整值写（el.value = …）→ 光标推到末尾、**撤销栈清空**；
//   · 增量写（execCommand insertText）→ 光标留在原处、**撤销栈保得住**。
//
// 光标那条**两条路径都满足**（整值写分支也还原了选区），单测它测不出改动；
// 撤销栈那条能区分 —— 且实测稳定：增量写后撤销回到插入前（4/4 一致），
// 整值写后撤销**无效**（3/3 一致）。
// （注意：**混合**场景「打字 + 追加」在 headless WebKit 里会被并成一组，
//   那才是之前判「不可靠」的原因 —— 纯追加场景没有这个问题。）
await page.locator('#text').fill('我手改的一行')
await page.evaluate(() => {
  const el = document.getElementById('text')
  el.focus()
  // 光标放在中间（第 3 个字后面）—— 用户正在这里改字。
  el.setSelectionRange(3, 3)
})
await page.evaluate(() => {
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '再来一句', start: 0, end: 1 })
})
await page.waitForTimeout(120)
const caretAfter = await page.evaluate(() => document.getElementById('text').selectionStart)
check(
  '实时追加后光标还在原处',
  caretAfter === 3,
  `光标位置=${caretAfter}（期望 3）`,
)
check(
  '实时追加后用户改的内容仍在',
  (await page.locator('#text').inputValue()).startsWith('我手改的一行'),
)
// 撤销栈：整值写会清空它 —— 撤销撤不掉刚追加的那句。
// （语义取「撤销后不再含刚追加的内容」：一次 undo 撤掉的是**一组**，
//   撤销栈里还留着更早的会话历史，撤掉的不一定只有这一句。）
const undoneAfterAppend = await page.evaluate(() => {
  const el = document.getElementById('text')
  el.focus()
  document.execCommand('undo')
  return el.value
})
check(
  '实时追加后仍能撤销掉刚追加的内容',
  !undoneAfterAppend.includes('再来一句'),
  `撤销后=${undoneAfterAppend.slice(-40)}`,
)

// 无焦点路径：`execCommand('insertText')` 要求元素有焦点，没焦点时它
// **返回 true 却什么也没做** —— 兜底必须退回整值写，文本照样要接上。
// （录音中用户没碰结果框时就是这条路径 —— 上一条断言是 focus 过的。）
await page.evaluate(() => document.getElementById('text').blur())
const unfocusedBefore = await page.locator('#text').inputValue()
await page.evaluate(() => {
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '无焦点追加', start: 0, end: 1 })
})
await page.waitForTimeout(120)
const unfocusedAfter = await page.locator('#text').inputValue()
check(
  '无焦点时实时追加照样接上',
  unfocusedAfter === `${unfocusedBefore}无焦点追加`,
  `结果=${unfocusedAfter.slice(-40)}`,
)

// ── 焦点在别处时，转写文本不能插进那个输入框 ──
// `execCommand('insertText')` 作用于**当前焦点所在的可编辑元素**，不是结果框。
// 实测：焦点在另一个 input 时，文本插进了那个 input，结果框纹丝不动。
// 文件转写时用户能切到设置页点进凭据框 —— 转写文本被插进去，
// 保存下来就成了凭据的一部分。
//
// 用**可见**的输入框：焦点在 display:none 的元素上时 execCommand 不生效，
// 那样测出来是假绿。
await page.evaluate(() => {
  const el = document.getElementById('text')
  el.focus()
  el.setSelectionRange(3, 3)
  let other = document.getElementById('focus-probe')
  if (!other) {
    other = document.createElement('input')
    other.id = 'focus-probe'
    other.style.cssText = 'position:fixed;top:0;left:0;width:200px;z-index:9999'
    document.body.appendChild(other)
  }
  other.value = '原凭据'
  other.focus()
  other.setSelectionRange(2, 2)
})
const otherBefore = await page.evaluate(() => document.getElementById('focus-probe')?.value ?? '')
await page.evaluate(() => {
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '串错了', start: 0, end: 1 })
})
await page.waitForTimeout(120)
const otherAfter = await page.evaluate(() => document.getElementById('focus-probe')?.value ?? '')
check(
  '焦点在别的输入框时转写文本不插进去',
  otherAfter === otherBefore,
  `输入框=${otherAfter.slice(0, 40)}`,
)
check(
  '焦点在别的输入框时结果框照样接到新句',
  (await page.locator('#text').inputValue()).includes('串错了'),
)
await page.evaluate(() => document.getElementById('focus-probe')?.remove())

// 编辑状态下收到缺口段：显示的必须是本地化文案，不能露出 [[gap:N]]。
await page.evaluate(() => {
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '[[gap:4]]', start: 0, end: 1 })
})
await page.waitForTimeout(120)
const liveGap = await page.locator('#text').inputValue()
check('编辑状态下缺口段显示本地化文案', !liveGap.includes('[[gap:') && liveGap.includes('秒未识别'), liveGap)

// 停止录音：后端返回全文，前端要用它替换结果框
await page.locator('#mic-btn').click()
await page.waitForTimeout(400)
const final = await page.locator('#text').inputValue()
check(
  '停止后结果框是后端返回的全文',
  ['第一句', '第二句', '第三句'].every((t) => final.includes(t)),
  final.slice(0, 60)
)
// 守的是「最终转写以后端返回的全文为准」这条契约（迟到的句子不混进去）。
// 注意这条比「段列表重建不会把它带进来」弱：迟到段发出时用户正处于编辑态，
// 它走的是编辑分支、根本没进段列表。
check('最终转写以后端返回的全文为准', !final.includes('迟到的一句'), final.slice(-60))
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
// 实时行 = 固定长度 + 内部滚动：中间结果长短变化不能把下面的模式行顶来顶去。
const emitPartial = (partial) =>
  page.evaluate((p) => {
    window.__emit('stream:partial', { sessionId: window.__mockSessionId, partial: p })
  }, partial)
const liveMetrics = () =>
  page.locator('#live-line').evaluate((el) => ({
    height: el.getBoundingClientRect().height,
    shown: !el.classList.contains('hidden'),
    empty: el.classList.contains('is-empty'),
    scrollWidth: el.scrollWidth,
    clientWidth: el.clientWidth,
    scrollLeft: el.scrollLeft,
  }))
// 录制一开始就保留占位（此时还没有任何中间结果）。
const liveStart = await liveMetrics()
check(
  '流式录制开始就保留实时行占位',
  liveStart.shown && liveStart.height > 0,
  JSON.stringify(liveStart),
)
await emitPartial('短句')
await page.waitForTimeout(80)
const liveShort = await liveMetrics()
await emitPartial('很长的一句实时中间结果'.repeat(40))
await page.waitForTimeout(80)
const liveLong = await liveMetrics()
check(
  '实时行高度固定，不被长句撑开',
  liveShort.height === liveLong.height && liveLong.height > 0,
  `短=${liveShort.height} 长=${liveLong.height}`,
)
check(
  '实时行横向滚动：超出的宽度往右滚',
  liveLong.scrollWidth > liveLong.clientWidth,
  JSON.stringify(liveLong),
)
check(
  '横向滚动跟到最新内容的末尾',
  liveLong.scrollLeft + liveLong.clientWidth >= liveLong.scrollWidth - 2,
  JSON.stringify(liveLong),
)
// 滚动条藏起来：横向滚动是自动跟随的，不需要一条可见的条。
check(
  '实时行不显示滚动条',
  await page.locator('#live-line').evaluate((el) => {
    const style = getComputedStyle(el)
    return style.scrollbarWidth === 'none' || style.scrollbarWidth === '0px'
  }),
  await page.locator('#live-line').evaluate((el) => getComputedStyle(el).scrollbarWidth),
)
// 手动往回拖了之后，下一段 partial 仍要把视口拽回最新内容
// —— 实时预览不该停在用户翻到的旧位置。
await page.locator('#live-line').evaluate((el) => {
  el.scrollLeft = 0
})
await emitPartial('又来了一句新的实时中间结果'.repeat(30))
await page.waitForTimeout(120)
const liveAfterManualScroll = await liveMetrics()
check(
  '手动翻走后仍自动跟到最新内容',
  liveAfterManualScroll.scrollLeft + liveAfterManualScroll.clientWidth >=
    liveAfterManualScroll.scrollWidth - 2,
  JSON.stringify(liveAfterManualScroll),
)
// 句末会发一条空 partial：不能把实时行收起来（否则下一段又把它顶下来）。
await emitPartial('')
await page.waitForTimeout(80)
const liveCleared = await liveMetrics()
check(
  '句末清空 partial 不收起实时行',
  liveCleared.shown && liveCleared.empty && liveCleared.height === liveLong.height,
  JSON.stringify(liveCleared),
)
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
// 非致命的缺口提示是「提示」不是「失败」：不该用红色错误样式。
await page.evaluate(() => window.__emit('audio:error', { code: 'record.gapWarning', seconds: 3, fatal: false }))
await page.waitForTimeout(200)
check(
  '缺口提示不是错误样式',
  await page.evaluate(() => !document.getElementById('toast')?.classList.contains('error')),
  await page.evaluate(() => document.getElementById('toast')?.className || '(没有 toast)'),
)
check('缺口提示照常显示', (await text('#toast')).includes('3'))
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
const configCallsBefore = await page.evaluate(() =>
  window.__mockCalls.filter(([cmd]) => cmd === 'set_language' || cmd === 'set_itn').length
)
await page.locator('#mic-btn').click()
await page.waitForTimeout(100)
// 启动中（requesting）也要锁住语言与 ITN：这一步后端还没开始收音频，
// 但用户已经点开了准备界面 —— 此时改配置会让前后端语言不一致。
check('启动中语言与 ITN 锁定', await page.evaluate(() =>
  document.querySelector('.app').dataset.phase === 'requesting' &&
  [...document.querySelectorAll('#lang-row .chip')].every((chip) => chip.disabled) &&
  document.getElementById('itn-toggle').disabled
))
check('启动中不发送语言或 ITN 变更', (await page.evaluate(() =>
  window.__mockCalls.filter(([cmd]) => cmd === 'set_language' || cmd === 'set_itn').length
)) === configCallsBefore)
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

// ── 停止超时 / 录音过短：错误串带细节时也要走字典文案 ──
// 后端把细节接在冒号后面（record.stopFailed:细节 / record.tooShort:0.2），
// 界面按第一个冒号切出错误码去查字典 —— 细节只填 {value} / {seconds}，
// 不能把错误码或细节原样甩给用户。
await page.evaluate(() => { window.__mockStopError = 'record.streamTimeout:通道关闭' })
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
await page.locator('#mic-btn').click()
await page.waitForTimeout(300)
const timeoutToast = await text('#toast')
check(
  '停止超时带细节仍走超时文案',
  timeoutToast.includes('超时') && !timeoutToast.includes('record.streamTimeout') && !timeoutToast.includes('通道关闭'),
  timeoutToast,
)

await page.evaluate(() => { window.__mockStopError = 'record.tooShort:0.2' })
await page.locator('#mic-btn').click()
await page.waitForTimeout(200)
await page.locator('#mic-btn').click()
await page.waitForTimeout(300)
const tooShortToast = await text('#toast')
check(
  '录音过短的提示带秒数',
  tooShortToast.includes('0.2') && tooShortToast.includes('秒') && !tooShortToast.includes('{seconds}'),
  tooShortToast,
)
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
  let release
  window.__mockFileGate = new Promise((resolve) => { release = resolve })
  const task = transcribeFilePath('/tmp/demo.mp3')
  await new Promise((resolve) => setTimeout(resolve, 50))
  window.__emit('stream:segment', { sessionId: window.__mockSessionId, segment: '文件句' })
  await new Promise((resolve) => setTimeout(resolve, 50))
  // 转写还在进行中：进度提示要按当前数据（句数、已用时）重绘
  window.__fileHint = document.getElementById('hint').textContent
  // renderHint 也要按当前数据重画 —— 阶段变化和切语言都会调它
  const { renderHint } = await import('./recorder.js')
  renderHint()
  window.__fileHintAfterRender = document.getElementById('hint').textContent
  release()
  await task
  window.__mockFileGate = null
})
await page.waitForTimeout(200)
check('文件进度按当前数据重绘', (await page.evaluate(() => window.__fileHint)).includes('已识别 1 句'))
check('renderHint 也按当前数据重绘进度', (await page.evaluate(() => window.__fileHintAfterRender)).includes('已识别 1 句'))
check('文件转写成功恢复空闲', (await phase()) === 'idle')
check('文件转写保存结果', (await page.locator('#text').inputValue()) === '文件结果')

// 文件没识别出东西时，结果区不能说「换个安静点的环境再试试」——
// 那是麦克风的建议；文件的内容已经定下来了。两条路径的文案要各归各。
await page.evaluate(async () => {
  window.__mockFileText = ''
  const { transcribeFilePath } = await import('./files.js')
  await transcribeFilePath('/tmp/empty.mp3')
})
await page.waitForTimeout(200)
check(
  '空文件结果不用麦克风的建议文案',
  await page.evaluate(() => {
    const meta = document.getElementById('meta')?.textContent || ''
    return meta.includes('没有识别到内容') && !meta.includes('换个安静点')
  }),
  await page.evaluate(() => document.getElementById('meta')?.textContent || '(没有 meta)'),
)
await page.evaluate(() => { window.__mockFileText = null })

// 过期任务退出时也要复位：不能把界面卡在「转写中」。
await page.evaluate(async () => {
  const { transcribeFilePath } = await import('./files.js')
  const { state } = await import('./core.js')
  let release
  window.__mockFileGate = new Promise((resolve) => { release = resolve })
  window.__mockFileError = 'record.stopFailed'
  const task = transcribeFilePath('/tmp/stale.mp3')
  await new Promise((resolve) => setTimeout(resolve, 50))
  state.fileAttemptId += 1 // 这次任务过期了（被后来的任务顶掉）
  release()
  await task
  window.__mockFileGate = null
  window.__mockFileError = null
  window.__mockFileText = null
  window.__staleState = {
    transcribingFile: state.transcribingFile,
    fileSessionId: state.fileSessionId,
    phase: state.phase,
  }
})
await page.waitForTimeout(150)
check('过期文件任务退出仍复位', await page.evaluate(() => {
  const s = window.__staleState
  return s.transcribingFile === false && s.fileSessionId === 0 && s.phase === 'idle'
}))

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

// 转写进行中载入历史：会覆盖正在写的结果，必须拒绝。
await page.evaluate(async () => {
  const { transcribeFilePath } = await import('./files.js')
  window.__mockFileGate = new Promise((resolve) => { window.__releaseHistLoad = resolve })
  window.__histTask = transcribeFilePath('/tmp/busy.mp3')
})
await page.waitForTimeout(100)
await page.locator('.nav-item[data-view="history"]').click()
await page.waitForTimeout(100)
await page.locator('.history-item .history-main').first().click()
await page.waitForTimeout(150)
check('转写中载入历史被拒绝', (await page.locator('.app').getAttribute('data-view')) === 'history')
await page.evaluate(async () => {
  window.__releaseHistLoad()
  await window.__histTask
  window.__mockFileGate = null
  window.__histTask = null
})
await page.waitForTimeout(150)
// 后续用例都在转写页：把视图还原。
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.waitForTimeout(150)

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

// 流式引擎报错：徽标底色必须跟整段一致（红底），不能一个红一个灰
//
// 必须点模式芯片切到流式：前端只在 setMode / refreshStatus 时读 status，
// 直接改 window.__mockStatus 不会让前端状态跟着变。
await page.locator('.mode-chip[data-mode="streaming"]').click()
await page.waitForTimeout(200)
await page.evaluate(() =>
  window.__emit('streaming:state', {
    phase: 'error',
    message: 'model.streamLoadFailed:/Users/someone/Library/Application Support/models/streaming/model.int8.onnx',
  }),
)
await page.waitForTimeout(200)
const streamingBadge = await page.locator('#model-badge').getAttribute('class')
check('流式引擎报错时徽标是红底', streamingBadge.includes('danger'), streamingBadge)
check(
  '流式引擎报错时状态点标红',
  (await page.locator('#engine-dot').getAttribute('data-state')) === 'error',
)
// 切回整段，后面的用例还按整段模式走
await page.locator('.mode-chip[data-mode="offline"]').click()
await page.waitForTimeout(200)

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

// ── 下载取消：界面必须说「已取消」，不能说失败，也不能露出内部哨兵 ──
// 真后端：取消下载会让在飞的下载以字典 key setup.cancelled 失败
//（models.rs 的 DOWNLOAD_CANCELLED 哨兵在命令层翻译过了）。
// 取消按钮默认是隐藏的（同下载按钮），直接在页面里点它的 click 事件。
await page.evaluate(() => document.getElementById('cancel-download-btn').click())
await page.waitForTimeout(300)
const cancelToast = await text('#toast')
check('下载取消显示「已取消」', cancelToast.includes('已取消'), cancelToast)
check(
  '下载取消不露出内部哨兵',
  !cancelToast.includes('setup.cancelled') && !cancelToast.includes('download.cancelled'),
  cancelToast,
)
const cancelProgress = await page.locator('#setup-progress-text').innerText()
check('下载取消的进度行不说失败', !cancelProgress.includes('失败'), cancelProgress)

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

// ── 历史里的缺口占位符要跟着界面语言走 ───────────────────
// 历史存的是**原文**，不是存档时那一种语言 —— 否则切语言后历史里
// 永远留着旧语言的占位符文案。
await page.evaluate(async () => {
  const { showTranscript } = await import('./result.js')
  showTranscript(
    { text: '历史里的缺口 [[gap:4]]', segments: [], durationSeconds: 2, elapsedMs: 100, language: 'zh' },
  )
})
await page.waitForTimeout(150)
await page.locator('.nav-item[data-view="history"]').click()
await page.waitForTimeout(150)
const historyText = () => page.locator('.history-item .history-text').first().innerText()
check('历史里的缺口占位符初始为中文', (await historyText()).includes('（约 4 秒未识别）'))
// 语言在下拉里，而下拉在设置页 —— 切完语言再回来看历史。
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(150)
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
await page.waitForTimeout(250)
await page.locator('.nav-item[data-view="history"]').click()
await page.waitForTimeout(150)
check('历史里的缺口占位符跟着语言变', (await historyText()).includes('(4s not recognised)'), await historyText())

// 历史页点条目：必须切回转写页，否则文字写进 display:none 的面板里，用户看不到。
await page.locator('.history-item .history-main').first().click()
await page.waitForTimeout(200)
check('历史载入切回转写页', (await page.locator('.app').getAttribute('data-view')) === 'transcribe')
check('历史载入内容可见', (await page.locator('#text').inputValue()).length > 0)

// 载入的历史要带上 markers：用户改过之后切语言，缺口占位符仍要换。
// 用 evaluate 改值：面板此时可能还是隐藏的，fill() 会等可见性超时。
await page.evaluate(() => {
  const box = document.getElementById('text')
  box.value = box.value + '（手改）'
  box.focus()
  box.setSelectionRange(3, 3)
})
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(150)
await page.locator('#locale-select .select-button').click()
await page.locator('.select-popup .select-item', { hasText: '简体中文' }).first().click()
await page.waitForTimeout(250)
check(
  '历史载入后切语言换掉缺口占位符',
  (await page.locator('#text').inputValue()).includes('（约 4 秒未识别）'),
)
check('历史载入后切语言保留用户编辑', (await page.locator('#text').inputValue()).includes('（手改）'))
// 切语言换占位符时文本长度变了（整值写会把光标推到末尾）——
// 光标要留在用户改字的那个位置，不能被推到末尾。
const caretAfterMarkerSwap = await page.evaluate(() => document.getElementById('text').selectionStart)
check(
  '切语言换占位符后光标没被推到末尾',
  caretAfterMarkerSwap === 3,
  `光标位置=${caretAfterMarkerSwap}（期望 3）`,
)

// 流式模型下载完、预热失败：不能弹「已就绪」。
await page.evaluate(() => {
  window.__mockDownloadOk = true
  window.__mockWarmError = 'model.streamLoadFailed'
})
await page.evaluate(async () => {
  const { downloadStreamingModel } = await import('./status.js')
  await downloadStreamingModel()
})
await page.waitForTimeout(200)
const warmToast = await page.locator('#toast').innerText()
check('预热失败不弹「已就绪」', !warmToast.includes('已就绪'), warmToast)
check('预热失败给出错误提示', warmToast.length > 0, warmToast)

// 整段模型同样：下载完预热失败，不能说「已就绪」。
await page.evaluate(() => {
  window.__mockDownloadOk = true
  window.__mockWarmUpError = 'model.loadFailed'
})
await page.evaluate(async () => {
  // 前面的「下载挂起」用例把 busy 标志留在 true，不复位会直接 return（空过）。
  const { state } = await import('./core.js')
  state.downloading = false
  const { downloadModel } = await import('./status.js')
  await downloadModel()
})
await page.waitForTimeout(200)
const offlineWarmToast = await page.locator('#toast').innerText()
check('整段预热失败不弹「已就绪」', !offlineWarmToast.includes('已就绪'), offlineWarmToast)
await page.evaluate(() => {
  window.__mockDownloadOk = false
  window.__mockWarmError = null
  window.__mockWarmUpError = null
})

// ── 云端模式：第三档芯片 + 隐私确认 + 配置表单 ──
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.waitForTimeout(200)
await page.evaluate(() => {
  localStorage.removeItem('voicenotes.cloudOptIn')
  window.__mockConfirmCalls = []
  window.__mockConfirmAnswer = false
})
check('模式芯片有三个', (await page.locator('#mode-row .mode-chip').count()) === 3)
await page.locator('#mode-row .mode-chip[data-mode="cloud"]').click()
await page.waitForTimeout(300)
const confirmCalls = await page.evaluate(() => window.__mockConfirmCalls || [])
check(
  '切云端前弹出上传提示',
  confirmCalls.length === 1 && confirmCalls[0].message.includes('上传'),
  confirmCalls[0]?.message || '(没有确认框)',
)
check(
  '拒绝确认就不切模式',
  (await page.evaluate(() => window.__mockStatus.mode)) === 'offline',
  await page.evaluate(() => window.__mockStatus.mode),
)
await page.evaluate(() => {
  window.__mockConfirmAnswer = true
})
await page.locator('#mode-row .mode-chip[data-mode="cloud"]').click()
await page.waitForTimeout(300)
check(
  '确认后切到云端模式',
  (await page.evaluate(() => window.__mockStatus.mode)) === 'cloud',
  await page.evaluate(() => window.__mockStatus.mode),
)

await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(200)
check(
  '云端连接列表存在',
  (await page.locator('#cloud-profiles').count()) === 1 &&
    (await page.locator('#cloud-add').count()) === 1,
)
// 编辑区默认收起 —— 列表没展开时不占地方。
check(
  '编辑区默认收起',
  !(await page.locator('#cloud-editor').isVisible()),
)
// 添��一条连接：编辑区展开 → 填 → 保存 → 列表里出现一行。
await page.locator('#cloud-add').click()
await page.waitForTimeout(150)
check('添加连接后编辑区展开', await page.locator('#cloud-editor').isVisible())
await page.locator('#cloud-label').fill('阿里·生产')
await page.locator('#cloud-base-url').fill('https://example.invalid/v1')
await page.locator('#cloud-api-key').fill('mock-key')
await page.locator('#cloud-model').fill('qwen-audio-3.1-asr-flash-streaming')
await page.locator('#cloud-save').click()
await page.waitForTimeout(300)
check(
  '保存后列表里有一条连接',
  (await page.locator('#cloud-profiles .cloud-profile').count()) === 1,
  `行数=${await page.locator('#cloud-profiles .cloud-profile').count()}`,
)
const profileRowText = await page.locator('#cloud-profiles .cloud-profile').first().innerText()
// 列表里只显示**掩码**，不显示明文。
check(
  '列表显示掩码、不显示明文',
  !profileRowText.includes('mock-key') && profileRowText.includes('…'),
  profileRowText,
)
check(
  '使用中的那条有徽章',
  profileRowText.includes('使用中'),
  profileRowText,
)
check('密钥不回显', (await page.locator('#cloud-api-key').inputValue()) === '')
check(
  '云端配置已保存',
  (await page.evaluate(() => window.__mockStatus.cloudConfigured)) === true,
)

// **第二家**：火山。添���一条 → 列表里两行，**第一条仍然是使用中**。
await page.locator('#cloud-add').click()
await page.waitForTimeout(150)
await page.locator('#cloud-label').fill('火山·小时版')
// 火山的「模型」字段是资源标识 —— 候选项按服务给，不让用户手打。
await page.locator('#cloud-provider-select .select-button').click()
await page.waitForTimeout(150)
await page.locator('.select-popup .select-item', { hasText: '火山' }).first().click()
await page.waitForTimeout(250)
// 密钥在**换完服务商之后**填：换 provider 会清掉输入框（换的是整套配置）。
await page.locator('#cloud-api-key').fill('volc-key')
const volcModelOptions = await page
  .locator('#cloud-model-options option')
  .allTextContents()
check(
  '火山的模型候选项是那两个资源标识',
  volcModelOptions.includes('volc.seadasr.sauc.duration') &&
    volcModelOptions.includes('volc.seadasr.sauc.concurrent'),
  volcModelOptions.join(','),
)
check(
  '火山那一列叫资源标识',
  (await page.locator('#cloud-model-label').innerText()).includes('资源标识'),
  await page.locator('#cloud-model-label').innerText(),
)
await page.locator('#cloud-model').fill('volc.seadasr.sauc.duration')
await page.locator('#cloud-save').click()
await page.waitForTimeout(300)
check(
  '两条连接都在列表里',
  (await page.locator('#cloud-profiles .cloud-profile').count()) === 2,
  `行数=${await page.locator('#cloud-profiles .cloud-profile').count()}`,
)

// **切 provider 不再清空别人的配置** —— 这是这次改动的核心：每家是独立的一条。
check(
  '切到火山没有清掉阿里那条',
  (await page.evaluate(() => window.__mockStatus.cloudProfiles.length)) === 2,
  await page.evaluate(() => JSON.stringify(window.__mockStatus.cloudProfiles.map((p) => p.provider))),
)

// 点另一条**整行** → 徽章移到那条（圆点只是指示，不再是按钮）。
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('.cloud-profile-mark').click()
await page.waitForTimeout(300)
const secondRowText = await page.locator('#cloud-profiles .cloud-profile').nth(1).innerText()
const firstRowText = await page.locator('#cloud-profiles .cloud-profile').nth(0).innerText()
check(
  '点整行能切换使用中的那条',
  secondRowText.includes('使用中') && !firstRowText.includes('使用中'),
  `第一行=${firstRowText} 第二行=${secondRowText}`,
)

// ── RC-E9：切换使用中的那条之后，焦点不能掉到 body ──
// 整表重建会把用户正在操作的那一行换掉 —— 焦点随之掉到 body，
// 键盘用户下一次 Tab 得从头开始。守的是「焦点还在那一行的按钮上」。
//
// 点**当前活动的那一行**（第 1 行）：活动项不变，后面的断言不受影响。
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('.cloud-profile-body').focus()
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('.cloud-profile-body').click()
await page.waitForTimeout(300)
const focusAfterActivate = await page.evaluate(() => {
  const active = document.activeElement
  return {
    tag: active ? active.tagName : '(没有)',
    cls: active ? active.className : '',
    id: active ? active.closest('.cloud-profile')?.dataset.profileId || '' : '',
  }
})
check(
  '切换使用中的那条后焦点仍在行内',
  focusAfterActivate.cls.includes('cloud-profile-body') && focusAfterActivate.id !== '',
  JSON.stringify(focusAfterActivate),
)

// 点**名称**（不可聚焦的 div）时焦点不能掉到 body ——
// mousedown 的默认行为会把焦点移到点击目标上，那目标不可聚焦 → 焦点丢。
// 上面那条断言是「先 focus 再 click」，走不到 mousedown 这条路径，所以要单独测。
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('.cloud-profile-body').focus()
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('.cloud-profile-name').click()
await page.waitForTimeout(200)
const focusAfterName = await page.evaluate(() => {
  const active = document.activeElement
  return {
    tag: active ? active.tagName : '(没有)',
    cls: active ? active.className : '',
    id: active ? active.closest('.cloud-profile')?.dataset.profileId || '' : '',
  }
})
check(
  '点连接名称后焦点仍在行内',
  focusAfterName.cls.includes('cloud-profile-body') && focusAfterName.id !== '',
  JSON.stringify(focusAfterName),
)
// 反过来：点**按钮**（编辑图标）不能因为拦 mousedown 而失灵 ——
// 编辑动作照常触发（编辑区要展开）。
//
// 注意「鼠标点按钮会不会拿到焦点」**因平台而异**：macOS 的 WebKit 默认
// 不给（Safari 行为，要开完全键盘访问），Windows 的 WebView2 给。
// 拦截只针对**不可聚焦的点击目标**（行内的名称 / 明细 div），
// 按钮自己不拦 —— 两边的行为因此一致。
await page.locator('#cloud-profiles .cloud-profile [data-action="edit"]').first().click()
await page.waitForTimeout(250)
check(
  '点编辑图标照常打开编辑区',
  await page.locator('#cloud-editor').isVisible(),
)

// ── RC-K22：provider 下拉绑定**正在编辑的那条连接** ──
// 编辑非活动连接时切界面语言，下拉不能被静默换成活动连接的服务商
// （`UISelect.setItems` 不触发 `onChange`，换掉用户看不见）。
await page.locator('#cloud-profiles .cloud-profile').nth(0).locator('[data-action="edit"]').click()
await page.waitForTimeout(200)
const providerBeforeLocale = await page
  .locator('#cloud-provider-select .select-button')
  .innerText()
check(
  '编辑非活动连接时下拉是它自己的服务商',
  providerBeforeLocale.includes('OpenAI Realtime'),
  providerBeforeLocale,
)
await page.locator('#locale-select .select-button').click()
await page.waitForTimeout(150)
await page.locator('.select-popup .select-item', { hasText: 'English' }).first().click()
await page.waitForTimeout(300)
const providerAfterLocale = await page
  .locator('#cloud-provider-select .select-button')
  .innerText()
check(
  '切语言后 provider 下拉不变',
  providerAfterLocale.includes('OpenAI Realtime'),
  providerAfterLocale,
)
await page.locator('#cloud-save').click()
await page.waitForTimeout(250)
check(
  '切语言后保存，被编辑那条的 provider 没变',
  (await page.evaluate(() => window.__mockStatus.cloudProfiles[0].provider)) === 'realtime',
  await page.evaluate(() => window.__mockStatus.cloudProfiles[0].provider),
)
// 切回中文：后面的断言按中文写。
await page.locator('#locale-select .select-button').click()
await page.waitForTimeout(150)
await page.locator('.select-popup .select-item', { hasText: '简体中文' }).first().click()
await page.waitForTimeout(300)

// RC-K22 同根：新增连接的默认 provider 跟**活动连接**走，不是恒为 realtime。
await page.locator('#cloud-add').click()
await page.waitForTimeout(200)
const newProviderText = await page.locator('#cloud-provider-select .select-button').innerText()
check(
  '新增连接的默认服务商跟活动连接走',
  newProviderText.includes('火山'),
  newProviderText,
)
await page.locator('#cloud-cancel').click()
await page.waitForTimeout(150)
// 圆点是**指示**，不是控件 —— 别让它再当按钮用。
check(
  '圆点只是指示、不可点',
  (await page.locator('#cloud-profiles .cloud-profile-mark').first().evaluate((el) => el.tagName)) === 'SPAN' &&
    (await page.locator('#cloud-profiles .cloud-profile-mark').first().getAttribute('data-action')) === null,
  await page.locator('#cloud-profiles .cloud-profile-mark').first().evaluate((el) => el.outerHTML),
)
// 行尾的动作平时藏起来，hover 才显形；使用中的那行一直显。
const inactiveActionsOpacity = await page
  .locator('#cloud-profiles .cloud-profile')
  .nth(0)
  .locator('.cloud-profile-actions')
  .evaluate((el) => getComputedStyle(el).opacity)
const activeActionsOpacity = await page
  .locator('#cloud-profiles .cloud-profile')
  .nth(1)
  .locator('.cloud-profile-actions')
  .evaluate((el) => getComputedStyle(el).opacity)
check(
  '动作图标 hover 才显形',
  inactiveActionsOpacity === '0' && activeActionsOpacity === '1',
  `未使用=${inactiveActionsOpacity} 使用中=${activeActionsOpacity}`,
)
// 按钮只是 opacity:0，仍在 Tab 序列里 —— 键盘聚焦到它时必须显形，
// 否则焦点落在两个看不见的按钮上（`:focus-within` 那条规则守着这里）。
await page
  .locator('#cloud-profiles .cloud-profile')
  .nth(0)
  .locator('[data-action="edit"]')
  .focus()
// 显形是 `transition: opacity 0.12s` —— 刚聚焦时读到的还是过渡中的值（≈0）。
await page.waitForTimeout(250)
const focusedActionsOpacity = await page
  .locator('#cloud-profiles .cloud-profile')
  .nth(0)
  .locator('.cloud-profile-actions')
  .evaluate((el) => getComputedStyle(el).opacity)
check(
  '键盘聚焦时动作图标也显形',
  focusedActionsOpacity === '1',
  `聚焦后=${focusedActionsOpacity}`,
)
// 图标按钮的点击区别太小。
const editIconBox = await page
  .locator('#cloud-profiles .cloud-profile')
  .nth(1)
  .locator('[data-action="edit"]')
  .boundingBox()
check(
  '图标按钮点击区不小于 32px',
  !!editIconBox && editIconBox.width >= 32 && editIconBox.height >= 32,
  editIconBox ? `${editIconBox.width}x${editIconBox.height}` : 'null',
)

// 编辑：改备注 → 列表里跟着变。
const editingRowId = await page
  .locator('#cloud-profiles .cloud-profile')
  .nth(1)
  .getAttribute('data-profile-id')
// 点编辑**不能**顺带把这条切成使用中。
const activeBeforeEdit = await page.evaluate(() => window.__mockStatus.cloudActiveProfile)
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('[data-action="edit"]').click()
await page.waitForTimeout(200)
check('编辑会打开编辑区', await page.locator('#cloud-editor').isVisible())
check(
  '点编辑不会顺带切换使用中',
  (await page.evaluate(() => window.__mockStatus.cloudActiveProfile)) === activeBeforeEdit,
  `编辑前=${activeBeforeEdit} 编辑后=${await page.evaluate(() => window.__mockStatus.cloudActiveProfile)}`,
)
// 编辑区就地展开在**被编辑的那一行下面** —— 放在别处的话视线要来回找。
const editorPrevId = await page.evaluate(() => {
  const editor = document.querySelector('#cloud-editor')
  return editor?.previousElementSibling?.dataset?.profileId || ''
})
check(
  '编辑区就地展开在被编辑的那一行下面',
  editorPrevId === editingRowId,
  `编辑的行=${editingRowId} 编辑区前一格=${editorPrevId}`,
)
await page.locator('#cloud-label').fill('火山·改过了')
await page.locator('#cloud-save').click()
await page.waitForTimeout(300)
check(
  '编辑后列表里跟着变',
  (await page.locator('#cloud-profiles .cloud-profile').nth(1).innerText()).includes('火山·改过了'),
  await page.locator('#cloud-profiles .cloud-profile').nth(1).innerText(),
)

// 取消：编辑区收起，列表不变。
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('[data-action="edit"]').click()
await page.waitForTimeout(200)
await page.locator('#cloud-label').fill('不该被保存')
await page.locator('#cloud-cancel').click()
await page.waitForTimeout(200)
check('取消后编辑区收起', !(await page.locator('#cloud-editor').isVisible()))
check(
  '取消后列表不变',
  !(await page.locator('#cloud-profiles .cloud-profile').nth(1).innerText()).includes('不该被保存'),
  await page.locator('#cloud-profiles .cloud-profile').nth(1).innerText(),
)

// 清凭据：凭据明文不回传 → 界面没法「读出来再改」，只能显式清。
// 编辑区里要能看到「清除凭据」，清完列表里那一行的掩码消失。
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('[data-action="edit"]').click()
await page.waitForTimeout(200)
check(
  '编辑已存凭据的连接时能看到「清除凭据」',
  await page.locator('#cloud-clear-key').isVisible(),
)
await page.evaluate(() => { window.__mockConfirmCalls = [] })
await page.locator('#cloud-clear-key').click()
await page.waitForTimeout(300)
check(
  '清除凭据前要确认',
  (await page.evaluate(() => window.__mockConfirmCalls || [])).length === 1,
  JSON.stringify(await page.evaluate(() => window.__mockConfirmCalls || [])),
)
check(
  '清完凭据后列表里不再有掩码',
  !(await page.locator('#cloud-profiles .cloud-profile').nth(1).innerText()).includes('sk-m'),
  await page.locator('#cloud-profiles .cloud-profile').nth(1).innerText(),
)
check(
  '清凭据后编辑区收起',
  !(await page.locator('#cloud-editor').isVisible()),
)

// 新增连接时不该出现「清除凭据」（新连接本来就没有凭据可清）。
await page.locator('#cloud-add').click()
await page.waitForTimeout(150)
check(
  '新增连接时没有「清除凭据」',
  !(await page.locator('#cloud-clear-key').isVisible()),
)
await page.locator('#cloud-cancel').click()
await page.waitForTimeout(150)

// 删除：点垃圾桶 → **先问一句** → 那一行没了。
await page.evaluate(() => { window.__mockConfirmCalls = [] })
await page.locator('#cloud-profiles .cloud-profile').nth(1).locator('[data-action="delete"]').click()
await page.waitForTimeout(300)
const deleteConfirmCalls = await page.evaluate(() => window.__mockConfirmCalls || [])
check(
  '删除连接前要确认',
  deleteConfirmCalls.length === 1 &&
    deleteConfirmCalls[0].message.includes('火山·改过了'),
  JSON.stringify(deleteConfirmCalls),
)
check(
  '删除后只剩一条',
  (await page.locator('#cloud-profiles .cloud-profile').count()) === 1,
  `行数=${await page.locator('#cloud-profiles .cloud-profile').count()}`,
)
// RC-K25：mock 与真后端同语义 —— 删掉「使用中」那条后活动项必须落到
// 真实存在的连接上（真后端：下一条，没有则前一条），不能停在已删的 id。
const activeAfterDelete = await page.evaluate(() => window.__mockStatus.cloudActiveProfile)
const remainingIds = await page.evaluate(() =>
  window.__mockStatus.cloudProfiles.map((profile) => profile.id),
)
check(
  '删掉使用中的连接后活动项指向剩下的第一条',
  remainingIds.includes(activeAfterDelete),
  `活动项=${activeAfterDelete} 剩下=${remainingIds.join(',')}`,
)
// RC-K25：不存在的 id 不能让活动项指到幽灵连接上。
await page.evaluate(() =>
  window.__TAURI__.core.invoke('set_active_cloud_profile', { id: 'ghost-profile' }),
)
check(
  'mock：不存在的 id 不会改活动项',
  (await page.evaluate(() => window.__mockStatus.cloudActiveProfile)) === activeAfterDelete,
  await page.evaluate(() => window.__mockStatus.cloudActiveProfile),
)

// 当前 provider 不支持术语表时，界面要提示 —— 术语表是本地流式与云端
// 共用的全局设置，只能提示，不能隐藏入口。
await page.evaluate(() => { window.__mockStatus.cloudSupportsKeywords = false })
// 状态是拉回来才更新的：切一次「使用中」触发刷新（编辑区默认收起，
// 点列表里的圆点就行）。
await page.locator('#cloud-profiles .cloud-profile').first().locator('[data-action="activate"]').click()
await page.waitForTimeout(300)
check(
  '云端不支持术语表时给出提示',
  (await page.locator('#cloud-keyword-hint').innerText()).includes('术语表'),
  await page.locator('#cloud-keyword-hint').innerText(),
)
await page.evaluate(() => { window.__mockStatus.cloudSupportsKeywords = true })
await page.locator('#cloud-profiles .cloud-profile').first().locator('[data-action="activate"]').click()
await page.waitForTimeout(300)
check(
  '云端支持术语表时不提示',
  (await page.locator('#cloud-keyword-hint').innerText()).trim() === '',
  await page.locator('#cloud-keyword-hint').innerText(),
)
check(
  '配置状态提示已配置',
  (await page.locator('#cloud-status').innerText()).includes('已配置'),
  await page.locator('#cloud-status').innerText(),
)
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.waitForTimeout(150)
// 云端模式录音：走的是云端自己的启动命令。
await page.locator('#mic-btn').click()
await page.waitForTimeout(300)
check(
  '云端模式调用云端启动命令',
  (await page.evaluate(() => window.__mockCalls.some(([cmd]) => cmd === 'start_cloud_recording'))),
  await page.evaluate(() => window.__mockCalls.map(([cmd]) => cmd).join(',')),
)
await page.locator('#mic-btn').click()
await page.waitForTimeout(300)

// ── 云端连接编辑：焦点 / 凭据 / provider 的守卫 ──
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(200)

// 先写入一把**已知**密钥：后面「留空 = 不改」才有可比的基准
// （只断言「没变」的话，mock 若每次都伪造同一把 key 也能蒙混过去）。
await page.locator('#cloud-profiles .cloud-profile').first().locator('[data-action="edit"]').click()
await page.waitForTimeout(150)
await page.locator('#cloud-api-key').fill('sk-known-key-123')
await page.locator('#cloud-save').click()
await page.waitForTimeout(250)
const knownMask = await page.evaluate(() => window.__mockStatus.cloudProfiles[0]?.keyMask)
check('测试前置：写入已知密钥后有掩码', !!knownMask, knownMask || '(空)')

// RC-K25 / 5.4：编辑时密钥留空 = **不改**（掩码保持不变）。
await page.locator('#cloud-profiles .cloud-profile').first().locator('[data-action="edit"]').click()
await page.waitForTimeout(150)
await page.locator('#cloud-label').fill('改个备注')
await page.locator('#cloud-save').click()
await page.waitForTimeout(250)
const maskAfterLabelEdit = await page.evaluate(
  () => window.__mockStatus.cloudProfiles[0].keyMask,
)
check(
  '编辑时留空密钥 → 掩码保持不变',
  maskAfterLabelEdit === knownMask,
  `原=${knownMask} 现=${maskAfterLabelEdit}`,
)

// RC-K23：编辑区展开、密钥框有焦点时，列表重建不能把焦点弄丢
// （摘掉再插回 DOM 会失焦，接着敲的字符会被整个吞掉）。
await page
  .locator('#cloud-profiles .cloud-profile')
  .first()
  .locator('[data-action="edit"]')
  .click()
await page.waitForTimeout(150)
await page.locator('#cloud-api-key').fill('sk-abc')
// 真实的异步触发源：流式引擎预热完成 → streaming:state → renderStatus → 重建列表。
await page.evaluate(() => window.__emit('streaming:state', { phase: 'ready' }))
await page.waitForTimeout(150)
check(
  '列表重建后密钥框仍保持焦点',
  (await page.evaluate(() => document.activeElement?.id)) === 'cloud-api-key',
  await page.evaluate(() => document.activeElement?.id || '(没有焦点)'),
)
await page.keyboard.type('XYZ')
check(
  '重建后继续输入的字符不丢',
  (await page.locator('#cloud-api-key').inputValue()) === 'sk-abcXYZ',
  await page.locator('#cloud-api-key').inputValue(),
)
await page.locator('#cloud-cancel').click()
await page.waitForTimeout(150)

// RC-K21（用户定案）：**已有连接的服务商锁定** —— 换服务商只能删掉重加。
// 这样旧服务商的凭据根本不可能被带到新服务商名下；后端仍留兜底
// （provider 变了且没给新凭据 → 清空），防别的调用方绕过去。
await page
  .locator('#cloud-profiles .cloud-profile')
  .first()
  .locator('[data-action="edit"]')
  .click()
await page.waitForTimeout(150)
check(
  '编辑已有连接时服务商下拉被禁用',
  await page.locator('#cloud-provider-select .select-button').isDisabled(),
)
await page.locator('#cloud-save').click()
await page.waitForTimeout(250)
const afterLockedSave = await page.evaluate(() => window.__mockStatus.cloudProfiles[0])
check(
  '锁定服务商后保存，provider 与凭据都没变',
  afterLockedSave.provider === 'realtime' && afterLockedSave.keyMask === knownMask,
  JSON.stringify(afterLockedSave),
)
// 新增连接才允许选服务商；也不该出现「不可更改」提示。
await page.locator('#cloud-add').click()
await page.waitForTimeout(150)
check(
  '新增连接时服务商下拉可点',
  !(await page.locator('#cloud-provider-select .select-button').isDisabled()),
)
await page.locator('#cloud-cancel').click()
await page.waitForTimeout(150)

// ── 侧边栏折叠：只留图标的一条窄栏 ──
const sidebarWidth = () =>
  page.locator('.sidebar').evaluate((el) => el.getBoundingClientRect().width)
const expandedWidth = await sidebarWidth()
await page.locator('#sidebar-toggle').click()
await page.waitForTimeout(300)
const collapsedWidth = await sidebarWidth()
check(
  '点折叠按钮后侧栏变窄到 80px',
  // 80px 是量出来的：macOS 的红绿灯固定在离窗口左缘 10/33/56pt，绿灯右缘
  // 在 68pt —— 折叠后的右边框必须落在它右边（72px 时只剩 3pt，太挤）。
  collapsedWidth === 80,
  `展开=${expandedWidth} 折叠=${collapsedWidth}`,
)
check(
  '折叠后导航文字不占位',
  await page.evaluate(() => {
    const span = document.querySelector('.nav-item[data-view="transcribe"] span')
    return span !== null && getComputedStyle(span).display === 'none'
  }),
)
check(
  '折叠后按钮的提示改成「展开侧边栏」',
  (await page.locator('#sidebar-toggle').getAttribute('aria-label')) === '展开侧边栏',
  await page.locator('#sidebar-toggle').getAttribute('aria-label'),
)
check(
  '折叠状态写进 localStorage',
  (await page.evaluate(() => localStorage.getItem('voicenotes.sidebarCollapsed'))) === '1',
)
// 折叠后图标仍可点：导航还是能切的。
await page.locator('.nav-item[data-view="settings"]').click()
await page.waitForTimeout(200)
check(
  '折叠状态下仍能切换视图',
  (await page.locator('.app').getAttribute('data-view')) === 'settings',
  await page.locator('.app').getAttribute('data-view'),
)
await page.locator('.nav-item[data-view="transcribe"]').click()
await page.waitForTimeout(200)
// 还原成展开：后面的断言按展开态跑。
await page.locator('#sidebar-toggle').click()
await page.waitForTimeout(300)
check(
  '再点一次恢复展开',
  (await sidebarWidth()) === expandedWidth,
  `展开=${expandedWidth} 现在=${await sidebarWidth()}`,
)

// ── Windows 场景：侧栏顶部的红绿灯占位不能套到 Windows 上 ──
// macOS 的标题栏是覆盖式的，侧栏顶部要留 78px 给红绿灯；
// Windows 没有红绿灯，占位必须跟着内容走 —— 写死个小高度会把 30px 高的
// 品牌行切掉一截。harness 的 UA 决定 body 上有没有 macos 类，所以这里换一个
// Windows UA 另开一页。
const WINDOWS_UA =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15'
const winContext = await browser.newContext({ viewport, locale: 'zh-CN', userAgent: WINDOWS_UA })
await winContext.addInitScript(installMock)
const winPage = await winContext.newPage()
collectErrors(winPage)
await winPage.goto(`${base}/index.html`)
await winPage.waitForSelector('#lang-row .chip')
await winPage.waitForTimeout(400)
check(
  'Windows 下不加 macOS 材质类',
  !(await winPage.evaluate(() => document.body.classList.contains('macos'))),
  await winPage.evaluate(() => document.body.className || '(body 没 class)'),
)
check(
  'Windows 下侧栏品牌行不被切掉',
  await winPage.evaluate(() => {
    const top = document.querySelector('.sidebar-top')
    const brand = document.querySelector('.brand')
    if (!top || !brand) return false
    // 顶部容器至少要装得下品牌行：写死 12px 就会小于品牌行的高度。
    return top.getBoundingClientRect().height >= brand.getBoundingClientRect().height
  }),
  await winPage.evaluate(() => {
    const top = document.querySelector('.sidebar-top')
    const brand = document.querySelector('.brand')
    if (!top || !brand) return '(找不到 .sidebar-top / .brand)'
    return `顶部 ${Math.round(top.getBoundingClientRect().height)}px / 品牌 ${Math.round(brand.getBoundingClientRect().height)}px`
  }),
)
await winContext.close()

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
