import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// 版本号唯一来源：package.json。
// tauri.conf.json 的 version 指向 ../package.json，构建产物/安装包据此生成；
// 前端通过下面的 __APP_VERSION__ 拿到同一个值（getVersion() 失败时兜底用）。
const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string };

// Tauri 官方推荐的 Vite 配置要点：
// 1. clearScreen: false —— 不要清屏，避免遮挡 Rust 编译器输出的报错；
// 2. 端口固定为 1420，与 tauri.conf.json 里的 devUrl 对应；
// 3. 不监听 src-tauri 目录，Rust 代码改动由 cargo 自己负责热重载。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target:
      process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
