/**
 * 悬浮字幕浮窗的根组件（label 为 overlay-{id} 的窗口渲染的就是它）。
 *
 * 职责：
 * 1. 监听 config-changed 事件 + 启动时 get_config，按自己的 id 渲染模板；
 * 2. 尺寸两种模式：
 *    - 自动（默认）：ResizeObserver 量内容尺寸 → set_overlay_size 让窗口贴合内容；
 *    - 手动：直接在浮窗边缘/角落拖动（startResizeDragging）改宽高，拖完写回配置。
 *      手动模式字幕撑满窗口（宽度决定折行），不再按内容反向改窗口。
 * 3. 每 30s 刷新一次时间类变量（{today} 跨天也能自动更新）；
 * 4. 未开鼠标穿透时，悬停显示小工具条：关于（悬浮看基本信息）/ 置顶 / 锁定穿透 / 隐藏，
 *    穿透开启后浮窗收不到鼠标事件，只能回主窗口解锁（主窗口始终可以控制）。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import {
  getConfig,
  resizeOverlay,
  setOverlayAlwaysTop,
  setOverlayClickThrough,
  setOverlayShown,
  setOverlaySize,
  subscribeConfig,
} from "../api";
import type { AppConfig } from "../types";
import { formatDateTime } from "../utils";
import { computeVars } from "../vars";
import SubtitleView from "./SubtitleView";

/** 对齐 Tauri 的 ResizeDirection（该类型未导出，这里手写同形） */
type ResizeDir =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

function parseId(): number {
  const label = getCurrentWindow().label; // overlay-{id}
  const id = Number(label.slice("overlay-".length));
  return Number.isFinite(id) ? id : -1;
}

export default function OverlayWindow() {
  const id = useMemo(parseId, []);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [tick, setTick] = useState(0);
  // 测量目标：字幕外框（含内边距）。它用 width:max-content 按内容真实宽度排版，
  // 不被当前窗口宽度反向约束；maxWidth 只在这里生效。
  const boxRef = useRef<HTMLDivElement>(null);
  const applied = useRef({ w: 0, h: 0 });
  // 手动尺寸模式下，拖完把新宽高写回配置（防抖）
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const manualRef = useRef(false);

  // 透明背景：浮窗页面的 html/body 不能有底色（见 index.css 的 .overlay-mode）
  useEffect(() => {
    document.documentElement.classList.add("overlay-mode");
    return () => document.documentElement.classList.remove("overlay-mode");
  }, []);

  // 订阅配置：先监听再拉取，避免中间窗口期丢更新
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;

    const apply = (c: AppConfig) => {
      if (c.overlays.some((o) => o.id === id)) setConfig(c);
    };

    const off = listen<AppConfig>("config-changed", (e) => apply(e.payload));
    off.then((f) => {
      if (alive) unlisten = f;
      else f();
    });

    getConfig().then(apply).catch(console.warn);

    // 命令返回 / 广播到达都走同一条路（见 api.ts 配置总线）
    const offBus = subscribeConfig(apply);

    const timer = setInterval(() => setTick((t) => t + 1), 30_000);

    return () => {
      alive = false;
      unlisten?.();
      offBus();
      clearInterval(timer);
    };
  }, [id]);

  // 本套浮窗自己的变量（tick 用于 30s 刷新时间类变量）；在 early return 之前求，保证 hooks 顺序稳定
  const own = config?.overlays.find((o) => o.id === id);
  const vars = useMemo(() => (own ? computeVars(own, new Date()) : {}), [own, tick]);

  const manual = !!own && own.width > 0 && own.height > 0;
  manualRef.current = manual;

  // 自动模式：文字盒尺寸 → 窗口尺寸（带 1px 迟滞，避免 ResizeObserver ↔ set_size 抖动循环）。
  // 手动模式跳过：窗口大小由用户拖动决定，字幕撑满窗口。
  useEffect(() => {
    if (manual) return;
    const el = boxRef.current;
    if (!el) return;
    const measure = () => {
      const w = Math.ceil(el.offsetWidth);
      const h = Math.ceil(el.offsetHeight);
      if (Math.abs(w - applied.current.w) > 1 || Math.abs(h - applied.current.h) > 1) {
        applied.current = { w, h };
        setOverlaySize(w, h).catch(console.warn);
      }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [manual, config !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  // 窗口被拖动宽高（手动模式）→ 防抖写回配置，主窗口能看到当前尺寸。
  // 始终订阅，处理函数里再按"当前是否手动"决定是否写回：从自动切手动的瞬间
  // 订阅已经就位，不会漏掉刚开始的 resize 事件。
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const un = win.onResized(({ payload }) => {
      if (!alive || !manualRef.current) return;
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(async () => {
        try {
          const scale = await win.scaleFactor();
          const w = Math.round(payload.width / scale);
          const h = Math.round(payload.height / scale);
          await resizeOverlay(id, w, h);
        } catch (e) {
          console.warn(e);
        }
      }, 200);
    });
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => {});
      if (saveTimer.current) clearTimeout(saveTimer.current);
    };
  }, [id]);

  if (!config) return null;
  const cfg = own;
  if (!cfg) return null;
  const interactive = !cfg.clickThrough;

  /** 开始拖动边缘改尺寸：自动模式先锁定成"手动=当前尺寸"，再交给系统 resize 拖拽，
   *  拖动过程中 onResized 持续写回，结束后配置里就是最终宽高。 */
  const beginResize = async (e: React.MouseEvent, dir: ResizeDir) => {
    e.preventDefault();
    e.stopPropagation();
    const win = getCurrentWindow();
    try {
      if (!manual) {
        manualRef.current = true; // 让 onResized 立即开始写回
        const [size, scale] = await Promise.all([win.innerSize(), win.scaleFactor()]);
        // 自动模式下字幕盒可能小于窗口，用内容盒的实际逻辑尺寸更贴合
        const box = boxRef.current;
        const w = Math.max(40, Math.round(box?.offsetWidth ?? size.width / scale));
        const h = Math.max(28, Math.round(box?.offsetHeight ?? size.height / scale));
        await resizeOverlay(id, w, h);
      }
    } catch (err) {
      console.warn(err);
    }
    void win.startResizeDragging(dir).catch(console.warn);
  };

  // 右上角 ℹ️ 的悬浮信息：原样搬自主窗口卡片，改成浮窗自身的原生 tooltip。
  // 浮窗尺寸只包住字幕，自定义浮层会被窗口裁掉，所以这里用 title 多行文本。
  const info = [
    cfg.name || "未命名浮窗",
    `创建 ${formatDateTime(cfg.createdAt)} · 修改 ${formatDateTime(cfg.updatedAt || cfg.createdAt)}`,
    `尺寸 ${manual ? `${cfg.width} × ${cfg.height}` : "自动按内容"}`,
    `${cfg.shown ? "显示中" : "已隐藏"} · ${cfg.alwaysOnTop ? "置顶" : "不置顶"} · 鼠标穿透${cfg.clickThrough ? "已开启" : "关闭"}`,
    `位置 X ${cfg.x} · Y ${cfg.y}`,
    `变量 ${cfg.variables.length} 自定义 · ${cfg.computed.length} 计算`,
  ].join("\n");

  return (
    <div
      className={`group relative ${manual ? "h-screen w-screen" : ""}`}
      style={{ cursor: interactive ? "default" : "inherit" }}
    >
      <SubtitleView
        key={manual ? "manual" : "auto"}
        cfg={cfg}
        vars={vars}
        interactive={interactive}
        fill={manual}
        boxRef={boxRef}
      />

      {interactive && (
        <>
          {/* 四边 + 四角拖拽把手：透明热区盖在窗口边缘，按下即进入系统 resize 拖拽 */}
          {RESIZE_HANDLES.map((h) => (
            <div
              key={h.dir}
              onMouseDown={(e) => void beginResize(e, h.dir)}
              className={`absolute z-10 opacity-0 transition-opacity group-hover:opacity-100 hover:opacity-100 ${h.bg} ${h.className}`}
            />
          ))}
        </>
      )}

      {interactive && (
        <div className="absolute right-1 top-1 z-20 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
          <OverlayToolButton title={info} ariaLabel="关于这套浮窗">
            ℹ️
          </OverlayToolButton>
          {manual && (
            <OverlayToolButton
              title="恢复自动尺寸（按内容自适应）"
              onClick={() => resizeOverlay(id, 0, 0).catch(console.warn)}
            >
              ⤡
            </OverlayToolButton>
          )}
          <OverlayToolButton
            title={cfg.alwaysOnTop ? "点击取消置顶" : "点击置顶"}
            onClick={() => setOverlayAlwaysTop(id, !cfg.alwaysOnTop).catch(console.warn)}
          >
            {cfg.alwaysOnTop ? "📌" : "📍"}
          </OverlayToolButton>
          <OverlayToolButton
            title="锁定：鼠标穿透（之后请回主窗口解锁）"
            onClick={() => setOverlayClickThrough(id, true).catch(console.warn)}
          >
            🔒
          </OverlayToolButton>
          <OverlayToolButton
            title="隐藏浮窗"
            onClick={() => setOverlayShown(id, false).catch(console.warn)}
          >
            ✕
          </OverlayToolButton>
        </div>
      )}
    </div>
  );
}

/** 拖拽把手热区：每条只有几个逻辑像素宽/高，悬停时显示一条细线提示可拖 */
const RESIZE_HANDLES: Array<{ dir: ResizeDir; className: string; bg: string }> = [
  // 四角（优先级最高，放后面覆盖边）
  { dir: "NorthWest", bg: "hover:bg-indigo-400/50", className: "left-0 top-0 h-2.5 w-2.5 cursor-nwse-resize" },
  { dir: "NorthEast", bg: "hover:bg-indigo-400/50", className: "right-0 top-0 h-2.5 w-2.5 cursor-nesw-resize" },
  { dir: "SouthWest", bg: "hover:bg-indigo-400/50", className: "bottom-0 left-0 h-2.5 w-2.5 cursor-nesw-resize" },
  { dir: "SouthEast", bg: "hover:bg-indigo-400/50", className: "bottom-0 right-0 h-2.5 w-2.5 cursor-nwse-resize" },
  // 四边
  { dir: "North", bg: "hover:bg-indigo-400/40", className: "left-2.5 right-2.5 top-0 h-1.5 cursor-ns-resize" },
  { dir: "South", bg: "hover:bg-indigo-400/40", className: "bottom-0 left-2.5 right-2.5 h-1.5 cursor-ns-resize" },
  { dir: "West", bg: "hover:bg-indigo-400/40", className: "bottom-2.5 left-0 top-2.5 w-1.5 cursor-ew-resize" },
  { dir: "East", bg: "hover:bg-indigo-400/40", className: "bottom-2.5 right-0 top-2.5 w-1.5 cursor-ew-resize" },
];

function OverlayToolButton({
  title,
  ariaLabel,
  onClick,
  children,
}: {
  title: string;
  ariaLabel?: string;
  onClick?: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      title={title}
      aria-label={ariaLabel}
      onClick={onClick}
      className="flex h-6 w-6 items-center justify-center rounded-full border border-slate-500/60 bg-slate-900/85 text-[11px] leading-none text-slate-300 shadow transition hover:border-cyan-400 hover:text-white"
    >
      {children}
    </button>
  );
}
