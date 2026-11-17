/**
 * 悬浮字幕页 —— 这是一个功能，所有配置都在这一页：
 * 每张卡片 = 一套完全独立的浮窗（各自的变量、计算变量、账号、模板与样式），
 * 不再需要跨页切换、也不用先选"编辑哪一套"。
 *
 * 卡片分区（从上到下）：
 * - 头部：名称 + 显示/置顶/鼠标穿透开关 + 删除
 * - 变量：自定义变量（−/＋ 计数）+ 计算变量（表达式），都是这一套自己的
 * - 显示内容：账号 + 模板（回车即换行；点芯片插入变量；↵ 插入换行）
 * - 外观：字号/颜色/加粗/描边/背景 + 自动换行与最大宽度
 * - 预览：模拟直播画面（所见即所得）+ 位置 X/Y
 *
 * 编辑策略：文本类输入用本地草稿 + 300ms 防抖提交；开关类立即提交。
 * 卡片右上角另有「保存」可立即落盘，并显示待保存/保存中/已保存状态。
 * Rust 侧每次变更都会 emit config-changed，回流的新配置在输入框未聚焦时覆盖草稿。
 * 任何命令失败都会显示在吸顶错误条里，不会"点了没反应"。
 */
import { useEffect, useRef, useState } from "react";
import {
  addComputed,
  addOverlay,
  addVariable,
  deleteComputed,
  deleteOverlay,
  deleteVariable,
  moveOverlay,
  setOverlayAlwaysTop,
  setOverlayClickThrough,
  setOverlayShown,
  stepVariable,
  updateComputed,
  updateOverlay,
  updateVariable,
} from "../api";
import type { AppConfig, ComputedVar, OverlayConfig, UserVar } from "../types";
import { computeVars, evalExpr, fmtNum, fmtPercent, makeLookup, varDefs } from "../vars";
import { formatDateTime } from "../utils";
import ConfirmDelete from "./ConfirmDelete";
import SubtitleView from "./SubtitleView";

/** 多行示例模板：一键替换成推荐排版（与后端 DEFAULT_TEMPLATE 一致） */
const EXAMPLE_TEMPLATE = [
  "今日{today} · 账号：{account}",
  "总场次：{B1}（人机 {A1}｜真人 {A2}）",
  "胜场：{B2}（人机 {A3}｜真人 {A4}）",
  "总胜率：{B3}｜真实胜率：{B4}",
].join("\n");

type Run = (p: Promise<unknown>) => Promise<boolean>;

export default function OverlaysPage({ config }: { config: AppConfig }) {
  const [error, setError] = useState<string | null>(null);
  const run: Run = async (p) => {
    try {
      await p;
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  return (
    <div className="space-y-5">
      {error && (
        <div className="sticky top-0 z-20 flex items-center gap-3 rounded-xl border border-rose-500/30 bg-rose-950 px-4 py-2.5 text-sm text-rose-300 shadow-xl" role="alert">
          <span className="min-w-0 flex-1">{error}</span>
          <button onClick={() => setError(null)} className="shrink-0 text-rose-400 hover:text-rose-200" aria-label="关闭错误提示">✕</button>
        </div>
      )}

      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-3xl text-sm leading-6 text-slate-500">
          每张卡片就是一套完整的浮窗：变量、模板、样式都在这张卡里，套与套互不影响。
          修改会自动写入本地 SQLite，也可点卡片右上角「保存」立即落盘。
          主窗口在前台时浮窗会临时取消置顶，不会挡住按钮；切到游戏/桌面后浮窗恢复置顶，可直接拖动。
        </p>
        <button
          onClick={() => run(addOverlay())}
          className="shrink-0 rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400"
        >
          ＋ 新建浮窗
        </button>
      </div>

      {config.overlays.map((overlay) => (
        <OverlayEditor key={overlay.id} overlay={overlay} run={run} />
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
  run,
}: {
  overlay: OverlayConfig;
  run: Run;
}) {
  // 本套浮窗自己的变量：模板芯片与预览都用它
  const vars = computeVars(overlay, new Date());
  const defs = varDefs(overlay);
  const lookup = makeLookup(overlay);

  const [draft, setDraft] = useState(overlay);
  const [saveState, setSaveState] = useState<"saved" | "dirty" | "saving" | "error">("saved");
  const editingText = useRef(false);
  const editVersion = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // 配置回流（自己提交的或别的窗口改的）：输入框没在编辑中就同步
  useEffect(() => {
    if (!editingText.current) setDraft(overlay);
  }, [overlay]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);

  /** 立即保存本卡名称、账号、模板与外观；旧请求完成不能覆盖新编辑的状态。 */
  const save = async (next: OverlayConfig, version = editVersion.current) => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    setSaveState("saving");
    const ok = await run(updateOverlay(next));
    if (version === editVersion.current) setSaveState(ok ? "saved" : "error");
  };

  const push = (next: OverlayConfig, immediate = false) => {
    editVersion.current += 1;
    const version = editVersion.current;
    setDraft(next);
    setSaveState("dirty");
    if (timer.current) clearTimeout(timer.current);
    const task = () => void save(next, version);
    if (immediate) task();
    else timer.current = setTimeout(task, 300);
  };

  const patch = (partial: Partial<OverlayConfig>, immediate = false) =>
    push({ ...draft, ...partial }, immediate);

  /** 在光标处插入文本（变量 token 或换行符） */
  const insertText = (text: string) => {
    const ta = taRef.current;
    if (!ta || document.activeElement !== ta) {
      patch({ template: draft.template + text });
      return;
    }
    const s = ta.selectionStart ?? draft.template.length;
    const e = ta.selectionEnd ?? s;
    patch({ template: draft.template.slice(0, s) + text + draft.template.slice(e) });
    requestAnimationFrame(() => {
      const pos = s + text.length;
      ta.focus();
      ta.setSelectionRange(pos, pos);
    });
  };

  const insertVar = (key: string) => insertText(`{${key}}`);

  return (
    <div
      className="rounded-2xl border border-slate-800 bg-slate-900/60 p-5"
      onFocusCapture={() => (editingText.current = true)}
      onBlurCapture={() => (editingText.current = false)}
      onKeyDown={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
          e.preventDefault();
          void save(draft);
        }
      }}
    >
      {/* 头部：名称 + 创建时间 + 三个开关 + 删除 */}
      <div className="flex flex-wrap items-center gap-3">
        <input
          value={draft.name}
          onChange={(e) => patch({ name: e.target.value })}
          placeholder="浮窗名称"
          className="w-40 rounded-lg border border-slate-700 bg-slate-950/60 px-3 py-1.5 text-sm font-medium text-slate-200 outline-none transition focus:border-indigo-500"
        />
        <span title="创建时间" className="font-mono text-[11px] text-slate-600">
          🕒 {formatDateTime(overlay.createdAt)}
        </span>
        {/* 开关一律以服务端配置为准（overlay.*），避免快速连点用到过期草稿 */}
        <Toggle
          on={overlay.shown}
          label={overlay.shown ? "显示中" : "已隐藏"}
          onClick={() => run(setOverlayShown(overlay.id, !overlay.shown))}
        />
        <Toggle
          on={overlay.alwaysOnTop}
          label="置顶"
          onClick={() => run(setOverlayAlwaysTop(overlay.id, !overlay.alwaysOnTop))}
        />
        <Toggle
          on={overlay.clickThrough}
          label={overlay.clickThrough ? "已锁定（穿透）" : "鼠标穿透"}
          tone={overlay.clickThrough ? "amber" : "default"}
          onClick={() => run(setOverlayClickThrough(overlay.id, !overlay.clickThrough))}
        />
        <div className="ml-auto flex items-center gap-2">
          <span
            className={`text-xs ${
              saveState === "error"
                ? "text-rose-400"
                : saveState === "dirty"
                  ? "text-amber-300"
                  : "text-slate-500"
            }`}
            role="status"
          >
            {saveState === "dirty"
              ? "待保存"
              : saveState === "saving"
                ? "保存中…"
                : saveState === "error"
                  ? "保存失败"
                  : "已保存"}
          </span>
          <button
            onClick={() => void save(draft)}
            disabled={saveState === "saving"}
            title="立即保存本卡的名称、账号、模板和外观（Ctrl+S）"
            className={`rounded-lg border px-3 py-1.5 text-xs font-medium transition ${
              saveState === "dirty" || saveState === "error"
                ? "border-indigo-400 bg-indigo-500 text-white hover:bg-indigo-400"
                : "border-slate-700 text-slate-300 hover:border-indigo-500 hover:text-indigo-300"
            } disabled:cursor-wait disabled:opacity-60`}
          >
            {saveState === "saving" ? "保存中…" : "保存"}
          </button>
          <ConfirmDelete
            title={`删除浮窗「${overlay.name}」`}
            onConfirm={() => run(deleteOverlay(overlay.id))}
          />
        </div>
      </div>

      {overlay.clickThrough && (
        <p className="mt-3 rounded-xl border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-300">
          已开启鼠标穿透：浮窗不响应任何鼠标点击（不会挡住游戏操作），
          位置只能用下方 X/Y 调整，解锁也在这里。
        </p>
      )}

      {/* 变量 | 显示内容 */}
      <div className="mt-4 grid gap-4 lg:grid-cols-5">
        {/* 变量区：这一套自己的自定义变量 + 计算变量 */}
        <section className="rounded-xl border border-slate-800 bg-slate-950/40 p-4 lg:col-span-3">
          <p className="text-sm font-medium text-slate-300">
            变量 <span className="text-xs font-normal text-slate-500">这套自己独立，其它浮窗看不到</span>
          </p>

          <div className="mt-3 flex items-center justify-between gap-2">
            <p className="text-xs text-slate-400">
              自定义变量 <span className="text-slate-600">−/＋ 计数，模板里 {"{A1}"} 引用</span>
            </p>
            <button
              onClick={() => run(addVariable(overlay.id, "新变量", null, 0, 1))}
              className="shrink-0 rounded-lg bg-indigo-500 px-2.5 py-1 text-xs font-medium text-white transition hover:bg-indigo-400"
            >
              ＋ 添加
            </button>
          </div>
          <div className="mt-2 space-y-2">
            {overlay.variables.map((v) => (
              <VarRow key={v.id} variable={v} overlayId={overlay.id} run={run} />
            ))}
            {overlay.variables.length === 0 && (
              <p className="rounded-xl border border-dashed border-slate-800 px-4 py-5 text-center text-xs text-slate-600">
                还没有变量，点「＋ 添加」（比如：总数、人机、真人…）
              </p>
            )}
          </div>

          <div className="mt-4 flex items-center justify-between gap-2">
            <p className="text-xs text-slate-400">
              计算变量 <span className="text-slate-600">表达式自动算，模板里 {"{B1}"} 引用</span>
            </p>
            <button
              onClick={() => run(addComputed(overlay.id, "新计算", null, "A1 + A2", "plain", 0))}
              className="shrink-0 rounded-lg bg-cyan-600 px-2.5 py-1 text-xs font-medium text-white transition hover:bg-cyan-500"
            >
              ＋ 添加
            </button>
          </div>
          <div className="mt-2 space-y-2">
            {overlay.computed.map((c) => (
              <ComputedRow key={c.id} computed={c} overlayId={overlay.id} lookup={lookup} run={run} />
            ))}
            {overlay.computed.length === 0 && (
              <p className="rounded-xl border border-dashed border-slate-800 px-4 py-5 text-center text-xs text-slate-600">
                比如「总胜率」= A4 / (A1 + A2)，格式选百分比
              </p>
            )}
          </div>
        </section>

        {/* 显示内容区：账号 + 模板 */}
        <section className="rounded-xl border border-slate-800 bg-slate-950/40 p-4 lg:col-span-2">
          <p className="text-sm font-medium text-slate-300">显示内容</p>

          <label className="mt-3 block text-xs text-slate-400">
            账号 <span className="text-slate-600">模板里 {"{account}"} 显示</span>
            <input
              value={draft.account}
              onChange={(e) => patch({ account: e.target.value })}
              placeholder="比如：泉奈"
              className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950/60 px-3 py-1.5 text-sm outline-none transition focus:border-indigo-500"
            />
          </label>

          <div className="mt-3 flex items-center justify-between gap-2">
            <label className="text-xs text-slate-400">
              模板 <span className="text-slate-600">回车即换行</span>
            </label>
            <button
              onClick={() => patch({ template: EXAMPLE_TEMPLATE })}
              title="把模板替换成多行示例排版（会覆盖现有内容，可再自行修改）"
              className="shrink-0 rounded-lg border border-slate-700 px-2.5 py-1 text-xs text-slate-400 transition hover:border-indigo-500 hover:text-indigo-300"
            >
              示例模板
            </button>
          </div>
          <textarea
            ref={taRef}
            value={draft.template}
            onChange={(e) => patch({ template: e.target.value })}
            rows={5}
            className="mt-1 w-full resize-y rounded-xl border border-slate-700 bg-slate-950/60 px-4 py-2.5 font-mono text-sm leading-6 outline-none transition focus:border-indigo-500"
          />
          <div className="mt-2 flex flex-wrap gap-1.5">
            <button
              onClick={() => insertText("\n")}
              title="在光标处插入换行"
              className="rounded-full border border-slate-700 bg-slate-950/60 px-2 py-0.5 font-mono text-[11px] text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
            >
              ↵ 换行
            </button>
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
          <p className="mt-1.5 text-xs leading-5 text-slate-500">
            点芯片在光标处插入变量；靛蓝色为计算变量（表达式自动算），
            <code className="text-indigo-300">{"{today}"}</code> /{" "}
            <code className="text-indigo-300">{"{date}"}</code> /{" "}
            <code className="text-indigo-300">{"{time}"}</code> 自动取当前时间。
          </p>
        </section>
      </div>

      {/* 外观 | 预览 */}
      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        {/* 外观：文字 + 背景 + 换行 */}
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
          <div className="col-span-2 border-t border-slate-800 pt-3">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <MiniToggle label="自动换行" on={draft.wrap} onClick={() => patch({ wrap: !draft.wrap }, true)} />
              {draft.wrap && (
                <label className="flex items-center gap-1.5 text-xs text-slate-400">
                  最大宽度
                  <InlineNumber value={draft.maxWidth} min={80} max={4000} suffix="px"
                    onChange={(v) => patch({ maxWidth: v })} />
                </label>
              )}
            </div>
            <p className="mt-1.5 text-xs text-slate-600">
              {draft.wrap
                ? "模板里的回车换行；超过最大宽度的长行自动折行，浮窗高度自适应。"
                : "只有模板里的回车会换行；长行保持一行（超过屏幕会被截断）。"}
            </p>
          </div>
        </div>

        {/* 预览：模拟直播画面 */}
        <div className="flex flex-col">
          {/* m-auto 在内容过宽时可滚动查看（justify-center 会裁剪） */}
          <div className="flex min-h-40 flex-1 overflow-auto rounded-xl border border-slate-800 bg-gradient-to-br from-emerald-900 via-slate-900 to-indigo-950 p-4">
            <div className="m-auto"><SubtitleView cfg={draft} vars={vars} /></div>
          </div>
          <div className="mt-3 flex items-center gap-3">
            <NumberField label="位置 X" value={overlay.x} min={-9999} max={9999} wide
              onChange={(v) => run(moveOverlay(overlay.id, v, overlay.y))} />
            <NumberField label="位置 Y" value={overlay.y} min={-9999} max={9999} wide
              onChange={(v) => run(moveOverlay(overlay.id, overlay.x, v))} />
            <span className="text-xs text-slate-500">拖动浮窗自动保存</span>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------- 变量编辑行（每套浮窗自己的） ----------

/** 编辑行的通用行为：本地草稿 + 防抖提交；输入中不被事件回包打断 */
function useRowDraft<T extends { id: number }>(source: T, run: Run, submit: (draft: T) => Promise<unknown>) {
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
  run: Run;
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
        className="w-14 rounded-lg border border-cyan-800/60 bg-slate-950 px-2 py-1 text-center font-mono text-xs text-cyan-300 outline-none transition focus:border-cyan-500"
      />
      <span className="text-slate-600">=</span>
      {/* 显示名 */}
      <input
        value={draft.name}
        onChange={(e) => commit({ name: e.target.value })}
        placeholder="名字，如：总数"
        className="w-24 rounded-lg border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-200 outline-none transition focus:border-indigo-500"
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
      <ConfirmDelete
        title={`删除变量「${draft.name}」`}
        onConfirm={() => run(deleteVariable(overlayId, draft.id))}
      />
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
  run: Run;
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
        className="w-14 rounded-lg border border-cyan-800/60 bg-slate-950 px-2 py-1 text-center font-mono text-xs text-cyan-300 outline-none transition focus:border-cyan-500"
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
      <ConfirmDelete
        title={`删除计算变量「${draft.name}」`}
        onConfirm={() => run(deleteComputed(overlayId, draft.id))}
      />
    </div>
  );
}

// ---------- 通用小控件 ----------

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
    const clamped = Math.min(max, Math.max(min, v));
    setText(String(clamped));
    if (clamped !== value) onChange(clamped);
  };

  return (
    <label className={`block text-xs text-slate-400 ${wide ? "w-24" : ""}`}>
      {label}
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
        className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950/60 px-2 py-1.5 font-mono text-sm text-slate-200 outline-none transition focus:border-indigo-500"
      />
    </label>
  );
}

/** 行内紧凑数字输入（如"最大宽度 px"）：与 NumberField 相同的草稿/失焦提交策略 */
function InlineNumber({
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
        className="w-20 rounded-lg border border-slate-700 bg-slate-950/60 px-2 py-1 font-mono text-sm text-slate-200 outline-none transition focus:border-indigo-500"
      />
      {suffix && <span className="text-xs text-slate-600">{suffix}</span>}
    </span>
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
