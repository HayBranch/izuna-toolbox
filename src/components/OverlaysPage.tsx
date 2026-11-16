/**
 * 悬浮字幕管理页：每个浮窗一张配置卡。
 *
 * 每个浮窗可独立配置：
 * - 显示/隐藏、窗口置顶、鼠标穿透（浮窗自身悬停也有工具条）
 * - 模板文本（点变量芯片插入 {变量}；内置 today/account，自定义 A1…，计算 B1…）
 * - 字号 / 字色 / 加粗 / 描边阴影 / 背景色 / 背景不透明度
 * - 位置 X/Y（也可以直接拖浮窗，拖完自动保存）
 *
 * 编辑策略：文本类输入用本地草稿 + 300ms 防抖提交；开关类立即提交。
 * Rust 侧每次变更都会 emit config-changed，回流的新配置在输入框未聚焦时覆盖草稿。
 */
import { useEffect, useRef, useState } from "react";
import { addOverlay, deleteOverlay, moveOverlay, setOverlayAlwaysTop, setOverlayClickThrough, setOverlayShown, updateOverlay } from "../api";
import type { AppConfig, OverlayConfig } from "../types";
import { computeVars, varDefs } from "../vars";
import { formatDateTime } from "../utils";
import SubtitleView from "./SubtitleView";

export default function OverlaysPage({ config }: { config: AppConfig }) {
  const vars = computeVars(config, new Date());
  const defs = varDefs(config);

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm leading-6 text-slate-500">
          浮窗是无边框透明小窗，可被 OBS 等直播软件「窗口捕获」；拖动浮窗自动保存位置。
          开启「鼠标穿透」后浮窗不再响应鼠标，回这里解锁。
        </p>
        <button
          onClick={() => addOverlay().catch(console.warn)}
          className="shrink-0 rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400"
        >
          ＋ 新建浮窗
        </button>
      </div>

      {config.overlays.map((overlay) => (
        <OverlayEditor key={overlay.id} overlay={overlay} vars={vars} defs={defs} />
      ))}

      {config.overlays.length === 0 && (
        <p className="rounded-2xl border border-dashed border-slate-800 p-10 text-center text-sm text-slate-600">
          还没有浮窗，点右上角「＋ 新建浮窗」创建一个
        </p>
      )}
    </div>
  );
}

function OverlayEditor({
  overlay,
  vars,
  defs,
}: {
  overlay: OverlayConfig;
  vars: ReturnType<typeof computeVars>;
  defs: ReturnType<typeof varDefs>;
}) {
  const [draft, setDraft] = useState(overlay);
  const editingText = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // 配置回流（自己提交的或别的窗口改的）：输入框没在编辑中就同步
  useEffect(() => {
    if (!editingText.current) setDraft(overlay);
  }, [overlay]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);

  const push = (next: OverlayConfig, immediate = false) => {
    setDraft(next);
    if (timer.current) clearTimeout(timer.current);
    const run = () => updateOverlay(next).catch(console.warn);
    if (immediate) run();
    else timer.current = setTimeout(run, 300);
  };

  const patch = (partial: Partial<OverlayConfig>, immediate = false) =>
    push({ ...draft, ...partial }, immediate);

  // 开关走专用命令（不经过草稿，立即生效）
  const flag = (p: Promise<unknown>) => p.catch(console.warn);

  /** 在光标处插入 {变量} */
  const insertVar = (key: string) => {
    const token = `{${key}}`;
    const ta = taRef.current;
    if (!ta || document.activeElement !== ta) {
      patch({ template: draft.template + token });
      return;
    }
    const s = ta.selectionStart ?? draft.template.length;
    const e = ta.selectionEnd ?? s;
    patch({ template: draft.template.slice(0, s) + token + draft.template.slice(e) });
    requestAnimationFrame(() => {
      const pos = s + token.length;
      ta.focus();
      ta.setSelectionRange(pos, pos);
    });
  };

  return (
    <div
      className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5"
      onFocusCapture={() => (editingText.current = true)}
      onBlurCapture={() => (editingText.current = false)}
    >
      {/* 头部：名称 + 创建时间 + 三个开关 + 删除 */}
      <div className="flex flex-wrap items-center gap-3">
        <input
          value={draft.name}
          onChange={(e) => patch({ name: e.target.value })}
          placeholder="浮窗名称"
          className="w-40 rounded-lg border border-slate-700 bg-slate-950/60 px-3 py-1.5 text-sm font-medium text-slate-200 outline-none transition focus:border-indigo-500"
        />
        <span
          title="创建时间"
          className="font-mono text-[11px] text-slate-600"
        >
          🕒 {formatDateTime(overlay.createdAt)}
        </span>
        <Toggle
          on={draft.shown}
          label={draft.shown ? "显示中" : "已隐藏"}
          onClick={() => flag(setOverlayShown(overlay.id, !draft.shown))}
        />
        <Toggle
          on={draft.alwaysOnTop}
          label="置顶"
          onClick={() => flag(setOverlayAlwaysTop(overlay.id, !draft.alwaysOnTop))}
        />
        <Toggle
          on={draft.clickThrough}
          label={draft.clickThrough ? "已锁定（穿透）" : "鼠标穿透"}
          tone={draft.clickThrough ? "amber" : "default"}
          onClick={() => flag(setOverlayClickThrough(overlay.id, !draft.clickThrough))}
        />
        <button
          onClick={() => {
            if (window.confirm(`删除浮窗「${draft.name}」？`)) {
              deleteOverlay(overlay.id).catch(console.warn);
            }
          }}
          className="ml-auto text-xs text-slate-600 transition hover:text-rose-400"
        >
          🗑 删除
        </button>
      </div>

      {draft.clickThrough && (
        <p className="mt-3 rounded-xl border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-300">
          已开启鼠标穿透：浮窗不响应任何鼠标点击（不会挡住游戏操作），
          位置只能用下方 X/Y 调整，解锁也在这里。
        </p>
      )}

      {/* 模板 + 变量芯片 */}
      <div className="mt-4">
        <p className="text-sm font-medium text-slate-300">显示内容（模板）</p>
        <textarea
          ref={taRef}
          value={draft.template}
          onChange={(e) => patch({ template: e.target.value })}
          rows={3}
          className="mt-2 w-full resize-y rounded-xl border border-slate-700 bg-slate-950/60 px-4 py-2.5 text-sm leading-6 outline-none transition focus:border-indigo-500"
        />
        <div className="mt-2 flex flex-wrap gap-1.5">
          {defs.map((def) => (
            <button
              key={def.key}
              onClick={() => insertVar(def.key)}
              title={def.label}
              className={`rounded-full border px-2 py-0.5 font-mono text-[11px] transition ${
                def.computed
                  ? "border-indigo-700/60 bg-indigo-950/40 text-indigo-300 hover:border-indigo-400 hover:text-indigo-200"
                  : "border-slate-700 bg-slate-950/60 text-cyan-300 hover:border-cyan-500 hover:text-cyan-200"
              }`}
            >
              {`{${def.key}}`}
            </button>
          ))}
        </div>
        <p className="mt-1.5 text-xs text-slate-500">
          点击芯片在光标处插入变量；靛蓝色为计算变量（表达式自动算），
          其余在「变量与计数」页维护。
        </p>
      </div>

      {/* 样式 + 位置 */}
      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        <div className="grid grid-cols-2 gap-3 rounded-xl border border-slate-800 bg-slate-950/40 p-4">
          <NumberField label="字号" value={draft.fontSize} min={10} max={72}
            onChange={(v) => patch({ fontSize: v })} />
          <ColorField label="字体颜色" value={draft.fontColor}
            onChange={(v) => patch({ fontColor: v })} />
          <div className="col-span-2 flex flex-wrap items-center gap-2">
            <MiniToggle label="加粗" on={draft.bold} onClick={() => patch({ bold: !draft.bold }, true)} />
            <MiniToggle label="描边阴影" on={draft.shadow} onClick={() => patch({ shadow: !draft.shadow }, true)} />
          </div>
          <ColorField label="背景颜色" value={draft.bgColor}
            onChange={(v) => patch({ bgColor: v })} />
          <div className="col-span-2">
            <div className="flex items-baseline justify-between">
              <span className="text-xs text-slate-400">背景不透明度</span>
              <span className="font-mono text-xs text-cyan-300">{draft.bgOpacity}%</span>
            </div>
            <input
              type="range" min={0} max={100} step={5} value={draft.bgOpacity}
              onChange={(e) => patch({ bgOpacity: Number(e.target.value) })}
              className="mt-1 w-full accent-cyan-400"
            />
          </div>
        </div>

        {/* 预览：模拟直播画面 */}
        <div className="flex flex-col">
          {/* 预览：模拟直播画面。m-auto 在内容过宽时可滚动查看（justify-center 会裁剪） */}
          <div className="flex flex-1 min-h-28 overflow-auto rounded-xl border border-slate-800 bg-gradient-to-br from-emerald-900 via-slate-900 to-indigo-950 p-4">
            <div className="m-auto"><SubtitleView cfg={draft} vars={vars} /></div>
          </div>
          <div className="mt-3 flex items-center gap-3">
            <NumberField label="位置 X" value={draft.x} min={-9999} max={9999} wide
              onChange={(v) => moveOverlay(overlay.id, v, draft.y).catch(console.warn)} />
            <NumberField label="位置 Y" value={draft.y} min={-9999} max={9999} wide
              onChange={(v) => moveOverlay(overlay.id, draft.x, v).catch(console.warn)} />
            <span className="text-xs text-slate-500">拖动浮窗自动保存</span>
          </div>
        </div>
      </div>
    </div>
  );
}

function Toggle({
  on,
  label,
  onClick,
  tone = "default",
}: {
  on: boolean;
  label: string;
  onClick: () => void;
  tone?: "default" | "amber";
}) {
  const onCls = tone === "amber"
    ? "border-amber-500/50 bg-amber-500/15 text-amber-300"
    : "border-cyan-500/50 bg-cyan-500/15 text-cyan-300";
  return (
    <button
      onClick={onClick}
      className={`flex items-center gap-2 rounded-full border px-3 py-1.5 text-xs transition ${
        on ? onCls : "border-slate-700 bg-slate-950/60 text-slate-400 hover:text-slate-200"
      }`}
    >
      <span
        className={`h-2 w-2 rounded-full ${on ? (tone === "amber" ? "bg-amber-400" : "bg-cyan-400") : "bg-slate-600"}`}
      />
      {label}
    </button>
  );
}

function MiniToggle({ label, on, onClick }: { label: string; on: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className={`rounded-lg border px-2.5 py-1 text-xs transition ${
        on
          ? "border-indigo-500/50 bg-indigo-500/15 text-indigo-300"
          : "border-slate-700 bg-slate-950/60 text-slate-500 hover:text-slate-300"
      }`}
    >
      {on ? "✓ " : ""}
      {label}
    </button>
  );
}

function NumberField({
  label,
  value,
  min,
  max,
  wide,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  wide?: boolean;
  onChange: (v: number) => void;
}) {
  return (
    <label className={`block text-xs text-slate-400 ${wide ? "w-24" : ""}`}>
      {label}
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        onChange={(e) => {
          const v = Number(e.target.value);
          if (Number.isFinite(v)) onChange(Math.min(max, Math.max(min, v)));
        }}
        className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950/60 px-2 py-1.5 font-mono text-sm text-slate-200 outline-none transition focus:border-indigo-500"
      />
    </label>
  );
}

function ColorField({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  return (
    <label className="block text-xs text-slate-400">
      {label}
      <span className="mt-1 flex items-center gap-2">
        <input
          type="color"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className="h-8 w-12 cursor-pointer rounded border border-slate-700 bg-slate-950"
        />
        <span className="font-mono text-xs text-slate-500">{value}</span>
      </span>
    </label>
  );
}
