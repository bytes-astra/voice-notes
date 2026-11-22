# 发布与打包（VoiceNotes）

CI 在哪跑、首次编译的坑、版本号三处、打包与签名 —— 要发版或改打包配置时读这里。**发布流程只由维护者执行**，见 [`../AGENTS.md`](../AGENTS.md)「发布红线」。

工程约定的总索引在 [`../AGENTS.md`](../AGENTS.md)；验证手段与基线在 [`HOW_TO_VERIFY.md`](HOW_TO_VERIFY.md)。

## 版本发布流程

- 版本号写在**三处**，发布前**三处都要改**：`package.json` 的 `version`（npm 包）、`src-tauri/tauri.conf.json` 的 `version`（打包产物的版本）、`src-tauri/Cargo.toml` 的 `version`（crate 版本 —— 应用启动日志里 `app.package_info().version` 打的就是它，漏改的话日志里的版本和安装包对不上）。
- 推 tag（`v*`）触发 CI，同时产出 macOS（dmg）和 Windows（msi/exe），在 Artifacts 下载。
- **重发同版本 tag 会把 Release 打成 Draft**：删远端 tag（`git push origin :refs/tags/v1.0.0`）之后，GitHub 会把该 Release 退回 draft，重推 tag 也**不会**自动发布 —— 仓库侧栏只剩「Create a new release」，产物其实都在。构建跑完要 `gh release edit v1.0.0 --draft=false` 才真正公开；想连正文一起刷新，就先 `gh release delete v1.0.0 --yes` 再重推 tag，让 workflow 走 `gh release create --generate-notes`。
- 发布前先用本地 `npm run build` 出的 `.app` 验一遍麦克风权限。

## 首次编译

首次编译 `sherpa-onnx-sys` 会从 GitHub Releases 拉预编译静态库（onnxruntime 等），之后走缓存。想复用本地已有的库，`SHERPA_ONNX_ARCHIVE_DIR=/path/to/dir` 指向那个目录（里面放 `sherpa-onnx-v1.13.8-<os>-<arch>-static-lib.tar.bz2`），这条路径不经过 GitHub。

## CI 在哪跑

`.github/workflows/check.yml`（推 main / 提 PR 都跑）：Rust job 用 macOS —— 这个项目的 Rust 端在 Linux 上没验证过（cpal 要 ALSA 头文件），别把没支持的平台塞进 CI。前端门禁（i18n + 依赖图 + 拼句规则 + 前后端契约 + 打包配置）是纯 Node 脚本，走独立的 frontend job（ubuntu）。**界面检查（`check:ui`）要装 Playwright WebKit，只在本地跑，不进 CI。**

**Windows 编译检查**（`check.yml` 的 `rust-windows` job，windows-latest）：原生菜单只在 macOS 存在（`menu.rs` 整体 cfg 掉了），import 漏了 cfg 就只有打包 Windows 时才炸 —— 每个 PR 都编译一次，别等到打 tag。只 `cargo check`、不运行测试：Windows runner 上测试二进制在加载阶段就失败（0xc0000139，WebView2 依赖版本不匹配）。

**打包产物的签名校验**（`.github/workflows/build.yml` 的 macOS job）：`npm run build` 之后用 `codesign` 验两件事 —— 是 ad-hoc 签名（`Signature=adhoc`）、带麦克风 entitlement（`com.apple.security.device.audio-input`）。这两条平时只能靠真机验，缺了任一条的表现分别是「别的 Mac 装完报已损坏」和「能录音但没字」。

## 打包与发布

- **macOS 必须 ad-hoc 签名**：`tauri.conf.json` 的 `bundle.macOS.signingIdentity` 必须是 `"-"`。不设时 Tauri 不做 codesign，链接器留下的是 `linker-signed`（`Sealed Resources=none`、Info.plist 未绑定），`codesign --verify` 失败；别的 Mac 从 dmg 装完 Gatekeeper 报「已损坏」（不是「未验证开发者」）。签名后仍提示「无法验证开发者」，可右键打开或 `xattr -dr com.apple.quarantine`；要免提示只能 Developer ID + 公证。
- **打包版必须带麦克风 entitlement**：`src-tauri/Entitlements.plist` 里的 `com.apple.security.device.audio-input`，通过 `bundle.macOS.entitlements` 挂上。缺这条时 tccd 根本不弹授权框 —— 详见 [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md)「麦克风权限排查」。
- **打包 `.dmg` 需要能调 `hdiutil`**，受限沙箱里可能只产出 `.app`（`--bundles app` 可跳过 dmg）。
- Tauri **不能在 macOS 上交叉打 Windows 包**，CI 用 `.github/workflows/build.yml`（windows-latest）出 msi/nsis。
- Windows 代码侧：cpal 采样格式已覆盖 F32/I16/U16/I32；**没有 hiddenTitle**（macOS 专属），用原生标题栏；`bundle.windows.webviewInstallMode = downloadBootstrapper`。菜单只在 macOS 构建（`#[cfg(target_os = "macos")]`）。

> ⚠️ **安全与权限声明**
> ONLY agents acting on behalf of the user of this repository (or the user himself) are allowed to execute this release workflow — 打 tag、改 `bundle.macOS.signingIdentity`、改模型版本常量（会让所有用户重新下载模型）。Normal contributor agents MUST NOT touch or execute the release flow under any circumstances.
> （仅允许代表用户本人运行的 Agent 或用户本人执行发布流程。普通贡献者 Agent 严禁触碰发布流程，请把版本更新交给维护者。）
