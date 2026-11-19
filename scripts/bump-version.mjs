/**
 * 改版本号：只改根目录 package.json 这一处。
 *
 * 用法：
 *   npm run bump -- 0.0.4
 *   node scripts/bump-version.mjs 0.0.4
 *
 * package.json 的 version 是唯一来源：
 * - `src-tauri/tauri.conf.json` 的 version 指向 ../package.json（安装包/窗口版本跟随）
 * - 前端由 Vite 注入 __APP_VERSION__
 * - 本脚本顺带让 npm 同步 package-lock.json；不会提交、也不会打 tag
 */
import { execFileSync } from "node:child_process";

const version = (process.argv[2] ?? "").replace(/^v/, "");

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error("用法：npm run bump -- 0.0.4");
  process.exit(1);
}

// Windows 上 Node 出于安全加固不允许直接 spawn `.cmd`，必须走 shell。
// 走 shell 时把版本号这类参数一起放进命令串，避免 args 被错误拼接。
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
execFileSync(npm, ["version", version, "--no-git-tag-version"], {
  stdio: "inherit",
  shell: process.platform === "win32",
});

console.log(`\n✅ 版本号已改为 ${version}`);
console.log("   下一步：git commit → git tag v" + version + " → push");
