/**
 * 排队队列内容渲染 —— 主窗口预览与排队弹窗共用，保证"所见即所得"。
 *
 * 排序在 `queues.ts` 的 sortedQueueEntries 里统一实现：
 * - time：按首次发送时间（先到先得）
 * - count：按发送次数（多的在前，次数相同先到先得）
 */
import { useMemo, type CSSProperties, type Ref } from "react";
import { sortedQueueEntries } from "../queues";
import type { QueueConfig } from "../types";
import { hexToRgba, TEXT_SHADOW } from "../utils";

/** 内边距常量：主窗口预览与弹窗共用（窗口尺寸直接按外框 offsetWidth/Height 设置） */
export const QUEUE_PAD = { x: 14, y: 10 };

export default function QueueView({
  cfg,
  interactive = false,
  fill = false,
  boxRef,
}: {
  cfg: QueueConfig;
  /** 是否允许拖拽（弹窗本体且未锁定鼠标穿透时为 true） */
  interactive?: boolean;
  /** 手动尺寸模式：撑满窗口（高度裁掉溢出内容） */
  fill?: boolean;
  /** 挂到内容外框上的 ref：弹窗用它测量真实内容尺寸 */
  boxRef?: Ref<HTMLDivElement>;
}) {
  const entries = useMemo(() => sortedQueueEntries(cfg), [cfg]);
  const visible = entries.slice(0, Math.max(1, cfg.maxEntries));
  const hidden = entries.length - visible.length;

  // "deep"：整块（含内部文本）都能触发拖动，否则点在文字上时拖不动
  const dragProps = interactive ? { "data-tauri-drag-region": "deep" } : {};

  const style: CSSProperties = {
    background: hexToRgba(cfg.bgColor, cfg.bgOpacity),
    color: cfg.fontColor,
    fontSize: cfg.fontSize,
    fontWeight: cfg.bold ? 700 : 500,
    textShadow: cfg.shadow ? TEXT_SHADOW : "none",
    borderRadius: 12,
    padding: `${QUEUE_PAD.y}px ${QUEUE_PAD.x}px`,
    lineHeight: 1.5,
    width: fill ? "100%" : "max-content",
    height: fill ? "100%" : undefined,
    maxWidth: fill ? "none" : cfg.maxWidth > 0 ? cfg.maxWidth : undefined,
    overflow: "hidden",
    boxSizing: fill ? "border-box" : undefined,
    userSelect: "none",
    cursor: interactive ? "default" : "inherit",
  };

  const dim = (pct: number) => hexToRgba(cfg.fontColor, pct);

  return (
    <div ref={boxRef} style={style} {...dragProps} className={fill ? "block" : "inline-block"}>
      <div
        className="flex items-center justify-between gap-3"
        style={{
          fontSize: Math.max(11, Math.round(cfg.fontSize * 0.72)),
          fontWeight: 600,
          color: dim(70),
          paddingBottom: 4,
          marginBottom: 4,
          borderBottom: `1px solid ${dim(22)}`,
        }}
      >
        <span className="min-w-0 truncate">{cfg.name || "排队队列"}</span>
        <span className="shrink-0 font-mono">{entries.length} 人</span>
      </div>

      {visible.length === 0 ? (
        <div style={{ color: dim(60) }} className="py-1 text-center">
          等待弹幕…
        </div>
      ) : (
        <ol className="space-y-0.5">
          {visible.map((entry, index) => (
            <li key={entry.id} className="flex items-baseline gap-2">
              <span
                className="shrink-0 text-right font-mono"
                style={{
                  minWidth: `${String(visible.length).length}ch`,
                  color: index < 3 ? "#fbbf24" : dim(65),
                }}
              >
                {index + 1}
              </span>
              <span className="min-w-0 flex-1 truncate">{entry.nickname}</span>
              <span className="shrink-0 font-mono" style={{ color: dim(85) }}>
                ×{entry.count}
              </span>
            </li>
          ))}
        </ol>
      )}

      {hidden > 0 && (
        <div
          className="pt-1 text-center"
          style={{ fontSize: Math.max(10, Math.round(cfg.fontSize * 0.65)), color: dim(55) }}
        >
          还有 {hidden} 人未显示
        </div>
      )}
    </div>
  );
}
