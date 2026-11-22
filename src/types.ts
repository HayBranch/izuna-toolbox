/** 与 src-tauri 中 Rust 结构体一一对应（serde camelCase） */

/** 用户自定义变量（手动维护数值的"框框"）：key 用于模板/表达式引用，如 A1。
 *  属于某一浮窗（一套配置），只在该浮窗内有效。 */
export interface UserVar {
  id: number;
  key: string;
  /** 界面显示名，如"人机场次" */
  name: string;
  value: number;
  /** 每按一次 −/＋ 增减多少 */
  step: number;
  /** 联动：跟随本套哪个变量（UserVar.id；0 = 不跟随）。
   *  被跟随变量每次 −/＋，本变量同步同样的实际增减幅度。 */
  followId: number;
  /** 毫秒时间戳 */
  createdAt: number;
}

/** 计算变量：表达式对同一浮窗内的其它变量求值，只读 */
export interface ComputedVar {
  id: number;
  key: string;
  name: string;
  /** 如 `A1 + A2`、`A4 / (A1 + A2)` */
  expr: string;
  /** plain 普通数 | percent 百分比（0.8 → "80%"） */
  format: "plain" | "percent";
  /** 小数点位数 0-6 */
  decimals: number;
  createdAt: number;
}

/** 单个浮窗配置 = 一套完全独立的数据：自己的账号、变量、模板与样式 */
export interface OverlayConfig {
  id: number;
  name: string;
  /** 本套浮窗自己的账号（模板 {account}） */
  account: string;
  /** 模板文本，{today} {A1} 这类占位符渲染时替换 */
  template: string;
  /** 是否显示（隐藏 ≠ 删除） */
  shown: boolean;
  /** 窗口置顶 */
  alwaysOnTop: boolean;
  /** 鼠标穿透：开启后浮窗不接收鼠标事件，不影响游戏操作 */
  clickThrough: boolean;
  /** 窗口位置（物理像素，拖动浮窗自动回写） */
  x: number;
  y: number;
  fontSize: number;
  /** #RRGGBB */
  fontColor: string;
  bold: boolean;
  /** 深色描边阴影，直播画面上更易读 */
  shadow: boolean;
  /** #RRGGBB */
  bgColor: string;
  /** 背景不透明度 0-100 */
  bgOpacity: number;
  /** 自动换行：开启后长行按 maxWidth 折行（模板里的回车始终换行） */
  wrap: boolean;
  /** 自动换行最大宽度（逻辑像素） */
  maxWidth: number;
  /** 手动尺寸（逻辑像素；0 = 自动按内容，>0 时窗口固定大小） */
  width: number;
  height: number;
  /** 创建时间（毫秒时间戳） */
  createdAt: number;
  /** 最后修改时间（毫秒时间戳；拖动位置不算修改） */
  updatedAt: number;
  /** 本套自己的自定义变量 */
  variables: UserVar[];
  /** 本套自己的计算变量 */
  computed: ComputedVar[];
}

/** 全量配置（Rust 侧 SQLite）：浮窗（套）列表 */
export interface AppConfig {
  overlays: OverlayConfig[];
}

/** 拖动浮窗时 Rust 发出的 overlay-moved 事件负载 */
export interface MovedEvent {
  id: number;
  x: number;
  y: number;
}
