/**
 * 界面主题（主窗口）：auto 跟随系统 / light 亮色 / dark 暗色。
 *
 * 实现方式：往 <html> 上增删 `dark` 类，Tailwind 的 `dark:` 变体（见 index.css
 * 的 @custom-variant）据此生效。偏好存 localStorage，与「窗口置顶」等惯例一致。
 * main.tsx 在渲染前先应用一次，避免启动瞬间闪一下错误配色的白屏。
 */

export type ThemeMode = "auto" | "light" | "dark";

const THEME_KEY = "izuna.theme";

export function getThemeMode(): ThemeMode {
  const v = localStorage.getItem(THEME_KEY);
  return v === "light" || v === "dark" ? v : "auto";
}

/** 记住用户选择（auto/light/dark）并立即应用，切换按钮无需等 React 重渲染。 */
export function setThemeMode(mode: ThemeMode): void {
  localStorage.setItem(THEME_KEY, mode);
  applyThemeMode(mode);
}

/** 系统当前是否深色外观。 */
export function prefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** 按模式解析出「实际是否深色」并同步到 <html>，返回结果。 */
export function applyThemeMode(mode: ThemeMode = getThemeMode()): boolean {
  const dark = mode === "dark" || (mode === "auto" && prefersDark());
  document.documentElement.classList.toggle("dark", dark);
  return dark;
}
