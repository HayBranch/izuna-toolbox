/**
 * 悬浮字幕浮窗的根组件（label 为 overlay-{id} 的窗口渲染的就是它）。
 *
 * 职责：
 * 1. 监听 config-changed 事件 + 启动时 get_config，按自己的 id 渲染模板；
 * 2. 尺寸两种模式：
 *    - 自动（默认）：ResizeObserver 量内容尺寸 → set_overlay_size 让窗口贴合内容；
 *      用户拖出来的宽高作为最小尺寸，内容超出时继续自动变大；
 *    - 固定：取消「自动宽高」后字幕撑满窗口（宽度决定折行），窗口不再按内容变化。
 * 3. 每 30s 刷新一次时间类变量（{today} 跨天也能自动更新）；
 * 4. 未开鼠标穿透时，悬停显示小工具条：关于 / 重置位置 / 恢复自动尺寸 / 置顶 / 锁定穿透 / 隐藏，
 *    穿透开启后浮窗收不到鼠标事件，只能回主窗口解锁（主窗口始终可以控制）。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import {
  getConfig,
  resetOverlayPosition,
  resizeOverlay,
  setOverlayAlwaysTop,
  setOverlayClickThrough,
  setOverlayAutoSize,
  setOverlayShown,
  setOverlaySize,
  subscribeConfig,
} from "../api";
import type { AppConfig } from "../types";
import { formatDateTime } from "../utils";
import { computeVars } from "../vars";
import { PopupResizeHandles, PopupToolButton, type ResizeDir } from "./popup";
import SubtitleView from "./SubtitleView";

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
  // 只有用户拖拽触发的 resize 才写回宽高；程序化自动调窗不能覆盖手动尺寸。
  const draggingRef = useRef(false);
  // Tauri 原生 resize 拖拽时 mouseup 常被系统窗口吞掉，webview 收不到，
  // draggingRef 会卡在 true 导致自动扩窗永久失效。用 resize 事件驱动的空闲计时器兜底。
  const dragIdleTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 保存最新测量函数，供拖拽结束 / 配置变化时主动触发。
  const measureRef = useRef<() => void>(() => {});

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

  // 自动模式：width/height 只是最小尺寸；关闭自动后才是固定尺寸。
  const manual = !!own && !own.autoSize;
  const minW = own?.width ?? 0;
  const minH = own?.height ?? 0;

  // 自动模式：量「内容真实需要的尺寸」扩窗，宽 / 高各自独立，不小于用户拖出的下限。
  // 测量盒固定按内容排版（width:max-content，受 maxWidth 约束），尺寸不随窗口拉伸变化，
  // 因此不会「扩窗 → 重新排版 → 再扩」自激抖动。拖拽期间暂停，避免和 startResizeDragging 抢宽度。
  useEffect(() => {
    if (manual) return;
    const measure = () => {
      if (draggingRef.current) return;
      const el = boxRef.current;
      if (!el) return;
      // 1px 内是取整/亚像素误差，忽略，避免反复 +1 抖动。
      const w = Math.max(minW, Math.ceil(el.offsetWidth));
      const h = Math.max(minH, Math.ceil(el.offsetHeight));
      if (Math.abs(w - applied.current.w) <= 1 && Math.abs(h - applied.current.h) <= 1) return;
      applied.current = { w, h };
      setOverlaySize(w, h).catch(console.warn);
    };
    measureRef.current = measure;
    const ro = new ResizeObserver(measure);
    if (boxRef.current) ro.observe(boxRef.current);
    measure();
    return () => ro.disconnect();
  }, [manual, config !== null, minW, minH]); // eslint-disable-line react-hooks/exhaustive-deps

  // 内容变化（换模板 / 变量刷新 / 成员增减）时主动量一次：ResizeObserver 只在盒子
  // 尺寸变化时触发，若内容变了但盒子尺寸恰好没变，就不会自动扩——这里补上。
  useEffect(() => {
    if (!manual && tick >= 0) measureRef.current();
  }, [own, tick, manual]); // eslint-disable-line react-hooks/exhaustive-deps

  // 窗口被拖动宽高（手动模式）→ 防抖写回配置，主窗口能看到当前尺寸。
  // 始终订阅，处理函数里再按"当前是否手动"决定是否写回：从自动切手动的瞬间
  // 订阅已经就位，不会漏掉刚开始的 resize 事件。
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const un = win.onResized(({ payload }) => {
      if (!alive) return;
      // 拖拽期间每次 resize 都续一次「空闲计时器」：停手 ~250ms 后必定解除拖拽态，
      // 不依赖 webview 能否收到 mouseup。否则 draggingRef 卡住会让自动扩窗彻底失效。
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
      if (dragIdleTimer.current) clearTimeout(dragIdleTimer.current);
    };
  }, [id]);

  if (!config) return null;
  const cfg = own;
  if (!cfg) return null;
  const interactive = !cfg.clickThrough;

  /** 拖动边缘改尺寸：不再退出自动模式，拖出来的宽高作为自动模式的最小尺寸写回。 */
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

  // 右上角 ℹ️ 的悬浮信息：原样搬自主窗口卡片，改成浮窗自身的原生 tooltip。
  // 浮窗尺寸只包住字幕，自定义浮层会被窗口裁掉，所以这里用 title 多行文本。
  const info = [
    cfg.name || "未命名浮窗",
    `创建 ${formatDateTime(cfg.createdAt)} · 修改 ${formatDateTime(cfg.updatedAt || cfg.createdAt)}`,
    `尺寸 ${manual ? `固定 ${cfg.width} × ${cfg.height}` : `自动按内容${cfg.width > 0 || cfg.height > 0 ? `（最小 ${cfg.width} × ${cfg.height}）` : ""}`}`,
    `${cfg.shown ? "显示中" : "已隐藏"} · ${cfg.alwaysOnTop ? "置顶" : "不置顶"} · 鼠标穿透${cfg.clickThrough ? "已开启" : "关闭"}`,
    `位置 X ${cfg.x} · Y ${cfg.y}`,
    `变量 ${cfg.variables.length} 自定义 · ${cfg.computed.length} 计算`,
  ].join("\n");

  return (
    <div
      className="group relative h-screen w-screen overflow-hidden"
      style={{ cursor: interactive ? "default" : "inherit" }}
    >
      {/* 可见内容始终铺满窗口，背景/圆角与实际占用一致。自动模式另用隐藏盒按内容
          自然尺寸测量——它的尺寸不随窗口变化，所以扩窗不会自激抖动。 */}
      <div className="relative h-full w-full">
        {!manual && (
          <div className="pointer-events-none absolute left-0 top-0 -z-10 invisible" aria-hidden>
            <SubtitleView cfg={cfg} vars={vars} boxRef={boxRef} />
          </div>
        )}
        <SubtitleView
          key={manual ? "manual" : "auto"}
          cfg={cfg}
          vars={vars}
          interactive={interactive}
          fill
        />

        {interactive && (
          <div className="absolute right-1 top-1 z-20 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
            <PopupToolButton title={info} ariaLabel="关于这套浮窗">
              ℹ️
            </PopupToolButton>
            <PopupToolButton
              title="重置位置（移回当前屏幕中央，拖出屏幕后可用）"
              ariaLabel="重置浮窗位置"
              onClick={() => resetOverlayPosition(id).catch(console.warn)}
            >
              🎯
            </PopupToolButton>
            <PopupToolButton
              title={cfg.autoSize ? "关闭自动宽高（固定为当前尺寸）" : "开启自动宽高（当前尺寸作最小尺寸）"}
              ariaLabel="切换自动宽高"
              onClick={() => setOverlayAutoSize(id, !cfg.autoSize).catch(console.warn)}
            >
              ⇔
            </PopupToolButton>
            {(manual || cfg.width > 0 || cfg.height > 0) && (
              <PopupToolButton
                title={manual ? "恢复自动宽高" : "清除最小宽高，完全按内容自适应"}
                ariaLabel="恢复自动尺寸"
                onClick={() => resizeOverlay(id, 0, 0).catch(console.warn)}
              >
                ⤡
              </PopupToolButton>
            )}
            <PopupToolButton
              title={cfg.alwaysOnTop ? "点击取消置顶" : "点击置顶"}
              onClick={() => setOverlayAlwaysTop(id, !cfg.alwaysOnTop).catch(console.warn)}
            >
              {cfg.alwaysOnTop ? "📌" : "📍"}
            </PopupToolButton>
            <PopupToolButton
              title="锁定：鼠标穿透（之后请回主窗口解锁）"
              onClick={() => setOverlayClickThrough(id, true).catch(console.warn)}
            >
              🔒
            </PopupToolButton>
            <PopupToolButton
              title="隐藏浮窗"
              onClick={() => setOverlayShown(id, false).catch(console.warn)}
            >
              ✕
            </PopupToolButton>
          </div>
        )}
      </div>

      {interactive && (
        <PopupResizeHandles onStart={(e, dir) => void beginResize(e, dir)} />
      )}
    </div>
  );
}
