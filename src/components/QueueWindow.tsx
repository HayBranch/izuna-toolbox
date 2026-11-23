/**
 * 排队队列弹窗的根组件（label 为 queue-{id} 的窗口渲染的就是它）。
 *
 * 行为与悬浮字幕浮窗一致：
 * 1. 监听 config-changed + 启动时 get_config，按自己的 id 渲染队列；
 * 2. 自动尺寸：ResizeObserver 量内容 → set_queue_size 让窗口贴合内容；
 *    手动尺寸：拖动边缘改宽高（startResizeDragging），拖完写回配置；
 * 3. 未锁定鼠标穿透时悬停显示工具条：信息 / 恢复自动尺寸 / 置顶 / 穿透 / 隐藏。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  getConfig,
  resizeQueue,
  setQueueAlwaysTop,
  setQueueClickThrough,
  setQueueShown,
  setQueueSize,
  subscribeConfig,
} from "../api";
import type { AppConfig } from "../types";
import { formatDateTime } from "../utils";
import { PopupResizeHandles, PopupToolButton, type ResizeDir } from "./popup";
import QueueView from "./QueueView";

function parseId(): number {
  const label = getCurrentWindow().label; // queue-{id}
  const id = Number(label.slice("queue-".length));
  return Number.isFinite(id) ? id : -1;
}

export default function QueueWindow() {
  const id = useMemo(parseId, []);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const applied = useRef({ w: 0, h: 0 });
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const manualRef = useRef(false);

  // 透明背景：弹窗页面不能有底色（见 index.css 的 .overlay-mode）
  useEffect(() => {
    document.documentElement.classList.add("overlay-mode");
    return () => document.documentElement.classList.remove("overlay-mode");
  }, []);

  // 订阅配置：先监听再拉取，避免中间窗口期丢更新
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;

    const apply = (c: AppConfig) => {
      if (c.queues.some((q) => q.id === id)) setConfig(c);
    };

    const off = listen<AppConfig>("config-changed", (e) => apply(e.payload));
    off.then((f) => {
      if (alive) unlisten = f;
      else f();
    });

    getConfig().then(apply).catch(console.warn);
    const offBus = subscribeConfig(apply);

    return () => {
      alive = false;
      unlisten?.();
      offBus();
    };
  }, [id]);

  const cfg = config?.queues.find((q) => q.id === id);
  const manual = !!cfg && cfg.width > 0 && cfg.height > 0;
  manualRef.current = manual;

  // 自动模式：内容尺寸 → 窗口尺寸（带 1px 迟滞，避免测量与改窗口互相抖动）
  useEffect(() => {
    if (manual) return;
    const el = boxRef.current;
    if (!el) return;
    const measure = () => {
      const w = Math.ceil(el.offsetWidth);
      const h = Math.ceil(el.offsetHeight);
      if (Math.abs(w - applied.current.w) > 1 || Math.abs(h - applied.current.h) > 1) {
        applied.current = { w, h };
        setQueueSize(w, h).catch(console.warn);
      }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [manual, config !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  // 手动模式拖动边缘 → 防抖写回配置（主窗口能看到当前尺寸）
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
          await resizeQueue(id, w, h);
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

  if (!config || !cfg) return null;
  const interactive = !cfg.clickThrough;

  /** 拖动边缘改尺寸：自动模式先锁定成当前内容尺寸，再交给系统 resize 拖拽 */
  const beginResize = async (e: React.MouseEvent, dir: ResizeDir) => {
    e.preventDefault();
    e.stopPropagation();
    const win = getCurrentWindow();
    try {
      if (!manual) {
        manualRef.current = true;
        const [size, scale] = await Promise.all([win.innerSize(), win.scaleFactor()]);
        const box = boxRef.current;
        const w = Math.max(60, Math.round(box?.offsetWidth ?? size.width / scale));
        const h = Math.max(40, Math.round(box?.offsetHeight ?? size.height / scale));
        await resizeQueue(id, w, h);
      }
    } catch (err) {
      console.warn(err);
    }
    void win.startResizeDragging(dir).catch(console.warn);
  };

  const info = [
    cfg.name || "未命名队列",
    `创建 ${formatDateTime(cfg.createdAt)} · 修改 ${formatDateTime(cfg.updatedAt || cfg.createdAt)}`,
    `直播间 ${cfg.roomId > 0 ? cfg.roomId : "未设置"} · 刷新间隔 ${cfg.pollSecs} 秒`,
    `排序 ${cfg.sortMode === "count" ? "按发送次数" : "按发送时间"} · 队列 ${cfg.entries.length} 人`,
    `尺寸 ${manual ? `${cfg.width} × ${cfg.height}` : "自动按内容"}`,
    `${cfg.shown ? "显示中" : "已隐藏"} · ${cfg.alwaysOnTop ? "置顶" : "不置顶"} · 鼠标穿透${cfg.clickThrough ? "已开启" : "关闭"}`,
    `位置 X ${cfg.x} · Y ${cfg.y}`,
  ].join("\n");

  return (
    <div className={`group relative ${manual ? "h-screen w-screen" : ""}`}>
      <QueueView
        key={manual ? "manual" : "auto"}
        cfg={cfg}
        interactive={interactive}
        fill={manual}
        boxRef={boxRef}
      />

      {interactive && <PopupResizeHandles onStart={(e, dir) => void beginResize(e, dir)} />}

      {interactive && (
        <div className="absolute right-1 top-1 z-20 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
          <PopupToolButton title={info} ariaLabel="关于这个排队弹窗">
            ℹ️
          </PopupToolButton>
          {manual && (
            <PopupToolButton
              title="恢复自动尺寸（按内容自适应）"
              onClick={() => resizeQueue(id, 0, 0).catch(console.warn)}
            >
              ⤡
            </PopupToolButton>
          )}
          <PopupToolButton
            title={cfg.alwaysOnTop ? "点击取消置顶" : "点击置顶"}
            onClick={() => setQueueAlwaysTop(id, !cfg.alwaysOnTop).catch(console.warn)}
          >
            {cfg.alwaysOnTop ? "📌" : "📍"}
          </PopupToolButton>
          <PopupToolButton
            title="锁定：鼠标穿透（之后请回主窗口解锁）"
            onClick={() => setQueueClickThrough(id, true).catch(console.warn)}
          >
            🔒
          </PopupToolButton>
          <PopupToolButton
            title="隐藏这个排队弹窗"
            onClick={() => setQueueShown(id, false).catch(console.warn)}
          >
            ✕
          </PopupToolButton>
        </div>
      )}
    </div>
  );
}
