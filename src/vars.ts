/**
 * 模板变量引擎（每个浮窗一套，互不影响）：
 * - 内置变量（today/date/time/account）+ 本套用户变量（A1…）+ 本套计算变量（B1…）；
 * - 表达式求值：支持 `+ - * /`、括号、一元负号、数字与变量名，
 *   例如 `A1 + A2`、`(A3 + A4) / (A1 + A2)`；
 * - 格式化：普通数 / 百分比（0.8 → "80%"），小数位数可控制；
 * - tokenize：模板 → 片段数组，未知写法原样显示。
 */
import type { OverlayConfig } from "./types";

export type Vars = Record<string, string>;

/** 数值求值时用的取值函数：按变量名取数，取不到返回 null */
type Lookup = (key: string) => number | null;

// ---------- 表达式求值（小型递归下降解析器） ----------

type Tok = { t: "num"; v: number } | { t: "id"; v: string } | { t: "op"; v: string };

function tokenizeExpr(src: string): Tok[] | null {
  const toks: Tok[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    if (/[0-9]/.test(c) || (c === "." && /[0-9]/.test(src[i + 1] ?? ""))) {
      const m = /^[0-9]*\.?[0-9]+/.exec(src.slice(i))!;
      toks.push({ t: "num", v: parseFloat(m[0]) });
      i += m[0].length;
      continue;
    }
    if (/[A-Za-z_]/.test(c)) {
      const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i))!;
      toks.push({ t: "id", v: m[0] });
      i += m[0].length;
      continue;
    }
    if ("+-*/()".includes(c)) {
      toks.push({ t: "op", v: c });
      i++;
      continue;
    }
    return null; // 非法字符
  }
  return toks;
}

/** 求值；任何错误（语法错、未知变量、除零）都返回 null → 显示为 "—" */
export function evalExpr(expr: string, lookup: Lookup): number | null {
  const toks = tokenizeExpr(expr);
  if (!toks || toks.length === 0) return null;
  const pos = { i: 0 };

  const peek = () => toks[pos.i];
  const eat = (op: string) => {
    const t = peek();
    if (t && t.t === "op" && t.v === op) {
      pos.i++;
      return true;
    }
    return false;
  };

  const additive = (): number | null => {
    let left = multiplicative();
    while (left !== null && (eat("+") || eat("-"))) {
      const op = toks[pos.i - 1].v;
      const right = multiplicative();
      if (right === null) return null;
      left = op === "+" ? left + right : left - right;
    }
    return left;
  };

  const multiplicative = (): number | null => {
    let left = unary();
    while (left !== null && (eat("*") || eat("/"))) {
      const op = toks[pos.i - 1].v;
      const right = unary();
      if (right === null || (op === "/" && right === 0)) return null;
      left = op === "*" ? left * right : left / right;
    }
    return left;
  };

  const unary = (): number | null => {
    if (eat("-")) {
      const v = unary();
      return v === null ? null : -v;
    }
    return primary();
  };

  const primary = (): number | null => {
    const t = peek();
    if (!t) return null;
    if (t.t === "num") {
      pos.i++;
      return t.v;
    }
    if (t.t === "id") {
      pos.i++;
      return lookup(t.v);
    }
    if (t.v === "(") {
      pos.i++;
      const v = additive();
      return eat(")") ? v : null;
    }
    return null;
  };

  const result = additive();
  return pos.i === toks.length ? result : null;
}

// ---------- 格式化 ----------

/** 普通数：decimals >= 0 按位数定；-1 表示"智能"（去掉无意义的尾零） */
export function fmtNum(v: number, decimals: number): string {
  if (decimals >= 0) return v.toFixed(decimals);
  return String(parseFloat(v.toFixed(6)));
}

/** 百分比：0.8 → "80%"、"87.5%"（decimals 控制小数位） */
export function fmtPercent(v: number, decimals: number): string {
  return `${fmtNum(v * 100, decimals)}%`;
}

// ---------- 全量变量表（某一套浮窗的） ----------

export interface VarDef {
  key: string;
  label: string;
  computed?: boolean;
}

/** 内置变量：不存库，每次渲染时计算 */
function builtinDefs(): VarDef[] {
  return [
    { key: "today", label: "今日日期" },
    { key: "date", label: "日期" },
    { key: "time", label: "时间" },
    { key: "account", label: "本套账号" },
  ];
}

/** 变量芯片列表（用于模板编辑器"点击插入"），来源 = 这一套浮窗自己的变量 */
export function varDefs(overlay: OverlayConfig): VarDef[] {
  return [
    ...builtinDefs(),
    ...overlay.variables.map((v) => ({ key: v.key, label: v.name })),
    ...overlay.computed.map((c) => ({
      key: c.key,
      label: `${c.name} = ${c.expr}`,
      computed: true,
    })),
  ];
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/**
 * 构建数值求值的 lookup：本套原始变量直接取值；计算变量惰性求值 + 循环检测。
 * computeVars 和界面上的实时预览共用。
 */
export function makeLookup(overlay: OverlayConfig): Lookup {
  const scope: Record<string, number> = {};
  for (const v of overlay.variables) scope[v.key] = v.value;
  const defs = new Map(overlay.computed.map((c) => [c.key, c]));
  const memo = new Map<string, number | null>();
  const evaluating = new Set<string>();

  const evalKey = (key: string): number | null => {
    if (key in scope) return scope[key];
    if (memo.has(key)) return memo.get(key)!;
    const def = defs.get(key);
    if (!def || evaluating.has(key)) return null; // 未知变量 / 循环引用
    evaluating.add(key);
    const val = evalExpr(def.expr, evalKey); // 递归：表达式里引用的计算变量同样走这里
    evaluating.delete(key);
    memo.set(key, val);
    return val;
  };
  return evalKey;
}

/**
 * 把一套浮窗配置 + 当前时间算成 `Record<变量名, 显示字符串>`。
 * 计算变量可以引用其它计算变量（循环引用求值为 "—"）。
 */
export function computeVars(overlay: OverlayConfig, now: Date): Vars {
  const y = now.getFullYear();
  const m = pad2(now.getMonth() + 1);
  const d = pad2(now.getDate());

  const lookup = makeLookup(overlay);

  const out: Vars = {
    today: `${y}${m}${d}`,
    date: `${y}-${m}-${d}`,
    time: `${pad2(now.getHours())}:${pad2(now.getMinutes())}`,
    account: overlay.account || "未设置",
  };
  for (const v of overlay.variables) out[v.key] = fmtNum(v.value, -1);
  for (const c of overlay.computed) {
    const val = lookup(c.key);
    out[c.key] =
      val === null ? "—" : c.format === "percent" ? fmtPercent(val, c.decimals) : fmtNum(val, c.decimals);
  }
  return out;
}

export interface Token {
  text: string;
  isVar: boolean;
}

/** 模板 → 片段数组；形如 {abc} 的已知变量替换，未知写法原样保留 */
export function tokenize(template: string, vars: Vars): Token[] {
  const tokens: Token[] = [];
  const re = /\{([a-zA-Z][a-zA-Z0-9_]*)\}/g;
  let last = 0;
  for (let match = re.exec(template); match !== null; match = re.exec(template)) {
    if (match.index > last) tokens.push({ text: template.slice(last, match.index), isVar: false });
    const key = match[1];
    tokens.push({ text: key in vars ? vars[key] : match[0], isVar: key in vars });
    last = match.index + match[0].length;
  }
  if (last < template.length) tokens.push({ text: template.slice(last), isVar: false });
  return tokens;
}
