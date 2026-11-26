# AGENTS.md

仓库约定：注释、提交信息、README、安装包文案一律使用**中文（zh-CN）**。新增面向用户的内容或提交信息时保持这一约定。

## 项目是什么

Tauri 2 桌面应用（优先 Windows）：悬浮字幕浮窗工具，字幕窗口供 OBS 捕获。前端 React 19 + TS + Vite 7 + Tailwind 4；后端 Rust + SQLite（`rusqlite` `bundled`）。单 crate + 单前端，不用 workspace，不用 monorepo。

## 常用命令

```bash
npm install             # 首次安装
npm run tauri dev       # 开发：Vite :1420 + Rust 改动自动重编译
npm run tauri build     # 正式构建（普通 exe + NSIS，输出在 src-tauri/target/release/bundle/）
npm run bundle          # tauri build（普通 exe + NSIS 安装包，输出在 src-tauri/target/release/bundle/nsis/）
npm run bundle:portable # 构建 exe 后用 node scripts/package-portable.mjs 打包到 dist-portable/
npm run bump -- 0.0.5   # 更新根 package.json 版本并同步 package-lock；不提交、不打标签
```

- 仓库里**没有测试、lint、格式化配置**。`npm run build` 会跑 `tsc`（严格模式，`noUnusedLocals`/`noUnusedParameters`），是唯一的类型检查关卡。
- Vite 开发服务器固定在 **1420** 端口（`strictPort`）——该端口必须空闲，`tauri.conf.json` 的 `devUrl` 依赖它。`clearScreen: false` 是有意为之，便于看到 Rust 编译错误；不要改动。
- **验证时不要自建临时编译目录。** 若用户的 `npm run tauri dev` / `cargo run` 正占着 `target/` 导致 `cargo check` 报「拒绝访问 / `.cargo-build-lock`」，直接让用户停掉服务再验证，**绝不**设置 `CARGO_TARGET_DIR=target-check` 之类另起炉灶（会生成数百 MB / 数千文件，清理还需批量确认）。等用户确认服务已停，用默认 `target/` 验证即可。

## 版本号

`package.json` 的 `version` 是**唯一来源**：

- `src-tauri/tauri.conf.json` 用 `"version": "../package.json"`（安装包/exe 版本跟着走）。
- 前端通过 Vite `define.__APP_VERSION__` 读取。
- `src-tauri/Cargo.toml` 的版本只是占位 `0.0.0`——不要动。

发布流程：`npm run bump -- X.Y.Z` → 提交 → `git tag -a vX.Y.Z` → 推送标签。`v*` 标签会触发发布工作流。

## 前置提醒：改代码前先核对版本（agent 每次开工必读）

动手改代码**之前**，先确认「当前 `package.json` 的版本」和「仓库里最新的 `v*` 标签」是否一致：

```bash
git describe --tags --abbrev=0   # 仓库里最新的 v* 标签
git tag -l "v$(node -p "require('./package.json').version")"  # 当前版本号是否已经打过标签
```

**若当前版本号已经存在对应的 `v*` 标签**（说明上一个版本已发版），必须按顺序完成这四步，
**再做具体的功能/修复改动**：

1. **版本号 +1**：`npm run bump -- X.Y.(Z+1)`（`package.json` / `package-lock.json` 是版本唯一来源，
   `tauri.conf.json` 跟着走，`Cargo.toml` 版本占位 `0.0.0` 不要动）；
2. **写 README 升级日志**：在 `README.md` 的「更新日志」小节最上方新增 `### vX.Y.Z`，
   把本版本将要发布的内容按 `feat` / `fix` 用中文列清楚（可先写占位，收尾再补全）；
3. **配置（pack）就绪**：确认 `package.json` 的 `name` / `version` 正确、新增依赖已同步
   `package-lock.json`，需要随包分发的资源已在打包配置里（见「发布流水线」）；
4. **再开始写代码**：功能实现、提交信息（`vX.Y.Z <类型>: <中文描述>`）、以及未完成事项的 README 补全，
   都挂在新版本号下。

**若当前版本号尚未打过标签**（上一个版本还停在「已提交、待打标签」），直接沿用当前版本号写代码即可，
不要重复 +1。判断口径：`git tag -l "v<当前版本>"` 有输出 = 已发版需 +1；无输出 = 直接用。

## 提交信息

- 格式：`vX.Y.Z <类型>: <中文描述>`，例如 `v0.1.1 docs: 将 AGENTS.md 中文化并补充提交信息规范`。
- 版本号 = 最新的 `v*` 标签加一；提交前用 `git describe --tags --abbrev=0` 查最新标签。
- **未经用户明确允许，不要新建或移动 `v*` 标签。** 打标签 / 推标签会触发发布流水线，
  由用户决定何时发版；改完代码、更新 README 后停在「已提交、待打标签」即可，等用户发话再 `git tag`。
- `<类型>` 用 `feat` / `fix` / `docs` / `chore` 等常见类别，描述一律中文。

## 发布流水线（两套，对应不同托管环境）

- `.github/workflows/main.yml` —— Windows runner；NSIS 安装包 + portable zip。主发布路径。
- `.cnb.yml`（CNB，Linux 容器）+ `.workflow/打包.yml` —— Linux 无法产 NSIS，所以用 `--target x86_64-pc-windows-gnu --no-bundle` 交叉编译 exe，并且在 `package-portable.mjs` 之前**必须把 `WebView2Loader.dll`**（从 cargo registry 或 `target/` 中找，仅 x64）拷到 exe 同目录。不要“简化”掉这一步。
- `tauri.conf.json` 的 `bundle.targets` 保持 `["nsis"]`；v0.1.3 起只发布 NSIS 安装包（exe）与便携 zip，不再构建 MSI。

## 架构要点

- **唯一真相是 Rust 的 SQLite。** 前端每次编辑都走 `invoke` 命令 → SQLite 提交 → Rust 通过 `emit("config-changed")` 广播完整配置 → 各窗口重新渲染。不要在前端缓存/本地改配置（乐观状态除外）；没走命令写入的改动会在下次广播时丢失。
- **窗口按 label 路由**，见 `src/App.tsx`：label `main` → 主界面；label `overlay-{id}` → `OverlayWindow`。新浮窗由 Rust（`overlay_win.rs`）按这个 label 规则创建；权限（`src-tauri/capabilities/default.json`）授权给 `["main", "overlay-*"]`——新增 Tauri API 必须补权限项，否则运行时失败。
- **状态注册顺序很重要**：`app.manage(store)` 必须在任何窗口创建之前，否则浮窗首次 `get_config` 会报 "state not managed"。见 `src-tauri/src/lib.rs`。
- 启动配置：`exe 同目录/conf/conf.toml` 决定 `portable`（数据库：`true` → exe 同目录 `data/`，`false` → `%APPDATA%/com.izuna.toolbox/`）和 `log_level`。日志固定写 exe 同目录 `logs/`；旧 `portable.flag` 只在首次生成配置时作为默认值参考。
- 自动更新仅 Windows（`Cargo.toml` 里 `reqwest`/`tokio` 用 `cfg(windows)` 控制）；下载放在 Rust 侧，因为发布资产的 CDN 重定向会阻止 webview 跨域读取。
- **装完自动重启**：NSIS 安装包靠 `src-tauri/windows/hooks.nsh` 的 `NSIS_HOOK_POSTINSTALL`（由 `tauri.conf.json` 的 `bundle.windows.nsis.installerHooks` 引入）在 `/UPDATE` 更新模式下拉起新版程序；v0.1.3 起只发布 NSIS 安装包，因此不再需要外部看门脚本。开发者选项「测试自动启动」会延迟启动一份带 `--relaunch-probe=` 的当前程序，由单实例插件回传参数验证启动链路。
- 初始/种子数据（默认浮窗、变量、计算变量）定义在 `src-tauri/src/store.rs` 顶部（`SEED_VARIABLES` / `SEED_COMPUTED`）——首次运行和清空浮窗后重建都会用到。

## 严格控制体积的构建配置

项目刻意压缩二进制体积：`rusqlite` bundled、`webviewInstallMode: skip`、release profile `lto + strip + opt-level "s" + panic abort`。除非用户明确要求，不要加大依赖或重新启用 WebView2 引导安装器。

## 参考

`README.md`（中文）详细记录功能、数据模型和更新流程，并保持更新——遇到行为疑问先查它，别猜。
