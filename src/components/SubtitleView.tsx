/**
 * 悬浮字幕内容渲染 —— 主窗口预览和浮窗本体共用，保证"所见即所得"。
 * interactive = true 时（浮窗本体、未开鼠标穿透）整块可以作为拖拽区域。
 */
import { useMemo, type CSSProperties, type Ref } from "react";
import type { OverlayConfig } from "../types";
import { hexToRgba, TEXT_SHADOW } from "../utils";
import { tokenize, type Vars } from "../vars";

/** 内边距常量：主窗口预览与浮窗共用（窗口尺寸直接按外框 offsetWidth/Height 设置） */
export const SUBTITLE_PAD = { x: 16, y: 6 };

export default function SubtitleView({
  cfg,
  vars,
  interactive = false,
  boxRef,
}: {
  cfg: OverlayConfig;
  vars: Vars;
  /** 是否允许拖拽（浮窗本体且未锁定穿透时为 true） */
  interactive?: boolean;
  /** 挂到字幕外框上的 ref：浮窗用它测量真实内容尺寸 */
  boxRef?: Ref<HTMLDivElement>;
}) {
  const tokens = useMemo(() => tokenize(cfg.template, vars), [cfg.template, vars]);

  // "deep"：整个字幕盒（含内部文本/变量 span）都能触发拖动。
  // 裸的 "true" 只认点击目标本身，点在文字上时 target 是内层 span，拖不动。
  const dragProps = interactive ? { "data-tauri-drag-region": "deep" } : {};

  const style: CSSProperties = {
    background: hexToRgba(cfg.bgColor, cfg.bgOpacity),
    color: cfg.fontColor,
    fontSize: cfg.fontSize,
    fontWeight: cfg.bold ? 700 : 500,
    textShadow: cfg.shadow ? TEXT_SHADOW : "none",
    borderRadius: 12,
    padding: `${SUBTITLE_PAD.y}px ${SUBTITLE_PAD.x}px`,
    lineHeight: 1.5,
    // pre：仅模板里的回车换行；pre-wrap：回车 + 长行按最大宽度折行
    whiteSpace: cfg.wrap ? "pre-wrap" : "pre",
    // 关键：按内容真实宽度排版，不受当前窗口宽度反向约束。
    // 否则窗口从小尺寸起步时，文字先按窄窗口折行，测量结果永远长不大，
    // 最大宽度调大也看不出效果。maxWidth 负责真正的宽度上限。
    width: "max-content",
    maxWidth: cfg.wrap && cfg.maxWidth > 0 ? cfg.maxWidth : undefined,
    // 超宽的连续字符（无空格长串）也强制折进最大宽度内
    overflowWrap: cfg.wrap ? "anywhere" : undefined,
    userSelect: "none",
    cursor: interactive ? "default" : "inherit",
  };

  return (
    <div ref={boxRef} style={style} {...dragProps} className="inline-block">
      <span>
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
