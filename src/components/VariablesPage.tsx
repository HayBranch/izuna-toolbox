/**
 * 变量页：每一套浮窗都有自己独立的账号、自定义变量与计算变量。
 *
 * - 顶部选择要编辑的浮窗（套）；不同套的变量互不影响；
 * - 自定义变量：名字随便起（如"总数"），key 用于模板/表达式引用（A1），
 *   每行有 −/＋ 按行内步长加减数值；
 * - 计算变量：写表达式引用**本套内**的其它变量自动算出（如 `A1 + A2`、
 *   `A4 / (A1 + A2)`），可选"普通数 / 百分比"格式与小数位，行内实时预览；
 * - 全部由 Rust 写入 SQLite 持久化。
 */
import { useEffect, useRef, useState } from "react";
import {
  addComputed,
  addOverlay,
  addVariable,
  deleteComputed,
  deleteVariable,
  setAccount,
  stepVariable,
  updateComputed,
  updateVariable,
} from "../api";
import type { AppConfig, ComputedVar, OverlayConfig, UserVar } from "../types";
import { evalExpr, fmtNum, fmtPercent, makeLookup } from "../vars";

export default function VariablesPage({ config }: { config: AppConfig }) {
  const [selectedId, setSelectedId] = useState<number | null>(null);
  // 优先记住用户选中的套；被删了就回落到第一套
  const overlay =
    config.overlays.find((o) => o.id === selectedId) ?? config.overlays[0];

  if (!overlay) {
    return (
      <div className="rounded-2xl border border-dashed border-slate-800 p-10 text-center">
        <p className="text-sm text-slate-500">
          还没有浮窗。每套浮窗都有自己独立的变量，先创建一套：
        </p>
        <button
          onClick={() => addOverlay().catch(console.warn)}
          className="mt-4 rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400"
        >
          ＋ 新建浮窗
        </button>
      </div>
    );
  }

  // key={overlay.id}：切换套时整个编辑器重挂载，所有草稿随之重置
  return (
    <VariablesEditor key={overlay.id} overlay={overlay} overlays={config.overlays} onSelect={setSelectedId} />
  );
}

function VariablesEditor({
  overlay,
  overlays,
  onSelect,
}: {
  overlay: OverlayConfig;
  overlays: OverlayConfig[];
  onSelect: (id: number) => void;
}) {
  const [error, setError] = useState<string | null>(null);

  const run = (p: Promise<unknown>) =>
    p.then(() => setError(null)).catch((e) => setError(String(e)));

  // 账号输入框本地草稿 + 防抖提交，避免每敲一个字符就写库
  const [draft, setDraft] = useState(overlay.account);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => setDraft(overlay.account), [overlay.account]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);

  const commitAccount = (value: string) => {
    setDraft(value);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => run(setAccount(overlay.id, value)), 300);
  };

  const lookup = makeLookup(overlay);

  return (
    <div className="space-y-5">
      {error && (
        <p className="rounded-xl border border-rose-500/30 bg-rose-500/10 px-4 py-2.5 text-sm text-rose-300">
          {error}
        </p>
      )}

      {/* 套选择器 */}
      <div className="rounded-2xl border border-slate-800 bg-slate-900/60 p-4">
        <p className="text-xs text-slate-500">
          正在编辑哪套浮窗的变量？每套浮窗完全独立：各自的账号、变量、模板互不影响。
        </p>
        <div className="mt-2.5 flex flex-wrap items-center gap-2">
          {overlays.map((o) => (
            <button
              key={o.id}
              onClick={() => onSelect(o.id)}
              className={`rounded-full border px-3 py-1.5 text-xs transition ${
                o.id === overlay.id
                  ? "border-sky-500/60 bg-sky-500/15 text-sky-300"
                  : "border-slate-700 bg-slate-950/60 text-slate-400 hover:text-slate-200"
              }`}
            >
              {o.id === overlay.id ? "● " : "○ "}
              {o.name || "未命名"}
            </button>
          ))}
          <button
            onClick={() => run(addOverlay())}
            title="新建一套浮窗"
            className="rounded-full border border-dashed border-slate-700 px-3 py-1.5 text-xs text-slate-500 transition hover:border-indigo-500 hover:text-indigo-300"
          >
            ＋ 新建
          </button>
        </div>
      </div>

      {/* 账号（本套） */}
      <div className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5">
        <label className="text-sm font-medium text-slate-300" htmlFor="account">
          账号（{overlay.name || "未命名"} 这一套）
        </label>
        <input
          id="account"
          value={draft}
          onChange={(e) => commitAccount(e.target.value)}
          placeholder="比如：泉奈"
          className="mt-2 w-full rounded-xl border border-slate-700 bg-slate-950/60 px-4 py-2.5 text-sm outline-none transition focus:border-indigo-500"
        />
        <p className="mt-2 text-xs text-slate-500">
          会作为 <code className="text-indigo-300">{"{account}"}</code> 变量显示在这一套的悬浮字幕里。
          另有内置变量 <code className="text-indigo-300">{"{today}"}</code> /{" "}
          <code className="text-indigo-300">{"{date}"}</code> /{" "}
          <code className="text-indigo-300">{"{time}"}</code> 自动取当前时间。
        </p>
      </div>

      {/* 自定义变量（本套） */}
      <div className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5">
        <div className="flex items-center justify-between">
          <div>
            <p className="text-sm font-medium text-slate-300">自定义变量</p>
            <p className="mt-0.5 text-xs text-slate-500">
              名字随便起（如"总数"），引用名（A1、A2…）用于模板与表达式；−/＋ 按行内步长增减，不允许减到负数。
            </p>
          </div>
          <button
            onClick={() => run(addVariable(overlay.id, "新变量", null, 0, 1))}
            className="shrink-0 rounded-xl bg-indigo-500 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-indigo-400"
          >
            ＋ 添加变量
          </button>
        </div>
        <div className="mt-3 space-y-2">
          {overlay.variables.map((v) => (
            <VarRow key={v.id} variable={v} overlayId={overlay.id} run={run} />
          ))}
          {overlay.variables.length === 0 && (
            <p className="rounded-xl border border-dashed border-slate-800 px-4 py-6 text-center text-xs text-slate-600">
              这套还没有变量，点右上角添加（比如：总数、人机、真人…）
            </p>
          )}
        </div>
      </div>

      {/* 计算变量（本套） */}
      <div className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5">
        <div className="flex items-center justify-between">
          <div>
            <p className="text-sm font-medium text-slate-300">计算变量</p>
            <p className="mt-0.5 text-xs text-slate-500">
              表达式引用本套的其它变量自动算出（支持 + − × ÷ 和括号，如{" "}
              <code className="text-cyan-300">A1 + A2</code>、
              <code className="text-cyan-300">A4 / (A1 + A2)</code>）；百分比格式会把 0.8 显示成 80%。
            </p>
          </div>
          <button
            onClick={() => run(addComputed(overlay.id, "新计算", null, "A1 + A2", "plain", 0))}
            className="shrink-0 rounded-xl bg-cyan-600 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-cyan-500"
          >
            ＋ 添加计算变量
          </button>
        </div>
        <div className="mt-3 space-y-2">
          {overlay.computed.map((c) => (
            <ComputedRow key={c.id} computed={c} overlayId={overlay.id} lookup={lookup} run={run} />
          ))}
          {overlay.computed.length === 0 && (
            <p className="rounded-xl border border-dashed border-slate-800 px-4 py-6 text-center text-xs text-slate-600">
              这套还没有计算变量，比如"总胜率"= 真人胜场 ÷ (人机场次 + 真人场次)，格式选百分比
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

/** 编辑行的通用行为：本地草稿 + 防抖提交；输入中不被事件回包打断 */
function useRowDraft<T extends { id: number }>(source: T, run: (p: Promise<unknown>) => void, submit: (draft: T) => Promise<unknown>) {
  const [draft, setDraft] = useState(source);
  const editing = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!editing.current) setDraft(source);
  }, [source]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);

  const commit = (partial: Partial<T>, immediate = false) => {
    const next = { ...draft, ...partial };
    setDraft(next);
    if (timer.current) clearTimeout(timer.current);
    const task = () => run(submit(next));
    if (immediate) task();
    else timer.current = setTimeout(task, 400);
  };

  return { draft, commit, editing };
}

function VarRow({
  variable,
  overlayId,
  run,
}: {
  variable: UserVar;
  overlayId: number;
  run: (p: Promise<unknown>) => void;
}) {
  const { draft, commit, editing } = useRowDraft(variable, run, (d) =>
    updateVariable(overlayId, { ...d, step: d.step > 0 ? d.step : 1 }),
  );

  return (
    <div
      className="flex flex-wrap items-center gap-2 rounded-xl border border-slate-800 bg-slate-950/40 px-3 py-2.5"
      onFocusCapture={() => (editing.current = true)}
      onBlurCapture={() => (editing.current = false)}
    >
      {/* 引用名（模板里用 {A1} 引用） */}
      <input
        value={draft.key}
        onChange={(e) => commit({ key: e.target.value })}
        title="引用名：模板和表达式里用 {A1} 这样引用"
        className="w-16 rounded-lg border border-cyan-800/60 bg-slate-950 px-2 py-1 text-center font-mono text-xs text-cyan-300 outline-none transition focus:border-cyan-500"
      />
      <span className="text-slate-600">=</span>
      {/* 显示名 */}
      <input
        value={draft.name}
        onChange={(e) => commit({ name: e.target.value })}
        placeholder="名字，如：总数"
        className="w-28 rounded-lg border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-200 outline-none transition focus:border-indigo-500"
      />
      <div className="ml-auto flex items-center gap-1.5">
        <button
          onClick={() => run(stepVariable(overlayId, draft.id, -draft.step))}
          className="h-9 w-9 rounded-lg border border-slate-700 text-lg text-slate-300 transition hover:border-rose-400 hover:text-rose-300 active:scale-95"
        >
          −
        </button>
        <span className="min-w-14 text-center font-mono text-xl font-semibold tabular-nums text-slate-100">
          {fmtNum(variable.value, -1)}
        </span>
        <button
          onClick={() => run(stepVariable(overlayId, draft.id, draft.step))}
          className="h-9 w-9 rounded-lg border border-slate-700 text-lg text-slate-300 transition hover:border-cyan-400 hover:text-cyan-300 active:scale-95"
        >
          ＋
        </button>
      </div>
      <label className="flex items-center gap-1 text-xs text-slate-500">
        步长
        <input
          type="number"
          min={0.1}
          step={0.1}
          value={draft.step}
          onChange={(e) => {
            const v = Number(e.target.value);
            if (Number.isFinite(v)) commit({ step: v });
          }}
          className="w-16 rounded-lg border border-slate-700 bg-slate-950 px-1.5 py-1 font-mono text-xs text-slate-300 outline-none transition focus:border-indigo-500"
        />
      </label>
      <DeleteButton onClick={() => { if (window.confirm(`删除变量「${draft.name}」？模板里引用它的地方会显示原文。`)) run(deleteVariable(overlayId, draft.id)); }} />
    </div>
  );
}

function ComputedRow({
  computed,
  overlayId,
  lookup,
  run,
}: {
  computed: ComputedVar;
  overlayId: number;
  lookup: (key: string) => number | null;
  run: (p: Promise<unknown>) => void;
}) {
  const { draft, commit, editing } = useRowDraft(computed, run, (d) =>
    updateComputed(overlayId, {
      ...d,
      format: d.format === "percent" ? "percent" : "plain",
      decimals: Math.min(6, Math.max(0, Math.round(d.decimals))),
    }),
  );

  // 实时预览：草稿表达式即时求值（不动数据库也能看到效果）
  const preview = evalExpr(draft.expr, lookup);

  return (
    <div
      className="flex flex-wrap items-center gap-2 rounded-xl border border-slate-800 bg-slate-950/40 px-3 py-2.5"
      onFocusCapture={() => (editing.current = true)}
      onBlurCapture={() => (editing.current = false)}
    >
      <input
        value={draft.key}
        onChange={(e) => commit({ key: e.target.value })}
        title="引用名：模板和其它表达式里用 {B1} 这样引用"
        className="w-16 rounded-lg border border-cyan-800/60 bg-slate-950 px-2 py-1 text-center font-mono text-xs text-cyan-300 outline-none transition focus:border-cyan-500"
      />
      <span className="text-slate-600">=</span>
      <input
        value={draft.name}
        onChange={(e) => commit({ name: e.target.value })}
        placeholder="名字，如：总胜率"
        className="w-24 rounded-lg border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-200 outline-none transition focus:border-indigo-500"
      />
      <input
        value={draft.expr}
        onChange={(e) => commit({ expr: e.target.value })}
        placeholder="表达式，如：A1 + A2"
        className={`min-w-32 flex-1 rounded-lg border bg-slate-950 px-2 py-1 font-mono text-sm outline-none transition ${
          preview === null ? "border-rose-700/60 text-rose-300" : "border-slate-700 text-slate-200 focus:border-indigo-500"
        }`}
      />
      <select
        value={draft.format}
        onChange={(e) => commit({ format: e.target.value === "percent" ? "percent" : "plain" }, true)}
        className="rounded-lg border border-slate-700 bg-slate-950 px-1.5 py-1 text-xs text-slate-300 outline-none"
      >
        <option value="plain">普通数</option>
        <option value="percent">百分比</option>
      </select>
      <select
        value={draft.decimals}
        onChange={(e) => commit({ decimals: Number(e.target.value) }, true)}
        title="小数点位数"
        className="rounded-lg border border-slate-700 bg-slate-950 px-1.5 py-1 font-mono text-xs text-slate-300 outline-none"
      >
        {[0, 1, 2, 3].map((n) => (
          <option key={n} value={n}>
            {n} 位
          </option>
        ))}
      </select>
      <span
        className={`min-w-16 text-right font-mono text-sm tabular-nums ${
          preview === null ? "text-rose-400" : "text-cyan-300"
        }`}
        title="实时预览"
      >
        {preview === null
          ? "—"
          : draft.format === "percent"
            ? fmtPercent(preview, draft.decimals)
            : fmtNum(preview, draft.decimals)}
      </span>
      <DeleteButton onClick={() => { if (window.confirm(`删除计算变量「${draft.name}」？`)) run(deleteComputed(overlayId, draft.id)); }} />
    </div>
  );
}

function DeleteButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      title="删除"
      className="text-sm text-slate-600 transition hover:text-rose-400"
    >
      🗑
    </button>
  );
}
