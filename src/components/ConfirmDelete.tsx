/** 两段式删除按钮：第一次点变成"确认删除?"，3 秒内再点才真正删除。
 *  不用 window.confirm —— WebView2 下原生弹窗不可靠，且破坏暗色观感。 */
import { useEffect, useState } from "react";

export default function ConfirmDelete({
  onConfirm,
  title = "删除",
}: {
  onConfirm: () => void;
  title?: string;
}) {
  const [armed, setArmed] = useState(false);

  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(t);
  }, [armed]);

  return (
    <button
      type="button"
      onClick={() => {
        if (!armed) {
          setArmed(true);
          return;
        }
        setArmed(false);
        onConfirm();
      }}
      title={armed ? "再点一次确认删除" : title}
      className={
        armed
          ? "shrink-0 rounded-lg border border-rose-500/60 bg-rose-500/15 px-2 py-1 text-xs text-rose-300"
          : "shrink-0 px-1 text-sm text-slate-600 transition hover:text-rose-400"
      }
    >
      {armed ? "确认删除?" : "🗑"}
    </button>
  );
}
