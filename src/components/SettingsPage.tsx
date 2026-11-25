/**
 * 设置页：外观 / 开发者选项 / 配置及同步 / 关于（关于固定放在最后一页）。
 *
 * 外观与日志等级都写在 exe 同目录 conf/conf.toml（Rust 侧设置命令），
 * 不再使用 localStorage；WebDAV 账号密码只存 WebView localStorage，
 * 不会跟着 conf.toml 或备份文件导出。
 */
import { getVersion } from "@tauri-apps/api/app";
import { ask, open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import {
  applyBackup,
  exportLocal,
  exportWebdav,
  openLogDir,
  openUrl,
  resetSettings,
  saveLogLevel,
  saveTheme,
  selectLocalBackup,
  selectWebdavBackup,
} from "../api";
import izunaIcon from "../assets/izuna-icon.png";
import type {
  BackupPreview,
  ImportResult,
  LogLevel,
  SettingsInfo,
  ThemeMode,
  WebdavConfig,
} from "../types";
import { onRelaunchProbe, testRelaunch } from "../updater";

type SettingsTab = "appearance" | "developer" | "sync" | "about";

const TABS: Array<{ id: SettingsTab; icon: string; label: string }> = [
  { id: "appearance", icon: "🎨", label: "外观" },
  { id: "developer", icon: "🛠", label: "开发者选项" },
  { id: "sync", icon: "🔄", label: "配置及同步" },
  { id: "about", icon: "ℹ️", label: "关于" },
];

const CARD =
  "rounded-2xl border border-slate-200 bg-white/70 p-5 shadow-sm dark:border-slate-800 dark:bg-slate-900/60";
const BTN_PRIMARY =
  "rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400 disabled:cursor-not-allowed disabled:opacity-50";
const BTN_SECONDARY =
  "rounded-xl border border-slate-300 bg-white/70 px-4 py-2 text-sm font-medium text-slate-600 transition hover:border-indigo-400 hover:text-indigo-600 disabled:cursor-not-allowed disabled:opacity-50 dark:border-slate-700 dark:bg-slate-900/60 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300";
const BTN_DANGER =
  "rounded-xl border border-rose-500/50 bg-rose-50 px-4 py-2 text-sm font-medium text-rose-600 transition hover:bg-rose-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-rose-950/40 dark:text-rose-300";
const INPUT =
  "mt-1 w-full rounded-lg border border-slate-300 bg-white px-2.5 py-1.5 text-sm text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200";

const DAV_KEYS = {
  baseUrl: "izuna.webdav.baseUrl",
  username: "izuna.webdav.username",
  password: "izuna.webdav.password",
  remoteDir: "izuna.webdav.remoteDir",
  remoteFile: "izuna.webdav.remoteFile",
};

const THEME_OPTIONS: Array<{ value: ThemeMode; icon: string; label: string; desc: string }> = [
  { value: "auto", icon: "🖥", label: "自动", desc: "跟随系统深浅色外观实时切换" },
  { value: "light", icon: "☀️", label: "白色", desc: "始终使用亮色界面" },
  { value: "dark", icon: "🌙", label: "黑色", desc: "始终使用暗色界面" },
];

const LOG_LEVELS: Array<{ value: LogLevel; label: string; desc: string }> = [
  { value: "off", label: "off", desc: "完全关闭日志输出" },
  { value: "error", label: "error", desc: "只记录错误（默认，基本不打印日志）" },
  { value: "warn", label: "warn", desc: "错误 + 警告" },
  { value: "info", label: "info", desc: "常规运行信息（排错时使用）" },
  { value: "debug", label: "debug", desc: "调试信息，日志量较大" },
  { value: "trace", label: "trace", desc: "最详细，仅开发调试用" },
];

function errorText(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  return String(e) || "未知错误";
}

function loadWebdav(): WebdavConfig {
  const read = (key: string) => {
    try {
      return localStorage.getItem(key) ?? "";
    } catch {
      return "";
    }
  };
  return {
    baseUrl: read(DAV_KEYS.baseUrl),
    username: read(DAV_KEYS.username),
    password: read(DAV_KEYS.password),
    remoteDir: read(DAV_KEYS.remoteDir) || "izuna-toolbox",
  };
}

function defaultBackupName(includeData: boolean): string {
  if (!includeData) return "conf.toml";
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `izuna-backup-${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(
    d.getHours(),
  )}${pad(d.getMinutes())}.json`;
}

export default function SettingsPage({
  settings,
  onSettings,
  onThemeModeChange,
  onCheckUpdate,
}: {
  settings: SettingsInfo | null;
  onSettings: (s: SettingsInfo) => void;
  onThemeModeChange: (mode: ThemeMode) => void;
  onCheckUpdate: () => void;
}) {
  const [tab, setTab] = useState<SettingsTab>("appearance");

  return (
    <div className="space-y-5">
      <div
        role="tablist"
        aria-label="设置分类"
        className="flex flex-wrap gap-1 rounded-2xl border border-slate-200 bg-white/60 p-1 dark:border-slate-800 dark:bg-slate-900/60"
      >
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            className={`flex items-center gap-2 rounded-xl px-4 py-2 text-sm transition ${
              tab === t.id
                ? "bg-indigo-500 font-medium text-white shadow-sm"
                : "text-slate-500 hover:bg-slate-500/10 hover:text-slate-700 dark:text-slate-400 dark:hover:text-slate-200"
            }`}
          >
            <span aria-hidden="true">{t.icon}</span>
            {t.label}
          </button>
        ))}
      </div>

      {tab === "appearance" && (
        <AppearanceTab settings={settings} onSettings={onSettings} onThemeModeChange={onThemeModeChange} />
      )}
      {tab === "developer" && <DeveloperTab settings={settings} onSettings={onSettings} />}
      {tab === "sync" && (
        <SyncTab settings={settings} onSettings={onSettings} onThemeModeChange={onThemeModeChange} />
      )}
      {tab === "about" && <AboutTab onCheckUpdate={onCheckUpdate} />}
    </div>
  );
}

function AppearanceTab({
  settings,
  onSettings,
  onThemeModeChange,
}: {
  settings: SettingsInfo | null;
  onSettings: (s: SettingsInfo) => void;
  onThemeModeChange: (mode: ThemeMode) => void;
}) {
  const current = settings?.theme ?? "auto";
  const [error, setError] = useState("");
  const [saving, setSaving] = useState<ThemeMode | null>(null);

  const choose = async (mode: ThemeMode) => {
    if (!settings || mode === current) return;
    setError("");
    setSaving(mode);
    // 先立即切换界面，保存失败再回退
    onThemeModeChange(mode);
    try {
      const next = await saveTheme(mode);
      onSettings(next);
      onThemeModeChange(next.theme);
    } catch (e) {
      setError(errorText(e));
      onThemeModeChange(settings.theme);
    } finally {
      setSaving(null);
    }
  };

  return (
    <section className={CARD}>
      <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">界面外观</h3>
      <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
        选择白天或黑夜配色。「自动」会跟随系统外观变化实时切换，原生 Windows 标题栏也会同步。
      </p>

      <div className="mt-4 grid gap-3 sm:grid-cols-3">
        {THEME_OPTIONS.map((o) => {
          const active = current === o.value;
          return (
            <button
              key={o.value}
              type="button"
              disabled={!settings || saving !== null}
              onClick={() => void choose(o.value)}
              aria-pressed={active}
              className={`rounded-2xl border p-4 text-left transition ${
                active
                  ? "border-indigo-500 bg-indigo-500/10 ring-1 ring-indigo-500/40"
                  : "border-slate-200 bg-white/60 hover:border-indigo-300 dark:border-slate-800 dark:bg-slate-950/40 dark:hover:border-indigo-600"
              } disabled:cursor-not-allowed disabled:opacity-60`}
            >
              <div className="flex items-center gap-2">
                <span className="text-xl" aria-hidden="true">
                  {o.icon}
                </span>
                <span className="font-medium text-slate-800 dark:text-slate-100">{o.label}</span>
                {saving === o.value && <span className="text-xs text-slate-400">保存中…</span>}
              </div>
              <p className="mt-2 text-xs leading-5 text-slate-500 dark:text-slate-400">{o.desc}</p>
            </button>
          );
        })}
      </div>

      {error && (
        <p className="mt-3 rounded-lg border border-rose-500/30 bg-rose-50 px-3 py-2 text-xs text-rose-700 dark:bg-rose-950/40 dark:text-rose-300">
          {error}
        </p>
      )}

      <p className="mt-4 text-xs leading-5 text-slate-400 dark:text-slate-500">
        选择保存在 <code className="font-mono">conf/conf.toml</code> 的 <code className="font-mono">theme</code> 项，
        重开应用后仍生效；旧的 localStorage 主题偏好已废弃。
      </p>
    </section>
  );
}

function DeveloperTab({
  settings,
  onSettings,
}: {
  settings: SettingsInfo | null;
  onSettings: (s: SettingsInfo) => void;
}) {
  const current = settings?.logLevel ?? "error";
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [diagBusy, setDiagBusy] = useState(false);
  const [diagNote, setDiagNote] = useState("");

  // ---- 更新后自动启动自检 ----
  const [testing, setTesting] = useState(false);
  const [probe, setProbe] = useState<{ ok: boolean; text: string } | null>(null);
  const probeCleanup = useRef<(() => void) | null>(null);
  const probeTimer = useRef<number | null>(null);

  const stopProbe = () => {
    probeCleanup.current?.();
    probeCleanup.current = null;
    if (probeTimer.current !== null) {
      window.clearTimeout(probeTimer.current);
      probeTimer.current = null;
    }
  };

  // 离开设置页时清掉监听与超时，避免泄漏
  useEffect(() => stopProbe, []);

  const runRelaunchTest = async () => {
    if (testing) return;
    stopProbe();
    setProbe(null);
    setTesting(true);
    try {
      const { token, seconds } = await testRelaunch();
      const started = Date.now();
      probeCleanup.current = await onRelaunchProbe((t) => {
        if (t !== token) return;
        stopProbe();
        setTesting(false);
        setProbe({
          ok: true,
          text: `✓ 自检通过：模拟安装（${seconds} 秒）后，新进程在 ${(
            (Date.now() - started) / 1000
          ).toFixed(1)} 秒内启动并连回本程序。`,
        });
      });
      probeTimer.current = window.setTimeout(() => {
        stopProbe();
        setTesting(false);
        setProbe({
          ok: false,
          text:
            "✗ 自检失败：30 秒内没有等到新进程。可把日志等级调到 info，查看 logs/ 里的记录。",
        });
      }, 30000);
    } catch (e) {
      stopProbe();
      setTesting(false);
      setProbe({ ok: false, text: `✗ 自检未能启动：${errorText(e)}` });
    }
  };

  const choose = async (level: LogLevel) => {
    if (!settings || level === current) return;
    setError("");
    setSaving(true);
    try {
      const next = await saveLogLevel(level);
      onSettings(next);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  // 一键排查：把日志等级提到 info（连接过程/写入/重连都会落盘），
  // 再打开 logs/ 目录让用户直接拿文件。error 是默认值，看不到这些记录。
  const startDanmakuDiag = async () => {
    setDiagBusy(true);
    setError("");
    setDiagNote("");
    try {
      if (!["info", "debug", "trace"].includes(current)) {
        const next = await saveLogLevel("info");
        onSettings(next);
      }
      const dir = await openLogDir();
      setDiagNote(
        `已开启 info 日志并打开日志目录：${dir}。复现问题后把当天日志发来即可；排完记得把等级调回 error。`,
      );
    } catch (e) {
      setError(errorText(e));
    } finally {
      setDiagBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">日志等级</h3>
        <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
          默认 <code className="font-mono">error</code>，基本不打印日志。排查问题时可临时调高，
          修改后立即生效，并写入 <code className="font-mono">conf/conf.toml</code>。
        </p>

        <div className="mt-4 space-y-1.5">
          {LOG_LEVELS.map((l) => {
            const active = current === l.value;
            return (
              <button
                key={l.value}
                type="button"
                disabled={!settings || saving}
                onClick={() => void choose(l.value)}
                aria-pressed={active}
                className={`flex w-full items-center gap-3 rounded-xl border px-3 py-2 text-left transition ${
                  active
                    ? "border-indigo-500 bg-indigo-500/10"
                    : "border-slate-200 bg-white/60 hover:border-indigo-300 dark:border-slate-800 dark:bg-slate-950/40 dark:hover:border-indigo-600"
                } disabled:cursor-not-allowed disabled:opacity-60`}
              >
                <span
                  className={`flex h-4 w-4 shrink-0 items-center justify-center rounded-full border ${
                    active
                      ? "border-indigo-500 bg-indigo-500"
                      : "border-slate-300 dark:border-slate-600"
                  }`}
                >
                  {active && <span className="h-1.5 w-1.5 rounded-full bg-white" />}
                </span>
                <span className="w-14 shrink-0 font-mono text-sm text-slate-700 dark:text-slate-200">
                  {l.label}
                </span>
                <span className="text-xs text-slate-500 dark:text-slate-400">{l.desc}</span>
              </button>
            );
          })}
        </div>

        {error && (
          <p className="mt-3 rounded-lg border border-rose-500/30 bg-rose-50 px-3 py-2 text-xs text-rose-700 dark:bg-rose-950/40 dark:text-rose-300">
            {error}
          </p>
        )}
      </section>

      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">
          弹幕连接排查
        </h3>
        <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
          队列计数卡住、弹幕连不上时，点下面按钮把日志等级临时调到
          <code className="font-mono"> info </code>并打开 logs/ 目录。日志按天分文件，
          每条弹幕记录都带 <code className="font-mono">queue_id</code>、
          <code className="font-mono">room</code>（直播间号）与真实房间号，可按队列分别排查：
          连接各阶段、鉴权结果、每分钟活跃统计（收包/弹幕/命中/重复/写入）、断线重连与错误原因都会落盘。
        </p>
        <button
          type="button"
          disabled={diagBusy}
          onClick={() => void startDanmakuDiag()}
          className={`mt-4 ${BTN_PRIMARY}`}
        >
          {diagBusy ? "正在开启…" : "开启详细日志并打开日志目录"}
        </button>
        {diagNote && (
          <p className="mt-3 rounded-lg border border-emerald-500/30 bg-emerald-50 px-3 py-2 text-xs text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300">
            {diagNote}
          </p>
        )}
      </section>

      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">路径</h3>
        <dl className="mt-3 space-y-2 text-xs">
          <div>
            <dt className="text-slate-400 dark:text-slate-500">配置文件</dt>
            <dd className="mt-0.5 break-all font-mono text-slate-600 dark:text-slate-300">
              {settings?.confPath || "加载中…"}
            </dd>
          </div>
          <div>
            <dt className="text-slate-400 dark:text-slate-500">日志目录</dt>
            <dd className="mt-0.5 break-all font-mono text-slate-600 dark:text-slate-300">
              {settings?.logDir || "加载中…"}
            </dd>
          </div>
        </dl>
      </section>

      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">
          更新后自动启动（自检）
        </h3>
        <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
          模拟「安装完成后启动新程序」：约 3 秒后带探针参数再启动一份当前程序，它会连回
          当前窗口，收到连接即代表重启链路正常。真正的安装时机由安装包钩子保证；自检
          不会退出当前程序，也不会改动已安装的文件。
        </p>
        <button
          type="button"
          disabled={testing}
          onClick={() => void runRelaunchTest()}
          className={`mt-4 ${BTN_SECONDARY}`}
        >
          {testing ? "自检中…（最多 30 秒）" : "测试自动启动"}
        </button>
        {probe && (
          <p
            className={`mt-3 rounded-lg border px-3 py-2 text-xs ${
              probe.ok
                ? "border-emerald-500/30 bg-emerald-50 text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300"
                : "border-rose-500/30 bg-rose-50 text-rose-700 dark:bg-rose-950/40 dark:text-rose-300"
            }`}
          >
            {probe.text}
          </p>
        )}
      </section>
    </div>
  );
}

function SyncTab({
  settings,
  onSettings,
  onThemeModeChange,
}: {
  settings: SettingsInfo | null;
  onSettings: (s: SettingsInfo) => void;
  onThemeModeChange: (mode: ThemeMode) => void;
}) {
  const [includeData, setIncludeData] = useState(true);
  const [dav, setDav] = useState<WebdavConfig>(loadWebdav);
  const [remoteFile, setRemoteFile] = useState(() => {
    try {
      return localStorage.getItem(DAV_KEYS.remoteFile) || "izuna-backup.json";
    } catch {
      return "izuna-backup.json";
    }
  });
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [preview, setPreview] = useState<BackupPreview | null>(null);
  const [result, setResult] = useState<ImportResult | null>(null);

  // WebDAV 连接信息与导入文件名只保存在本机 WebView 存储里，不随备份导出
  useEffect(() => {
    try {
      localStorage.setItem(DAV_KEYS.baseUrl, dav.baseUrl);
      localStorage.setItem(DAV_KEYS.username, dav.username);
      localStorage.setItem(DAV_KEYS.password, dav.password);
      localStorage.setItem(DAV_KEYS.remoteDir, dav.remoteDir);
      localStorage.setItem(DAV_KEYS.remoteFile, remoteFile);
    } catch {
      // 隐私模式等无法写 localStorage 时忽略，本次仍可使用
    }
  }, [dav, remoteFile]);

  const updateDav = (patch: Partial<WebdavConfig>) => setDav((prev) => ({ ...prev, ...patch }));

  const requireDav = (): boolean => {
    if (dav.baseUrl.trim()) return true;
    setError("请先填写 WebDAV 服务器地址");
    return false;
  };

  const doExportLocal = async () => {
    setError("");
    setNotice("");
    const path = await saveDialog({
      title: includeData ? "导出配置与数据" : "导出 conf.toml",
      defaultPath: defaultBackupName(includeData),
      filters: includeData
        ? [{ name: "泉奈备份", extensions: ["json"] }]
        : [{ name: "配置文件", extensions: ["toml"] }],
    }).catch((e) => {
      setError(errorText(e));
      return null;
    });
    if (!path) return;
    setBusy("export-local");
    try {
      const saved = await exportLocal(path, includeData);
      setNotice(`已导出到 ${saved}`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const doExportWebdav = async () => {
    if (!requireDav()) return;
    setError("");
    setNotice("");
    setBusy("export-webdav");
    try {
      const url = await exportWebdav(dav, includeData);
      setNotice(`已上传到 ${url}`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const doImportLocal = async () => {
    setError("");
    setNotice("");
    setResult(null);
    const path = await openDialog({
      title: "选择备份文件",
      multiple: false,
      directory: false,
      filters: [{ name: "泉奈备份 / conf.toml", extensions: ["json", "toml"] }],
    }).catch((e) => {
      setError(errorText(e));
      return null;
    });
    if (!path || Array.isArray(path)) return;
    setBusy("import-local");
    try {
      setPreview(await selectLocalBackup(path));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const doImportWebdav = async () => {
    if (!requireDav()) return;
    if (!remoteFile.trim()) {
      setError("请填写要下载的远程文件名");
      return;
    }
    setError("");
    setNotice("");
    setResult(null);
    setBusy("import-webdav");
    try {
      setPreview(await selectWebdavBackup(dav, remoteFile.trim()));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const doReset = async () => {
    setError("");
    setNotice("");
    const ok = await ask(
      "将外观恢复为「自动」、日志等级恢复为默认（error）。SQLite 里的浮窗、变量与排队名单不会被删除。",
      { title: "恢复默认设置", kind: "warning", okLabel: "恢复", cancelLabel: "取消" },
    ).catch(() => false);
    if (!ok) return;
    setBusy("reset");
    try {
      const next = await resetSettings();
      onSettings(next);
      onThemeModeChange(next.theme);
      setNotice("已恢复默认设置（外观 + 日志等级）。");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const confirmImport = async (mode: "overwrite" | "merge", applySettings: boolean) => {
    if (!preview) return;
    setBusy("apply");
    setError("");
    try {
      const r = await applyBackup(mode, applySettings);
      onSettings(r.settings);
      onThemeModeChange(r.settings.theme);
      setPreview(null);
      setResult(r);
      setNotice(importSummary(r));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-4">
      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">WebDAV 连接</h3>
        <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
          填写你的 WebDAV 服务器地址与账号。支持坚果云、群晖、Nextcloud 等兼容 WebDAV 的服务。
          账号密码只保存在本机 WebView 存储里，<strong>不会</strong>写进 conf.toml 或导出文件。
        </p>
        <div className="mt-4 grid gap-3 sm:grid-cols-2">
          <label className="block text-xs text-slate-500 dark:text-slate-400 sm:col-span-2">
            服务器地址
            <input
              type="url"
              value={dav.baseUrl}
              onChange={(e) => updateDav({ baseUrl: e.target.value })}
              placeholder="https://dav.example.com/remote.php/dav/files/me"
              className={INPUT}
            />
          </label>
          <label className="block text-xs text-slate-500 dark:text-slate-400">
            用户名
            <input
              type="text"
              value={dav.username}
              onChange={(e) => updateDav({ username: e.target.value })}
              autoComplete="username"
              className={INPUT}
            />
          </label>
          <label className="block text-xs text-slate-500 dark:text-slate-400">
            密码 / 应用密码
            <input
              type="password"
              value={dav.password}
              onChange={(e) => updateDav({ password: e.target.value })}
              autoComplete="current-password"
              className={INPUT}
            />
          </label>
          <label className="block text-xs text-slate-500 dark:text-slate-400">
            远程目录
            <input
              type="text"
              value={dav.remoteDir}
              onChange={(e) => updateDav({ remoteDir: e.target.value })}
              placeholder="izuna-toolbox"
              className={INPUT}
            />
          </label>
          <label className="block text-xs text-slate-500 dark:text-slate-400">
            导入时下载的文件名
            <input
              type="text"
              value={remoteFile}
              onChange={(e) => setRemoteFile(e.target.value)}
              placeholder="izuna-backup.json"
              className={INPUT}
            />
          </label>
        </div>
        <p className="mt-2 text-xs text-slate-400 dark:text-slate-500">
          导出到 WebDAV 时：勾选数据会上传 <code className="font-mono">izuna-backup.json</code>，
          不勾选则上传 <code className="font-mono">conf.toml</code>。
        </p>
      </section>

      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">导出</h3>
        <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
          导出 <code className="font-mono">conf.toml</code>（外观、日志等级）；勾选数据后会把 SQLite
          里的浮窗、变量、计算变量、排队队列与名单一起打包成 JSON 备份。
        </p>
        <label className="mt-3 flex cursor-pointer items-start gap-2 text-sm text-slate-600 dark:text-slate-300">
          <input
            type="checkbox"
            checked={includeData}
            onChange={(e) => setIncludeData(e.target.checked)}
            className="mt-0.5 h-4 w-4 accent-indigo-500"
          />
          <span>
            同时导出数据（SQLite）
            <span className="block text-xs text-slate-400 dark:text-slate-500">
              默认勾选；取消后只导出 conf.toml
            </span>
          </span>
        </label>
        <div className="mt-4 flex flex-wrap gap-2">
          <button type="button" disabled={busy !== null} onClick={() => void doExportLocal()} className={BTN_PRIMARY}>
            {busy === "export-local" ? "导出中…" : "导出到本地"}
          </button>
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => void doExportWebdav()}
            className={BTN_SECONDARY}
          >
            {busy === "export-webdav" ? "上传中…" : "导出到 WebDAV"}
          </button>
        </div>
      </section>

      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">导入</h3>
        <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
          支持 JSON 完整备份或单份 <code className="font-mono">conf.toml</code>。
          读取后会先显示摘要，再让你选择「覆盖」或「增量」，不会直接改动现有数据。
        </p>
        <div className="mt-4 flex flex-wrap gap-2">
          <button type="button" disabled={busy !== null} onClick={() => void doImportLocal()} className={BTN_SECONDARY}>
            {busy === "import-local" ? "读取中…" : "从本地导入"}
          </button>
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => void doImportWebdav()}
            className={BTN_SECONDARY}
          >
            {busy === "import-webdav" ? "下载中…" : "从 WebDAV 导入"}
          </button>
        </div>
      </section>

      <section className={CARD}>
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">恢复默认设置</h3>
        <p className="mt-1 text-sm leading-6 text-slate-500 dark:text-slate-400">
          把外观恢复为「自动」、日志等级恢复为默认 <code className="font-mono">error</code>。
          只重置应用设置，不会删除 SQLite 里的浮窗与排队数据。
        </p>
        <button
          type="button"
          disabled={busy !== null || !settings}
          onClick={() => void doReset()}
          className={`mt-4 ${BTN_DANGER}`}
        >
          {busy === "reset" ? "恢复中…" : "恢复所有默认设置"}
        </button>
      </section>

      {(notice || error || result) && (
        <div className="space-y-2">
          {notice && (
            <p className="rounded-xl border border-emerald-500/30 bg-emerald-50 px-3 py-2 text-sm text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300">
              {notice}
            </p>
          )}
          {error && (
            <p className="rounded-xl border border-rose-500/30 bg-rose-50 px-3 py-2 text-sm text-rose-700 dark:bg-rose-950/40 dark:text-rose-300">
              {error}
            </p>
          )}
          {result && (
            <p className="text-xs text-slate-400 dark:text-slate-500">
              导入后当前共有浮窗 {result.config.overlays.length} 套、排队队列 {result.config.queues.length} 个。
            </p>
          )}
        </div>
      )}

      {preview && (
        <ImportDialog
          preview={preview}
          busy={busy === "apply"}
          onClose={() => setPreview(null)}
          onConfirm={(mode, applySettings) => void confirmImport(mode, applySettings)}
        />
      )}
    </div>
  );
}

function importSummary(r: ImportResult): string {
  const s = r.stats;
  if (r.mode === "overwrite") {
    return `已覆盖导入：浮窗 ${s.overlaysAdded} 套、排队队列 ${s.queuesAdded} 个、名单 ${s.entriesAdded} 条。`;
  }
  return `增量导入完成：新增浮窗 ${s.overlaysAdded} 套（跳过同名 ${s.overlaysSkipped} 套）、新增队列 ${s.queuesAdded} 个、合并队列 ${s.queuesMerged} 个；新增名单 ${s.entriesAdded} 条、合并 ${s.entriesMerged} 条。`;
}

/** 导入确认弹窗：先看摘要，再选覆盖 / 增量（默认增量，避免误删数据） */
function ImportDialog({
  preview,
  busy,
  onClose,
  onConfirm,
}: {
  preview: BackupPreview;
  busy: boolean;
  onClose: () => void;
  onConfirm: (mode: "overwrite" | "merge", applySettings: boolean) => void;
}) {
  const [mode, setMode] = useState<"overwrite" | "merge">("merge");
  const [applySettings, setApplySettings] = useState(preview.hasSettings);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" onClick={onClose}>
      <div
        className="w-full max-w-lg rounded-2xl border border-slate-200 bg-white p-6 shadow-2xl dark:border-slate-700 dark:bg-slate-900"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-lg font-semibold text-slate-900 dark:text-slate-100">确认导入备份</h3>
        <p className="mt-1 break-all text-xs text-slate-400 dark:text-slate-500">来源：{preview.source}</p>

        <dl className="mt-4 grid grid-cols-2 gap-2 rounded-xl border border-slate-200 bg-slate-50 p-3 text-xs dark:border-slate-800 dark:bg-slate-950/60">
          <div>
            <dt className="text-slate-400 dark:text-slate-500">包含配置</dt>
            <dd className="text-slate-700 dark:text-slate-200">
              {preview.hasSettings ? "是（外观 / 日志等级）" : "否"}
            </dd>
          </div>
          <div>
            <dt className="text-slate-400 dark:text-slate-500">包含数据</dt>
            <dd className="text-slate-700 dark:text-slate-200">{preview.hasData ? "是" : "否"}</dd>
          </div>
          {preview.hasData && (
            <>
              <div>
                <dt className="text-slate-400 dark:text-slate-500">浮窗 / 队列</dt>
                <dd className="text-slate-700 dark:text-slate-200">
                  {preview.overlayCount} 套 / {preview.queueCount} 个
                </dd>
              </div>
              <div>
                <dt className="text-slate-400 dark:text-slate-500">排队名单</dt>
                <dd className="text-slate-700 dark:text-slate-200">{preview.entryCount} 条</dd>
              </div>
            </>
          )}
          {preview.appVersion && (
            <div>
              <dt className="text-slate-400 dark:text-slate-500">导出程序版本</dt>
              <dd className="text-slate-700 dark:text-slate-200">v{preview.appVersion}</dd>
            </div>
          )}
        </dl>

        {preview.hasData && (
          <div className="mt-4 space-y-2">
            <p className="text-xs font-medium text-slate-500 dark:text-slate-400">数据冲突处理</p>
            <label className="flex cursor-pointer items-start gap-2 rounded-xl border border-slate-200 p-3 text-sm dark:border-slate-800">
              <input
                type="radio"
                name="import-mode"
                checked={mode === "merge"}
                onChange={() => setMode("merge")}
                className="mt-0.5 accent-indigo-500"
              />
              <span className="text-slate-700 dark:text-slate-200">
                增量导入
                <span className="mt-0.5 block text-xs leading-5 text-slate-400 dark:text-slate-500">
                  保留现有数据：只新增本地没有的同名浮窗 / 队列；同名队列按 UID 合并名单（次数取较大值）。
                </span>
              </span>
            </label>
            <label className="flex cursor-pointer items-start gap-2 rounded-xl border border-rose-500/30 p-3 text-sm dark:border-rose-500/30">
              <input
                type="radio"
                name="import-mode"
                checked={mode === "overwrite"}
                onChange={() => setMode("overwrite")}
                className="mt-0.5 accent-rose-500"
              />
              <span className="text-slate-700 dark:text-slate-200">
                覆盖导入
                <span className="mt-0.5 block text-xs leading-5 text-rose-500 dark:text-rose-300">
                  先清空现有浮窗、队列与名单，再完全恢复为备份内容。此操作不可撤销。
                </span>
              </span>
            </label>
          </div>
        )}

        {preview.hasSettings && (
          <label className="mt-4 flex cursor-pointer items-start gap-2 text-sm text-slate-600 dark:text-slate-300">
            <input
              type="checkbox"
              checked={applySettings}
              onChange={(e) => setApplySettings(e.target.checked)}
              className="mt-0.5 h-4 w-4 accent-indigo-500"
            />
            <span>
              同时导入配置（外观 / 日志等级）
              <span className="block text-xs text-slate-400 dark:text-slate-500">
                取消后只导入数据，保留本机当前的 conf.toml 设置
              </span>
            </span>
          </label>
        )}

        <div className="mt-5 flex gap-2">
          <button type="button" disabled={busy} onClick={onClose} className={`flex-1 ${BTN_SECONDARY}`}>
            取消
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => onConfirm(preview.hasData ? mode : "merge", applySettings)}
            className={`flex-1 ${mode === "overwrite" && preview.hasData ? BTN_DANGER : BTN_PRIMARY}`}
          >
            {busy ? "导入中…" : mode === "overwrite" && preview.hasData ? "确认覆盖导入" : "确认导入"}
          </button>
        </div>
      </div>
    </div>
  );
}

function AboutTab({ onCheckUpdate }: { onCheckUpdate: () => void }) {
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(__APP_VERSION__));
  }, []);

  return (
    <section className={CARD}>
      <div className="flex items-center gap-4">
        <div className="flex h-14 w-14 items-center justify-center overflow-hidden rounded-2xl bg-white shadow-lg shadow-indigo-500/20 ring-1 ring-slate-200 dark:bg-slate-950 dark:ring-slate-700/70">
          <img
            src={izunaIcon}
            alt="泉奈工具箱图标"
            draggable={false}
            className="h-full w-full object-cover"
          />
        </div>
        <div>
          <h3 className="text-lg font-semibold text-slate-900 dark:text-slate-100">泉奈工具箱</h3>
          <p className="text-xs text-slate-500">Izuna Toolbox{version ? ` · v${version}` : ""}</p>
        </div>
      </div>

      <p className="mt-4 text-sm leading-6 text-slate-500 dark:text-slate-400">
        为直播玩家打造的本地小工具：把战绩数据做成一枚可被 OBS
        等直播软件捕获的悬浮字幕浮窗，还能接入 B 站直播弹幕、按正则自动统计排队名单并显示在独立弹窗里。
        支持多套浮窗（各自独立的变量与模板）、多套排队队列（各自直播间/规则/排序）、
        自定义变量与表达式计算、百分比格式化、多行换行排版；数据全部保存在本地
        SQLite、不会上传，仅「检查更新」会连接 GitHub，排队队列会连接 B 站获取公开弹幕，
        WebDAV 同步只会在你主动点击时连接你填写的服务器。
      </p>

      <div className="mt-4 space-y-1.5 rounded-xl border border-slate-200 bg-slate-50 p-4 text-xs leading-5 dark:border-slate-800 dark:bg-slate-950/60">
        <p className="text-slate-500">
          仓库：{" "}
          <button
            type="button"
            onClick={() => openUrl("https://github.com/HayBranch/izuna-toolbox").catch(console.warn)}
            title="在浏览器中打开仓库"
            className="break-all font-mono text-sky-600 underline decoration-sky-500/40 underline-offset-2 transition hover:text-sky-700 hover:decoration-sky-500 dark:text-sky-400 dark:hover:text-sky-300"
          >
            https://github.com/HayBranch/izuna-toolbox
          </button>
        </p>
        <p className="text-slate-500">
          协议：<span className="text-slate-600 dark:text-slate-400">MIT License</span>
        </p>
        <p className="text-slate-500">
          版权：<span className="text-slate-600 dark:text-slate-400">Copyright © 2026 HayBranch</span>
        </p>
      </div>

      <div className="mt-5 flex gap-2">
        <button type="button" onClick={onCheckUpdate} className={`flex-1 ${BTN_SECONDARY}`}>
          检查更新
        </button>
      </div>
    </section>
  );
}
