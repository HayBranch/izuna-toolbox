/**
 * 「检查更新」弹窗：检查 → 下载（带进度/取消）→ 安装并退出，外加启动检查开关。
 * 由 MainShell 条件渲染（挂载即自动检查一次），卸载时中止未完成的请求/下载。
 */
import { useEffect, useRef, useState } from "react";
import { openUrl } from "../api";
import {
    buildDownloadUrl,
    cancelUpdate,
    checkForUpdate,
    downloadUpdate,
    fetchDownloadLines,
    FALLBACK_LINES,
    getAutoCheck,
    getSavedLinePrefix,
    installUpdate,
    onUpdateProgress,
    RELEASES_URL,
    setAutoCheck,
    setDownloadLine,
    type DownloadLine,
    type UpdateInfo,
} from "../updater";

type Phase = "checking" | "up-to-date" | "available" | "downloading" | "ready" | "error";

const BTN_PRIMARY =
  "flex-1 rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400";
const BTN_SECONDARY =
  "flex-1 rounded-xl border border-slate-700 bg-slate-900/60 px-4 py-2 text-sm font-medium text-slate-300 transition hover:border-slate-500 hover:text-slate-100";
const BTN_DANGER =
  "flex-1 rounded-xl border border-rose-500/50 bg-rose-950/40 px-4 py-2 text-sm font-medium text-rose-300 transition hover:bg-rose-500/20";

function errorMessage(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  return String(e) || "未知错误";
}

export default function UpdateDialog({ onClose }: { onClose: () => void }) {
  const alive = useRef(true);
  const checkAbort = useRef<AbortController | null>(null);
  const unlisten = useRef<(() => void) | null>(null);

  const [phase, setPhase] = useState<Phase>("checking");
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [assetIndex, setAssetIndex] = useState(0);
  // 下载线路：先渲染内置兜底，再异步换上 xiake.pro 实时榜单（前三镜像 + 直连）
  const [lines, setLines] = useState<DownloadLine[]>(FALLBACK_LINES);
  const [linesLive, setLinesLive] = useState(false);
  const [linePrefix, setLinePrefix] = useState<string | null>(getSavedLinePrefix);
  const activeLine = lines.find((l) => l.prefix === linePrefix) ?? lines[0];
  const [progress, setProgress] = useState({ downloaded: 0, total: 0 });
  const [installedPath, setInstalledPath] = useState("");
  const [autoCheck, setAutoCheckState] = useState(getAutoCheck);

  const runCheck = async () => {
    checkAbort.current?.abort();
    const ctrl = new AbortController();
    checkAbort.current = ctrl;
    setPhase("checking");
    setError("");
    setNotice("");
    try {
      const r = await checkForUpdate({ signal: ctrl.signal });
      if (!alive.current) return;
      setInfo(r);
      setAssetIndex(0);
      setPhase(r.hasUpdate ? "available" : "up-to-date");
    } catch (e) {
      if (!alive.current) return;
      setError(errorMessage(e));
      setPhase("error");
    }
  };

  const runDownload = async () => {
    if (!info) return;
    const asset = info.assets[assetIndex] ?? info.assets[0];
    if (!asset) {
      setError("该版本没有可用的安装包，请前往发布页手动下载。");
      setPhase("error");
      return;
    }
    setPhase("downloading");
    setProgress({ downloaded: 0, total: asset.size });
    setError("");
    setNotice("");
    unlisten.current?.();
    try {
      unlisten.current = await onUpdateProgress((p) => {
        if (alive.current) {
          setProgress({ downloaded: p.downloaded, total: p.total || asset.size });
        }
      });
    } catch {
      // 进度监听注册失败不影响下载本身
    }
    try {
      const path = await downloadUpdate(buildDownloadUrl(asset.url, activeLine.prefix), asset.name);
      if (!alive.current) return;
      setInstalledPath(path);
      setPhase("ready");
    } catch (e) {
      if (!alive.current) return;
      const msg = errorMessage(e);
      if (msg.includes("取消")) {
        setNotice("已取消下载");
        setPhase("available");
      } else {
        setError(msg);
        setPhase("error");
      }
    } finally {
      unlisten.current?.();
      unlisten.current = null;
    }
  };

  const runCancelDownload = () => {
    void cancelUpdate().catch(() => {});
  };

  const runInstall = () => {
    if (!installedPath) return;
    void installUpdate(installedPath).catch((e) => {
      if (!alive.current) return;
      setError(errorMessage(e));
      setPhase("error");
    });
  };

  const toggleAutoCheck = () => {
    const next = !autoCheck;
    setAutoCheckState(next);
    setAutoCheck(next);
  };

  const chooseLine = (prefix: string) => {
    setLinePrefix(prefix);
    setDownloadLine(prefix);
  };

  /** 关闭时若正在下载，先置取消位，避免留下孤儿下载。 */
  const requestClose = () => {
    if (phase === "downloading") runCancelDownload();
    onClose();
  };

  useEffect(() => {
    alive.current = true;
    void runCheck();
    // 检查版本的同时拉一份实时加速节点榜（失败静默，保持内置线路）
    fetchDownloadLines().then((r) => {
      if (!alive.current) return;
      setLines(r.lines);
      setLinesLive(r.live);
    });
    return () => {
      alive.current = false;
      checkAbort.current?.abort();
      unlisten.current?.();
    };
  }, []);

  const selected = info?.assets[assetIndex] ?? info?.assets[0];
  const pct =
    progress.total > 0
      ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100))
      : 0;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6"
      onClick={requestClose}
    >
      <div
        className="w-full max-w-md rounded-2xl border border-slate-700 bg-slate-900 p-6 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h3 className="text-base font-semibold text-slate-100">检查更新</h3>
          <button
            onClick={requestClose}
            aria-label="关闭"
            className="text-slate-500 transition hover:text-slate-200"
          >
            ✕
          </button>
        </div>

        <div className="mt-4 min-h-[7.5rem] space-y-3 text-sm">
          {phase === "checking" && (
            <div className="flex items-center gap-3 text-slate-400">
              <span className="h-4 w-4 animate-spin rounded-full border-2 border-slate-600 border-t-indigo-400" />
              正在连接 GitHub 检查更新…
            </div>
          )}

          {phase === "up-to-date" && info && (
            <div>
              <p className="font-medium text-emerald-400">✓ 当前已是最新版本</p>
              <p className="mt-1 text-xs text-slate-500">
                当前版本 v{info.currentVersion} · GitHub 最新 v{info.latestVersion || "—"}
              </p>
            </div>
          )}

          {phase === "available" && info && (
            <div className="space-y-3">
              <div>
                <p className="font-medium text-indigo-300">发现新版本 v{info.latestVersion}</p>
                <p className="mt-1 text-xs text-slate-500">当前版本 v{info.currentVersion}</p>
              </div>
              {notice && <p className="text-xs text-amber-400">{notice}</p>}
              {info.notes && (
                <div className="max-h-28 overflow-y-auto whitespace-pre-wrap rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs leading-5 text-slate-400">
                  {info.notes}
                </div>
              )}
              {info.assets.length > 1 && (
                <select
                  value={assetIndex}
                  onChange={(e) => setAssetIndex(Number(e.target.value))}
                  className="w-full rounded-lg border border-slate-700 bg-slate-950 px-2 py-1.5 text-xs text-slate-300"
                >
                  {info.assets.map((a, i) => (
                    <option key={a.url} value={i}>
                      {a.kind === "msi" ? "MSI 安装包" : "NSIS 安装包（推荐）"} · {a.name}
                    </option>
                  ))}
                </select>
              )}
              {selected ? (
                <p className="truncate text-xs text-slate-500">
                  安装包：{selected.name}
                  {selected.size ? ` · ${(selected.size / 1048576).toFixed(1)} MB` : ""}
                </p>
              ) : (
                <p className="text-xs text-amber-400">该版本暂无安装包，请打开发布页手动下载。</p>
              )}
              <label className="block space-y-1">
                <span className="text-xs text-slate-500">
                  下载线路（国内推荐加速）
                  {linesLive ? " · 节点榜已更新" : ""}
                </span>
                <select
                  value={activeLine.prefix}
                  onChange={(e) => chooseLine(e.target.value)}
                  className="w-full rounded-lg border border-slate-700 bg-slate-950 px-2 py-1.5 text-xs text-slate-300"
                >
                  {lines.map((l) => (
                    <option key={l.prefix} value={l.prefix}>
                      {l.label}
                    </option>
                  ))}
                </select>
                {!linesLive && (
                  <span className="block text-[11px] text-slate-600">
                    实时节点榜获取失败，当前为内置线路；下载失败可换线路或直连重试。
                  </span>
                )}
              </label>
            </div>
          )}

          {phase === "downloading" && (
            <div className="space-y-2">
              <p className="text-slate-300">
                正在下载安装包…
                <span className="ml-1 text-xs text-slate-500">（{activeLine.label}）</span>
              </p>
              <div className="h-2 w-full overflow-hidden rounded-full bg-slate-800">
                <div
                  className={`h-full rounded-full bg-indigo-500 transition-all ${
                    progress.total > 0 ? "" : "w-full animate-pulse"
                  }`}
                  style={progress.total > 0 ? { width: `${pct}%` } : undefined}
                />
              </div>
              <p className="text-xs text-slate-500">
                {progress.total > 0
                  ? `${pct}% · ${(progress.downloaded / 1048576).toFixed(1)} / ${(progress.total / 1048576).toFixed(1)} MB`
                  : `已下载 ${(progress.downloaded / 1048576).toFixed(1)} MB`}
              </p>
              <p className="text-xs text-slate-600">下载可能较慢，可随时取消。</p>
            </div>
          )}

          {phase === "ready" && (
            <div>
              <p className="font-medium text-emerald-400">✓ 下载完成</p>
              <p className="mt-1 text-xs leading-5 text-slate-500">
                点击「立即安装」将原地替换安装并退出本程序（桌面图标及其位置保持不变），
                安装进度窗口会自动关闭，完成后请重新打开。
              </p>
            </div>
          )}

          {phase === "error" && (
            <div className="space-y-1">
              <p className="font-medium text-rose-400">检查 / 下载失败</p>
              <p className="break-all text-xs leading-5 text-slate-400">{error}</p>
              {activeLine.prefix && (
                <p className="text-xs text-amber-400">
                  当前线路「{activeLine.label}」可能失效，回「检查更新」换一条线路或直连重试。
                </p>
              )}
              <p className="text-xs text-slate-600">
                GitHub 在国内可能连不上，可打开发布页手动下载。
              </p>
            </div>
          )}
        </div>

        <div className="mt-5 flex gap-2">
          {phase === "checking" && (
            <button
              onClick={() => {
                checkAbort.current?.abort();
                onClose();
              }}
              className={BTN_SECONDARY}
            >
              取消
            </button>
          )}
          {phase === "available" && selected && (
            <button onClick={() => void runDownload()} className={BTN_PRIMARY}>
              下载并安装
            </button>
          )}
          {phase === "downloading" && (
            <button onClick={runCancelDownload} className={BTN_DANGER}>
              取消下载
            </button>
          )}
          {phase === "ready" && (
            <button onClick={runInstall} className={BTN_PRIMARY}>
              立即安装并退出
            </button>
          )}
          {phase === "error" && (
            <button onClick={() => void runCheck()} className={BTN_SECONDARY}>
              重试
            </button>
          )}
          <button
            onClick={() => openUrl(RELEASES_URL).catch(console.warn)}
            className={BTN_SECONDARY}
          >
            打开发布页
          </button>
          {phase !== "checking" && (
            <button onClick={requestClose} className={BTN_SECONDARY}>
              关闭
            </button>
          )}
        </div>

        <label className="mt-4 flex cursor-pointer items-center gap-2 text-xs text-slate-500">
          <input
            type="checkbox"
            checked={autoCheck}
            onChange={toggleAutoCheck}
            className="h-3.5 w-3.5 accent-indigo-500"
          />
          启动时自动检查更新（12 小时内不重复检查）
        </label>
      </div>
    </div>
  );
}
