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

export type AssetKind = "nsis" | "msi";

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

/** 整理出可下载的安装包（跳过便携 zip）：NSIS 优先、MSI 次之。 */
function pickAssets(raw: unknown): UpdateAsset[] {
  if (!Array.isArray(raw)) return [];
  const out: UpdateAsset[] = [];
  for (const a of raw as GhAsset[]) {
    const name = typeof a?.name === "string" ? a.name : "";
    const url = typeof a?.browser_download_url === "string" ? a.browser_download_url : "";
    const size = typeof a?.size === "number" ? a.size : 0;
    const lower = name.toLowerCase();
    const kind: AssetKind | null = lower.endsWith(".msi")
      ? "msi"
      : lower.endsWith("-setup.exe")
        ? "nsis"
        : null;
    if (kind && url) out.push({ name, url, size, kind });
  }
  return out.sort((x, y) => (x.kind === y.kind ? 0 : x.kind === "nsis" ? -1 : 1));
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

export const onUpdateProgress = (cb: (p: UpdateProgress) => void) =>
  listen<UpdateProgress>("update-progress", (e) => cb(e.payload));
