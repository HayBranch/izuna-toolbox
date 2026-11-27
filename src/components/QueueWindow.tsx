/**
 * 排队队列弹窗的根组件（label 为 queue-{id} 的窗口渲染的就是它）。
 *
 * 行为与悬浮字幕浮窗一致：
 * 1. 监听 config-changed + 启动时 get_config，按自己的 id 渲染队列；
 * 2. 自动宽高：ResizeObserver 量内容 → set_queue_size 让窗口贴合内容，
 *    用户拖出来的宽高作为最小尺寸；关闭自动后才是固定尺寸；
 * 3. 未锁定鼠标穿透时悬停显示工具条：信息 / 重置位置 / 恢复自动尺寸 / 置顶 / 穿透 / 隐藏。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  getConfig,
  resetQueuePosition,
  resizeQueue,
  setQueueAlwaysTop,
  setQueueAutoSize,
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
  // 只记录用户拖拽；程序化自动调窗不写回宽高。
  const draggingRef = useRef(false);
  // Tauri 原生 resize 拖拽的 mouseup 常被系统窗口吞掉，用 resize 空闲计时器兜底解除。
  const dragIdleTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const measureRef = useRef<() => void>(() => {});

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
  // 自动模式下 width/height 是最小尺寸；关闭自动后才是固定尺寸。
  const manual = !!cfg && !cfg.autoSize;
  const minW = cfg?.width ?? 0;
  const minH = cfg?.height ?? 0;

  // 自动模式：量内容真实需要的尺寸扩窗，宽 / 高各自独立；测量盒不随窗口拉伸变化，不抖。
  // 拖拽期间暂停，避免和 startResizeDragging 抢宽度。
  useEffect(() => {
    if (manual) return;
    const measure = () => {
      if (draggingRef.current) return;
      const el = boxRef.current;
      if (!el) return;
      const w = Math.max(minW, Math.ceil(el.offsetWidth));
      const h = Math.max(minH, Math.ceil(el.offsetHeight));
      if (Math.abs(w - applied.current.w) <= 1 && Math.abs(h - applied.current.h) <= 1) return;
      applied.current = { w, h };
      setQueueSize(w, h).catch(console.warn);
    };
    measureRef.current = measure;
    const ro = new ResizeObserver(measure);
    if (boxRef.current) ro.observe(boxRef.current);
    measure();
    return () => ro.disconnect();
  }, [manual, config !== null, minW, minH]); // eslint-disable-line react-hooks/exhaustive-deps

  // 成员增减 / 配置变化时主动量一次，补上 ResizeObserver 不会触发的情况。
  useEffect(() => {
    if (!manual && cfg) measureRef.current();
  }, [cfg, manual]); // eslint-disable-line react-hooks/exhaustive-deps

  // 手动模式拖动边缘 → 防抖写回配置（主窗口能看到当前尺寸）
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const un = win.onResized(({ payload }) => {
      if (!alive) return;
      // 拖完停手 ~250ms 后必定解除拖拽态，避免 mouseup 丢失导致自动扩窗失效。
      if (draggingRef.current) {
        if (dragIdleTimer.current) clearTimeout(dragIdleTimer.current);
        dragIdleTimer.current = setTimeout(() => {
          draggingRef.current = false;
          measureRef.current();
        }, 250);
      }
      // 自动调窗自己触发的 resize 不写回，否则会把最小尺寸覆盖成内容尺寸。
      if (!draggingRef.current) return;
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
      if (dragIdleTimer.current) clearTimeout(dragIdleTimer.current);
    };
  }, [id]);

  if (!config || !cfg) return null;
  const interactive = !cfg.clickThrough;

  /** 拖动边缘改尺寸：不退出自动模式，拖出来的宽高作为最小尺寸写回。 */
  const beginResize = (e: React.MouseEvent, dir: ResizeDir) => {
    e.preventDefault();
    e.stopPropagation();
    draggingRef.current = true;
    const stop = () => {
      draggingRef.current = false;
      window.removeEventListener("mouseup", stop);
      window.removeEventListener("pointerup", stop);
      measureRef.current();
    };
    window.addEventListener("mouseup", stop);
    window.addEventListener("pointerup", stop);
    void getCurrentWindow().startResizeDragging(dir).catch(console.warn);
  };

  const info = [
    cfg.name || "未命名队列",
    `创建 ${formatDateTime(cfg.createdAt)} · 修改 ${formatDateTime(cfg.updatedAt || cfg.createdAt)}`,
    `直播间 ${cfg.roomId > 0 ? cfg.roomId : "未设置"} · 刷新间隔 ${cfg.pollSecs} 秒`,
    `排序 ${cfg.sortMode === "count" ? "按发送次数" : "按发送时间"} · 队列 ${cfg.entries.length} 人`,
    `尺寸 ${manual ? `固定 ${cfg.width} × ${cfg.height}` : `自动按内容${cfg.width > 0 || cfg.height > 0 ? `（最小 ${cfg.width} × ${cfg.height}）` : ""}`}`,
    `${cfg.shown ? "显示中" : "已隐藏"} · ${cfg.alwaysOnTop ? "置顶" : "不置顶"} · 鼠标穿透${cfg.clickThrough ? "已开启" : "关闭"}`,
    `位置 X ${cfg.x} · Y ${cfg.y}`,
  ].join("\n");

  return (
    <div className="group relative h-screen w-screen overflow-hidden">
      {/* 可见内容铺满窗口；自动模式另用隐藏盒测自然尺寸，不随窗口变化，拖拽时不测量。 */}
      <div className="relative h-full w-full">
        {!manual && (
          <div className="pointer-events-none absolute left-0 top-0 -z-10 invisible" aria-hidden>
            <QueueView cfg={cfg} boxRef={boxRef} />
          </div>
        )}
        <QueueView
          key={manual ? "manual" : "auto"}
          cfg={cfg}
          interactive={interactive}
          fill
        />

        {interactive && (
          <div className="absolute right-1 top-1 z-20 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
            <PopupToolButton title={info} ariaLabel="关于这个排队弹窗">
              ℹ️
            </PopupToolButton>
            <PopupToolButton
              title="重置位置（移回当前屏幕中央，拖出屏幕后可用）"
              ariaLabel="重置排队弹窗位置"
              onClick={() => resetQueuePosition(id).catch(console.warn)}
            >
              🎯
            </PopupToolButton>
            <PopupToolButton
              title={cfg.autoSize ? "关闭自动宽高（固定为当前尺寸）" : "开启自动宽高（当前尺寸作最小尺寸）"}
              ariaLabel="切换自动宽高"
              onClick={() => setQueueAutoSize(id, !cfg.autoSize).catch(console.warn)}
            >
              ⇔
            </PopupToolButton>
            {(manual || cfg.width > 0 || cfg.height > 0) && (
              <PopupToolButton
                title={manual ? "恢复自动宽高" : "清除最小宽高，完全按内容自适应"}
                ariaLabel="恢复自动尺寸"
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

      {interactive && <PopupResizeHandles onStart={(e, dir) => void beginResize(e, dir)} />}
    </div>
  );
}
