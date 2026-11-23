/** 透明弹窗（悬浮字幕 / 排队弹窗）共用的拖拽热区与工具按钮。 */
import type { ReactNode } from "react";

/** 对齐 Tauri 的 ResizeDirection（该类型未导出，这里手写同形） */
export type ResizeDir =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

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

/** 四边 + 四角透明热区：按下交给系统 startResizeDragging */
export function PopupResizeHandles({
  onStart,
}: {
  onStart: (e: React.MouseEvent, dir: ResizeDir) => void;
}) {
  return (
    <>
      {RESIZE_HANDLES.map((h) => (
        <div
          key={h.dir}
          onMouseDown={(e) => onStart(e, h.dir)}
          className={`absolute z-10 opacity-0 transition-opacity group-hover:opacity-100 hover:opacity-100 ${h.bg} ${h.className}`}
        />
      ))}
    </>
  );
}

/** 浮窗右上角悬停出现的小圆按钮 */
export function PopupToolButton({
  title,
  ariaLabel,
  onClick,
  children,
}: {
  title: string;
  ariaLabel?: string;
  onClick?: () => void;
  children: ReactNode;
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
