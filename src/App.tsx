/**
 * 应用入口：
 * - 纯浏览器打开（没跑 tauri）：显示提示；
 * - 浮窗窗口（label = overlay-{id}）：渲染 OverlayWindow；
 * - 排队弹窗（label = queue-{id}）：渲染 QueueWindow；
 * - 其他：主窗口外壳（悬浮字幕 / 排队队列两个页面）。
 */
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { getConfig, getSettings, setMainAlwaysOnTop, subscribeConfig } from "./api";
import izunaIcon from "./assets/izuna-icon.png";
import OverlayWindow from "./components/OverlayWindow";
import OverlaysPage from "./components/OverlaysPage";
import QueuesPage from "./components/QueuesPage";
import QueueWindow from "./components/QueueWindow";
import SettingsPage from "./components/SettingsPage";
import { UnsavedGuardProvider, useUnsavedGuard } from "./components/UnsavedGuard";
import UpdateDialog from "./components/UpdateDialog";
import { applyThemeMode, type ThemeMode } from "./theme";
import type { AppConfig, MovedEvent, SettingsInfo } from "./types";
import { checkForUpdate, markChecked, shouldStartupCheck, type UpdateInfo } from "./updater";

const isTauri = "__TAURI_INTERNALS__" in window;
const isOverlay = isTauri && getCurrentWindow().label.startsWith("overlay-");
const isQueue = isTauri && getCurrentWindow().label.startsWith("queue-");
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
  if (isQueue) return <QueueWindow />;
  return (
    <UnsavedGuardProvider>
      <MainShell />
    </UnsavedGuardProvider>
  );
}

type MainPage = "overlays" | "queues" | "settings";

function MainShell() {
  const guard = useUnsavedGuard();
  const [page, setPage] = useState<MainPage>("overlays");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [settings, setSettings] = useState<SettingsInfo | null>(null);
  const [pinned, setPinned] = useState(false);
  const [updateOpen, setUpdateOpen] = useState(false);
  const [updateToast, setUpdateToast] = useState<UpdateInfo | null>(null);
  const [version, setVersion] = useState("");
  const [themeMode, setThemeModeState] = useState<ThemeMode>("auto");

  // 左上角显示当前版本号（与「关于」页面同源：Tauri 运行时版本，构建期注入兜底）
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(__APP_VERSION__));
  }, []);

  // 外观偏好保存在 conf/conf.toml：启动时读取一次，之后由设置页命令写回。
  useEffect(() => {
    let alive = true;
    getSettings()
      .then((s) => {
        if (!alive) return;
        setSettings(s);
        setThemeModeState(s.theme);
      })
      .catch((e) => console.warn("读取应用设置失败", e));
    return () => {
      alive = false;
    };
  }, []);

  // 主题：切换模式立即生效；auto 模式跟随系统外观（监听变化实时同步）。
  // 同时把原生标题栏也切到同一主题（null = 跟随系统），避免白页面配黑标题栏。
  useEffect(() => {
    applyThemeMode(themeMode);
    syncWindowTheme(themeMode);
    if (themeMode !== "auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      const dark = applyThemeMode("auto");
      // 系统外观变了：显式下发当前实际主题，确保原生标题栏跟着刷新
      syncWindowTheme(dark ? "dark" : "light");
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [themeMode]);

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

  // 切换页面：详情页有未保存修改时先询问（保存 / 丢弃 / 取消）
  const goPage = async (next: MainPage) => {
    if (next === page) return;
    if (guard && !(await guard.confirmLeave())) return;
    setPage(next);
  };

  const togglePinned = async () => {    const next = !pinned;
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
      offs.push(
        await listen<MovedEvent>("queue-moved", (e) => {
          if (!alive) return;
          setConfig((prev) =>
            prev
              ? {
                  ...prev,
                  queues: prev.queues.map((q) =>
                    q.id === e.payload.id ? { ...q, x: e.payload.x, y: e.payload.y } : q,
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
      {/* 侧边导航：悬浮字幕 / 排队队列；设置按钮在左下角 */}
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

        <nav className="space-y-1 px-3">
          <NavItem
            icon="🪟"
            label="悬浮字幕"
            active={page === "overlays"}
            onClick={() => void goPage("overlays")}
          />
          <NavItem
            icon="💬"
            label="排队队列"
            active={page === "queues"}
            onClick={() => void goPage("queues")}
          />
        </nav>

        <footer className="mt-auto space-y-3 px-5 pb-5 text-xs leading-5 text-slate-500 dark:text-slate-600">
          <p>
            关闭窗口不会退出，
            <br />
            会最小化到系统托盘 🫥
            <br />
            浮窗继续悬浮，可被直播软件捕获。
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void goPage("settings")}
              aria-current={page === "settings" ? "page" : undefined}
              title="外观、开发者选项、配置同步与关于"
              className={`flex items-center gap-1.5 rounded-lg border px-2 py-1 text-xs transition ${
                page === "settings"
                  ? "border-indigo-400 bg-indigo-500 text-white hover:bg-indigo-400"
                  : "border-slate-200 text-slate-500 hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-800 dark:text-slate-500 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
              }`}
            >
              <GearIcon />
              设置
            </button>
            <button
              type="button"
              onClick={() => void togglePinned()}
              aria-pressed={pinned}
              title={pinned ? "取消主窗口置顶" : "让主窗口始终显示在最前面"}
              className={`flex items-center gap-1.5 rounded-lg border px-2 py-1 text-xs transition ${
                pinned
                  ? "border-indigo-400 bg-indigo-500 text-white hover:bg-indigo-400"
                  : "border-slate-200 text-slate-500 hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-800 dark:text-slate-500 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
              }`}
            >
              <PinIcon filled={pinned} />
              {pinned ? "已置顶" : "置顶窗口"}
            </button>
          </div>
        </footer>
      </aside>

      {/* 内容区：按左侧导航切换「悬浮字幕 / 排队队列 / 设置」 */}
      <main className="min-w-0 flex-1 overflow-y-auto p-8">
        <header className="mb-6">
          <h2 className="text-xl font-semibold text-slate-900 dark:text-slate-100">
            {page === "overlays" ? "悬浮字幕" : page === "queues" ? "排队队列" : "设置"}
          </h2>
          <p className="mt-1 text-sm text-slate-500">
            {page === "overlays"
              ? "可被 OBS 等直播软件捕获的悬浮字幕：多套浮窗各自独立（变量、模板、样式）。先在列表里看基本信息，点进去配置详情。"
              : page === "queues"
                ? "接入 B 站直播弹幕，按正则把「排队」等弹幕记成名单：每个队列独立直播间、规则、排序与透明弹窗。"
                : "外观、开发者选项、配置及同步，以及关于信息。修改会立即保存到 exe 同目录的 conf/conf.toml。"}
          </p>
        </header>

        {page === "settings" ? (
          <SettingsPage
            settings={settings}
            onSettings={setSettings}
            onThemeModeChange={setThemeModeState}
            onCheckUpdate={() => setUpdateOpen(true)}
          />
        ) : !config ? (
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
        ) : page === "overlays" ? (
          <OverlaysPage config={config} />
        ) : (
          <QueuesPage config={config} />
        )}
      </main>

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

/** 左侧导航项：当前页高亮 */
function NavItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon: string;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={`flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm transition ${
        active
          ? "bg-sky-500/15 text-sky-700 dark:text-sky-300"
          : "text-slate-500 hover:bg-slate-500/10 hover:text-slate-700 dark:text-slate-400 dark:hover:text-slate-200"
      }`}
    >
      <span className="text-base">{icon}</span>
      <span className={active ? "font-medium" : ""}>{label}</span>
    </button>
  );
}

/** 左下角设置按钮的齿轮图标（内联 SVG，避免额外图标库） */
function GearIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-4 w-4"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="3.2" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06A1.7 1.7 0 0 0 15 19.4a1.7 1.7 0 0 0-1 .6 1.7 1.7 0 0 0-.4 1.1V21a2 2 0 1 1-4 0v-.1A1.7 1.7 0 0 0 8.6 19.4a1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-.6-1 1.7 1.7 0 0 0-1.1-.4H3a2 2 0 1 1 0-4h.1A1.7 1.7 0 0 0 4.6 8.6a1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-.6 1.7 1.7 0 0 0 .4-1.1V3a2 2 0 1 1 4 0v.1A1.7 1.7 0 0 0 15.4 4.6a1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.7 1.7 0 0 0 19.4 9c.14.37.36.7.64.96.3.27.68.42 1.08.44H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5.6Z" />
    </svg>
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
