/** 主窗口表单通用小控件：悬浮字幕配置卡与排队队列页共用，保持两页观感一致。 */
import { useEffect, useRef, useState } from "react";

/** 列表里的状态开关标签（点击直接切换，不用进详情） */
export function ChipToggle({
  on,
  onLabel,
  offLabel,
  title,
  onClick,
  tone = "default",
}: {
  on: boolean;
  onLabel: string;
  offLabel: string;
  title: string;
  onClick: () => void;
  tone?: "default" | "amber" | "emerald";
}) {
  const onCls =
    tone === "amber"
      ? "border-amber-500/40 bg-amber-500/10 text-amber-700 hover:bg-amber-500/20 dark:text-amber-300"
      : tone === "emerald"
        ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 hover:bg-emerald-500/20 dark:text-emerald-300"
        : "border-cyan-500/40 bg-cyan-500/10 text-cyan-700 hover:bg-cyan-500/20 dark:text-cyan-300";
  return (
    <button
      type="button"
      aria-pressed={on}
      title={title}
      onClick={onClick}
      className={`rounded-full border px-2 py-0.5 text-[11px] transition focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500 ${
        on
          ? onCls
          : "border-slate-200 bg-slate-100 text-slate-400 hover:border-slate-300 hover:text-slate-600 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-500 dark:hover:text-slate-300"
      }`}
    >
      {on ? onLabel : offLabel}
    </button>
  );
}

export function Toggle({
  on,
  label,
  onClick,
  tone = "default",
}: {
  on: boolean;
  label: string;
  onClick: () => void;
  tone?: "default" | "amber" | "emerald";
}) {
  const onCls =
    tone === "amber"
      ? "border-amber-500/50 bg-amber-500/15 text-amber-700 dark:text-amber-300"
      : tone === "emerald"
        ? "border-emerald-500/50 bg-emerald-500/15 text-emerald-700 dark:text-emerald-300"
        : "border-cyan-500/50 bg-cyan-500/15 text-cyan-700 dark:text-cyan-300";
  return (
    <button
      onClick={onClick}
      className={`flex items-center gap-2 rounded-full border px-3 py-1.5 text-xs transition ${
        on
          ? onCls
          : "border-slate-300 bg-white/60 text-slate-500 hover:text-slate-700 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-400 dark:hover:text-slate-200"
      }`}
    >
      <span
        className={`h-2 w-2 rounded-full ${
          on
            ? tone === "amber"
              ? "bg-amber-500 dark:bg-amber-400"
              : tone === "emerald"
                ? "bg-emerald-500 dark:bg-emerald-400"
                : "bg-cyan-500 dark:bg-cyan-400"
            : "bg-slate-400 dark:bg-slate-600"
        }`}
      />
      {label}
    </button>
  );
}

export function MiniToggle({ label, on, onClick }: { label: string; on: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className={`rounded-lg border px-2.5 py-1 text-xs transition ${
        on
          ? "border-indigo-500/50 bg-indigo-500/15 text-indigo-700 dark:text-indigo-300"
          : "border-slate-300 bg-white/60 text-slate-500 hover:text-slate-700 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-500 dark:hover:text-slate-300"
      }`}
    >
      {on ? "✓ " : ""}
      {label}
    </button>
  );
}

export function NumberField({
  label,
  value,
  min,
  max,
  wide,
  integer = false,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  wide?: boolean;
  /** 只允许整数（直播间号、轮询秒数等） */
  integer?: boolean;
  onChange: (v: number) => void;
}) {
  // 本地草稿：输入过程中不回弹，失焦/回车才提交（数字框直接绑服务端值会卡顿）
  const [text, setText] = useState(String(value));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setText(String(value));
  }, [value]);

  const commit = () => {
    const v = Number(text);
    if (!Number.isFinite(v)) {
      setText(String(value));
      return;
    }
    const rounded = integer ? Math.round(v) : v;
    const clamped = Math.min(max, Math.max(min, rounded));
    setText(String(clamped));
    if (clamped !== value) onChange(clamped);
  };

  return (
    <label className={`block text-xs text-slate-500 dark:text-slate-400 ${wide ? "w-24" : ""}`}>
      {label}
      <input
        type="number"
        value={text}
        min={min}
        max={max}
        step={integer ? 1 : "any"}
        onFocus={() => (focused.current = true)}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          focused.current = false;
          commit();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            (e.target as HTMLInputElement).blur();
          }
        }}
        className="mt-1 w-full rounded-lg border border-slate-300 bg-white px-2 py-1.5 font-mono text-sm text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200"
      />
    </label>
  );
}

/** 行内紧凑数字输入（如"最大宽度 px"）：与 NumberField 相同的草稿/失焦提交策略 */
export function InlineNumber({
  value,
  min,
  max,
  suffix,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  suffix?: string;
  onChange: (v: number) => void;
}) {
  const [text, setText] = useState(String(value));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setText(String(value));
  }, [value]);

  const commit = () => {
    const v = Number(text);
    if (!Number.isFinite(v)) {
      setText(String(value));
      return;
    }
    const clamped = Math.min(max, Math.max(min, Math.round(v)));
    setText(String(clamped));
    if (clamped !== value) onChange(clamped);
  };

  return (
    <span className="inline-flex items-center gap-1">
      <input
        type="number"
        value={text}
        min={min}
        max={max}
        onFocus={() => (focused.current = true)}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          focused.current = false;
          commit();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            (e.target as HTMLInputElement).blur();
          }
        }}
        className="w-20 rounded-lg border border-slate-300 bg-white px-2 py-1 font-mono text-sm text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200"
      />
      {suffix && <span className="text-xs text-slate-400 dark:text-slate-600">{suffix}</span>}
    </span>
  );
}

export function ColorField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="block text-xs text-slate-500 dark:text-slate-400">
      {label}
      <span className="mt-1 flex items-center gap-2">
        <input
          type="color"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className="h-8 w-12 cursor-pointer rounded border border-slate-300 bg-white dark:border-slate-700 dark:bg-slate-950"
        />
        <span className="font-mono text-xs text-slate-400 dark:text-slate-500">{value}</span>
      </span>
    </label>
  );
}
