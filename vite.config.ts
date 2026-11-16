import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri 官方推荐的 Vite 配置要点：
// 1. clearScreen: false —— 不要清屏，避免遮挡 Rust 编译器输出的报错；
// 2. 端口固定为 1420，与 tauri.conf.json 里的 devUrl 对应；
// 3. 不监听 src-tauri 目录，Rust 代码改动由 cargo 自己负责热重载。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
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
