/**
 * 界面主题（主窗口）：auto 跟随系统 / light 亮色 / dark 暗色。
 *
 * 偏好现在保存在 exe 同目录 `conf/conf.toml` 的 `theme` 项（由 Rust 设置命令读写），
 * 不再使用 WebView localStorage。这里只负责把模式解析成 <html> 上的 `dark` 类，
 * Tailwind 的 `dark:` 变体（见 index.css 的 @custom-variant）据此生效。
 * main.tsx 在渲染前先按系统外观应用一次，避免启动瞬间闪一下错误配色。
 */

import type { ThemeMode } from "./types";

export type { ThemeMode } from "./types";

/** 系统当前是否深色外观。 */
export function prefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** 按模式解析出「实际是否深色」并同步到 <html>，返回结果。 */
export function applyThemeMode(mode: ThemeMode = "auto"): boolean {
  const dark = mode === "dark" || (mode === "auto" && prefersDark());
  document.documentElement.classList.toggle("dark", dark);
  return dark;
}
