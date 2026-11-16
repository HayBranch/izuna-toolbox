/**
 * 悬浮字幕浮窗的根组件（label 为 overlay-{id} 的窗口渲染的就是它）。
 *
 * 职责：
 * 1. 监听 config-changed 事件 + 启动时 get_config，按自己的 id 渲染模板；
 * 2. 用 ResizeObserver 量内容尺寸，调 set_overlay_size 让窗口贴合内容；
 * 3. 每 30s 刷新一次时间类变量（{today} 跨天也能自动更新）；
 * 4. 未开鼠标穿透时，悬停显示小工具条：切换置顶 / 锁定穿透 / 隐藏，
 *    穿透开启后浮窗收不到鼠标事件，只能回主窗口解锁（主窗口始终可以控制）。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import {
  getConfig,
  setOverlayAlwaysTop,
  setOverlayClickThrough,
  setOverlayShown,
  setOverlaySize,
} from "../api";
import type { AppConfig } from "../types";
import { computeVars } from "../vars";
import SubtitleView, { SUBTITLE_PAD } from "./SubtitleView";

function parseId(): number {
  const label = getCurrentWindow().label; // overlay-{id}
  const id = Number(label.slice("overlay-".length));
  return Number.isFinite(id) ? id : -1;
}

export default function OverlayWindow() {
  const id = useMemo(parseId, []);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [tick, setTick] = useState(0);
  // 测量目标：文字内层元素。它的尺寸只由内容决定，不受窗口宽度约束，
  // 模板再长也能量出真实宽度（窗口宽度由 Rust 侧夹到显示器范围内）。
  const spanRef = useRef<HTMLSpanElement>(null);
  const applied = useRef({ w: 0, h: 0 });

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

    const timer = setInterval(() => setTick((t) => t + 1), 30_000);

    return () => {
      alive = false;
      unlisten?.();
      clearInterval(timer);
    };
  }, [id]);

  // 文字尺寸 + 内边距 → 窗口尺寸（带 1px 迟滞，避免 ResizeObserver ↔ set_size 抖动循环）
  useEffect(() => {
    const el = spanRef.current;
    if (!el) return;
    const measure = () => {
      const w = Math.ceil(el.offsetWidth) + SUBTITLE_PAD.x * 2;
      const h = Math.ceil(el.offsetHeight) + SUBTITLE_PAD.y * 2;
      if (Math.abs(w - applied.current.w) > 1 || Math.abs(h - applied.current.h) > 1) {
        applied.current = { w, h };
        setOverlaySize(w, h).catch(console.warn);
      }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [config !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  // 本套浮窗自己的变量（tick 用于 30s 刷新时间类变量）；在 early return 之前求，保证 hooks 顺序稳定
  const vars = useMemo(() => {
    const own = config?.overlays.find((o) => o.id === id);
    return own ? computeVars(own, new Date()) : {};
  }, [config, tick, id]);

  if (!config) return null;
  const cfg = config.overlays.find((o) => o.id === id);
  if (!cfg) return null;
  const interactive = !cfg.clickThrough;

  return (
    <div className="group relative">
      <SubtitleView cfg={cfg} vars={vars} interactive={interactive} innerRef={spanRef} />

      {interactive && (
        <div className="absolute right-1 top-1 z-10 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
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

function OverlayToolButton({
  title,
  onClick,
  children,
}: {
  title: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className="flex h-6 w-6 items-center justify-center rounded-full border border-slate-500/60 bg-slate-900/85 text-[11px] leading-none text-slate-300 shadow transition hover:border-cyan-400 hover:text-white"
    >
      {children}
    </button>
  );
}
