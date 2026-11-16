/** #RRGGBB + 不透明度(0-100) → rgba() 颜色值 */
export function hexToRgba(hex: string, opacityPercent: number): string {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return `rgba(0, 0, 0, ${opacityPercent / 100})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${opacityPercent / 100})`;
}

/** 深色描边阴影（直播低画质压缩下比纯色描边更稳） */
export const TEXT_SHADOW = "0 1px 3px rgba(0,0,0,0.9), 0 0 6px rgba(0,0,0,0.6)";

/** 毫秒时间戳 → "2026-11-16 21:30" */
export function formatDateTime(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
