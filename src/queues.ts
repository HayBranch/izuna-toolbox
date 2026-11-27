/** 排队队列的前端小工具：排序、规则校验、分组配色与特效选项。 */
import type { QueueCountEffect, QueueConfig, QueueEnterEffect, QueueEntry, QueueRule } from "./types";

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

/** 逐条校验正则，返回每条非空规则对应的错误（下标与原始规则一致） */
export function patternErrors(rules: QueueRule[]): Array<{ line: number; message: string }> {
  const errors: Array<{ line: number; message: string }> = [];
  rules.forEach((rule, index) => {
    const value = rule.pattern.trim();
    if (!value) return;
    try {
      new RegExp(value);
    } catch (e) {
      errors.push({ line: index + 1, message: String(e) });
    }
  });
  return errors;
}

/** 分组色板：取自常见的 Tableau / Tailwind 500 色系，明暗背景都容易区分 */
export const GROUP_COLORS = [
  "#3b82f6", // 蓝
  "#ef4444", // 红
  "#10b981", // 翠绿
  "#f59e0b", // 琥珀
  "#8b5cf6", // 紫
  "#ec4899", // 粉
  "#14b8a6", // 青
  "#f97316", // 橙
  "#6366f1", // 靛蓝
  "#84cc16", // 黄绿
  "#06b6d4", // 天蓝
  "#a855f7", // 洋紫
];

export const DEFAULT_GROUP_COLOR = GROUP_COLORS[0];

/** 为新分组挑一个尚未使用的色板颜色；同组已有颜色时直接沿用 */
export function nextGroupColor(rules: QueueRule[], group: string, skipIndex = -1): string {
  const name = group.trim();
  if (name) {
    const same = rules.find((r, i) => i !== skipIndex && r.group.trim() === name && r.color);
    if (same) return same.color;
  }
  const used = new Set(
    rules
      .filter((_, i) => i !== skipIndex)
      .map((r) => r.color.trim().toLowerCase())
      .filter(Boolean),
  );
  return GROUP_COLORS.find((c) => !used.has(c.toLowerCase())) ?? GROUP_COLORS[used.size % GROUP_COLORS.length];
}

/** 保存前统一：分组去空白，同组使用第一次出现的颜色，空色回退默认色 */
export function normalizeRules(rules: QueueRule[]): QueueRule[] {
  const colors = new Map<string, string>();
  return rules
    .map((r) => ({
      pattern: r.pattern.trim(),
      group: r.group.trim(),
      color: r.color.trim() || DEFAULT_GROUP_COLOR,
    }))
    .filter((r) => r.pattern !== "")
    .map((r) => {
      if (!r.group) return r;
      const first = colors.get(r.group);
      if (first) return { ...r, color: first };
      colors.set(r.group, r.color);
      return r;
    });
}

/** 弹窗里把分组名映射到颜色（同组取第一条规则的颜色） */
export function groupColorMap(rules: QueueRule[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const rule of rules) {
    const group = rule.group.trim();
    if (group && !map.has(group)) map.set(group, rule.color || DEFAULT_GROUP_COLOR);
  }
  return map;
}

/** 昵称脱敏：保留首尾字符，中间替换成 *（1 个字变 *，2 个字保留首字） */
export function maskNickname(name: string): string {
  const chars = [...name];
  if (chars.length <= 1) return "*";
  if (chars.length === 2) return `${chars[0]}*`;
  return `${chars[0]}${"*".repeat(chars.length - 2)}${chars[chars.length - 1]}`;
}

export const ENTER_EFFECTS: Array<{ value: QueueEnterEffect; label: string }> = [
  { value: "none", label: "无特效" },
  { value: "fade", label: "淡入" },
  { value: "slide", label: "滑入" },
  { value: "zoom", label: "缩放" },
  { value: "pop", label: "弹跳" },
];

export const COUNT_EFFECTS: Array<{ value: QueueCountEffect; label: string }> = [
  { value: "none", label: "无特效" },
  { value: "bump", label: "跳动" },
  { value: "zoom", label: "放大" },
  { value: "flash", label: "闪烁" },
  { value: "glow", label: "发光" },
  { value: "rise", label: "上浮" },
];
