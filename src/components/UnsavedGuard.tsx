/**
 * 未保存修改的离开确认。
 *
 * 悬浮字幕 / 排队队列的详情页改成「显式保存」后，切换左侧导航或返回列表前
 * 需要先问一句：保存、丢弃还是取消。各页面通过 `useUnsavedGuard` 注册自己的
 * 守卫（是否有未保存修改 / 保存 / 丢弃），由这里统一弹窗，避免每个页面各写一套。
 */
import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";

export interface UnsavedGuard {
  /** 当前是否有未保存的修改 */
  isDirty: () => boolean;
  /** 保存；成功返回 true */
  save: () => Promise<boolean>;
  /** 放弃修改（恢复为最近一次服务端配置） */
  discard: () => void;
}

type Choice = "save" | "discard" | "cancel";

interface GuardContextValue {
  register: (guard: UnsavedGuard | null) => void;
  /** 有未保存修改时弹窗询问；返回 true 表示可以离开（已保存或已丢弃） */
  confirmLeave: () => Promise<boolean>;
}

const GuardContext = createContext<GuardContextValue | null>(null);

/** 页面里的编辑器用它注册守卫、以及自己的「返回列表」前询问 */
export function useUnsavedGuard() {
  return useContext(GuardContext);
}

export function UnsavedGuardProvider({ children }: { children: ReactNode }) {
  const guard = useRef<UnsavedGuard | null>(null);
  const [ask, setAsk] = useState<((choice: Choice) => void) | null>(null);
  const [saving, setSaving] = useState(false);
  const asking = useRef(false);

  const register = useCallback((next: UnsavedGuard | null) => {
    guard.current = next;
  }, []);

  const confirmLeave = useCallback(async () => {
    const current = guard.current;
    if (!current || !current.isDirty()) return true;
    // 弹窗已打开时忽略重复触发（例如连点导航）
    if (asking.current) return false;
    asking.current = true;
    try {
      const choice = await new Promise<Choice>((resolve) => setAsk(() => resolve));
      if (choice === "cancel") return false;
      if (choice === "discard") {
        current.discard();
        return true;
      }
      setSaving(true);
      const ok = await current.save();
      return ok;
    } finally {
      setSaving(false);
      setAsk(null);
      asking.current = false;
    }
  }, []);

  return (
    <GuardContext.Provider value={{ register, confirmLeave }}>
      {children}
      {ask && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4"
          role="dialog"
          aria-modal="true"
          aria-labelledby="unsaved-title"
        >
          <div className="w-full max-w-sm rounded-2xl border border-slate-200 bg-white p-5 shadow-2xl dark:border-slate-700 dark:bg-slate-900">
            <h3
              id="unsaved-title"
              className="text-base font-semibold text-slate-900 dark:text-slate-100"
            >
              有未保存的修改
            </h3>
            <p className="mt-2 text-sm leading-6 text-slate-500 dark:text-slate-400">
              离开前要保存这些修改吗？选择「丢弃修改」后无法恢复。
            </p>
            <div className="mt-5 flex flex-wrap justify-end gap-2">
              <button
                type="button"
                disabled={saving}
                onClick={() => ask("cancel")}
                className="rounded-lg border border-slate-300 px-3 py-1.5 text-xs font-medium text-slate-600 transition hover:border-slate-400 disabled:opacity-50 dark:border-slate-700 dark:text-slate-300 dark:hover:border-slate-500"
              >
                取消
              </button>
              <button
                type="button"
                disabled={saving}
                onClick={() => ask("discard")}
                className="rounded-lg border border-rose-500/40 px-3 py-1.5 text-xs font-medium text-rose-600 transition hover:bg-rose-500/10 disabled:opacity-50 dark:text-rose-300"
              >
                丢弃修改
              </button>
              <button
                type="button"
                disabled={saving}
                onClick={() => ask("save")}
                className="rounded-lg bg-indigo-500 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-indigo-400 disabled:cursor-wait disabled:opacity-60"
              >
                {saving ? "保存中…" : "保存"}
              </button>
            </div>
          </div>
        </div>
      )}
    </GuardContext.Provider>
  );
}
