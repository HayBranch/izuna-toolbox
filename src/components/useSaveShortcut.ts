/**
 * 详情页的 Ctrl+S / ⌘+S 保存快捷键。
 *
 * 挂在 window 上而不是某张卡片上：键盘事件只会从「当前获得焦点的元素」向上冒泡，
 * 焦点落在页面空白、预览区或左侧导航（此时是 <body>）时，卡片上的 onKeyDown
 * 根本收不到，快捷键就会像失效一样。
 */
import { useEffect, useRef } from "react";

export function useSaveShortcut(save: () => void | Promise<unknown>) {
  // 每次渲染都更新引用：监听只注册一次，但永远调用最新的草稿与状态
  const latest = useRef(save);
  latest.current = save;

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
      // 中文输入法组词过程中不抢按键；按住不放也不重复保存
      if (e.isComposing || e.repeat) return;
      if (e.code !== "KeyS" && e.key.toLowerCase() !== "s") return;
      e.preventDefault();
      void latest.current();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
