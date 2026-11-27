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

/** 排队队列里的一个成员：同一 uid 累计发送次数 */
export interface QueueEntry {
  id: number;
  /** B 站 uid：>0 真实 UID；<0 游客弹幕按昵称生成的虚拟 UID；0 主窗口手动添加 */
  uid: number;
  nickname: string;
  /** 匹配到的弹幕条数 */
  count: number;
  /** 首次排队时间（毫秒时间戳） */
  firstAt: number;
  /** 最近一次排队时间（毫秒时间戳） */
  lastAt: number;
  /** 最近一次命中规则所属的分组（空 = 未分组） */
  group: string;
}

/** 一条匹配规则：正则 + 分组名 + 分组颜色 */
export interface QueueRule {
  pattern: string;
  /** 命中后给成员打的组名（空 = 不分组） */
  group: string;
  /** #RRGGBB，同组共用 */
  color: string;
}

/** 新人入队特效 */
export type QueueEnterEffect = "none" | "fade" | "slide" | "zoom" | "pop";
/** 数字 +1 特效 */
export type QueueCountEffect = "none" | "bump" | "zoom" | "flash" | "glow" | "rise";

/** 一个排队队列：独立的直播间、规则、排序与弹窗样式 */
export interface QueueConfig {
  id: number;
  name: string;
  /** 直播间号（支持短号；0 = 未设置） */
  roomId: number;
  /** 弹幕批量写入 / 界面刷新间隔（秒） */
  pollSecs: number;
  /** 匹配弹幕文本的规则（任一命中即计数） */
  rules: QueueRule[];
  /** time 按首次发送时间 | count 按发送次数 */
  sortMode: "time" | "count";
  /** 是否监听弹幕 */
  enabled: boolean;
  /** 弹窗最多显示多少人 */
  maxEntries: number;
  /** 弹窗里是否在昵称后显示分组色块 */
  showGroup: boolean;
  /** 弹窗里是否对昵称做脱敏 */
  maskNickname: boolean;
  /** 新人入队特效 */
  enterEffect: QueueEnterEffect;
  /** 数字 +1 特效 */
  countEffect: QueueCountEffect;
  shown: boolean;
  alwaysOnTop: boolean;
  clickThrough: boolean;
  x: number;
  y: number;
  fontSize: number;
  fontColor: string;
  bold: boolean;
  shadow: boolean;
  bgColor: string;
  bgOpacity: number;
  /** 自动宽度最大宽度（逻辑像素） */
  maxWidth: number;
  /** 手动尺寸（0 = 自动按内容） */
  width: number;
  height: number;
  createdAt: number;
  updatedAt: number;
  entries: QueueEntry[];
}

/** 弹幕连接状态（Rust 内存，不落库） */
export interface QueueStatus {
  queueId: number;
  connected: boolean;
  realRoomId: number;
  lastMessageAt: number;
  lastError: string;
  updatedAt: number;
}

/** 全量配置（Rust 侧 SQLite）：浮窗（套）列表 + 排队队列列表 */
export interface AppConfig {
  overlays: OverlayConfig[];
  queues: QueueConfig[];
}

/** 界面外观（保存在 conf/conf.toml，不再使用 localStorage） */
export type ThemeMode = "auto" | "light" | "dark";

/** 日志等级（conf/conf.toml → log_level；默认 error，基本不打印日志） */
export type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";

/** 应用设置（Rust 侧 conf.toml，设置页读写） */
export interface SettingsInfo {
  portable: boolean;
  theme: ThemeMode;
  logLevel: LogLevel;
  /** conf/conf.toml 的完整路径 */
  confPath: string;
  /** 日志目录（exe 同目录 logs/） */
  logDir: string;
  /** B 站登录 uid（来自 DedeUserID，缺失时为 0） */
  biliUid: number;
  /** 是否已配置登录态（存在 SESSDATA）。判断登录状态用这个字段 */
  biliLoggedIn: boolean;
}

/** WebDAV 连接信息（保存在本机 conf/auth.toml 的 [webdav] 段，不写 conf.toml、不进备份） */
export interface WebdavConfig {
  baseUrl: string;
  username: string;
  password: string;
  remoteDir: string;
  /** 导入时下载的远程文件名 */
  remoteFile: string;
}

/** 导入前预览：只给摘要，不把整份名单传回前端 */
export interface BackupPreview {
  source: string;
  exportedAt: number;
  appVersion: string;
  hasSettings: boolean;
  theme: string | null;
  logLevel: string | null;
  hasData: boolean;
  overlayCount: number;
  queueCount: number;
  entryCount: number;
}

/** 增量导入统计 */
export interface ImportStats {
  overlaysAdded: number;
  overlaysSkipped: number;
  queuesAdded: number;
  queuesMerged: number;
  entriesAdded: number;
  entriesMerged: number;
}

/** 导入结果：最新设置 + 最新配置 + 统计 */
export interface ImportResult {
  settings: SettingsInfo;
  config: AppConfig;
  stats: ImportStats;
  mode: "overwrite" | "merge";
}

/** 拖动浮窗 / 排队弹窗时 Rust 发出的事件负载 */
export interface MovedEvent {
  id: number;
  x: number;
  y: number;
}
