/**
 * 排队队列内容渲染 —— 主窗口预览与排队弹窗共用，保证"所见即所得"。
 *
 * 排序在 `queues.ts` 的 sortedQueueEntries 里统一实现：
 * - time：按首次发送时间（先到先得）
 * - count：按发送次数（多的在前，次数相同先到先得）
 *
 * 分组色块显示在昵称与次数之间；入队 / 数字 +1 特效通过对比上一次渲染的
 * 成员 id 与次数触发，主窗口的「预览」也会注入一条临时示例成员。
 */
import { useEffect, useMemo, useRef, useState, type CSSProperties, type Ref } from "react";
import { groupColorMap, maskNickname, sortedQueueEntries } from "../queues";
import type { QueueConfig, QueueEntry } from "../types";
import { hexToRgba, TEXT_SHADOW } from "../utils";

/** 内边距常量：主窗口预览与弹窗共用（窗口尺寸直接按外框 offsetWidth/Height 设置） */
export const QUEUE_PAD = { x: 14, y: 10 };

/** 主窗口配置页请求预览一次特效（seq 每次点击递增） */
export interface QueueEffectPreview {
  kind: "enter" | "count";
  seq: number;
}

/** 根据背景色亮度选择黑/白文字，保证分组名在色块里清晰可读 */
function readableTextColor(hex: string): string {
  const value = hex.replace("#", "");
  if (value.length !== 6) return "#ffffff";
  const r = parseInt(value.slice(0, 2), 16);
  const g = parseInt(value.slice(2, 4), 16);
  const b = parseInt(value.slice(4, 6), 16);
  const luminance = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return luminance > 0.62 ? "#0f172a" : "#ffffff";
}

export default function QueueView({
  cfg,
  interactive = false,
  fill = false,
  boxRef,
  preview = null,
}: {
  cfg: QueueConfig;
  /** 是否允许拖拽（弹窗本体且未锁定鼠标穿透时为 true） */
  interactive?: boolean;
  /** 手动尺寸模式：撑满窗口（高度裁掉溢出内容） */
  fill?: boolean;
  /** 挂到内容外框上的 ref：弹窗用它测量真实内容尺寸 */
  boxRef?: Ref<HTMLDivElement>;
  /** 配置页「预览特效」：临时演示入队 / +1 动画，不影响真实名单 */
  preview?: QueueEffectPreview | null;
}) {
  const entries = useMemo(() => sortedQueueEntries(cfg), [cfg]);
  const visible = entries.slice(0, Math.max(1, cfg.maxEntries));
  const hidden = entries.length - visible.length;

  const colors = useMemo(() => groupColorMap(cfg.rules), [cfg.rules]);
  const demoGroup = useMemo(() => cfg.rules.find((r) => r.group.trim())?.group.trim() ?? "", [cfg.rules]);

  // 真实名单变化：新成员 → 入队特效；次数变大 → 数字特效。
  // 首次渲染不播（prev 为 null），避免弹窗打开时整列表一起动。
  const prevRef = useRef<{ ids: Set<number>; counts: Map<number, number> } | null>(null);
  const seqRef = useRef(0);
  const [enterFx, setEnterFx] = useState<Record<number, number>>({});
  const [countFx, setCountFx] = useState<Record<number, number>>({});
  const [demo, setDemo] = useState<QueueEntry | null>(null);

  useEffect(() => {
    const ids = new Set(cfg.entries.map((e) => e.id));
    const counts = new Map(cfg.entries.map((e) => [e.id, e.count] as const));
    const prev = prevRef.current;
    prevRef.current = { ids, counts };
    if (!prev) return;
    if (cfg.enterEffect !== "none") {
      const next: Record<number, number> = {};
      for (const entry of cfg.entries) {
        if (!prev.ids.has(entry.id)) next[entry.id] = ++seqRef.current;
      }
      if (Object.keys(next).length > 0) setEnterFx(next);
    }
    if (cfg.countEffect !== "none") {
      const next: Record<number, number> = {};
      for (const entry of cfg.entries) {
        const old = prev.counts.get(entry.id);
        if (old !== undefined && entry.count > old) next[entry.id] = ++seqRef.current;
      }
      if (Object.keys(next).length > 0) setCountFx(next);
    }
  }, [cfg.entries, cfg.enterEffect, cfg.countEffect]);

  // 配置页预览：入队插一条临时成员；+1 动第一行（没有成员时也插临时成员）
  const previewSeq = preview?.seq ?? 0;
  useEffect(() => {
    if (!preview) return;
    let timer: number | undefined;
    const fake = (id: number): QueueEntry => ({
      id,
      uid: 0,
      nickname: "预览新人",
      count: 1,
      firstAt: 0,
      lastAt: 0,
      group: demoGroup,
    });
    if (preview.kind === "enter") {
      const id = -1000 - (preview.seq % 1000);
      setDemo(fake(id));
      setEnterFx({ [id]: ++seqRef.current });
      timer = window.setTimeout(() => setDemo(null), 1800);
    } else {
      const first = visible[0];
      if (first) {
        setCountFx({ [first.id]: ++seqRef.current });
      } else {
        const id = -2000 - (preview.seq % 1000);
        setDemo(fake(id));
        setCountFx({ [id]: ++seqRef.current });
        timer = window.setTimeout(() => setDemo(null), 1800);
      }
    }
    return () => {
      if (timer) window.clearTimeout(timer);
      setDemo(null);
    };
  }, [previewSeq]); // eslint-disable-line react-hooks/exhaustive-deps

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
  const rows = demo ? [...visible, demo] : visible;
  // 0 = 沿用旧的自动字号（整体字号的 62%）；显式设置时按用户配置。
  const groupFont = cfg.groupFontSize > 0 ? cfg.groupFontSize : Math.max(10, Math.round(cfg.fontSize * 0.62));

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

      {rows.length === 0 ? (
        <div style={{ color: dim(60) }} className="py-1 text-center">
          等待弹幕…
        </div>
      ) : (
        <ol className="space-y-0.5">
          {rows.map((entry, index) => {
            const enterSeq = enterFx[entry.id];
            const countSeq = countFx[entry.id];
            const enterClass =
              enterSeq !== undefined && cfg.enterEffect !== "none" ? `qfx-enter-${cfg.enterEffect}` : "";
            const countClass =
              countSeq !== undefined && cfg.countEffect !== "none" ? `qfx-count-${cfg.countEffect}` : "";
            const group = entry.group.trim();
            const groupBg = group ? (colors.get(group) ?? "#64748b") : "";
            return (
              <li
                key={enterSeq !== undefined ? `${entry.id}:enter:${enterSeq}` : entry.id}
                className={`flex items-center gap-2 ${enterClass}`}
              >
                <span
                  className="shrink-0 text-right font-mono"
                  style={{
                    minWidth: `${String(Math.max(visible.length, 1)).length}ch`,
                    color: index < 3 ? "#fbbf24" : dim(65),
                  }}
                >
                  {index + 1}
                </span>
                <span className="min-w-0 flex-1 truncate">
                  {cfg.maskNickname ? maskNickname(entry.nickname) : entry.nickname}
                </span>
                {cfg.showGroup && group && (
                  <span
                    className="inline-flex shrink-0 items-center justify-center self-center rounded-md font-semibold"
                    style={{
                      backgroundColor: groupBg,
                      color: readableTextColor(groupBg),
                      fontSize: groupFont,
                      lineHeight: 1,
                      padding: "3px 6px",
                      textShadow: "none",
                    }}
                    title={`分组：${group}`}
                  >
                    {group}
                  </span>
                )}
                <span
                  key={countSeq !== undefined ? `${entry.id}:count:${countSeq}` : "count"}
                  className={`shrink-0 font-mono ${countClass}`}
                  style={{ color: dim(85) }}
                >
                  ×{entry.count}
                </span>
              </li>
            );
          })}
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
