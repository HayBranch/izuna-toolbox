# 泉奈工具箱 · Izuna Toolbox

轻量 Tauri 2 桌面工具箱，核心功能是**悬浮字幕**：一枚像 QQ 音乐歌词那样的透明小窗，
悬浮在游戏/任意窗口上方，实时显示你配置的战绩数据，并且**能被 OBS 等直播软件窗口捕获**。

**Tauri 2 + React 19 + TypeScript + Vite 7 + TailwindCSS 4 + SQLite**

## 关于

泉奈工具箱（Izuna Toolbox）是一个免费的开源小工具，为直播玩家打造：
把「打了多少场、胜率多少」这类战绩数据做成一枚可被直播软件捕获的悬浮字幕，
所有数据保存在本地 SQLite 数据库里，不联网、不上传。

- 仓库：<https://gitee.com/HayBranch/izuna-toolbox>
- 许可证：[MIT](./LICENSE)

## 功能

### 🧮 变量与计数（每套浮窗独立）

**每个浮窗就是一套完全独立的配置**：自己的账号、变量、计算变量、模板，互不影响。
主窗口「变量与计数」页顶部选择要编辑的套即可。

- **自定义变量**：数量不限、显示名随意起（如"总数""人机场次"），
  引用名（A1、A2…）可自定义也可自动分配，每行有 − / ＋ 按行内步长加减；
- **计算变量**：写表达式引用**本套内**的其它变量自动算出，支持 `+ - * /` 与括号，
  例如 `A1 + A2`、`A4 / (A1 + A2)`；行内实时预览；
- **格式化**：计算变量可选"普通数 / 百分比"，百分比会把 0.8 显示成 80%，
  小数位数 0~3 位可选；
- 内置变量（自动计算，无需维护）：

| 变量 | 含义 | 示例 |
|---|---|---|
| `{today}` | 今日日期 | 20261116 |
| `{date}` / `{time}` | 日期 / 当前时间 | 2026-11-16 / 21:30 |
| `{account}` | 本套账号 | 泉奈 |

- 首次运行自带一套示例：A1 人机场次 / A2 真人场次 / A3 人机胜场 / A4 真人胜场，
  以及 B1 总场次 = `A1 + A2`、B2 总胜场 = `A3 + A4`、
  B3 总胜率 = `B2 / B1`（百分比，1 位小数）、B4 真实胜率 = `A4 / B1`。

### 🪟 悬浮字幕（配置队列，可多开）

- **浮窗配置是一个队列**：可创建任意多个，各自命名，带创建时间等基本信息，
  配置全部持久化，重启后原样恢复；
- **每套浮窗完全独立**：自己的账号、变量、计算变量、模板与样式；
- 样式：字号、字体颜色、加粗、描边阴影、背景颜色、**背景不透明度**（0 = 全透明）
- 主窗口有实时预览，所见即所得
- 每个浮窗可单独设置：
  - **显示 / 隐藏**（隐藏 ≠ 删除，配置保留）
  - **窗口置顶**（浮窗左上角悬停也有 📌 按钮可切换）
  - **鼠标穿透**：开启后浮窗完全不响应鼠标，绝不挡游戏操作（🔒 按钮或主窗口均可开关）
- 直接拖动浮窗调整位置，自动保存；也可在主窗口输入 X/Y
- 浮窗内容尺寸自适应：改模板、改字号后窗口自动贴合文字

### 被 OBS / 直播软件捕获

- 浮窗是普通的无边框透明顶层窗口，OBS「窗口捕获」（WGC / BitBlt）均可抓取
- 若想让捕获画面不带透明度（有些直播软件对透明支持不好），把背景不透明度拉到 100% 即可
- 窗口显示时不抢焦点，不会把游戏切到后台

### 💾 SQLite 持久化

- 数据保存在 `%APPDATA%/com.izuna.toolbox/izuna-toolbox.db`
  （SQLite，`rusqlite` bundled 静态编译，无需安装任何东西）；
- 表结构：`overlays`（浮窗队列，含每套自己的账号）、
  `variables` / `computed`（套内变量，按 `overlay_id` 归属）、`meta`（杂项）；
- 每次变更即提交，退出无需保存；删除数据库文件即恢复出厂；
- 旧版本（JSON 文件、以及全局变量的 v1 SQLite）会自动迁移：
  旧变量名（{botGames} 等）改写为 {A1}…，缺失的计算变量自动补齐。

## 与体积有关的打包设计

老项目（tauri-toolbox）用 `"targets": "all"`（NSIS + MSI 一起打）且默认带 WebView2 引导器，
产物又多又大。本项目全部按"现代设备"精简：

| 措施 | 位置 | 效果 |
|---|---|---|
| 只打 NSIS 安装包 | `tauri.conf.json` → `bundle.targets: ["nsis"]` | 不再产出 MSI/WiX，体积减半 |
| 跳过 WebView2 安装 | `bundle.windows.webviewInstallMode.type: "skip"` | Win10/11 都自带 WebView2，不再打包引导器 |
| SQLite 静态编译 | `Cargo.toml` → `rusqlite` (`bundled`) | 数据库直接编进 exe，约 +1MB，免安装免服务 |
| release 全套瘦身 | `Cargo.toml` → `lto + strip + opt-level "s" + panic abort` | exe 最小化 |

> 兼容性兜底：如果将来要给一台**没有 WebView2** 的旧机器用，把 `webviewInstallMode`
> 改回 `{"type": "downloadBootstrapper"}` 重打即可。

## 快速开始

前置条件：Rust stable、Node.js 20+、Windows 10/11（自带 WebView2）。

```bash
npm install        # 装前端依赖（一次）
npm run tauri dev  # 开发模式：前端热更新 + Rust 自动重编
npm run bundle     # 打包：release exe + NSIS 安装包
```

产物位置：

| 产物 | 位置 |
|---|---|
| 绿色单文件 exe | `src-tauri/target/release/izuna-toolbox.exe` |
| NSIS 安装包 | `src-tauri/target/release/bundle/nsis/` |

## 目录结构

```
src/
├── main.tsx / App.tsx        入口 + 主窗口外壳（浮窗窗口按 label 路由到 OverlayWindow）
├── types.ts                  与 Rust 结构体一一对应的 TS 类型
├── vars.ts                   变量引擎：表达式求值、百分比格式化、模板分词
├── api.ts                    invoke 命令封装
└── components/
    ├── VariablesPage.tsx     变量与计数（自定义变量 + 计算变量）
    ├── OverlaysPage.tsx      悬浮字幕管理（多浮窗配置卡 + 预览）
    ├── OverlayWindow.tsx     浮窗窗口本体（监听配置、自适应尺寸、悬停工具条）
    └── SubtitleView.tsx      字幕渲染（主窗口预览与浮窗共用，所见即所得）
src-tauri/src/
├── lib.rs                    应用装配：启动浮窗、窗口事件、命令注册
├── store.rs                  SQLite 持久化（建表、示例种子、旧 JSON 迁移）
├── commands.rs               全部 #[tauri::command]
├── overlay_win.rs            浮窗窗口生命周期（创建/置顶/穿透/位置/显隐）
└── tray.rs                   系统托盘（关闭 = 最小化，托盘退出）
```

## 数据流

单一真相在 Rust 的 SQLite：主窗口/浮窗的所有修改都走 invoke 命令 → 提交数据库 →
`emit("config-changed")` 广播全量配置 → 各窗口监听事件刷新 UI。
拖动浮窗由 Rust 侧 `Moved` 事件回写位置（节流广播 `overlay-moved`），前端零感知。

## 许可证

[MIT](./LICENSE) © HayBranch
