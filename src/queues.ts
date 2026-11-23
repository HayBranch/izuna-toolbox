/** 排队队列的前端小工具：排序、正则校验。 */
import type { QueueConfig, QueueEntry } from "./types";

/**
 * 按队列配置排序：
 * - time：首次发送时间升序（先到先得），同一时间按 id；
 * - count：发送次数降序（多的在前），次数相同仍先到先得。
 */
export function sortedQueueEntries(queue: Pick<QueueConfig, "entries" | "sortMode">): QueueEntry[] {
  const entries = [...queue.entries];
  if (queue.sortMode === "count") {
    entries.sort((a, b) => b.count - a.count || a.firstAt - b.firstAt || a.id - b.id);
  } else {
    entries.sort((a, b) => a.firstAt - b.firstAt || a.id - b.id);
  }
  return entries;
}

/** 逐行校验正则，返回每条非空规则对应的错误（下标与原始行一致） */
export function patternErrors(patterns: string[]): Array<{ line: number; message: string }> {
  const errors: Array<{ line: number; message: string }> = [];
  patterns.forEach((pattern, index) => {
    const value = pattern.trim();
    if (!value) return;
    try {
      new RegExp(value);
    } catch (e) {
      errors.push({ line: index + 1, message: String(e) });
    }
  });
  return errors;
}

/** 把正则文本按行拆成规则（保留空行，提交时由后端清洗） */
export function splitPatterns(text: string): string[] {
  return text.split(/\r?\n/);
}
