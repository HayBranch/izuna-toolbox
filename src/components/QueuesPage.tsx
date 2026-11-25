/**
 * 排队队列页 —— 两级界面：
 * 1) 列表：每个队列的直播间、匹配规则数、人数、连接状态与显隐开关；
 * 2) 详情：直播间 / 刷新间隔 / 正则 / 排序 / 弹窗样式 / 成员管理 / 预览。
 *
 * 数据流与其他页面一致：命令写 SQLite → Rust 广播全量配置 → 这里按 id 重新取最新值。
 * 编辑策略（v0.1.3 起）：直播间 / 刷新间隔 / 正则 / 排序 / 弹窗样式只写本地草稿，
 * 点「保存」或按 Ctrl+S 才写入 SQLite；监听、显隐、置顶、穿透、成员增删改仍立即生效。
 * 有未保存修改时返回列表 / 切换页面会弹窗询问「保存 / 丢弃 / 取消」。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import {
  addQueue,
  addQueueEntry,
  clearQueueEntries,
  deleteQueue,
  deleteQueueEntry,
  getQueueStatus,
  moveQueue,
  resetQueuePosition,
  renameQueueEntry,
  resizeQueue,
  setQueueAlwaysTop,
  setQueueClickThrough,
  setQueueEnabled,
  setQueueEntryCount,
  setQueueShown,
  stepQueueEntry,
  updateQueue,
} from "../api";
import { patternErrors, splitPatterns, sortedQueueEntries } from "../queues";
import type { AppConfig, QueueConfig, QueueStatus } from "../types";
import { formatDateTime } from "../utils";
import ConfirmDelete from "./ConfirmDelete";
import { ChipToggle, ColorField, InlineNumber, MiniToggle, NumberField, Toggle } from "./Fields";
import QueueView from "./QueueView";
import { useUnsavedGuard } from "./UnsavedGuard";

type Run = (p: Promise<unknown>) => Promise<boolean>;

/** 刷新间隔说明：它控制的是批量写入/界面刷新频率，不是 HTTP 轮询 */
const POLL_HINT = "弹幕实时接收，按这个间隔批量写入并刷新界面（1-60 秒）";

export default function QueuesPage({ config }: { config: AppConfig }) {
  const [error, setError] = useState<string | null>(null);
  const [openId, setOpenId] = useState<number | null>(null);
  const [statuses, setStatuses] = useState<Record<number, QueueStatus>>({});

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

  // 连接状态只在 Rust 内存里，定时拉取（弹幕状态变化不触发 config-changed）
  useEffect(() => {
    let alive = true;
    const load = () => {
      getQueueStatus()
        .then((list) => {
          if (!alive) return;
          const next: Record<number, QueueStatus> = {};
          for (const s of list) next[s.queueId] = s;
          setStatuses(next);
        })
        .catch(() => {});
    };
    load();
    const timer = setInterval(load, 5000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  const open = openId === null ? null : (config.queues.find((q) => q.id === openId) ?? null);

  const create = async () => {
    try {
      const next = await addQueue();
      setError(null);
      const created = next.queues[next.queues.length - 1];
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
        <QueueEditor
          key={open.id}
          queue={open}
          allQueues={config.queues}
          status={statuses[open.id]}
          onBack={() => setOpenId(null)}
          run={run}
          reportError={setError}
        />
      ) : (
        <>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="max-w-3xl text-sm leading-6 text-slate-500 dark:text-slate-400">
              每个队列独立配置直播间、匹配正则与排序方式；命中的弹幕会按 UID 累计次数，
              并实时显示在自己的透明弹窗里。弹幕走 B 站直播长连接，实时接收，
              <strong className="font-medium text-slate-600 dark:text-slate-300">
                「刷新间隔」只控制批量写入与界面刷新频率
              </strong>
              。列表里的监听 / 显示 / 置顶 / 穿透可以直接点标签切换。详情页改完点「保存」或按 Ctrl+S 才写入本地 SQLite，
              未保存时切换页面会提示。
            </p>
            <button
              onClick={() => void create()}
              className="shrink-0 rounded-xl bg-indigo-500 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-400"
            >
              ＋ 新建队列
            </button>
          </div>

          <ul className="space-y-2">
            {config.queues.map((queue) => (
              <QueueRow
                key={queue.id}
                queue={queue}
                status={statuses[queue.id]}
                run={run}
                onOpen={() => setOpenId(queue.id)}
              />
            ))}
          </ul>

          {config.queues.length === 0 && (
            <p className="rounded-2xl border border-dashed border-slate-300 p-10 text-center text-sm text-slate-500 dark:border-slate-800 dark:text-slate-600">
              还没有排队队列，点右上角「＋ 新建队列」创建第一个
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** 连接状态徽标：未启用 / 未设置房间 / 连接中 / 已连接 / 连接失败 */
function connectionBadge(queue: QueueConfig, status?: QueueStatus) {
  if (!queue.enabled) {
    return { text: "已暂停", cls: "bg-slate-500/10 text-slate-500 dark:text-slate-400", title: "点击队列右侧「监听中」恢复监听" };
  }
  if (queue.roomId <= 0) {
    return { text: "未设置直播间", cls: "bg-amber-500/15 text-amber-700 dark:text-amber-300", title: "进入配置填写直播间号" };
  }
  if (status?.connected) {
    const real =
      status.realRoomId > 0 && status.realRoomId !== queue.roomId ? `（真实房间 ${status.realRoomId}）` : "";
    return { text: "已连接", cls: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300", title: `弹幕连接正常${real}` };
  }
  if (status?.lastError) {
    return { text: "连接失败", cls: "bg-rose-500/15 text-rose-700 dark:text-rose-300", title: status.lastError };
  }
  return { text: "连接中…", cls: "bg-sky-500/10 text-sky-700 dark:text-sky-300", title: "正在连接 B 站弹幕服务器" };
}

/** 列表行：名称 + 基本配置 + 连接状态 + 四个可直接切换的开关 */
function QueueRow({
  queue,
  status,
  run,
  onOpen,
}: {
  queue: QueueConfig;
  status?: QueueStatus;
  run: Run;
  onOpen: () => void;
}) {
  const updated = queue.updatedAt || queue.createdAt;
  const badge = connectionBadge(queue, status);
  return (
    <li className="rounded-2xl border border-slate-200 bg-white/70 transition hover:border-indigo-400 hover:bg-white hover:shadow-lg hover:shadow-indigo-500/10 dark:border-slate-800 dark:bg-slate-900/60 dark:hover:border-indigo-500 dark:hover:bg-slate-900">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-3 px-5 py-4">
        <button
          type="button"
          onClick={onOpen}
          title="打开这个队列的配置"
          className="flex min-w-0 flex-1 items-center gap-3 rounded-lg text-left focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500"
        >
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-emerald-500/10 text-base text-emerald-600 dark:text-emerald-300">
            💬
          </span>
          <span className="min-w-0">
            <span className="flex flex-wrap items-center gap-2">
              <span className="truncate font-medium text-slate-900 dark:text-slate-100">
                {queue.name || "未命名队列"}
              </span>
              <span className={`rounded-full px-2 py-0.5 text-[11px] font-normal ${badge.cls}`} title={badge.title}>
                {badge.text}
              </span>
            </span>
            <span className="mt-0.5 block truncate text-xs text-slate-500 dark:text-slate-500">
              {queue.roomId > 0 ? `直播间 ${queue.roomId}` : "未设置直播间"}
              {" · "}
              {queue.sortMode === "count" ? "按发送次数" : "按发送时间"}
              {" · "}
              {queue.entries.length} 人 · 间隔 {queue.pollSecs}s · 规则 {queue.patterns.length} 条
              {" · "}修改 {formatDateTime(updated)}
            </span>
          </span>
        </button>

        <div className="flex flex-wrap items-center gap-1.5">
          <ChipToggle
            on={queue.enabled}
            onLabel="监听中"
            offLabel="已暂停"
            tone="emerald"
            title={queue.enabled ? "点击暂停接收弹幕（已有名单保留）" : "点击恢复接收弹幕"}
            onClick={() => run(setQueueEnabled(queue.id, !queue.enabled))}
          />
          <ChipToggle
            on={queue.shown}
            onLabel="显示中"
            offLabel="已隐藏"
            title={queue.shown ? "点击隐藏这个弹窗（配置与名单保留）" : "点击显示这个弹窗"}
            onClick={() => run(setQueueShown(queue.id, !queue.shown))}
          />
          <ChipToggle
            on={queue.alwaysOnTop}
            onLabel="置顶"
            offLabel="不置顶"
            title={queue.alwaysOnTop ? "点击取消弹窗置顶" : "点击让弹窗始终置顶"}
            onClick={() => run(setQueueAlwaysTop(queue.id, !queue.alwaysOnTop))}
          />
          <ChipToggle
            on={queue.clickThrough}
            onLabel="鼠标穿透"
            offLabel="可点击"
            tone="amber"
            title={queue.clickThrough ? "点击关闭鼠标穿透" : "点击开启鼠标穿透（弹窗不挡游戏操作）"}
            onClick={() => run(setQueueClickThrough(queue.id, !queue.clickThrough))}
          />
        </div>

        <button
          type="button"
          onClick={onOpen}
          title="打开这个队列的配置"
          className="flex shrink-0 items-center gap-1 rounded-lg px-2 py-1 text-xs font-medium text-slate-400 transition hover:bg-indigo-500/10 hover:text-indigo-600 focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500 dark:text-slate-500 dark:hover:text-indigo-300"
        >
          配置 <span aria-hidden="true">›</span>
        </button>
      </div>
    </li>
  );
}

function QueueEditor({
  queue,
  allQueues,
  status,
  onBack,
  run,
  reportError,
}: {
  queue: QueueConfig;
  allQueues: QueueConfig[];
  status?: QueueStatus;
  onBack: () => void;
  run: Run;
  reportError: (message: string | null) => void;
}) {
  const guard = useUnsavedGuard();
  const [draft, setDraft] = useState(queue);
  const [patternText, setPatternText] = useState(queue.patterns.join("\n"));
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const [nickname, setNickname] = useState("");
  const [addCount, setAddCount] = useState(1);

  const patternList = useMemo(() => splitPatterns(patternText), [patternText]);
  const patternErrs = useMemo(() => patternErrors(patternList), [patternList]);
  const patternsValid = patternErrs.length === 0 && patternList.some((p) => p.trim() !== "");

  // 正则测试框：输入弹幕文本后延迟 0.5 秒验证是否命中当前正则（防止逐字输入时频繁校验）。
  // null 表示尚未验证（输入为空 / 正则有语法错误 / 没有规则）。
  const [testText, setTestText] = useState("");
  const [testMatched, setTestMatched] = useState<boolean | null>(null);
  useEffect(() => {
    const value = testText.trim();
    if (!value || patternErrs.length > 0 || !patternList.some((p) => p.trim())) {
      setTestMatched(null);
      return;
    }
    const timer = window.setTimeout(() => {
      const regs = patternList
        .filter((p) => p.trim())
        .flatMap((p) => {
          try {
            return [new RegExp(p.trim())];
          } catch {
            return [];
          }
        });
      setTestMatched(regs.some((re) => re.test(value)));
    }, 500);
    return () => window.clearTimeout(timer);
  }, [testText, patternList, patternErrs]);

  const sorted = useMemo(() => sortedQueueEntries(queue), [queue]);
  const live = useMemo(() => ({ ...queue, ...draft, entries: queue.entries }), [queue, draft]);
  const badge = connectionBadge(queue, status);

  // 配置回流（自己保存的或别的窗口改的）：没有未保存修改时整体同步；
  // 有修改时保留草稿，只跟随成员名单这类运行时字段（live 已单独处理）。
  useEffect(() => {
    if (dirty) return;
    setDraft(queue);
    setPatternText(queue.patterns.join("\n"));
  }, [queue, dirty]);

  /** 改草稿：只动本地状态，点「保存」/ Ctrl+S 才写数据库 */
  const patch = (partial: Partial<QueueConfig>) => {
    setDraft((d) => ({ ...d, ...partial }));
    setDirty(true);
    setFailed(false);
  };

  const changePatterns = (text: string) => {
    setPatternText(text);
    setDirty(true);
    setFailed(false);
  };

  const discard = () => {
    setDraft(queue);
    setPatternText(queue.patterns.join("\n"));
    setDirty(false);
    setFailed(false);
  };

  /** 保存整套配置：直播间 / 间隔 / 正则 / 排序 / 弹窗样式 */
  const saveNow = async (): Promise<boolean> => {
    if (saving) return false;
    if (!dirty) return true;
    if (patternErrors(patternList).length > 0 || !patternsValid) {
      setFailed(true);
      reportError("正则不合法或为空，请修正后再保存");
      return false;
    }
    setSaving(true);
    setFailed(false);
    try {
      const next = { ...draft, patterns: patternList };
      await updateQueue(next);
      setDirty(false);
      reportError(null);
      return true;
    } catch (e) {
      setFailed(true);
      reportError(String(e));
      return false;
    } finally {
      setSaving(false);
    }
  };

  // 守卫给「切换页面 / 返回列表」用：始终读最新一次渲染的状态与草稿。
  const latest = useRef({ dirty, saving, saveNow, discard });
  latest.current = { dirty, saving, saveNow, discard };
  useEffect(() => {
    if (!guard) return;
    guard.register({
      isDirty: () => latest.current.dirty && !latest.current.saving,
      save: () => latest.current.saveNow(),
      discard: () => latest.current.discard(),
    });
    return () => guard.register(null);
  }, [guard]);

  const leave = async () => {
    if (guard && !(await guard.confirmLeave())) return;
    onBack();
  };

  const addEntry = async () => {
    const name = nickname.trim();
    if (!name) return;
    const ok = await run(addQueueEntry(queue.id, name, addCount));
    if (ok) setNickname("");
  };

  const realRoom = status?.realRoomId && status.realRoomId !== queue.roomId ? status.realRoomId : null;

  const saveState: "saved" | "dirty" | "saving" | "error" = saving
    ? "saving"
    : failed
      ? "error"
      : dirty
        ? "dirty"
        : "saved";

  return (
    <div
      className="space-y-5"
      onKeyDown={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
          e.preventDefault();
          void saveNow();
        }
      }}
    >
      {/* 头部 */}
      <div className="rounded-2xl border border-slate-200 bg-white/70 p-5 dark:border-slate-800 dark:bg-slate-900/60">
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            onClick={() => void leave()}
            title="返回队列列表（有未保存修改时会先询问）"
            className="shrink-0 rounded-lg border border-slate-300 px-2.5 py-1.5 text-xs font-medium text-slate-600 transition hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
          >
            ← 返回列表
          </button>
          <input
            value={draft.name}
            onChange={(e) => patch({ name: e.target.value })}
            placeholder="队列名称（例如：排队上车）"
            aria-label="队列名称"
            className="min-w-40 flex-1 rounded-lg border border-transparent bg-transparent px-2 py-1 text-lg font-semibold text-slate-900 outline-none transition hover:border-slate-300 focus:border-indigo-500 dark:text-slate-100 dark:hover:border-slate-700"
          />
          <span
            className={`rounded-full px-2.5 py-1 text-[11px] ${badge.cls}`}
            title={badge.title}
          >
            {badge.text}
          </span>

          <SaveBadge state={saveState} onClick={() => void saveNow()} />
          <ConfirmDelete
            title={`删除队列「${queue.name || "未命名队列"}」及其全部名单`}
            onConfirm={() => void run(deleteQueue(queue.id))}
          />
        </div>

        <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
          <Toggle
            on={queue.enabled}
            label={queue.enabled ? "监听中" : "已暂停"}
            tone="emerald"
            onClick={() => void run(setQueueEnabled(queue.id, !queue.enabled))}
          />
          <Toggle
            on={queue.shown}
            label={queue.shown ? "弹窗显示中" : "弹窗已隐藏"}
            onClick={() => void run(setQueueShown(queue.id, !queue.shown))}
          />
          <Toggle
            on={queue.alwaysOnTop}
            label={queue.alwaysOnTop ? "置顶" : "不置顶"}
            onClick={() => void run(setQueueAlwaysTop(queue.id, !queue.alwaysOnTop))}
          />
          <Toggle
            on={queue.clickThrough}
            label={queue.clickThrough ? "鼠标穿透" : "可点击"}
            tone="amber"
            onClick={() => void run(setQueueClickThrough(queue.id, !queue.clickThrough))}
          />
          <span className="ml-auto text-[11px] text-slate-400 dark:text-slate-600">
            创建 {formatDateTime(queue.createdAt)} · 修改 {formatDateTime(queue.updatedAt || queue.createdAt)}
          </span>
        </div>

        {status?.lastError && queue.enabled && (
          <p className="mt-3 break-all rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
            连接提示：{status.lastError}
            {realRoom ? `（已解析真实房间 ${realRoom}）` : ""}
          </p>
        )}
      </div>

      {/* 直播间与匹配规则 */}
      <section className="rounded-2xl border border-slate-200 bg-white/70 p-5 dark:border-slate-800 dark:bg-slate-900/60">
        <h3 className="text-sm font-semibold text-slate-900 dark:text-slate-100">直播间与匹配规则</h3>
        <p className="mt-1 text-xs leading-5 text-slate-500 dark:text-slate-400">
          每分钟收到的弹幕里，文本命中任意一条正则就记一次排队；同一个 UID 累计次数。
          支持 B 站短号，保存后自动解析真实房间号。
        </p>

        <div className="mt-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <NumberField
            label="直播间号"
            value={draft.roomId}
            min={0}
            max={999_999_999}
            integer
            onChange={(v) => patch({ roomId: v })}
          />
          <NumberField
            label="刷新间隔（秒）"
            value={draft.pollSecs}
            min={1}
            max={60}
            integer
            onChange={(v) => patch({ pollSecs: v })}
          />
          <NumberField
            label="弹窗最多显示（人）"
            value={draft.maxEntries}
            min={1}
            max={100}
            integer
            onChange={(v) => patch({ maxEntries: v })}
          />
          <div className="text-xs text-slate-500 dark:text-slate-400">
            排序方式
            <div className="mt-1 flex gap-1.5">
              <MiniToggle
                label="发送时间"
                on={draft.sortMode === "time"}
                onClick={() => patch({ sortMode: "time" })}
              />
              <MiniToggle
                label="发送次数"
                on={draft.sortMode === "count"}
                onClick={() => patch({ sortMode: "count" })}
              />
            </div>
            <p className="mt-1 text-[11px] text-slate-400 dark:text-slate-600">
              {draft.sortMode === "count" ? "次数多的在前，同次数先到先得" : "先到先得，次数只影响显示"}
            </p>
          </div>
        </div>

        <div className="mt-4">
          <p className="text-xs text-slate-500 dark:text-slate-400">匹配正则（每行一条，任意命中即计数）</p>
          <textarea
            value={patternText}
            onChange={(e) => changePatterns(e.target.value)}
            rows={4}
            spellCheck={false}
            placeholder={"排队\n^排队$"}
            className={`mt-1 w-full resize-y rounded-xl border bg-white px-3 py-2 font-mono text-sm text-slate-800 outline-none transition dark:bg-slate-950/60 dark:text-slate-200 ${
              patternErrs.length > 0
                ? "border-rose-500/60 focus:border-rose-500"
                : "border-slate-300 focus:border-indigo-500 dark:border-slate-700"
            }`}
          />
          {/* 正则测试：输入一条弹幕文本，0.5 秒后显示是否命中（✓ 命中 / ✗ 未命中） */}
          <div className="mt-2 flex items-center gap-2">
            <input
              value={testText}
              onChange={(e) => setTestText(e.target.value)}
              spellCheck={false}
              placeholder="粘贴一条弹幕文本，验证是否命中上面的正则"
              className="min-w-0 flex-1 rounded-xl border border-slate-300 bg-white px-3 py-2 text-sm text-slate-800 outline-none transition placeholder:text-slate-400 focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200 dark:placeholder:text-slate-600"
            />
            <span
              aria-live="polite"
              title={testMatched === null ? "输入文本后验证" : testMatched ? "命中正则" : "未命中正则"}
              className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-lg font-semibold ${
                testMatched === true
                  ? "bg-emerald-100 text-emerald-600 dark:bg-emerald-500/20 dark:text-emerald-400"
                  : testMatched === false
                    ? "bg-rose-100 text-rose-600 dark:bg-rose-500/20 dark:text-rose-400"
                    : "bg-slate-100 text-slate-400 dark:bg-slate-800 dark:text-slate-600"
              }`}
            >
              {testMatched === true ? "✓" : testMatched === false ? "✗" : "·"}
            </span>
          </div>
          {testText.trim() && patternErrs.length > 0 && (
            <p className="mt-1 text-[11px] text-rose-600 dark:text-rose-400">正则有语法错误，无法验证</p>
          )}
          {patternErrs.length > 0 ? (
            <p className="mt-1 break-all text-[11px] text-rose-600 dark:text-rose-400">
              第 {patternErrs[0].line} 条正则不合法，暂未保存：{patternErrs[0].message}
            </p>
          ) : patternList.filter((p) => p.trim()).length === 0 ? (
            <p className="mt-1 text-[11px] text-rose-600 dark:text-rose-400">至少保留一条正则（例如：排队）</p>
          ) : (
            <p className="mt-1 text-[11px] text-slate-400 dark:text-slate-600">
              示例：`排队` 命中含「排队」的弹幕；`^排队$` 只命中正好是「排队」的弹幕。
            </p>
          )}
        </div>

        <p className="mt-3 rounded-lg bg-slate-100 px-3 py-2 text-[11px] leading-5 text-slate-500 dark:bg-slate-950/60 dark:text-slate-500">
          {POLL_HINT}。直播间必须正在开播；下播后连接会断开并自动重试。
        </p>
      </section>

      {/* 弹窗外观 */}
      <section className="rounded-2xl border border-slate-200 bg-white/70 p-5 dark:border-slate-800 dark:bg-slate-900/60">
        <h3 className="text-sm font-semibold text-slate-900 dark:text-slate-100">弹窗外观与位置</h3>
        <p className="mt-1 text-xs leading-5 text-slate-500 dark:text-slate-400">
          弹窗与悬浮字幕一样是透明无边框窗口，可被 OBS「窗口采集」捕获；
          悬停弹窗可拖动位置、拖边缘改尺寸。
        </p>

        <div className="mt-4 grid items-end gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <NumberField
            label="字号"
            value={draft.fontSize}
            min={10}
            max={72}
            integer
            onChange={(v) => patch({ fontSize: v })}
          />
          <ColorField label="文字颜色" value={draft.fontColor} onChange={(v) => patch({ fontColor: v })} />
          <ColorField label="背景颜色" value={draft.bgColor} onChange={(v) => patch({ bgColor: v })} />
          <NumberField
            label="背景不透明度（%）"
            value={draft.bgOpacity}
            min={0}
            max={100}
            integer
            onChange={(v) => patch({ bgOpacity: v })}
          />
        </div>

        <div className="mt-4 flex flex-wrap items-center gap-4 text-xs text-slate-500 dark:text-slate-400">
          <MiniToggle label="加粗" on={draft.bold} onClick={() => patch({ bold: !draft.bold })} />
          <MiniToggle label="文字描边" on={draft.shadow} onClick={() => patch({ shadow: !draft.shadow })} />
          <label className="flex items-center gap-2">
            自动宽度上限
            <InlineNumber
              value={draft.maxWidth}
              min={80}
              max={3000}
              suffix="px"
              onChange={(v) => patch({ maxWidth: v })}
            />
          </label>
          <span className="text-slate-400 dark:text-slate-600">
            手动尺寸：{queue.width > 0 && queue.height > 0 ? `${queue.width} × ${queue.height}` : "自动按内容"}
          </span>
          {queue.width > 0 && queue.height > 0 && (
            <button
              type="button"
              onClick={() => void run(resizeQueue(queue.id, 0, 0))}
              className="rounded-lg border border-slate-300 px-2 py-0.5 text-[11px] text-slate-500 transition hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-400 dark:hover:border-indigo-500"
            >
              恢复自动尺寸
            </button>
          )}
        </div>

        <div className="mt-4 flex flex-wrap items-end gap-3">
          <NumberField label="位置 X" value={queue.x} min={-20_000} max={20_000} integer wide onChange={(v) => void run(moveQueue(queue.id, v, queue.y))} />
          <NumberField label="位置 Y" value={queue.y} min={-20_000} max={20_000} integer wide onChange={(v) => void run(moveQueue(queue.id, queue.x, v))} />
          <button
            type="button"
            onClick={() => void run(resetQueuePosition(queue.id))}
            title="把排队弹窗移回当前屏幕中央（拖出屏幕后可在这里找回）"
            className="rounded-lg border border-slate-300 px-2 py-1.5 text-xs text-slate-500 transition hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
          >
            重置位置
          </button>
          <button
            type="button"
            onClick={() => void run(setQueueShown(queue.id, true))}
            title="显示弹窗并带到前台"
            className="rounded-lg border border-slate-300 px-3 py-1.5 text-xs font-medium text-slate-600 transition hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-300 dark:hover:border-indigo-500 dark:hover:text-indigo-300"
          >
            显示弹窗
          </button>
        </div>
      </section>

      {/* 预览 */}
      <section className="rounded-2xl border border-slate-200 bg-white/70 p-5 dark:border-slate-800 dark:bg-slate-900/60">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-semibold text-slate-900 dark:text-slate-100">实时预览</h3>
          <span className="text-[11px] text-slate-400 dark:text-slate-600">
            共 {queue.entries.length} 人 · 弹窗显示前 {Math.min(queue.entries.length, draft.maxEntries)} 人
          </span>
        </div>
        <div className="mt-3 flex min-h-36 items-start justify-center rounded-xl bg-gradient-to-br from-slate-800 via-slate-900 to-black p-6">
          <QueueView cfg={live} />
        </div>
      </section>

      {/* 成员管理 */}
      <section className="rounded-2xl border border-slate-200 bg-white/70 p-5 dark:border-slate-800 dark:bg-slate-900/60">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h3 className="text-sm font-semibold text-slate-900 dark:text-slate-100">队列成员</h3>
            <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
              可删除成员、修改次数或昵称；弹幕实时累加与手动修改都走原子更新，不会互相覆盖。
            </p>
          </div>
          {queue.entries.length > 0 && (
            <ConfirmDelete
              title={`清空队列「${queue.name || "未命名队列"}」的全部 ${queue.entries.length} 名成员`}
              onConfirm={() => void run(clearQueueEntries(queue.id))}
            />
          )}
        </div>

        <div className="mt-4 flex flex-wrap items-end gap-3 rounded-xl border border-slate-200 bg-slate-50/70 p-3 dark:border-slate-800 dark:bg-slate-950/40">
          <label className="text-xs text-slate-500 dark:text-slate-400">
            昵称（手动添加）
            <input
              value={nickname}
              onChange={(e) => setNickname(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void addEntry();
              }}
              placeholder="例如：路人甲"
              className="mt-1 block w-48 rounded-lg border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-800 outline-none transition focus:border-indigo-500 dark:border-slate-700 dark:bg-slate-950/60 dark:text-slate-200"
            />
          </label>
          <NumberField label="次数" value={addCount} min={1} max={99_999} integer onChange={setAddCount} />
          <button
            type="button"
            onClick={() => void addEntry()}
            disabled={!nickname.trim()}
            className="rounded-lg bg-indigo-500 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-indigo-400 disabled:cursor-not-allowed disabled:opacity-50"
          >
            ＋ 添加成员
          </button>
          <span className="text-[11px] text-slate-400 dark:text-slate-600">
            同名成员会直接累加次数；手动添加的成员 UID 记为 0。
          </span>
        </div>

        {sorted.length === 0 ? (
          <p className="mt-4 rounded-xl border border-dashed border-slate-300 p-8 text-center text-sm text-slate-500 dark:border-slate-800 dark:text-slate-600">
            还没有成员。开播后命中正则的弹幕会自动进来，也可以在上面手动添加。
          </p>
        ) : (
          <ul className="mt-4 divide-y divide-slate-200 dark:divide-slate-800">
            {sorted.map((entry, index) => (
              <li key={entry.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 py-2">
                <span className="w-8 shrink-0 text-right font-mono text-xs text-slate-400 dark:text-slate-600">
                  {index + 1}
                </span>
                <input
                  key={`${entry.id}:${entry.nickname}`}
                  defaultValue={entry.nickname}
                  aria-label={`修改 ${entry.nickname} 的昵称`}
                  onBlur={(e) => {
                    const name = e.target.value.trim();
                    if (name && name !== entry.nickname) void run(renameQueueEntry(queue.id, entry.id, name));
                    else if (!name) e.target.value = entry.nickname;
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                  className="w-40 min-w-0 rounded-lg border border-transparent bg-transparent px-2 py-1 text-sm text-slate-800 outline-none transition hover:border-slate-300 focus:border-indigo-500 dark:text-slate-200 dark:hover:border-slate-700"
                />
                <span className="min-w-0 flex-1 truncate text-[11px] text-slate-400 dark:text-slate-600">
                  {entry.uid > 0 ? `UID ${entry.uid}` : entry.uid < 0 ? "游客" : "手动添加"} · 首次 {formatDateTime(entry.firstAt)} ·
                  最近 {formatDateTime(entry.lastAt)}
                </span>
                <div className="flex shrink-0 items-center gap-1.5">
                  <button
                    type="button"
                    title="次数 -1"
                    onClick={() => void run(stepQueueEntry(queue.id, entry.id, -1))}
                    className="h-7 w-7 rounded-lg border border-slate-300 text-sm text-slate-500 transition hover:border-rose-400 hover:text-rose-500 dark:border-slate-700 dark:text-slate-400"
                  >
                    −
                  </button>
                  <InlineNumber
                    value={entry.count}
                    min={0}
                    max={999_999}
                    onChange={(v) => void run(setQueueEntryCount(queue.id, entry.id, v))}
                  />
                  <button
                    type="button"
                    title="次数 +1"
                    onClick={() => void run(stepQueueEntry(queue.id, entry.id, 1))}
                    className="h-7 w-7 rounded-lg border border-slate-300 text-sm text-slate-500 transition hover:border-emerald-400 hover:text-emerald-600 dark:border-slate-700 dark:text-slate-400"
                  >
                    ＋
                  </button>
                  <ConfirmDelete
                    title={`从队列删除「${entry.nickname}」`}
                    onConfirm={() => void run(deleteQueueEntry(queue.id, entry.id))}
                  />
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <p className="text-[11px] leading-5 text-slate-400 dark:text-slate-600">
        房间 {queue.name || "未命名队列"} 使用独立弹窗（window label：queue-{queue.id}）。
        这份队列只是把「谁发了排队」记成名单，不涉及账号登录；所有数据仅保存在本地 SQLite。
        {allQueues.length > 1 ? ` 当前共 ${allQueues.length} 个队列。` : ""}
      </p>
    </div>
  );
}

/** 保存状态按钮：显示待保存 / 保存中 / 已保存 / 保存失败，点击立即保存 */
function SaveBadge({ state, onClick }: { state: "saved" | "dirty" | "saving" | "error"; onClick: () => void }) {
  const text =
    state === "saved" ? "已保存" : state === "dirty" ? "待保存 · 点此立即保存" : state === "saving" ? "保存中…" : "保存失败 · 重试";
  const cls =
    state === "saved"
      ? "border-emerald-500/40 text-emerald-700 dark:text-emerald-300"
      : state === "error"
        ? "border-rose-500/50 text-rose-700 dark:text-rose-300"
        : "border-slate-300 text-slate-500 hover:border-indigo-400 hover:text-indigo-600 dark:border-slate-700 dark:text-slate-400 dark:hover:border-indigo-500";
  return (
    <button
      type="button"
      onClick={onClick}
      title="把当前配置立即写入本地 SQLite（Ctrl+S）"
      className={`shrink-0 rounded-lg border px-2.5 py-1.5 text-xs font-medium transition ${cls}`}
    >
      {text}
    </button>
  );
}
