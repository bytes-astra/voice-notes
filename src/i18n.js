// 文案集中管理：加一种语言 = 加一个字典，不用改业务代码。
//
// 用法：
//   JS  ：t('toast.copied')  /  t('result.chars', { count })
//   HTML：<span data-i18n="toast.copied"></span>，启动时 applyI18n() 自动填充
//         data-i18n-title / data-i18n-placeholder / data-i18n-aria 同理
;(function () {
  const LOCALES = {
    'zh-CN': {
      'app.title': '语音转文字',
      'app.brand': '语音转文字',
      'app.startFailed': '启动失败：{error}',

      'theme.toDark': '切换到深色',
      'theme.toLight': '切换到浅色',
      'nav.new': '开始新转写',
      'nav.transcribe': '转写',
      'nav.history': '历史',
      'nav.settings': '设置',
      'nav.label': '主导航',

      'view.transcribe': '转写',
      'view.history': '历史',
      'view.settings': '设置',

      'lang.auto': '自动检测',
      'lang.zh': '中文',
      'lang.en': 'English',
      'lang.group': '识别语言',
      'model.source.app_data': '应用数据',
      'model.source.custom': '自定义目录',
      'lang.yue': '粤语',
      'lang.ja': '日本語',
      'lang.ko': '한국어',

      'setup.title': '首次使用：下载整段模型',
      'setup.desc':
        'SenseVoiceSmall INT8 约 228 MB，只需下载一次，之后完全离线运行。默认从 Hugging Face 下载，失败时自动切到 hf-mirror；也可以直接指向一个已有的本地模型目录。',
      'setup.download': '下载整段模型',
      'setup.useExisting': '使用已有模型目录',
      'setup.chooseDir': '选择包含 model.int8.onnx 和 tokens.txt 的目录',
      'setup.noModel': '这个目录里没有找到 model.int8.onnx / tokens.txt',
      'setup.preparing': '准备中…',
      'setup.done': '下载完成',
      'setup.downloadFailed': '下载失败：{error}',
      'setup.cached': '已缓存的模型：{done} / {total}',

      'record.idle': '点击麦克风开始录音',
      'record.section': '转写区',
      'record.preparing': '正在准备麦克风…（首次会弹系统授权框）',
      'record.recording': '正在录音，点方块或空格结束',
      'record.transcribing': '正在转写…',
      'record.transcribingFirst': '正在转写…（第一次会先加载模型）',
      'record.loadingStreaming': '正在加载流式模型…',
      'record.live': '实时转写中…',
      'record.empty': '没有识别到内容，换个安静点的环境再试试',
      'record.micStart': '开始录音',
      'record.micStop': '结束录音',
      'record.hint': '空格 开始 / 结束 · Esc 取消 · 也可以把音频文件拖进窗口',
      'record.device': '设备',
      'record.micDenied': '没有麦克风权限：{error}',
      'record.deviceDefault': '系统默认',
      'record.deviceSystemDefault': '系统默认',
      'record.deviceChanged': '已切换输入设备：{name}',
      'record.deviceChangedDefault': '已改用系统默认输入设备',

      'mode.offline': '整段',
      'mode.group': '转写模式',
      'mode.offlineDesc': '录完再转 · 带标点',
      'mode.streaming': '流式',
      'mode.streamingDesc': '实时出字 · 仅中文',
      'mode.streamingDownload': '下载流式模型',
      'mode.streamingMissing': '流式模型（中文，约 {size}）还没下载',
      'mode.streamingOnlyZh': '流式模式只支持中文',
      'mode.langSwitched': '流式模式只支持中文，语言已切到自动',
      'mode.streamingDownloadFirst': '先下载流式模型再使用',
      'mode.switchedBack': '流式模型只支持中文，已切回整段模式',

      'model.badge.streamingReady': '流式模型已就绪',
      'model.badge.streamingMissing': '流式模型未下载',
      'model.badge.offlineMissing': '整段模型未下载',
      'model.badge.error': '{name}加载失败',
      'model.badge.loading': '正在加载{name}…',
      'model.badge.ready': '模型已就绪',
      'model.badge.checking': '正在检查模型…',
      'model.label.streaming': '流式模型',
      'model.label.offline': '整段模型',
      'model.name.streaming': 'Zipformer · 仅中文',
      'model.name.offline': 'SenseVoice · 多语言',
      'model.streamingMissing': '流式模型还没下载',
      'model.offlineMissing': '整段模型还没下载，先点上面的「下载模型」',
      'model.needStreaming': '需要先下载流式模型',
      'model.needOffline': '需要先下载整段模型',
      'model.ready': '模型已就绪',
      'model.readyToast': '整段模型已就绪，可以开始录音了',

      'result.title': '转写结果',
      'result.copy': '复制',
      'result.save': '保存',
      'result.clear': '清空',
      'result.waiting': '等待转写',
      'result.placeholder': '转写结果会出现在这里，可以直接编辑',
      'result.pickFile': '转写音频 / 视频文件…',
      'result.audio': '音频 {duration}',
      'result.elapsed': '转写 {seconds} 秒',
      'result.rtf': '实时率 {rtf}×',
      'result.chars': '{count} 字',
      'result.corrected': '已用整段模型复核',
      'result.done': '转写完成',
      'result.none': '没有识别到语音',
      'result.noCopy': '没有可复制的文字',
      'result.copied': '已复制到剪贴板',
      'result.copyFailed': '复制失败',
      'result.noSave': '没有可保存的文字',
      'result.saved': '已保存：{path}',
      'result.readStatusFailed': '读取状态失败：{error}',
      'result.fileDecode': '正在解码 {name}…',
      'result.fileTranscribe': '正在转写 {name}…',
      'result.fileDone': '已转写 {name}',
      'result.fileFailed': '无法转写这个文件：{error}',
      'result.fileNeedModel': '整段模型还没下载，先下载模型再转写文件',

      'history.title': '历史记录',
      'history.empty': '还没有记录，录一段试试。',
      'history.clear': '清空',
      'history.load': '载入到结果框',
      'history.from': '来自历史 · {when}',
      'history.copy': '复制',
      'history.save': '保存',
      'history.delete': '删除',
      'history.filename': '{head}-{stamp}',

      'settings.interface': '界面语言',
      'settings.localeSystem': '跟随系统',
      'settings.localeZh': '简体中文',
      'settings.localeEn': 'English',
      'settings.model': '整段模型',
      'settings.modelDownload': '下载 / 更新模型',
      'settings.chooseDir': '选择目录…',
      'settings.useAppModel': '使用应用内模型',
      'settings.output': '保存目录',
      'settings.chooseOutputTitle': '选择保存目录',
      'settings.openFolder': '打开文件夹',
      'settings.outputChanged': '保存目录已更新',
      'settings.recognition': '识别',
      'settings.itn': '数字与标点（ITN）',
      'settings.warn': '异常提醒',
      'settings.warnToggle': '识别结果明显异常时提醒我',
      'settings.warnReady':
        '结果明显异常时（重复字、空结果、字数和时长对不上）提醒你一次；不会自动改写已经识别出来的内容',
      'settings.warnOn': '结果异常时会提醒你',
      'settings.warnOff': '已关闭异常提醒',
      'result.qualityWarn': '识别结果可能有问题（内容异常），建议切到整段模式重录一次',
      'settings.punct': '流式标点',
      'settings.punctToggle': '流式结果自动加标点',
      'settings.punctReady': '标点模型已就绪（CT-Transformer zh-en），流式结果会自动加标点',
      'settings.punctMissing': '标点模型未下载（约 {size}），装上后流式结果会自动加标点',
      'settings.punctDownload': '下载标点模型',
      'settings.punctNeedDownload': '先下载标点模型（约 281 MB）',
      'settings.punctEnabled': '标点模型已就绪，已开启自动标点',
      'settings.punctAutoOn': '流式结果会自动加标点',
      'settings.punctAutoOff': '已关闭自动标点',
      'settings.modelPath': '{source} · {dir}',
      'settings.modelNotReady': '未就绪 · 需要下载 {size}',
      'settings.version': 'VoiceNotes {version} · SenseVoiceSmall INT8',
      'settings.switchedAppModel': '已切换为应用内模型目录',
      'settings.switchedAppModelNeedDownload': '已切换为应用内模型目录，需要先下载模型',

      'progress.downloading': '下载中',
      'progress.verifying': '校验中',
      'progress.connecting': '连接中',
      'progress.complete': '完成',
      'progress.cache': '本地缓存',
      'progress.punct': '标点模型下载中 {pct}% · {done} / {total}',

      'duration.seconds': '{seconds} 秒',
      'duration.minutes': '{minutes} 分 {seconds} 秒',

      'common.cancel': '取消',
      'drop.hint': '松开开始转写音频',
      'toast.recordingBusy': '先结束当前录音',
    },
  }

  // 外部字典（src/locales/*.js）合并进来，加语言不用改这个文件
  for (const [locale, dict] of Object.entries(window.I18N_LOCALES || {})) {
    LOCALES[locale] = { ...(LOCALES[locale] || {}), ...dict }
  }

  let current = 'zh-CN'

  function t(key, vars) {
    const dict = LOCALES[current] || {}
    let text = dict[key]
    if (text === undefined) return key
    if (vars) {
      for (const [name, value] of Object.entries(vars)) {
        text = text.split(`{${name}}`).join(String(value))
      }
    }
    return text
  }

  function apply(root) {
    const scope = root || document
    for (const el of scope.querySelectorAll('[data-i18n]')) {
      el.textContent = t(el.dataset.i18n)
    }
    for (const el of scope.querySelectorAll('[data-i18n-title]')) {
      el.title = t(el.dataset.i18nTitle)
    }
    for (const el of scope.querySelectorAll('[data-i18n-placeholder]')) {
      el.placeholder = t(el.dataset.i18nPlaceholder)
    }
    for (const el of scope.querySelectorAll('[data-i18n-aria]')) {
      el.setAttribute('aria-label', t(el.dataset.i18nAria))
    }
  }

  /// 'system' 表示跟随系统语言。
  function resolveLocale(preference) {
    if (preference && LOCALES[preference]) return preference
    const wanted = (navigator.language || 'zh-CN').toLowerCase()
    if (wanted.startsWith('zh')) return 'zh-CN'
    return Object.keys(LOCALES).find((locale) => locale !== 'zh-CN') || 'zh-CN'
  }

  function setLocale(locale) {
    const resolved = resolveLocale(locale)
    if (!LOCALES[resolved]) return false
    current = resolved
    document.documentElement.lang = resolved
    apply()
    return true
  }

  window.I18N = {
    t,
    apply,
    setLocale,
    locales: LOCALES,
    get locale() {
      return current
    },
  }
})()
