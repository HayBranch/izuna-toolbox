/**
 * 应用入口：
 * - 纯浏览器打开（没跑 tauri）：显示提示；
 * - 浮窗窗口（label = overlay-{id}）：渲染 OverlayWindow；
 * - 其他：主窗口外壳（单页：悬浮字幕的全部配置）。
 */
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { getConfig, openUrl, setMainAlwaysOnTop, subscribeConfig } from "./api";
import izunaIcon from "./assets/izuna-icon.png";
import OverlayWindow from "./components/OverlayWindow";
import OverlaysPage from "./components/OverlaysPage";
import UpdateDialog from "./components/UpdateDialog";
import type { AppConfig, MovedEvent } from "./types";
import { checkForUpdate, markChecked, shouldStartupCheck, type UpdateInfo } from "./updater";
import { applyThemeMode, getThemeMode, setThemeMode, type ThemeMode } from "./theme";

const isTauri = "__TAURI_INTERNALS__" in window;
const isOverlay = isTauri && getCurrentWindow().label.startsWith("overlay-");
/** 主窗口置顶偏好（localStorage 跟随 WebView 用户目录，重开窗口后恢复） */
const PIN_STORAGE_KEY = "izuna.main.alwaysOnTop";

/** 让原生标题栏跟随界面主题（auto 传 null 跟随系统）；平台不支持时静默忽略 */
function syncWindowTheme(mode: ThemeMode) {
  try {
    void getCurrentWindow()
      .setTheme(mode === "auto" ? null : mode)
      .catch(() => {});
  } catch {
    // 旧版 Tauri / 个别平台不支持原生主题切换，不影响页面配色
  }
}

export default function App() {
  if (!isTauri) {
    return (
      <div className="flex h-full items-center justify-center p-8 text-center text-sm text-slate-500">
        请通过 <code className="mx-1 rounded bg-slate-800 px-1.5 py-0.5 text-indigo-300">npm run tauri dev</code>{" "}
        启动（此页面依赖 Tauri 的 IPC）。
      </div>
    );
  }
  if (isOverlay) return <OverlayWindow />;
  return <MainShell />;
}

function MainShell() {
  const [aboutOpen, setAboutOpen] = useState(false);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [pinned, setPinned] = useState(false);
  const [updateOpen, setUpdateOpen] = useState(false);
  const [updateToast, setUpdateToast] = useState<UpdateInfo | null>(null);
  const [version, setVersion] = useState("");
  const [themeMode, setThemeModeState] = useState<ThemeMode>(getThemeMode);

  // 左上角显示当前版本号（与「关于」弹窗同源：Tauri 运行时版本，构建期注入兜底）
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(__APP_VERSION__));
  }, []);

  // 主题：切换模式立即生效；auto 模式跟随系统外观（监听变化实时同步）。
  // 同时把原生标题栏也切到同一主题（null = 跟随系统），避免白页面配黑标题栏。
  useEffect(() => {
    applyThemeMode(themeMode);
    syncWindowTheme(themeMode);
    if (themeMode !== "auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      applyThemeMode("auto");
      syncWindowTheme("auto");
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [themeMode]);

  const chooseTheme = (mode: ThemeMode) => {
    setThemeModeState(mode);
    setThemeMode(mode);
  };

  // 重开应用时恢复上次的「主窗口置顶」选择
  useEffect(() => {
    if (localStorage.getItem(PIN_STORAGE_KEY) !== "1") return;
    let alive = true;
    setMainAlwaysOnTop(true)
      .then((value) => {
        if (alive) setPinned(value);
      })
      .catch(console.warn);
    return () => {
      alive = false;
    };
  }, []);

  const togglePinned = async () => {
    const next = !pinned;
    try {
      await setMainAlwaysOnTop(next);
      setPinned(next);
      localStorage.setItem(PIN_STORAGE_KEY, next ? "1" : "0");
    } catch (e) {
      console.warn(e);
    }
  };

  // 拉取配置（带超时）：首次加载与错误态「重试」共用。
  // 超时是关键——若 IPC 永久挂起（数据库锁死、命令 panic 等），
  // 不至于永远停在"正在加载"，而是进错误态让用户能看到原因并重试。
  const loadConfig = (attempt = 0) => {
    let timer: ReturnType<typeof setTimeout>;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error("加载超时（10s）")), 10_000);
    });
    setConfigError(null);
    Promise.race([getConfig(), timeout])
      .then((c) => {
        clearTimeout(timer);
        setConfig(c);
        setConfigError(null);
      })
      .catch((e) => {
        clearTimeout(timer);
        const message = String(e);
        // 启动瞬间状态可能还没注册完（Rust setup 竞态），这类错误自动重试；
        // 真正的数据库/权限错误仍直接显示，不无限重试。
        if (attempt < 3 && /state\s+not\s+managed/i.test(message)) {
          setTimeout(() => loadConfig(attempt + 1), 150 * (attempt + 1));
          return;
        }
        // 失败必须让人看见：否则 config 一直是 null，界面停在"正在加载…"，
        // 看起来像卡死，其实是 IPC 一次性失败（或挂起后被超时打断）。
        console.warn(e);
        setConfigError(message);
      });
  };

  useEffect(() => {
    let alive = true;
    const offs: Array<() => void> = [];

    const register = async () => {
      offs.push(
        await listen<AppConfig>("config-changed", (e) => {
          if (alive) setConfig(e.payload);
        }),
      );
      offs.push(
        await listen<MovedEvent>("overlay-moved", (e) => {
          if (!alive) return;
          setConfig((prev) =>
            prev
              ? {
                  ...prev,
                  overlays: prev.overlays.map((o) =>
                    o.id === e.payload.id ? { ...o, x: e.payload.x, y: e.payload.y } : o,
                  ),
                }
              : prev,
          );
        }),
      );
    };
    register();
    loadConfig();

    // 命令返回的配置也直接采用：不依赖事件是否送达（见 api.ts 配置总线）
    const offBus = subscribeConfig((c) => {
      if (alive) setConfig(c);
    });

    return () => {
      alive = false;
      offBus();
      offs.forEach((f) => f());
    };
  }, []);

  // 启动检查：静默（失败一律忽略），12 小时内只查一次；GitHub 连不上时
  // checkForUpdate 自带 8s 超时，不会拖慢启动或卡住界面。
  useEffect(() => {
    if (!shouldStartupCheck()) return;
    const ctrl = new AbortController();
    let alive = true;
    checkForUpdate({ signal: ctrl.signal })
      .then((r) => {
        if (alive && r.hasUpdate) setUpdateToast(r);
      })
      .catch(() => {})
      .finally(() => markChecked());
    return () => {
      alive = false;
      ctrl.abort();
    };
  }, []);

  return (
    <div className="flex h-full">
      {/* 侧边导航：只有一个功能，固定高亮，不需要切换 */}
      <aside className="flex w-60 shrink-0 flex-col border-r border-slate-200 bg-white/60 dark:border-slate-800/80 dark:bg-slate-900/60">
        <div className="flex items-center gap-3 px-5 pt-6 pb-5">
          <div className="flex h-10 w-10 items-center justify-center overflow-hidden rounded-xl bg-white shadow-lg shadow-indigo-500/20 ring-1 ring-slate-200 dark:bg-slate-950 dark:ring-slate-700/70">
            <img
              src={izunaIcon}
              alt="泉奈工具箱图标"
              draggable={false}
              className="h-full w-full object-cover"
            />
          </div>
          <div>
            <h1 className="font-semibold text-slate-900 dark:text-slate-100">泉奈工具箱</h1>
            <p className="text-xs text-slate-500">Izuna Toolbox{version ? ` · v${version}` : ""}</p>
          </div>
        </div>

        <nav className="px-3">
          <div className="flex items-center gap-3 rounded-xl bg-sky-500/15 px-3 py-2.5 text-sm text-sky-700 dark:text-sky-300">
            <span className="text-base">🪟</span>
            <span className="font-medium">悬浮字幕</span>
          </div>
        </nav>

        <footer className="mt-auto space-y-3 px-5 pb-5 text-xs leading-5 text-slate-500 dark:text-slate-600">
          <ThemeSwitcher mode={themeMode} onChoose={chooseTheme} />
          <p>
            关闭窗口不会退出，
            <br />
            会最小化到系统托盘 🫥
            <br />
            浮窗继续悬浮，可被直播软件捕获。
          </p>
          <button
            onClick={() => setAboutOpen(true)}
            className="rounded-lg border border-slate-200 px-2.5 py-1 text-xs text-slate-500 transition hover:border-slate-400 hover:text-slate-700 dark:border-slate-800 dark:hover:border-slate-600 dark:hover:text-slate-300"
          >
            ℹ️ 关于
          </button>
        </footer>
      </aside>

      {/* 内容区：列表 → 点开一张卡进入该浮窗的全部配置 */}
      <main className="min-w-0 flex-1 overflow-y-auto p-8">
        <header className="mb-6 flex flex-wrap items-start justify-between gap-4">
          <div>
            <h2 className="text-xl font-semibold text-slate-900 dark:text-slate-100">悬浮字幕</h2>
            <p className="mt-1 text-sm text-slate-500">
              可被 OBS 等直播软件捕获的悬浮字幕：多套浮窗各自独立（变量、模板、样式）。
              先在列表里看基本信息，点进去配置详情。
            </p>
          </div>
          <button
            type="button"
            onClick={() => void togglePinned()}
            aria-pressed={pinned}
            title={pinned ? "取消主窗口置顶" : "让主窗口始终显示在最前面"}
            className={`flex shrink-0 items-center gap-2 rounded-xl border px-3.5 py-2 text-sm font-medium transition ${
              pinned
                ? "border-indigo-400 bg-indigo-500 text-white hover:bg-indigo-400"
                : "border-slate-300 bg-white/70 text-slate-600 hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:bg-slate-900/60 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
            }`}
          >
            <PinIcon filled={pinned} />
            {pinned ? "已置顶" : "窗口置顶"}
          </button>
        </header>

        {!config ? (
          configError ? (
            <div className="rounded-xl border border-rose-500/30 bg-rose-50 px-4 py-3 text-sm text-rose-700 dark:bg-rose-950 dark:text-rose-300" role="alert">
              <p className="font-medium">加载配置失败</p>
              <p className="mt-1 break-all font-mono text-xs text-rose-500 dark:text-rose-400">{configError}</p>
              <button
                type="button"
                onClick={() => loadConfig()}
                className="mt-3 rounded-lg border border-rose-500/50 px-3 py-1.5 text-xs font-medium text-rose-600 transition hover:bg-rose-500/20 dark:text-rose-200"
              >
                重试
              </button>
            </div>
          ) : (
            <p className="animate-pulse text-sm text-slate-500">正在从 Rust 加载配置…</p>
          )
        ) : (
          <OverlaysPage config={config} />
        )}
      </main>

      {aboutOpen && (
        <AboutDialog
          onClose={() => setAboutOpen(false)}
          onCheckUpdate={() => {
            setAboutOpen(false);
            setUpdateOpen(true);
          }}
        />
      )}
      {updateOpen && <UpdateDialog onClose={() => setUpdateOpen(false)} />}

      {updateToast && (
        <div className="fixed bottom-5 right-5 z-40 flex items-center gap-3 rounded-xl border border-indigo-500/40 bg-white px-4 py-3 text-sm shadow-2xl dark:bg-slate-900">
          <span className="text-slate-700 dark:text-slate-200">发现新版本 v{updateToast.latestVersion}</span>
          <button
            onClick={() => {
              setUpdateOpen(true);
              setUpdateToast(null);
            }}
            className="rounded-lg bg-indigo-500 px-2.5 py-1 text-xs font-medium text-white transition hover:bg-indigo-400"
          >
            查看更新
          </button>
          <button
            onClick={() => setUpdateToast(null)}
            aria-label="关闭提示"
            className="text-slate-400 transition hover:text-slate-600 dark:text-slate-500 dark:hover:text-slate-200"
          >
            ✕
          </button>
        </div>
      )}
    </div>
  );
}

/** 左下角主题切换：自动（跟随系统）/ 白 / 黑 三段选择 */
function ThemeSwitcher({ mode, onChoose }: { mode: ThemeMode; onChoose: (m: ThemeMode) => void }) {
  const options: Array<{ value: ThemeMode; icon: string; label: string }> = [
    { value: "auto", icon: "🖥", label: "自动" },
    { value: "light", icon: "☀️", label: "白" },
    { value: "dark", icon: "🌙", label: "黑" },
  ];
  return (
    <div className="space-y-1.5">
      <p className="text-[11px] text-slate-400 dark:text-slate-600">主题</p>
      <div
        role="radiogroup"
        aria-label="界面主题"
        className="flex items-center gap-1 rounded-xl border border-slate-200 bg-white/70 p-1 dark:border-slate-800 dark:bg-slate-900/60"
      >
        {options.map((o) => (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={mode === o.value}
            onClick={() => onChoose(o.value)}
            title={
              o.value === "auto"
                ? "自动：跟随系统深浅色外观"
                : o.value === "light"
                  ? "白色（亮色）主题"
                  : "黑色（暗色）主题"
            }
            className={`flex flex-1 items-center justify-center gap-1 rounded-lg px-1.5 py-1 text-xs transition ${
              mode === o.value
                ? "bg-indigo-500 font-medium text-white"
                : "text-slate-500 hover:text-slate-700 dark:hover:text-slate-300"
            }`}
          >
            <span aria-hidden="true">{o.icon}</span>
            {o.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/** 主窗口置顶按钮的图钉图标（不引入图标库，沿用内联 SVG） */
function PinIcon({ filled }: { filled: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-4 w-4"
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M9 3h6l-1 6 3 3v2H7v-2l3-3-1-6Z" />
      <path d="M12 14v7" />
    </svg>
  );
}

/** 「关于」弹窗：应用介绍、版本、仓库与开源协议 */
function AboutDialog({
  onClose,
  onCheckUpdate,
}: {
  onClose: () => void;
  onCheckUpdate: () => void;
}) {
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(__APP_VERSION__));
  }, []);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md rounded-2xl border border-slate-200 bg-white p-6 shadow-2xl dark:border-slate-700 dark:bg-slate-900"
        onClick={(e) => e.stopPropagation()}
      >
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
          等直播软件捕获的悬浮字幕浮窗。支持多套浮窗（各自独立的变量与模板）、
          自定义变量与表达式计算、百分比格式化、多行换行排版；数据全部保存在本地
          SQLite、不会上传，仅「检查更新」会连接 GitHub。
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
          <button
            onClick={onCheckUpdate}
            className="flex-1 rounded-xl border border-slate-300 bg-white/70 px-4 py-2 text-sm font-medium text-slate-600 transition hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:bg-slate-900/60 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
          >
            检查更新
          </button>
          <button
            onClick={onClose}
            className="flex-1 rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400"
          >
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
