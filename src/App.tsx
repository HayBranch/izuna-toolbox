/**
 * 应用入口：
 * - 纯浏览器打开（没跑 tauri）：显示提示；
 * - 浮窗窗口（label = overlay-{id}）：渲染 OverlayWindow；
 * - 其他：主窗口外壳（侧边栏 + 页面）。
 */
import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getConfig } from "./api";
import type { AppConfig, MovedEvent } from "./types";
import OverlayWindow from "./components/OverlayWindow";
import OverlaysPage from "./components/OverlaysPage";
import VariablesPage from "./components/VariablesPage";

const isTauri = "__TAURI_INTERNALS__" in window;
const isOverlay = isTauri && getCurrentWindow().label.startsWith("overlay-");

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

type TabId = "vars" | "overlays";

const TABS: { id: TabId; label: string; icon: string; desc: string }[] = [
  {
    id: "vars",
    label: "变量与计数",
    icon: "🧮",
    desc: "每套浮窗都有自己独立的变量：自定义任意变量并加减计数，表达式自动算总数、胜率（SQLite 持久化）",
  },
  {
    id: "overlays",
    label: "悬浮字幕",
    icon: "🪟",
    desc: "浮窗配置队列：可创建多套、各自命名，模板变量、样式、置顶与鼠标穿透",
  },
];

function MainShell() {
  const [tab, setTab] = useState<TabId>("vars");
  const [aboutOpen, setAboutOpen] = useState(false);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const active = TABS.find((t) => t.id === tab)!;

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

    getConfig()
      .then((c) => alive && setConfig(c))
      .catch(console.warn);

    return () => {
      alive = false;
      offs.forEach((f) => f());
    };
  }, []);

  return (
    <div className="flex h-full">
      {/* 侧边导航 */}
      <aside className="flex w-60 shrink-0 flex-col border-r border-slate-800/80 bg-slate-900/60">
        <div className="flex items-center gap-3 px-5 pt-6 pb-5">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-gradient-to-br from-sky-400 to-indigo-500 text-lg shadow-lg shadow-indigo-500/30">
            🥷
          </div>
          <div>
            <h1 className="font-semibold text-slate-100">泉奈工具箱</h1>
            <p className="text-xs text-slate-500">Izuna Toolbox · Tauri 2</p>
          </div>
        </div>

        <nav className="flex-1 space-y-1 px-3">
          {TABS.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={`flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm transition-colors ${
                tab === t.id
                  ? "bg-sky-500/15 text-sky-300"
                  : "text-slate-400 hover:bg-slate-800/60 hover:text-slate-200"
              }`}
            >
              <span className="text-base">{t.icon}</span>
              <span className="font-medium">{t.label}</span>
            </button>
          ))}
        </nav>

        <footer className="space-y-3 px-5 pb-5 text-xs leading-5 text-slate-600">
          <p>
            关闭窗口不会退出，
            <br />
            会最小化到系统托盘 🫥
            <br />
            浮窗继续悬浮，可被直播软件捕获。
          </p>
          <button
            onClick={() => setAboutOpen(true)}
            className="rounded-lg border border-slate-800 px-2.5 py-1 text-xs text-slate-500 transition hover:border-slate-600 hover:text-slate-300"
          >
            ℹ️ 关于
          </button>
        </footer>
      </aside>

      {/* 内容区 */}
      <main className="min-w-0 flex-1 overflow-y-auto p-8">
        <header className="mb-6">
          <h2 className="text-xl font-semibold text-slate-100">{active.label}</h2>
          <p className="mt-1 text-sm text-slate-500">{active.desc}</p>
        </header>

        {!config ? (
          <p className="animate-pulse text-sm text-slate-500">正在从 Rust 加载配置…</p>
        ) : tab === "vars" ? (
          <VariablesPage config={config} />
        ) : (
          <OverlaysPage config={config} />
        )}
      </main>

      {aboutOpen && <AboutDialog onClose={() => setAboutOpen(false)} />}
    </div>
  );
}

/** 「关于」弹窗：应用介绍、版本、仓库与开源协议 */
function AboutDialog({ onClose }: { onClose: () => void }) {
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion("0.1.0"));
  }, []);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md rounded-2xl border border-slate-700 bg-slate-900 p-6 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-4">
          <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-gradient-to-br from-sky-400 to-indigo-500 text-2xl shadow-lg shadow-indigo-500/30">
            🥷
          </div>
          <div>
            <h3 className="text-lg font-semibold text-slate-100">泉奈工具箱</h3>
            <p className="text-xs text-slate-500">Izuna Toolbox{version ? ` · v${version}` : ""}</p>
          </div>
        </div>

        <p className="mt-4 text-sm leading-6 text-slate-400">
          为直播玩家打造的本地小工具：把战绩数据做成一枚可被 OBS
          等直播软件捕获的悬浮字幕浮窗。支持多套浮窗（各自独立的变量与模板）、
          自定义变量与表达式计算、百分比格式化；数据全部保存在本地 SQLite，不联网、不上传。
        </p>

        <div className="mt-4 space-y-1.5 rounded-xl border border-slate-800 bg-slate-950/60 p-4 text-xs leading-5">
          <p className="text-slate-500">
            仓库：<span className="font-mono text-slate-400">https://gitee.com/HayBranch/izuna-toolbox</span>
          </p>
          <p className="text-slate-500">
            协议：<span className="text-slate-400">MIT License</span>
          </p>
          <p className="text-slate-500">
            版权：<span className="text-slate-400">Copyright © 2026 HayBranch</span>
          </p>
        </div>

        <button
          onClick={onClose}
          className="mt-5 w-full rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400"
        >
          关闭
        </button>
      </div>
    </div>
  );
}
