/**
 * 悬浮字幕页 —— 两级界面：
 * 1) 列表：一眼看到每套浮窗的基本信息（名称 / 创建 / 最后修改 / 显示 / 置顶 / 鼠标穿透），
 *    点一行进入这一套的配置；
 * 2) 详情：这一套浮窗的全部配置（变量、模板、样式、预览）。左上角返回列表。
 *
 * 每张卡片 = 一套完全独立的浮窗（各自的变量、计算变量、账号、模板与样式）。
 * 卡片分区（从上到下）：
 * - 头部：返回 + 名称 + 创建时间 + 显示/置顶/鼠标穿透开关 + 保存/删除
 * - 变量：自定义变量（−/＋ 计数，可设置跟随联动）+ 计算变量（表达式），都是这一套自己的
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
  resizeOverlay,
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
import { ChipToggle, ColorField, InlineNumber, MiniToggle, NumberField, Toggle } from "./Fields";
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
  // null = 列表；数字 = 正在配置的那套浮窗（删除后自动回列表）
  const [openId, setOpenId] = useState<number | null>(null);

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

  // 配置回流后按 id 重新查找：开关/编辑拿到的一定是最新值
  const open = openId === null ? null : (config.overlays.find((o) => o.id === openId) ?? null);

  /** 新建后直接进入配置（少点一次；失败仍走统一错误条） */
  const create = async () => {
    try {
      const next = await addOverlay();
      setError(null);
      const created = next.overlays[next.overlays.length - 1];
      if (created) setOpenId(created.id);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="space-y-5">
      {error && (
        <div
          className="sticky top-0 z-20 flex items-center gap-3 rounded-xl border border-rose-500/30 bg-rose-50 px-4 py-2.5 text-sm text-rose-700 shadow-xl dark:bg-rose-950 dark:text-rose-300"
          role="alert"
        >
          <span className="min-w-0 flex-1">{error}</span>
          <button
            onClick={() => setError(null)}
            className="shrink-0 text-rose-500 hover:text-rose-700 dark:text-rose-400 dark:hover:text-rose-200"
            aria-label="关闭错误提示"
          >
            ✕
          </button>
        </div>
      )}

      {open ? (
        <OverlayEditor key={open.id} overlay={open} onBack={() => setOpenId(null)} run={run} />
      ) : (
        <>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="max-w-3xl text-sm leading-6 text-slate-500 dark:text-slate-400">
              列表先看基本信息，点名称进入配置；<strong className="font-medium text-slate-600 dark:text-slate-300">显示 / 置顶 / 鼠标穿透可以直接点标签开关</strong>，不用进详情。
              每套浮窗的变量、模板、样式完全独立。修改会自动写入本地 SQLite；
              主窗口在前台时浮窗会临时取消置顶，切到游戏/桌面后恢复置顶，可直接拖动。
            </p>
            <button
              onClick={() => void create()}
              className="shrink-0 rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400"
            >
              ＋ 新建浮窗
            </button>
          </div>

          <ul className="space-y-2">
            {config.overlays.map((overlay) => (
              <OverlayRow
                key={overlay.id}
                overlay={overlay}
                run={run}
                onOpen={() => setOpenId(overlay.id)}
              />
            ))}
          </ul>

          {config.overlays.length === 0 && (
            <p className="rounded-2xl border border-dashed border-slate-300 p-10 text-center text-sm text-slate-500 dark:border-slate-800 dark:text-slate-600">
              还没有浮窗，点右上角「＋ 新建浮窗」创建一个
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** 列表行：名称 + 创建/修改时间 + 可直接开关的三个状态；点名称进入配置 */
function OverlayRow({
  overlay,
  run,
  onOpen,
}: {
  overlay: OverlayConfig;
  run: Run;
  onOpen: () => void;
}) {
  const updated = overlay.updatedAt || overlay.createdAt;
  return (
    <li className="rounded-2xl border border-slate-200 bg-white/70 transition hover:border-indigo-400 hover:bg-white hover:shadow-lg hover:shadow-indigo-500/10 dark:border-slate-800 dark:bg-slate-900/60 dark:hover:border-indigo-500 dark:hover:bg-slate-900">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-3 px-5 py-4">
        <button
          type="button"
          onClick={onOpen}
          title="打开这套浮窗的配置"
          className="flex min-w-0 flex-1 items-center gap-3 rounded-lg text-left focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500"
        >
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-indigo-500/10 text-base text-indigo-600 dark:text-indigo-300">
            🪟
          </span>
          <span className="min-w-0">
            <span className="block truncate font-medium text-slate-900 dark:text-slate-100">
              {overlay.name || "未命名浮窗"}
            </span>
            <span className="mt-0.5 block text-xs text-slate-500 dark:text-slate-500">
              创建 {formatDateTime(overlay.createdAt)} · 修改 {formatDateTime(updated)}
              {overlay.account ? ` · 账号 ${overlay.account}` : ""}
            </span>
          </span>
        </button>

        {/* 状态标签本身就是开关：不必进详情就能显示/隐藏、置顶、穿透 */}
        <div className="flex flex-wrap items-center gap-1.5">
          <ChipToggle
            on={overlay.shown}
            onLabel="显示中"
            offLabel="已隐藏"
            title={overlay.shown ? "点击隐藏这套浮窗（配置保留）" : "点击显示这套浮窗"}
            onClick={() => run(setOverlayShown(overlay.id, !overlay.shown))}
          />
          <ChipToggle
            on={overlay.alwaysOnTop}
            onLabel="置顶"
            offLabel="不置顶"
            title={overlay.alwaysOnTop ? "点击取消浮窗置顶" : "点击让浮窗始终置顶"}
            onClick={() => run(setOverlayAlwaysTop(overlay.id, !overlay.alwaysOnTop))}
          />
          <ChipToggle
            on={overlay.clickThrough}
            onLabel="鼠标穿透"
            offLabel="可点击"
            tone="amber"
            title={
              overlay.clickThrough
                ? "点击关闭鼠标穿透（浮窗重新响应鼠标）"
                : "点击开启鼠标穿透（浮窗不挡游戏操作）"
            }
            onClick={() => run(setOverlayClickThrough(overlay.id, !overlay.clickThrough))}
          />
        </div>

        <button
          type="button"
          onClick={onOpen}
          title="打开这套浮窗的配置"
          className="flex shrink-0 items-center gap-1 rounded-lg px-2 py-1 text-xs font-medium text-slate-400 transition hover:bg-indigo-500/10 hover:text-indigo-600 focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500 dark:text-slate-500 dark:hover:text-indigo-300"
        >
          配置 <span aria-hidden="true">›</span>
        </button>
      </div>
    </li>
  );
}

function OverlayEditor({
  overlay,
  onBack,
  run,
}: {
  overlay: OverlayConfig;
  onBack: () => void;
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
  // 防抖窗口内点了「返回列表」：组件卸载时把还没提交的草稿补一次，
  // 避免刚改完就切走导致最后一次输入丢失。
  const pending = useRef<{ next: OverlayConfig; version: number } | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // 配置回流（自己提交的或别的窗口改的）：输入框没在编辑中就同步
  useEffect(() => {
    if (!editingText.current) setDraft(overlay);
  }, [overlay]);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
      const p = pending.current;
      if (!p) return;
      pending.current = null;
      void updateOverlay(p.next).catch(console.warn);
    },
    [],
  );

  /** 立即保存本卡名称、账号、模板与外观；旧请求完成不能覆盖新编辑的状态。 */
  const save = async (next: OverlayConfig, version = editVersion.current) => {
    if (timer.current) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    setSaveState("saving");
    const ok = await run(updateOverlay(next));
    if (version === editVersion.current) {
      pending.current = null;
      setSaveState(ok ? "saved" : "error");
    }
  };

  const push = (next: OverlayConfig, immediate = false) => {
    editVersion.current += 1;
    const version = editVersion.current;
    pending.current = { next, version };
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
      className="rounded-2xl border border-slate-200 bg-white/70 p-5 dark:border-slate-800 dark:bg-slate-900/60"
      onFocusCapture={() => (editingText.current = true)}
      onBlurCapture={() => (editingText.current = false)}
      onKeyDown={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
          e.preventDefault();
          void save(draft);
        }
      }}
    >
      {/* 头部：返回 + 名称 + 创建时间 + 三个开关 + 保存/删除 */}
      <div className="flex flex-wrap items-center gap-3">
        <button
          type="button"
          onClick={onBack}
          title="返回浮窗列表"
          className="shrink-0 rounded-lg border border-slate-300 px-2.5 py-1.5 text-xs font-medium text-slate-600 transition hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
        >
          ← 返回列表
        </button>
        <input
          value={draft.name}
          onChange={(e) => patch({ name: e.target.value })}
          placeholder="浮窗名称"
          className="w-40 rounded-lg border border-slate-300 bg-white px-3 py-1.5 text-sm font-medium text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200"
        />
        <span
          title="创建时间"
          className="font-mono text-[11px] text-slate-400 dark:text-slate-600"
        >
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
                ? "text-rose-500 dark:text-rose-400"
                : saveState === "dirty"
                  ? "text-amber-600 dark:text-amber-300"
                  : "text-slate-400 dark:text-slate-500"
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
                : "border-slate-300 text-slate-600 hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
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
        <p className="mt-3 rounded-xl border border-amber-500/30 bg-amber-50 px-3 py-2 text-xs text-amber-700 dark:bg-amber-500/10 dark:text-amber-300">
          已开启鼠标穿透：浮窗不响应任何鼠标点击（不会挡住游戏操作），
          位置只能用下方 X/Y 调整，解锁也在这里。
        </p>
      )}

      {/* 变量 | 显示内容 */}
      <div className="mt-4 grid gap-4 lg:grid-cols-5">
        {/* 变量区：这一套自己的自定义变量 + 计算变量 */}
        <section className="rounded-xl border border-slate-200 bg-slate-50/80 p-4 dark:border-slate-800 dark:bg-slate-950/40 lg:col-span-3">
          <p className="text-sm font-medium text-slate-700 dark:text-slate-300">
            变量{" "}
            <span className="text-xs font-normal text-slate-400 dark:text-slate-500">
              这套自己独立，其它浮窗看不到
            </span>
          </p>

          <div className="mt-3 flex items-center justify-between gap-2">
            <p className="text-xs text-slate-500 dark:text-slate-400">
              自定义变量{" "}
              <span className="text-slate-400 dark:text-slate-600">
                −/＋ 计数，模板里 {"{A1}"} 引用；要让 A1 跟着 A3，在 A1 行设置「联动」
              </span>
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
              <VarRow
                key={v.id}
                variable={v}
                variables={overlay.variables}
                overlayId={overlay.id}
                run={run}
              />
            ))}
            {overlay.variables.length === 0 && (
              <p className="rounded-xl border border-dashed border-slate-300 px-4 py-5 text-center text-xs text-slate-500 dark:border-slate-800 dark:text-slate-600">
                还没有变量，点「＋ 添加」（比如：总数、人机、真人…）
              </p>
            )}
          </div>

          <div className="mt-4 flex items-center justify-between gap-2">
            <p className="text-xs text-slate-500 dark:text-slate-400">
              计算变量{" "}
              <span className="text-slate-400 dark:text-slate-600">
                表达式自动算，模板里 {"{B1}"} 引用
              </span>
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
              <p className="rounded-xl border border-dashed border-slate-300 px-4 py-5 text-center text-xs text-slate-500 dark:border-slate-800 dark:text-slate-600">
                比如「总胜率」= A4 / (A1 + A2)，格式选百分比
              </p>
            )}
          </div>
        </section>

        {/* 显示内容区：账号 + 模板 */}
        <section className="rounded-xl border border-slate-200 bg-slate-50/80 p-4 dark:border-slate-800 dark:bg-slate-950/40 lg:col-span-2">
          <p className="text-sm font-medium text-slate-700 dark:text-slate-300">显示内容</p>

          <label className="mt-3 block text-xs text-slate-500 dark:text-slate-400">
            账号{" "}
            <span className="text-slate-400 dark:text-slate-600">模板里 {"{account}"} 显示</span>
            <input
              value={draft.account}
              onChange={(e) => patch({ account: e.target.value })}
              placeholder="比如：泉奈"
              className="mt-1 w-full rounded-lg border border-slate-300 bg-white px-3 py-1.5 text-sm text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200"
            />
          </label>

          <div className="mt-3 flex items-center justify-between gap-2">
            <label className="text-xs text-slate-500 dark:text-slate-400">
              模板 <span className="text-slate-400 dark:text-slate-600">回车即换行</span>
            </label>
            <button
              onClick={() => patch({ template: EXAMPLE_TEMPLATE })}
              title="把模板替换成多行示例排版（会覆盖现有内容，可再自行修改）"
              className="shrink-0 rounded-lg border border-slate-300 px-2.5 py-1 text-xs text-slate-500 transition hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-400 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
            >
              示例模板
            </button>
          </div>
          <textarea
            ref={taRef}
            value={draft.template}
            onChange={(e) => patch({ template: e.target.value })}
            rows={5}
            className="mt-1 w-full resize-y rounded-xl border border-slate-300 bg-white px-4 py-2.5 font-mono text-sm leading-6 text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200"
          />
          <div className="mt-2 flex flex-wrap gap-1.5">
            <button
              onClick={() => insertText("\n")}
              title="在光标处插入换行"
              className="rounded-full border border-slate-300 bg-white px-2 py-0.5 font-mono text-[11px] text-slate-600 transition hover:border-slate-400 hover:text-slate-900 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-300 dark:hover:border-slate-500 dark:hover:text-slate-100"
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
                    ? "border-indigo-300 bg-indigo-50 text-indigo-700 hover:border-indigo-400 hover:text-indigo-800 dark:border-indigo-700/60 dark:bg-indigo-950/40 dark:text-indigo-300 dark:hover:border-indigo-400 dark:hover:text-indigo-200"
                    : "border-slate-300 bg-white text-cyan-700 hover:border-cyan-500 hover:text-cyan-800 dark:border-slate-700 dark:bg-slate-950/60 dark:text-cyan-300 dark:hover:border-cyan-500 dark:hover:text-cyan-200"
                }`}
              >
                {`{${def.key}}`}
              </button>
            ))}
          </div>
          <p className="mt-1.5 text-xs leading-5 text-slate-500 dark:text-slate-500">
            点芯片在光标处插入变量；靛蓝色为计算变量（表达式自动算），
            <code className="text-indigo-600 dark:text-indigo-300">{"{today}"}</code> /{" "}
            <code className="text-indigo-600 dark:text-indigo-300">{"{date}"}</code> /{" "}
            <code className="text-indigo-600 dark:text-indigo-300">{"{time}"}</code> 自动取当前时间。
          </p>
        </section>
      </div>

      {/* 外观 | 预览 */}
      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        {/* 外观：文字 + 背景 + 换行 */}
        <div className="grid grid-cols-2 gap-3 rounded-xl border border-slate-200 bg-slate-50/80 p-4 dark:border-slate-800 dark:bg-slate-950/40">
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
              <span className="text-xs text-slate-500 dark:text-slate-400">背景不透明度</span>
              <span className="font-mono text-xs text-cyan-700 dark:text-cyan-300">{draft.bgOpacity}%</span>
            </div>
            <input
              type="range" min={0} max={100} step={5} value={draft.bgOpacity}
              onChange={(e) => patch({ bgOpacity: Number(e.target.value) })}
              className="mt-1 w-full accent-cyan-500 dark:accent-cyan-400"
            />
          </div>
          <div className="col-span-2 border-t border-slate-200 pt-3 dark:border-slate-800">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <MiniToggle label="自动换行" on={draft.wrap} onClick={() => patch({ wrap: !draft.wrap }, true)} />
              {draft.wrap && (
                <label className="flex items-center gap-1.5 text-xs text-slate-500 dark:text-slate-400">
                  最大宽度
                  <InlineNumber value={draft.maxWidth} min={80} max={4000} suffix="px"
                    onChange={(v) => patch({ maxWidth: v })} />
                </label>
              )}
            </div>
            <p className="mt-1.5 text-xs text-slate-400 dark:text-slate-600">
              {draft.wrap
                ? "自动模式：模板里的回车换行；超过最大宽度的长行折行。直接拖动浮窗边缘可改成固定宽高。"
                : "只有模板里的回车会换行；长行保持一行（超过屏幕会被截断）。"}
            </p>
          </div>
        </div>

        {/* 预览：模拟直播画面 */}
        <div className="flex flex-col">
          {/* m-auto 在内容过宽时可滚动查看（justify-center 会裁剪） */}
          <div className="flex min-h-40 flex-1 overflow-auto rounded-xl border border-slate-200 bg-gradient-to-br from-emerald-900 via-slate-900 to-indigo-950 p-4 dark:border-slate-800">
            <div className="m-auto"><SubtitleView cfg={draft} vars={vars} /></div>
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-3">
            <NumberField label="位置 X" value={overlay.x} min={-9999} max={9999} wide
              onChange={(v) => run(moveOverlay(overlay.id, v, overlay.y))} />
            <NumberField label="位置 Y" value={overlay.y} min={-9999} max={9999} wide
              onChange={(v) => run(moveOverlay(overlay.id, overlay.x, v))} />
            <span className="text-xs text-slate-400 dark:text-slate-500">拖动浮窗自动保存</span>
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
            <span className="text-slate-500 dark:text-slate-400">
              尺寸{" "}
              <span className="font-mono text-cyan-700 dark:text-cyan-300">
                {overlay.width > 0 && overlay.height > 0
                  ? `${overlay.width} × ${overlay.height}`
                  : "自动按内容"}
              </span>
            </span>
            <button
              type="button"
              onClick={() => run(resizeOverlay(overlay.id, 0, 0))}
              disabled={!(overlay.width > 0 && overlay.height > 0)}
              title="恢复为按内容自动调整宽高"
              className="rounded-lg border border-slate-300 px-2 py-1 text-xs text-slate-500 transition hover:border-indigo-400 hover:text-indigo-600 disabled:cursor-default disabled:opacity-40 disabled:hover:border-slate-300 disabled:hover:text-slate-500 dark:border-slate-700 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
            >
              恢复自动尺寸
            </button>
            <span className="text-slate-400 dark:text-slate-600">
              直接拖动浮窗的边缘 / 角落即可改宽高
            </span>
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
  // 防抖窗口内切回列表：卸载时补交最后一次修改，避免丢字
  const pending = useRef<T | null>(null);

  useEffect(() => {
    if (!editing.current) setDraft(source);
  }, [source]);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
      const next = pending.current;
      if (!next) return;
      pending.current = null;
      void run(submit(next));
    },
    [],
  );

  const commit = (partial: Partial<T>, immediate = false) => {
    const next = { ...draft, ...partial };
    pending.current = next;
    setDraft(next);
    if (timer.current) clearTimeout(timer.current);
    const task = () => {
      pending.current = null;
      void run(submit(next));
    };
    if (immediate) task();
    else timer.current = setTimeout(task, 400);
  };

  return { draft, commit, editing };
}

/** root 的所有下游（直接/间接跟随 root 的变量）。选跟随目标时排除，避免成环。 */
function descendantsOf(vars: UserVar[], rootId: number): Set<number> {
  const children = new Map<number, number[]>();
  for (const v of vars) {
    if (v.followId) {
      const list = children.get(v.followId) ?? [];
      list.push(v.id);
      children.set(v.followId, list);
    }
  }
  const out = new Set<number>();
  const stack = [rootId];
  while (stack.length > 0) {
    const cur = stack.pop()!;
    for (const child of children.get(cur) ?? []) {
      if (!out.has(child)) {
        out.add(child);
        stack.push(child);
      }
    }
  }
  return out;
}

function VarRow({
  variable,
  variables,
  overlayId,
  run,
}: {
  variable: UserVar;
  variables: UserVar[];
  overlayId: number;
  run: Run;
}) {
  const { draft, commit, editing } = useRowDraft(variable, run, (d) =>
    updateVariable(overlayId, { ...d, step: d.step > 0 ? d.step : 1 }),
  );

  // 联动选项：不能选自己，也不能选自己的下游（否则会成环）
  const blocked = descendantsOf(variables, draft.id);
  const followOptions = variables.filter((v) => v.id !== draft.id && !blocked.has(v.id));
  const followSource = variables.find((v) => v.id === draft.followId);
  const followers = variables.filter((v) => v.followId === draft.id);

  return (
    <div
      className="flex flex-wrap items-center gap-2 rounded-xl border border-slate-200 bg-white/70 px-3 py-2.5 dark:border-slate-800 dark:bg-slate-950/40"
      onFocusCapture={() => (editing.current = true)}
      onBlurCapture={() => (editing.current = false)}
    >
      {/* 引用名（模板里用 {A1} 引用） */}
      <input
        value={draft.key}
        onChange={(e) => commit({ key: e.target.value })}
        title="引用名：模板和表达式里用 {A1} 这样引用"
        className="w-14 rounded-lg border border-cyan-300 bg-white px-2 py-1 text-center font-mono text-xs text-cyan-700 outline-none transition focus:border-cyan-500 dark:border-cyan-800/60 dark:bg-slate-950 dark:text-cyan-300"
      />
      <span className="text-slate-400 dark:text-slate-600">=</span>
      {/* 显示名 */}
      <input
        value={draft.name}
        onChange={(e) => commit({ name: e.target.value })}
        placeholder="名字，如：总数"
        className="w-24 rounded-lg border border-slate-300 bg-white px-2 py-1 text-sm text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200"
      />
      <div className="ml-auto flex items-center gap-1.5">
        <button
          onClick={() => run(stepVariable(overlayId, draft.id, -draft.step))}
          className="h-9 w-9 rounded-lg border border-slate-300 text-lg text-slate-600 transition hover:border-rose-400 hover:text-rose-600 active:scale-95 dark:border-slate-700 dark:text-slate-300 dark:hover:border-rose-400 dark:hover:text-rose-300"
        >
          −
        </button>
        <span className="min-w-14 text-center font-mono text-xl font-semibold tabular-nums text-slate-900 dark:text-slate-100">
          {fmtNum(variable.value, -1)}
        </span>
        <button
          onClick={() => run(stepVariable(overlayId, draft.id, draft.step))}
          className="h-9 w-9 rounded-lg border border-slate-300 text-lg text-slate-600 transition hover:border-cyan-400 hover:text-cyan-600 active:scale-95 dark:border-slate-700 dark:text-slate-300 dark:hover:border-cyan-400 dark:hover:text-cyan-300"
        >
          ＋
        </button>
      </div>
      <label className="flex items-center gap-1 text-xs text-slate-500 dark:text-slate-500">
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
          className="w-16 rounded-lg border border-slate-300 bg-white px-1.5 py-1 font-mono text-xs text-slate-700 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
        />
      </label>
      <ConfirmDelete
        title={`删除变量「${draft.name}」`}
        onConfirm={() => run(deleteVariable(overlayId, draft.id))}
      />

      {/* 联动：本变量跟随谁。被跟随变量 −/＋ 时，本变量同步同幅度的增减（单向）。 */}
      <div className="flex w-full flex-wrap items-center gap-2 border-t border-slate-200 pt-2 dark:border-slate-800/70">
        <span className="text-xs text-slate-500 dark:text-slate-500">🔗 联动</span>
        <select
          value={draft.followId}
          onChange={(e) => commit({ followId: Number(e.target.value) }, true)}
          aria-label="联动：跟随哪个变量"
          title="被跟随的变量每次 −/＋，这个变量也跟着同样幅度变化（单向）"
          className="rounded-lg border border-slate-300 bg-white px-2 py-1 text-xs text-slate-700 outline-none transition focus:border-cyan-500 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
        >
          <option value={0}>不跟随</option>
          {followOptions.map((v) => (
            <option key={v.id} value={v.id}>
              {v.key} · {v.name}
            </option>
          ))}
        </select>
        <span className="text-[11px] leading-5 text-slate-500 dark:text-slate-500">
          {followSource
            ? `跟随 ${followSource.key}：它 −/＋ 时，这个变量同步 −/＋（同幅度，单向）`
            : ""}
        </span>
        {followers.length > 0 && (
          <span className="rounded-full border border-cyan-300 bg-cyan-50 px-2 py-0.5 font-mono text-[10px] text-cyan-700 dark:border-cyan-800/60 dark:bg-cyan-950/30 dark:text-cyan-400">
            被 {followers.map((v) => v.key).join("、")} 跟随
          </span>
        )}
      </div>
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
      className="flex flex-wrap items-center gap-2 rounded-xl border border-slate-200 bg-white/70 px-3 py-2.5 dark:border-slate-800 dark:bg-slate-950/40"
      onFocusCapture={() => (editing.current = true)}
      onBlurCapture={() => (editing.current = false)}
    >
      <input
        value={draft.key}
        onChange={(e) => commit({ key: e.target.value })}
        title="引用名：模板和其它表达式里用 {B1} 这样引用"
        className="w-14 rounded-lg border border-cyan-300 bg-white px-2 py-1 text-center font-mono text-xs text-cyan-700 outline-none transition focus:border-cyan-500 dark:border-cyan-800/60 dark:bg-slate-950 dark:text-cyan-300"
      />
      <span className="text-slate-400 dark:text-slate-600">=</span>
      <input
        value={draft.name}
        onChange={(e) => commit({ name: e.target.value })}
        placeholder="名字，如：总胜率"
        className="w-24 rounded-lg border border-slate-300 bg-white px-2 py-1 text-sm text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-200"
      />
      <input
        value={draft.expr}
        onChange={(e) => commit({ expr: e.target.value })}
        placeholder="表达式，如：A1 + A2"
        className={`min-w-32 flex-1 rounded-lg border bg-white px-2 py-1 font-mono text-sm outline-none transition dark:bg-slate-950 ${
          preview === null
            ? "border-rose-400 text-rose-600 dark:border-rose-700/60 dark:text-rose-300"
            : "border-slate-300 text-slate-800 focus:border-indigo-500 dark:border-slate-700 dark:text-slate-200"
        }`}
      />
      <select
        value={draft.format}
        onChange={(e) => commit({ format: e.target.value === "percent" ? "percent" : "plain" }, true)}
        className="rounded-lg border border-slate-300 bg-white px-1.5 py-1 text-xs text-slate-700 outline-none dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
      >
        <option value="plain">普通数</option>
        <option value="percent">百分比</option>
      </select>
      <select
        value={draft.decimals}
        onChange={(e) => commit({ decimals: Number(e.target.value) }, true)}
        title="小数点位数"
        className="rounded-lg border border-slate-300 bg-white px-1.5 py-1 font-mono text-xs text-slate-700 outline-none dark:border-slate-700 dark:bg-slate-950 dark:text-slate-300"
      >
        {[0, 1, 2, 3].map((n) => (
          <option key={n} value={n}>
            {n} 位
          </option>
        ))}
      </select>
      <span
        className={`min-w-16 text-right font-mono text-sm tabular-nums ${
          preview === null ? "text-rose-500 dark:text-rose-400" : "text-cyan-700 dark:text-cyan-300"
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
