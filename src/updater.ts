/**
 * 自动更新（GitHub Releases）。
 *
 * 分工：
 * - 「检查」在前端：GitHub API 返回 `access-control-allow-origin: *`，简单 GET
 *   即可跨域读取；用 AbortController 做超时与取消，国内连不上 GitHub 时能快速失败。
 * - 「下载 + 安装」在 Rust：release 资源下载会 302 跳到**没有 CORS 头**的
 *   `release-assets.githubusercontent.com`，前端 `fetch` 读不到字节，只能交给
 *   Rust 命令（见 src-tauri/src/updater.rs）。
 */
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const REPO = "HayBranch/izuna-toolbox";
export const RELEASES_URL = `https://github.com/${REPO}/releases`;
const API_LATEST = `https://api.github.com/repos/${REPO}/releases/latest`;

const AUTO_CHECK_KEY = "izuna.autoUpdateCheck";
const LAST_CHECK_KEY = "izuna.lastUpdateCheck";
/** 启动检查节流：12 小时内不重复请求，缓解 GitHub 未认证 60 次/小时限额。 */
const STARTUP_THROTTLE_MS = 12 * 60 * 60 * 1000;

export type AssetKind = "nsis";

export interface UpdateAsset {
  name: string;
  url: string;
  size: number;
  kind: AssetKind;
}

export interface UpdateInfo {
  currentVersion: string;
  latestVersion: string;
  hasUpdate: boolean;
  notes: string;
  htmlUrl: string;
  assets: UpdateAsset[];
}

export interface UpdateProgress {
  downloaded: number;
  total: number;
}

/** GitHub release 的字段都按外部输入对待，逐项兜底。 */
interface GhAsset {
  name?: string;
  browser_download_url?: string;
  size?: number;
}
interface GhRelease {
  tag_name?: string;
  body?: string;
  html_url?: string;
  assets?: GhAsset[];
}

/** 当前版本：优先 Tauri 运行时版本，`__APP_VERSION__` 兜底。 */
export async function currentVersion(): Promise<string> {
  try {
    return await getVersion();
  } catch {
    return __APP_VERSION__;
  }
}

/** 数值比较 `x.y.z`（去 v 前缀）；a>b 返回 1、a<b 返回 -1、相等 0。 */
export function compareVersions(a: string, b: string): number {
  const pa = a.replace(/^v/i, "").split(".").map((n) => parseInt(n, 10) || 0);
  const pb = b.replace(/^v/i, "").split(".").map((n) => parseInt(n, 10) || 0);
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d > 0) return 1;
    if (d < 0) return -1;
  }
  return 0;
}

/** 整理出可下载的安装包（只认 NSIS 安装包，跳过便携 zip）。 */
function pickAssets(raw: unknown): UpdateAsset[] {
  if (!Array.isArray(raw)) return [];
  const out: UpdateAsset[] = [];
  for (const a of raw as GhAsset[]) {
    const name = typeof a?.name === "string" ? a.name : "";
    const url = typeof a?.browser_download_url === "string" ? a.browser_download_url : "";
    const size = typeof a?.size === "number" ? a.size : 0;
    if (name.toLowerCase().endsWith("-setup.exe") && url) {
      out.push({ name, url, size, kind: "nsis" });
    }
  }
  return out;
}

/**
 * 检查更新。自带 8s 超时（GitHub 连不上时快速失败），并支持外部 signal 取消。
 * 不带任何自定义请求头，避免触发 CORS 预检。
 */
export async function checkForUpdate(
  opts: { signal?: AbortSignal; timeoutMs?: number } = {},
): Promise<UpdateInfo> {
  const { signal, timeoutMs = 8000 } = opts;
  const cur = await currentVersion();

  const ctrl = new AbortController();
  const onAbort = () => ctrl.abort();
  if (signal) {
    if (signal.aborted) ctrl.abort();
    else signal.addEventListener("abort", onAbort, { once: true });
  }
  const timer = setTimeout(
    () => ctrl.abort(new Error("连接 GitHub 超时，请检查网络或稍后重试")),
    timeoutMs,
  );

  try {
    const res = await fetch(API_LATEST, { signal: ctrl.signal });
    if (!res.ok) throw new Error(`GitHub 返回 HTTP ${res.status}`);
    const data = (await res.json()) as GhRelease;
    const latest = String(data?.tag_name ?? "").replace(/^v/i, "").trim();
    return {
      currentVersion: cur,
      latestVersion: latest,
      hasUpdate: latest !== "" && compareVersions(latest, cur) > 0,
      notes: String(data?.body ?? ""),
      htmlUrl: String(data?.html_url ?? "") || RELEASES_URL,
      assets: pickAssets(data?.assets),
    };
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", onAbort);
  }
}

// ---------- 下载线路设置（沿用 localStorage 偏好的惯例） ----------

/**
 * 下载线路：默认走加速镜像（本应用受众以国内为主，GitHub 直连常超时/龟速）。
 * 加速节点实时取自 <https://xiake.pro/github/>——站方按实测延迟排序节点池，
 * 榜单会更新，所以每次打开更新弹窗都拉一次它的 script.js 解析出**前三个**镜像
 * （站点没有 CORS 头，拉取走 Rust 的 `fetch_mirror_list` 命令）。
 * 拉取失败时退回下面的内置兜底线路（写作本日站方榜单前二）。
 */
export interface DownloadLine {
  label: string;
  /** 镜像前缀（以 / 结尾）；空串表示 GitHub 直连。 */
  prefix: string;
}

export const DIRECT_LINE: DownloadLine = { label: "直连 GitHub（不加速）", prefix: "" };

export const FALLBACK_LINES: DownloadLine[] = [
  { label: "加速下载①（git.tangbai.cc）", prefix: "https://git.tangbai.cc/" },
  { label: "加速下载②（gh.927223.xyz）", prefix: "https://gh.927223.xyz/" },
  DIRECT_LINE,
];

/** 榜单里保留的加速镜像数量（其余忽略）。 */
const MAX_MIRRORS = 3;
const LINE_NUMS = ["①", "②", "③", "④", "⑤"];

/** 从 xiake.pro 的 script.js 里解析出 `mirrors` 数组（按站方延迟排序）。 */
export function parseMirrors(js: string): string[] {
  const block = js.match(/mirrors\s*=\s*\[([^\]]*)\]/)?.[1];
  if (!block) return [];
  const urls = block.match(/https:\/\/[^"',\s]+/g) ?? [];
  return [...new Set(urls)]
    .slice(0, MAX_MIRRORS)
    .map((u) => (u.endsWith("/") ? u : `${u}/`));
}

function hostOf(prefix: string): string {
  try {
    return new URL(prefix).host;
  } catch {
    return prefix;
  }
}

export function linesFromMirrors(mirrors: string[]): DownloadLine[] {
  return [
    ...mirrors.slice(0, MAX_MIRRORS).map((prefix, i) => ({
      label: `加速下载${LINE_NUMS[i] ?? i + 1}（${hostOf(prefix)}）`,
      prefix,
    })),
    DIRECT_LINE,
  ];
}

/** 拉取实时榜单线路；失败（站点连不上/格式变了）静默退回内置线路。 */
export async function fetchDownloadLines(): Promise<{
  lines: DownloadLine[];
  live: boolean;
}> {
  try {
    const js = await invoke<string>("fetch_mirror_list");
    const lines = linesFromMirrors(parseMirrors(js));
    // 至少要有一条加速线（只剩直连说明解析失败）
    if (lines.length > 1) return { lines, live: true };
  } catch {
    // 落到兜底
  }
  return { lines: FALLBACK_LINES, live: false };
}

/** 按前缀持久化（榜单是动态的，存下标会漂移；null = 未选过，默认第一条）。 */
const LINE_KEY = "izuna.downloadLinePrefix";

export function getSavedLinePrefix(): string | null {
  const v = localStorage.getItem(LINE_KEY);
  return v === null || v === "" || v.startsWith("http") ? v : null;
}

export function setDownloadLine(prefix: string): void {
  localStorage.setItem(LINE_KEY, prefix);
}

/** 给 GitHub 资源链接套上所选线路的镜像前缀（直连则原样返回）。 */
export function buildDownloadUrl(url: string, prefix: string): string {
  return prefix ? `${prefix}${url}` : url;
}

// ---------- 启动检查设置（沿用 App.tsx 里 localStorage 偏好的惯例） ----------

export function getAutoCheck(): boolean {
  return localStorage.getItem(AUTO_CHECK_KEY) !== "0";
}

export function setAutoCheck(value: boolean): void {
  localStorage.setItem(AUTO_CHECK_KEY, value ? "1" : "0");
}

/** 是否该做启动检查：开关关闭或距上次检查不足 12h 则跳过。 */
export function shouldStartupCheck(): boolean {
  if (!getAutoCheck()) return false;
  const last = Number(localStorage.getItem(LAST_CHECK_KEY) ?? 0);
  return Date.now() - last > STARTUP_THROTTLE_MS;
}

/** 记录一次检查尝试（成功或失败都记），避免连不上 GitHub 时每次启动都重试。 */
export function markChecked(): void {
  localStorage.setItem(LAST_CHECK_KEY, String(Date.now()));
}

// ---------- Rust 命令封装 ----------

export const downloadUpdate = (url: string, fileName: string) =>
  invoke<string>("download_update", { url, fileName });

export const cancelUpdate = () => invoke<void>("cancel_update");

export const installUpdate = (path: string) =>
  invoke<void>("install_update", { path });

/** 开发者选项「测试自动启动」的返回值（token 用于匹配探针事件） */
export interface RelaunchTest {
  token: string;
  seconds: number;
}

/**
 * 开发者选项自检：延迟数秒后带探针参数再启动一份当前程序，验证「新进程能启动并
 * 连回本实例」。不会退出当前程序，也不碰已安装的文件。
 */
export const testRelaunch = () => invoke<RelaunchTest>("test_relaunch");

/** 自检探针进程连回本实例时触发，payload 是 token。 */
export const onRelaunchProbe = (cb: (token: string) => void) =>
  listen<string>("relaunch-probe", (e) => cb(e.payload));

export const onUpdateProgress = (cb: (p: UpdateProgress) => void) =>
  listen<UpdateProgress>("update-progress", (e) => cb(e.payload));
