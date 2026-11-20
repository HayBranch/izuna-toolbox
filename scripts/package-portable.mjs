/**
 * 打包 Windows 便携版（绿色版）：
 *
 * 1. 读取 `tauri build --no-bundle` 产出的 release exe；
 * 2. 组装 `dist-portable/Izuna-Toolbox-<版本>/`：
 *    - `Izuna Toolbox.exe` 主程序
 *    - `portable.flag`    便携模式标记（Rust 侧据此把数据写到 exe 同目录的 data/）
 *    - `WebView2Loader.dll` 仅 windows-gnu 交叉编译产物需要（见下方说明）
 *    - `data/`            SQLite 数据库目录（首次运行自动建库）
 *    - `README.txt`       使用说明
 * 3. 用系统自带能力压缩成 `dist-portable/Izuna-Toolbox-<版本>-portable-x64.zip`。
 *
 * 用法：
 *   npm run bundle:portable   # 先构建 exe，再打包便携 zip（推荐）
 *   npm run portable          # 复用 src-tauri/target/release 里已构建的 exe，只打包
 *
 * GitHub Actions 里在 tauri-action 构建完成后直接运行本脚本即可。
 */
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const exePath = path.join(root, "src-tauri", "target", "release", "izuna-toolbox.exe");

if (!existsSync(exePath)) {
  console.error(`未找到构建产物：${exePath}`);
  console.error("请先运行：npm run bundle:portable（或 npm run tauri -- build --no-bundle）");
  process.exit(1);
}

const pkg = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8"));
const version = pkg.version ?? "0.0.0";
const outRoot = path.join(root, "dist-portable");
const appDirName = `Izuna-Toolbox-${version}`;
const appDir = path.join(outRoot, appDirName);
const zipPath = path.join(outRoot, `${appDirName}-portable-x64.zip`);

// 每次重新生成，避免把上一次的残留文件带进包里
rmSync(appDir, { recursive: true, force: true });
rmSync(zipPath, { force: true });
mkdirSync(path.join(appDir, "data"), { recursive: true });

copyFileSync(exePath, path.join(appDir, "Izuna Toolbox.exe"));

// webview2-com-sys 在 MSVC 目标上把 WebView2Loader 静态链接进 exe（GitHub Actions
// 的 windows-latest 属于这种），但在 x86_64-pc-windows-gnu 上只链接导入库，运行时
// 要求 WebView2Loader.dll 与 exe 同目录（CNB 的 Linux 交叉编译属于这种）。
// 用 PE 导入表里的 DLL 名判断是否需要附带，避免给 MSVC 产物塞多余文件。
const needsLoader = readFileSync(exePath).includes(Buffer.from("WebView2Loader.dll"));
if (needsLoader) {
  const candidates = [
    process.env.WEBVIEW2_LOADER_DLL,
    path.join(path.dirname(exePath), "WebView2Loader.dll"),
  ].filter(Boolean);

  // webview2-com-sys 的 build.rs 会把 SDK 的 loader 复制进 OUT_DIR/<arch>/
  const buildRoots = [
    path.join(root, "src-tauri", "target", "x86_64-pc-windows-gnu", "release", "build"),
    path.join(root, "src-tauri", "target", "release", "build"),
  ];
  for (const buildRoot of buildRoots) {
    if (!existsSync(buildRoot)) continue;
    for (const dir of readdirSync(buildRoot)) {
      if (dir.startsWith("webview2-com-sys-")) {
        candidates.push(path.join(buildRoot, dir, "out", "x64", "WebView2Loader.dll"));
      }
    }
  }

  const loaderDll = candidates.find((p) => existsSync(p));
  if (!loaderDll) {
    console.error("此 exe 动态依赖 WebView2Loader.dll，但未找到该 DLL。");
    console.error("可设置环境变量 WEBVIEW2_LOADER_DLL 指向 WebView2 SDK 中的 x64/WebView2Loader.dll。");
    process.exit(1);
  }
  copyFileSync(loaderDll, path.join(appDir, "WebView2Loader.dll"));
  console.log(`附带 WebView2Loader.dll：${loaderDll}`);
}

// 便携模式标记：只要它和 exe 同目录，程序就把数据库放到同目录的 data/ 下。
writeFileSync(
  path.join(appDir, "portable.flag"),
  [
    "Izuna Toolbox portable mode marker.",
    "Keep this file next to the executable to store data in .\\data.",
    "Delete it to fall back to %APPDATA%\\com.izuna.toolbox.",
    "",
  ].join("\r\n"),
  "utf8",
);

writeFileSync(
  path.join(appDir, "README.txt"),
  [
    `泉奈工具箱（Izuna Toolbox）便携版 v${version}`,
    "==============================================",
    "",
    "1. 双击「Izuna Toolbox.exe」运行。",
    "   需要 Windows 10/11 自带的 WebView2 运行时（系统一般已包含）。",
    "",
    "2. 所有数据都保存在本文件夹的 data\\izuna-toolbox.db，",
    "   删除它即可恢复出厂；整个文件夹可以随意拷贝/移动到 U 盘或其它电脑。",
    "",
    "3. 请不要删除 portable.flag：它是便携模式标记。",
    "   删除后程序会改用系统目录 %APPDATA%\\com.izuna.toolbox 保存数据。",
    "",
    "4. 安装版（NSIS 安装包）不受影响，仍然使用系统 %APPDATA% 目录。",
    "",
    ...(needsLoader
      ? [
          "5. 请不要删除 WebView2Loader.dll：本包是 MinGW 交叉编译产物，",
          "   运行时需要它和 exe 放在同一目录（安装版/Microsoft 原生构建版不需要）。",
          "",
        ]
      : []),
  ].join("\r\n"),
  "utf8",
);

// 压缩：Windows 用系统自带的 Compress-Archive（无需额外依赖），其它平台用 zip。
if (process.platform === "win32") {
  const quote = (s) => `'${s.replace(/'/g, "''")}'`;
  execFileSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      `Compress-Archive -LiteralPath ${quote(appDir)} -DestinationPath ${quote(zipPath)} -Force`,
    ],
    { stdio: "inherit" },
  );
} else {
  execFileSync("zip", ["-r", zipPath, appDirName], { cwd: outRoot, stdio: "inherit" });
}

console.log(`\n✅ 便携版目录：${appDir}`);
console.log(`✅ 便携版压缩包：${zipPath}`);
