/* 模型状态、识别模式、语言、输入设备、模型下载、设置项 */

import { $, CLOUD_OPT_IN_KEY, LANGS, STREAM_LANGS, els, errorCode, errorText, fmtBytes, fmtError, idleHint, state, t, toast } from './core.js'
import { confirmDialog, invoke, logEvent, openDialog } from './tauri.js'
import { dotIcon, editIcon, iconAction, trashIcon } from './icons.js'

/* 下拉框实例由入口创建后交给这里（status 不该认识 applyLocale，否则会和入口互相依赖） */
let deviceSelect = null
let cloudProviderSelect = null
let localeSelect = null

export function setSelects(device, locale, cloudProvider) {
  deviceSelect = device
  localeSelect = locale
  cloudProviderSelect = cloudProvider
}

/// 模型徽标文案：流式与整段用同一套说法，避免两处提示对不上。
export function modelBadgeText(phase) {
  const mode = state.status?.mode
  const name = mode === 'streaming'
    ? t('model.label.streaming')
    : mode === 'cloud'
      ? t('model.label.cloud')
      : t('model.label.offline')
  if (phase === 'error') return t('model.badge.error', { name })
  if (phase === 'loading') return t('model.badge.loading', { name })
  return t('model.badge.ready', { name })
}

/// 引擎三态的唯一出处：状态点 + 徽标底色 + 徽标文案。
///
/// 徽标以前有两套写法且已经漂移（events.js 的 engine:state 一套、
/// renderStatus() 一套），流式引擎报错时一个红底一个灰底。
/// 两种模式必须一致 —— 所以状态怎么显示只看这里。
export function applyEngineState(phase) {
  const shown = phase === 'error' || phase === 'loading' ? phase : 'ready'
  els.dot.dataset.state = shown
  els.badge.className = shown === 'error' ? 'badge danger' : 'badge'
  els.badge.textContent = modelBadgeText(shown)
}

export async function refreshStatus() {
  try {
    state.status = await invoke('status')
    renderStatus()
  } catch (error) {
    toast(t('result.readStatusFailed', { error: fmtError(error) }), 'error')
  }
}

export function renderStatus() {
  const status = state.status
  if (!status) return
  const streamingMode = status.mode === 'streaming'
  const cloudMode = status.mode === 'cloud'
  // 云端模式不装本地模型：能不能用只看有没有填凭据。
  const ready = cloudMode ? !!status.cloudConfigured : streamingMode ? status.streamingReady : status.modelReady
  els.setup.classList.toggle('hidden', streamingMode || cloudMode || status.modelReady)

  if (streamingMode) {
    if (!ready) {
      // 模型没装好：不是引擎三态，徽标用黄色（跟整段一致）。
      els.badge.className = 'badge warn'
      els.badge.textContent = t('model.badge.streamingMissing')
      els.dot.dataset.state = state.engine === 'error' ? 'error' : state.engine === 'loading' ? 'loading' : 'idle'
    } else {
      applyEngineState(state.engine)
    }
  } else if (cloudMode) {
    // 云端没有本地引擎：徽标只说配没配凭据，不装作「模型已就绪」。
    els.badge.className = ready ? 'badge' : 'badge warn'
    els.badge.textContent = ready ? t('model.badge.cloudReady') : t('model.badge.cloudMissing')
    els.dot.dataset.state = 'idle'
  } else if (!ready) {
    els.badge.textContent = t('model.badge.offlineMissing')
    els.badge.className = 'badge warn'
    els.dot.dataset.state = 'idle'
    const partial = status.installedBytes || 0
    if (partial > 0 && !state.downloading) {
      els.setupProgress.classList.remove('hidden')
      setProgress(partial, status.totalBytes)
      els.setupText.textContent = t('setup.cached', { done: fmtBytes(partial), total: fmtBytes(status.totalBytes) })
    }
  } else {
    applyEngineState(state.engine)
  }

  els.mic.classList.toggle('disabled', !ready)
  els.mic.title = ready ? '' : streamingMode ? t('model.needStreaming') : cloudMode ? t('model.needCloud') : t('model.needOffline')

  els.settingsClearModel.classList.toggle('hidden', !status.modelDirCustom)
  // 设置页这块只讲整段模型：流式模式下不能用 streamingReady 冒充它已就绪。
  els.settingsModel.textContent = status.modelReady
    ? t('settings.modelPath', {
        source: t(`model.source.${status.modelSource || 'app_data'}`),
        dir: status.modelDir,
      })
    : t('settings.modelNotReady', { size: fmtBytes(status.totalBytes) })
  els.settingsOutput.textContent = status.outputDir
  els.settingsVersion.textContent = t('settings.version', { version: status.appVersion })
  els.itnToggle.checked = !!status.itn
  els.punctToggle.checked = !!status.punct
  els.punctStatus.textContent = status.punctReady
    ? t('settings.punctReady')
    : t('settings.punctMissing', { size: fmtBytes(status.punctTotalBytes || 0) })
  // 回填术语表；用户正在编辑时不要覆盖输入
  if (document.activeElement !== els.hotwordsInput) {
    els.hotwordsInput.value = (status.hotwords || []).join('\n')
  }
  els.modelMode.textContent = streamingMode
    ? t('model.name.streaming')
    : cloudMode
      ? t('model.name.cloud')
      : t('model.name.offline')
  renderCloudConfig()

  for (const button of els.langRow.children) {
    button.setAttribute('aria-pressed', String(button.dataset.lang === status.language))
  }
  renderMode()
}

/// 云端服务（provider）选项。
export function cloudProviderItems() {
  return [
    { value: 'realtime', label: t('settings.cloudProviderRealtime') },
    { value: 'volcengine', label: t('settings.cloudProviderVolcengine') },
    { value: 'aliyun', label: t('settings.cloudProviderAliyun') },
  ]
}

/// 各家「模型 / 资源标识」的候选项。
///
/// **不让用户手打**：阿里的模型走白名单（填错直接报错），火山的资源标识
/// 填成模型 1.0 的 id 会握手 403 —— 手打很容易踩，踩了只有连不上时才看得见。
///
/// Realtime 的官方还有别的模型，所以它的候选项后面**允许手打**（datalist）。
const CLOUD_MODEL_OPTIONS = {
  realtime: ['gpt-live-transcribe'],
  aliyun: ['qwen-audio-3.1-asr-flash-streaming'],
  volcengine: ['volc.seadasr.sauc.duration', 'volc.seadasr.sauc.concurrent'],
}

/// 火山那一列叫「资源标识」，别家叫「模型」—— 同一个字段，按服务解释。
function cloudModelLabel(provider) {
  return provider === 'volcengine'
    ? t('settings.cloudResourceId')
    : t('settings.cloudModel')
}

/// 刷新模型候选项（换服务就换一批）。
export function refreshCloudModelOptions(provider) {
  const options = CLOUD_MODEL_OPTIONS[provider] || []
  els.cloudModelOptions.replaceChildren(
    ...options.map((value) => {
      const option = document.createElement('option')
      option.value = value
      // 有的引擎的 datalist 只显示文本、不看 value —— 不写会显示成空白。
      option.textContent = value
      return option
    }),
  )
  els.cloudModelLabel.textContent = cloudModelLabel(provider)
}

/// 编辑区的状态：编辑哪条 / 当前选中的 provider。
///
/// provider 必须由编辑区自己持有：`UISelect.setItems` **不触发** `onChange`，
/// 程序换值是静默的 —— 列表重绘或切语言时若跟着「活动连接」走，
/// 保存时就会把正在编辑那条的 provider 悄悄改掉（RC-K22）。
///
/// **已有连接的服务商不可更改**：一条连接 = 一整套配置，换服务商就是换一套；
/// 已有连接换 provider 只会带来「旧密钥发给新端点」这类麻烦，所以界面直接
/// 锁住下拉 —— 要换服务商请删除这条后重新添加。新连接还没有已存凭据，
/// 换服务商不存在跨家沿用的问题。
let editingProfileId = null
let editorOpen = false
let editorProvider = 'realtime'

/// 活动连接的 provider（没有就按默认走）。
function activeCloudProvider() {
  return state.status?.cloudProvider || 'realtime'
}

/// 打开编辑区（`profile` 为空 = 新增一条）。
export function openCloudEditor(profile) {
  editingProfileId = profile?.id || null
  editorOpen = true
  els.cloudEditor.classList.remove('hidden')
  const provider = profile?.provider || activeCloudProvider()
  editorProvider = provider
  if (cloudProviderSelect && cloudProviderSelect.value !== provider) {
    cloudProviderSelect.setItems(cloudProviderItems(), provider)
  }
  // 已有连接：服务商只读（换服务商 = 删掉重加，见上面的注释）。
  cloudProviderSelect?.setDisabled(!!profile)
  refreshCloudModelOptions(provider)
  els.cloudLabel.value = profile?.label || ''
  els.cloudBaseUrl.value = profile?.baseUrl || ''
  // 凭据不回传前端 —— 留空表示「不改」（要清空就在编辑区里显式清）。
  els.cloudApiKey.value = ''
  els.cloudModel.value = profile?.model || ''
  // 「清除凭据」只对**已存凭据**的已有连接有意义。
  els.cloudClearKey.classList.toggle('hidden', !profile?.keyMask)
  placeEditor()
}

/// provider 下拉变了（用户操作，`app.js` 的 `onChange` 转过来）。
///
/// 只会发生在**新增**连接时：换服务 = 换一套配置含义，
/// 地址 / 模型 / 输入框里的密钥都清掉、换模型候选项。
export function cloudProviderChanged(value) {
  editorProvider = value
  els.cloudModel.value = ''
  els.cloudBaseUrl.value = ''
  els.cloudApiKey.value = ''
  refreshCloudModelOptions(value)
}

/// 编辑区放在**触发它的那一处下方**：改已有配置 → 就在那一行下面展开；
/// 新增 → 就在「＋ 添加配置」按钮上面展开。
///
/// 放在别处（列表统一在下方）的话，点完铅笔视线要来回找 —— 编辑的是哪一行？
function placeEditor() {
  const row = editingProfileId
    ? els.cloudProfiles.querySelector(`[data-profile-id="${CSS.escape(editingProfileId)}"]`)
    : null
  if (row) {
    row.after(els.cloudEditor)
  } else {
    els.cloudAdd.before(els.cloudEditor)
  }
}

/// 收起编辑区。
export function closeCloudEditor() {
  editingProfileId = null
  editorOpen = false
  els.cloudEditor.classList.add('hidden')
  els.cloudApiKey.value = ''
  // 下次打开可能是「新增」—— 把只读状态复位。
  cloudProviderSelect?.setDisabled(false)
  placeEditor()
}

/// 云端配置：连接列表 + 状态行。
///
/// 凭据不会回传前端 —— 列表里只显示**掩码**（按露出比例分档，见 `mask_secret`）。
function renderCloudConfig() {
  const status = state.status
  if (!status) return
  els.cloudStatus.textContent = status.cloudConfigured
    ? t('settings.cloudConfigured')
    : t('settings.cloudNotConfigured')
  // 当前服务不支持请求级术语表时给出提示 —— 术语表是本地流式与云端
  // 共用的全局设置，只能提示，不能隐藏入口（藏了就顺手废掉本地流式的术语表）。
  els.cloudKeywordHint.textContent = status.cloudSupportsKeywords
    ? ''
    : t('settings.cloudNoKeywords')
  renderCloudProfiles(status)
  // 编辑区没开时，provider 下拉跟**活动连接**走。`bindUI` 时 `state.status`
  // 还是 null，只能先填 realtime —— 这里补上回填（RC-K22 的同根问题）。
  // 编辑区开着时不碰它：编辑的是哪条连接由编辑区自己持有。
  if (cloudProviderSelect && !editorOpen) {
    cloudProviderSelect.setItems(cloudProviderItems(), activeCloudProvider())
  }
}

/// 一行连接（掩码 + 服务 + 备注 + 「使用中」）。
///
/// 行元素**按 id 复用**：整表重建会把用户正在操作的那一行换掉 —— 焦点随之
/// 掉到 body，键盘用户下一次 Tab 得从头开始。复用的行只改内容，不换节点。
const cloudRows = new Map()

/// 造一行连接（掩码 + 服务 + 备注 + 「使用中」）。
function createProfileRow(profile, status) {
  const row = document.createElement('div')
  row.className = 'cloud-profile'
  row.dataset.profileId = profile.id
  // 点整行 = 设为使用中（图标上的动作由更近的 [data-action] 接走）。
  row.dataset.action = 'activate'

  // 状态点：纯指示（实心 = 使用中），不可点 —— 设为使用中是「点整行」做的。
  const mark = document.createElement('span')
  mark.className = 'cloud-profile-mark'
  mark.setAttribute('aria-hidden', 'true')
  row.appendChild(mark)

  // 名称 + 明细做成按钮：鼠标点整行都行，键盘也能 Tab 到这里按回车。
  const body = document.createElement('button')
  body.type = 'button'
  body.className = 'cloud-profile-body'
  body.dataset.action = 'activate'
  const name = document.createElement('div')
  name.className = 'cloud-profile-name'
  const detail = document.createElement('div')
  detail.className = 'muted small'
  body.append(name, detail)
  row.appendChild(body)

  // 「使用中」徽章：**一直存在**，用的时候才显示。
  // 增删节点会让落在行里的焦点丢失，所以徽章改成显隐而不是加删。
  const badge = document.createElement('span')
  badge.className = 'cloud-profile-badge hidden'
  row.appendChild(badge)

  const actions = document.createElement('div')
  actions.className = 'cloud-profile-actions'
  const edit = iconAction(t('settings.cloudEdit'), editIcon())
  edit.dataset.action = 'edit'
  const remove = iconAction(t('settings.cloudDelete'), trashIcon())
  remove.dataset.action = 'delete'
  actions.append(edit, remove)
  row.appendChild(actions)

  const entry = { row, mark, body, name, detail, badge, edit, remove }
  cloudRows.set(profile.id, entry)
  updateProfileRow(entry, profile, status)
  return entry
}

/// 刷新一行的内容（掩码、服务名、「使用中」徽章、按钮提示）。
///
/// 文案随语言变，所以每次渲染都要重刷 —— 但**不换节点**，
/// 焦点留在原处。
function updateProfileRow(entry, profile, status) {
  const active = profile.id === status.cloudActiveProfile
  entry.row.classList.toggle('active', active)
  entry.mark.innerHTML = dotIcon(active)
  entry.body.title = t('settings.cloudUseThis')
  entry.name.textContent = profile.label || profile.provider
  // 掩码 + 服务名（凭据明文永远不回传前端）。
  entry.detail.textContent = [profile.keyMask, cloudProviderLabel(profile.provider)]
    .filter(Boolean)
    .join(' · ')
  entry.badge.textContent = t('settings.cloudInUse')
  entry.badge.classList.toggle('hidden', !active)
  entry.edit.title = t('settings.cloudEdit')
  entry.edit.setAttribute('aria-label', t('settings.cloudEdit'))
  entry.remove.title = t('settings.cloudDelete')
  entry.remove.setAttribute('aria-label', t('settings.cloudDelete'))
}

/// 渲染连接列表。
///
/// **行按 id 复用**：换掉的只有内容，节点还在 —— 焦点不丢（RC-E9）。
/// 编辑区同样不被搬动（RC-K23）：它在列表里就留在被编辑那行下面，
/// 不在列表里（新增一条时）就留在外面。
function renderCloudProfiles(status) {
  const profiles = status.cloudProfiles || []
  const editor = els.cloudEditor
  const rows = profiles.map((profile) => {
    const existing = cloudRows.get(profile.id)
    if (existing) {
      updateProfileRow(existing, profile, status)
      return existing.row
    }
    return createProfileRow(profile, status).row
  })
  // 已经删掉的连接：把它的行从 DOM 和缓存里一起清掉。
  for (const [id, entry] of [...cloudRows]) {
    if (!profiles.some((profile) => profile.id === id)) {
      entry.row.remove()
      cloudRows.delete(id)
    }
  }
  // 目标顺序：编辑区在被编辑的那一行后面（它在列表里时），其余按连接顺序。
  const index = profiles.findIndex((profile) => profile.id === editingProfileId)
  const editorInside = editor.parentElement === els.cloudProfiles
  let desired = rows
  if (editorInside) {
    if (index < 0) {
      // 正在编辑的那条被删了（`editingProfileId` 指向已不存在的 id）：
      // 编辑区**节点本身不动**（它马上就要被 closeCloudEditor 移出列表，
      // 这一帧里搬它等于白搬一次 —— 移动 DOM 节点会丢焦点），
      // 其余行按顺序排在它**后面**。
      desired = [editor, ...rows]
    } else {
      desired = [...rows.slice(0, index + 1), editor, ...rows.slice(index + 1)]
    }
  }
  syncRowOrder(els.cloudProfiles, desired)
}

/// 把容器里的节点按目标顺序摆好 —— **已经在位的节点不动**。
///
/// 移动 DOM 节点会让落在它里面的焦点丢失（WebKit 实测），所以只在
/// 顺序真的变了时才动。
///
/// 从后往前摆：每个节点只要不是「正好在 anchor 前面」就插过去。
/// 已经在容器里的节点 `nextSibling` 可能与 anchor 相同 —— 那是「已在位」；
/// 不在容器里的节点 `nextSibling` 是 null，要靠 `parentElement` 区分，
/// 否则新行会被当成已在位而永远插不进去。
function syncRowOrder(container, desired) {
  let anchor = null
  for (let i = desired.length - 1; i >= 0; i -= 1) {
    const node = desired[i]
    const inPlace = node.parentElement === container && node.nextSibling === anchor
    if (!inPlace) container.insertBefore(node, anchor)
    anchor = node
  }
}

/// provider 的显示名（列表里用）。
function cloudProviderLabel(provider) {
  const found = cloudProviderItems().find((item) => item.value === provider)
  return found ? found.label : provider
}

/// 云端模式会把录音上传出去：第一次必须用户明确同意。
///
/// 同意过一次就记住，不再反复问（换服务随时可以在设置里改）。
async function confirmCloudUpload() {
  if (localStorage.getItem(CLOUD_OPT_IN_KEY) === '1') return true
  try {
    const ok = await confirmDialog(t('cloud.privacyMessage'), {
      title: t('cloud.privacyTitle'),
      kind: 'warning',
    })
    if (!ok) return false
    localStorage.setItem(CLOUD_OPT_IN_KEY, '1')
    logEvent('INFO', '用户已确认云端上传提示')
    return true
  } catch (error) {
    logEvent('ERROR', `云端上传提示失败: ${errorText(error)}`)
    return false
  }
}

/// 破坏性操作前的确认框（删除 / 清除凭据）。
///
/// 确认框打不开时**当作没同意** —— 破坏性操作保守放弃，比误删强。
/// 但也不能静默：原因写进日志，界面那条操作就是「没发生」。
async function confirmDestructive(message, title) {
  try {
    return await confirmDialog(message, { title, kind: 'warning' })
  } catch (error) {
    logEvent('ERROR', `确认框打开失败: ${errorText(error)}`)
    return false
  }
}

/// 保存（新增或改）一条连接。
///
/// 凭据不会回传前端，所以密钥框留空 = 不改；要清空走「清除凭据」按钮
/// （`clearCloudProfileKey`）—— 空串与「没改」在前端分不开。
///
/// 已有连接的服务商在界面上是只读的（换服务商 = 删掉重加），所以这里
/// 不会出现「换了 provider 但没填新密钥」的情况；后端 `upsert_cloud_profile`
/// 仍留了兜底（provider 变了且没给新凭据 → 清空），防别的调用方绕过去。
export async function saveCloudProfile() {
  const typed = els.cloudApiKey.value.trim()
  try {
    state.status = await invoke('save_cloud_profile', {
      id: editingProfileId,
      label: els.cloudLabel.value.trim(),
      baseUrl: els.cloudBaseUrl.value.trim(),
      apiKey: typed ? typed : undefined,
      model: els.cloudModel.value.trim(),
      provider: editorProvider,
    })
    renderStatus()
    closeCloudEditor()
    toast(t('settings.cloudSaved'))
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

/// 清掉一条连接的凭据（地址 / 模型 / 备注都不动）。
///
/// 凭据明文不回传前端 → 界面没法「读出来再改」，只能显式清。
export async function clearCloudProfileKey() {
  const id = editingProfileId
  if (!id) return
  // 凭据明文不回传 → 清除后找不回来，跟删配置一样要问一句。
  const ok = await confirmDestructive(
    t('settings.cloudClearKeyConfirm'),
    t('settings.cloudClearKey'),
  )
  if (!ok) return
  try {
    state.status = await invoke('clear_cloud_profile_key', { id })
    renderStatus()
    closeCloudEditor()
    toast(t('settings.cloudKeyCleared'))
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

/// 删一条连接。
export async function deleteCloudProfile(id) {
  // 删掉就找不回来了（凭据明文永不回传）→ 删之前问一句。
  const profile = state.status?.cloudProfiles?.find((item) => item.id === id)
  const name = profile?.label || profile?.provider || ''
  const ok = await confirmDestructive(
    t('settings.cloudDeleteConfirm', { name }),
    t('settings.cloudDelete'),
  )
  if (!ok) return
  try {
    state.status = await invoke('delete_cloud_profile', { id })
    renderStatus()
    if (editingProfileId === id) closeCloudEditor()
    toast(t('settings.cloudProfileDeleted'))
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

/// 指定正在使用的那条连接（转写时用它）。
export async function setActiveCloudProfile(id) {
  try {
    state.status = await invoke('set_active_cloud_profile', { id })
    renderStatus()
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export function renderMode() {  const mode = state.status?.mode || 'offline'
  els.itnToggle.disabled = state.phase !== 'idle'
  for (const chip of els.modeRow.children) {
    chip.setAttribute('aria-pressed', String(chip.dataset.mode === mode))
    chip.disabled = state.phase !== 'idle'
  }
  if (state.phase === 'idle') els.hint.textContent = idleHint()
  if (state.status) {
    for (const chip of els.langRow.children) {
      chip.disabled = state.phase !== 'idle' || (state.status.mode === 'streaming' && !STREAM_LANGS.includes(chip.dataset.lang))
      chip.title = state.phase !== 'idle'
        ? t('record.configLocked')
        : chip.disabled ? t('mode.streamingOnlyZh') : ''
    }
  }
  const needStream = mode === 'streaming' && !state.status?.streamingReady
  els.streamSetup.classList.toggle('hidden', !needStream)
  if (needStream && state.status) {
    els.streamSetupText.textContent = t('mode.streamingMissing', {
      size: fmtBytes(state.status.streamingTotalBytes || 0),
    })
  }
}

export async function setMode(mode) {
  if (state.phase !== 'idle' || state.status?.mode === mode) return
  // 云端模式会把录音上传出去：没得到用户同意就不切。
  if (mode === 'cloud' && !(await confirmCloudUpload())) return
  try {
    state.status = await invoke('set_mode', { mode })
    state.engine = 'idle'
    renderStatus()
    if (mode === 'streaming') {
      if (!STREAM_LANGS.includes(state.status.language)) {
        state.status = await invoke('set_language', { language: 'auto' })
        toast(t('mode.langSwitched'))
        renderStatus()
      }
      if (!state.status.streamingReady) {
        toast(t('mode.streamingDownloadFirst'))
      } else {
        void invoke('warm_up_streaming').catch(() => {})
      }
    } else if (mode === 'cloud') {
      // 云端不装本地模型：没有凭据就提示去哪里填，别装作能用。
      if (!state.status.cloudConfigured) toast(t('mode.cloudNeedConfig'))
    } else if (state.status.modelReady) {
      // 切回整段前先把 239 MB 引擎加载好（后端切模式时已经释放了另一份）。
      void warmUp().catch(() => {})
    }
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export async function setLanguage(language) {
  if (state.phase !== 'idle') return
  try {
    if (state.status?.mode === 'streaming' && !STREAM_LANGS.includes(language)) {
      toast(t('mode.switchedBack'))
      state.status = await invoke('set_mode', { mode: 'offline' })
    }
    state.status = await invoke('set_language', { language })
    renderStatus()
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

export function renderLang() {
  els.langRow.innerHTML = ''
  for (const code of LANGS) {
    const button = document.createElement('button')
    button.type = 'button'
    button.className = 'chip'
    button.dataset.lang = code
    button.textContent = t(`lang.${code}`)
    button.setAttribute('aria-pressed', 'false')
    button.addEventListener('click', () => setLanguage(code))
    els.langRow.appendChild(button)
  }
}

/// 已列出的输入设备（列设备会触发 macOS 权限弹窗，所以只在用户打开下拉框时才去列）
let deviceCache = []

export function deviceItems() {
  const items = [{ value: '', label: t('record.deviceDefault') }]
  for (const device of deviceCache) {
    // value 用设备名（后端按名字精确匹配），label 同源 —— 展示文案不能当身份。
    // 后端不会返回无名设备：按名字选不中的设备列出来也没用。
    items.push({
      value: device.name,
      label: device.isDefault ? `${device.name} · ${t('record.deviceSystemDefault')}` : device.name,
    })
  }
  return items
}

export async function loadDevices() {
  if (!deviceSelect) return
  let devices = []
  try {
    devices = (await invoke('list_input_devices')) || []
  } catch (error) {
    // 权限/宿主错误不等于设备被拔掉，不能清掉用户保存的选择。
    logEvent('ERROR', `输入设备枚举失败: ${errorText(error)}`)
    return
  }
  deviceCache = devices
  const current = state.status?.inputDevice || deviceSelect.value || ''
  const items = deviceItems()
  const selected = items.some((item) => item.value === current) ? current : ''
  deviceSelect.setItems(items, selected)
  if (current && !selected) {
    // 设备已拔掉：界面回到默认，后端也必须同步，否则下次仍使用旧设备名。
    try {
      state.status = await invoke('set_input_device', { name: null })
    } catch (error) {
      logEvent('ERROR', `失效输入设备回退失败: ${errorText(error)}`)
    }
  }
}

/// 界面语言下拉。
export function localeItems() {
  return [
    { value: 'system', label: t('settings.localeSystem') },
    { value: 'zh-CN', label: t('settings.localeZh') },
    { value: 'en', label: t('settings.localeEn') },
  ]
}

/// 切语言 / 重绘后同步两个下拉框的选项。
export function refreshSelectItems(localeValue) {
  if (deviceSelect) {
    const items = deviceItems()
    const current = deviceSelect.value || state.status?.inputDevice || ''
    deviceSelect.setItems(items, items.some((item) => item.value === current) ? current : '')
  }
  if (localeSelect) localeSelect.setItems(localeItems(), localeValue)
  if (cloudProviderSelect) {
    // 编辑区开着时用**编辑中那条**的 provider：切语言只是换文案，不能换值
    // （`setItems` 不触发 `onChange`，换掉用户看不见 —— RC-K22）。
    cloudProviderSelect.setItems(
      cloudProviderItems(),
      editorOpen ? editorProvider : activeCloudProvider(),
    )
  }
}

/// 百分比：两个进度条共用一份算法。
const pctOf = (done, total) => (total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0)

export function setProgress(done, total) {
  const pct = pctOf(done, total)
  els.setupFill.style.width = `${pct}%`
  els.setupPct.textContent = `${pct}%`
}

export function setStreamProgress(done, total) {
  const pct = pctOf(done, total)
  els.streamProgressFill.style.width = `${pct}%`
  els.streamProgressPct.textContent = `${pct}%`
}

/// 下载模型：整段与流式的流程完全一样，只有命令名、字节数、
/// 文案、额外步骤和预热方式不同，统一走这里。
async function runDownload({
  isBusy, setBusy, button, cancelButton, progressBox, progressText, setProgress,
  command, totalBytes, afterDownload, readyToast, warm, label,
}) {
  if (isBusy()) return
  logEvent('INFO', `开始下载模型: ${label}`)
  setBusy(true)
  button.disabled = true
  cancelButton.classList.remove('hidden')
  progressBox.classList.remove('hidden')
  setProgress(0, totalBytes)
  progressText.textContent = t('setup.preparing')
  try {
    state.status = await invoke(command)
    if (afterDownload) await afterDownload()
    progressText.textContent = t('setup.done')
    renderStatus()
    // 和整段模型一样：下载完先把引擎加载起来，第一次录音不用等。
    // 预热失败就别说「已就绪」—— 那会让用户以为可以直接录音。
    // 单独一层 try：加载失败不是「下载失败」，文案不能混。
    try {
      await warm()
      toast(t(readyToast))
    } catch (error) {
      logEvent('ERROR', `模型预热失败: ${errorText(error)}`)
      toast(fmtError(error), 'error')
    }
  } catch (error) {
    // 取消不是失败：文案要分开说 —— 用户点了取消，进度行不能写「下载失败」。
    const cancelled = errorCode(error) === 'setup.cancelled'
    logEvent(cancelled ? 'INFO' : 'ERROR', `模型下载${cancelled ? '取消' : '失败'}: ${errorText(error)}`)
    toast(fmtError(error), cancelled ? 'info' : 'error')
    progressText.textContent = cancelled ? t('setup.cancelled') : t('setup.downloadFailed')
  } finally {
    setBusy(false)
    button.disabled = false
    cancelButton.classList.add('hidden')
  }
}

export async function downloadStreamingModel() {
  await runDownload({
    isBusy: () => state.downloadingStreaming,
    setBusy: (value) => {
      state.downloadingStreaming = value
    },
    button: els.streamDownload,
    cancelButton: els.streamCancel,
    progressBox: els.streamProgress,
    progressText: els.streamProgressText,
    setProgress: setStreamProgress,
    command: 'download_streaming_model',
    totalBytes: state.status?.streamingTotalBytes || 0,
    // 标点模型和流式模型一起下载：标点下失败只是降级为无标点，
    // 所以只有它确实就绪时才自动开启，否则开关别装作打开。
    afterDownload: async () => {
      if (state.status.punctReady) {
        state.status = await invoke('set_punct', { enabled: true })
      }
    },
    readyToast: 'model.badge.streamingReady',
    label: '流式模型',
    // 返回 Promise：加载失败要让 runDownload 知道，不能还弹「已就绪」。
    warm: async () => {
      await invoke('warm_up_streaming')
    },
  })
}

export async function downloadModel() {
  await runDownload({
    isBusy: () => state.downloading,
    setBusy: (value) => {
      state.downloading = value
    },
    button: els.downloadBtn,
    cancelButton: els.cancelDownloadBtn,
    progressBox: els.setupProgress,
    progressText: els.setupText,
    setProgress,
    command: 'download_model',
    totalBytes: state.status?.totalBytes || 0,
    afterDownload: null,
    readyToast: 'model.readyToast',
    label: '整段模型',
    // 返回 Promise：加载失败要让 runDownload 知道，不能还弹「已就绪」。
    warm: () => warmUp(),
  })
}

export async function pickModelDir() {
  const dir = await openDialog({ directory: true, multiple: false, title: t('setup.chooseDir') })
  if (!dir) return
  try {
    state.status = await invoke('set_model_dir', { dir })
    renderStatus()
    if (state.status.modelReady) {
      toast(t('model.ready'))
      void warmUp().catch(() => {})
    } else {
      toast(t('setup.noModel'), 'error')
    }
  } catch (error) {
    toast(fmtError(error), 'error')
  }
}

/// 预热整段引擎。失败时**抛出**，由调用方决定是否提示 ——
/// 静默吞掉会让「下载完成」之后的「已就绪」变成假成功。
export async function warmUp() {
  if (!state.status?.modelReady) return
  // 流式模式用不到整段引擎：下载 / 选目录后不要把它常驻在内存里。
  if (state.status?.mode === 'streaming') return
  await invoke('warm_up')
}
