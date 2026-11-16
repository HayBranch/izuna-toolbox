/**
 * 悬浮字幕内容渲染 —— 主窗口预览和浮窗本体共用，保证"所见即所得"。
 * interactive = true 时（浮窗本体、未开鼠标穿透）整块可以作为拖拽区域。
 */
import { useMemo, type CSSProperties, type Ref } from "react";
import type { OverlayConfig } from "../types";
import { hexToRgba, TEXT_SHADOW } from "../utils";
import { tokenize, type Vars } from "../vars";

/** 内边距常量：浮窗窗口自适应尺寸时要用（内容尺寸 + 内边距 = 窗口尺寸） */
export const SUBTITLE_PAD = { x: 16, y: 6 };

export default function SubtitleView({
  cfg,
  vars,
  interactive = false,
  innerRef,
}: {
  cfg: OverlayConfig;
  vars: Vars;
  /** 是否允许拖拽（浮窗本体且未锁定穿透时为 true） */
  interactive?: boolean;
  /** 挂到文字内层元素上的 ref：浮窗用它测量真实内容尺寸 */
  innerRef?: Ref<HTMLSpanElement>;
}) {
  const tokens = useMemo(() => tokenize(cfg.template, vars), [cfg.template, vars]);

  const dragProps = interactive ? { "data-tauri-drag-region": "true" } : {};

  const style: CSSProperties = {
    background: hexToRgba(cfg.bgColor, cfg.bgOpacity),
    color: cfg.fontColor,
    fontSize: cfg.fontSize,
    fontWeight: cfg.bold ? 700 : 500,
    textShadow: cfg.shadow ? TEXT_SHADOW : "none",
    borderRadius: 12,
    padding: `${SUBTITLE_PAD.y}px ${SUBTITLE_PAD.x}px`,
    lineHeight: 1.5,
    whiteSpace: "nowrap",
    userSelect: "none",
    cursor: interactive ? "default" : "inherit",
  };

  return (
    <div style={style} {...dragProps} className="inline-block">
      <span ref={innerRef} {...dragProps}>
        {tokens.map((t, i) =>
          t.isVar ? (
            // 变量渲染成虚线下划线样式，比纯文本更容易看出"这里会变"
            <span key={i} style={{ color: "inherit" }} className="underline decoration-dotted decoration-1 underline-offset-4">
              {t.text}
            </span>
          ) : (
            <span key={i}>{t.text}</span>
          ),
        )}
      </span>
    </div>
  );
}
