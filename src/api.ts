/** 类型化的 invoke 封装：所有命令都返回最新全量配置（单一数据源），前端直接采用。
 *  每个浮窗是一套独立配置，套内操作都带 overlayId。
 *
 *  为什么要有「配置总线」：
 *  界面若只靠 `config-changed` 广播事件刷新，一旦事件没送达（窗口未就绪、
 *  监听注册竞态等），开关类按钮会显得「点了没反应」——因为它们的值是读服务端
 *  状态的（点一次发 !当前值）。所以这里让**每个返回配置的命令**在 resolve 时
 *  直接 publish 最新配置，本窗口立即刷新；广播只负责其它窗口的同步。
 */
import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  BackupPreview,
  ComputedVar,
  ImportResult,
  LogLevel,
  OverlayConfig,
  QueueConfig,
  QueueStatus,
  SettingsInfo,
  ThemeMode,
  UserVar,
  WebdavConfig,
} from "./types";

type ConfigListener = (c: AppConfig) => void;
const listeners = new Set<ConfigListener>();

/** 订阅“最新配置”：命令返回 / 广播到达时都会触发。返回退订函数。 */
export function subscribeConfig(cb: ConfigListener): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

function publish(c: AppConfig) {
  for (const cb of listeners) {
    try {
      cb(c);
    } catch (e) {
      console.warn("config listener error", e);
    }
  }
}

/** 包装返回全量配置的命令：resolve 后立即发布，同时原样返回。 */
function cfged(p: Promise<AppConfig>): Promise<AppConfig> {
  return p.then((c) => {
    publish(c);
    return c;
  });
}

export const getConfig = () => cfged(invoke<AppConfig>("get_config"));

// ---------- 浮窗（套） ----------

export const addOverlay = () => cfged(invoke<AppConfig>("add_overlay"));

export const updateOverlay = (overlay: OverlayConfig) =>
  cfged(invoke<AppConfig>("update_overlay", { overlay }));

/** 主窗口直接修改浮窗位置 */
export const moveOverlay = (id: number, x: number, y: number) =>
  cfged(invoke<AppConfig>("move_overlay", { id, x, y }));

/** 拖到屏幕外时，把浮窗移回当前显示器中央 */
export const resetOverlayPosition = (id: number) =>
  cfged(invoke<AppConfig>("reset_overlay_position", { id }));

export const setOverlayShown = (id: number, value: boolean) =>
  cfged(invoke<AppConfig>("set_overlay_shown", { id, value }));

export const setOverlayAlwaysTop = (id: number, value: boolean) =>
  cfged(invoke<AppConfig>("set_overlay_always_top", { id, value }));

export const setOverlayClickThrough = (id: number, value: boolean) =>
  cfged(invoke<AppConfig>("set_overlay_click_through", { id, value }));

export const deleteOverlay = (id: number) =>
  cfged(invoke<AppConfig>("delete_overlay", { id }));

/** 浮窗页面内容尺寸自适应后调用（仅浮窗可调） */
export const setOverlaySize = (width: number, height: number) =>
  invoke<void>("set_overlay_size", { width, height });

/** 保存浮窗宽高（逻辑像素）：自动模式下作最小尺寸，固定模式下作实际尺寸；0/0 恢复自动 */
export const resizeOverlay = (id: number, width: number, height: number) =>
  cfged(invoke<AppConfig>("resize_overlay", { id, width, height }));

/** 切换浮窗自动宽高；关闭时 Rust 会用当前窗口尺寸固化宽高 */
export const setOverlayAutoSize = (id: number, auto: boolean) =>
  cfged(invoke<AppConfig>("set_overlay_auto_size", { id, auto }));

// ---------- 套内：自定义变量 / 计算变量 ----------
// （账号只是浮窗的一个字段，走 updateOverlay 整卡保存）

/** key 传 null 让后端自动分配（A1、A2…） */
export const addVariable = (overlayId: number, name: string, key: string | null, value: number, step: number) =>
  cfged(invoke<AppConfig>("add_variable", { overlayId, key, name, value, step }));

export const updateVariable = (overlayId: number, variable: UserVar) =>
  cfged(invoke<AppConfig>("update_variable", { overlayId, variable }));

export const stepVariable = (overlayId: number, id: number, delta: number) =>
  cfged(invoke<AppConfig>("step_variable", { overlayId, id, delta }));

export const deleteVariable = (overlayId: number, id: number) =>
  cfged(invoke<AppConfig>("delete_variable", { overlayId, id }));

/** key 传 null 让后端自动分配（B1、B2…） */
export const addComputed = (
  overlayId: number,
  name: string,
  key: string | null,
  expr: string,
  format: "plain" | "percent",
  decimals: number,
) => cfged(invoke<AppConfig>("add_computed", { overlayId, key, name, expr, format, decimals }));

export const updateComputed = (overlayId: number, computed: ComputedVar) =>
  cfged(invoke<AppConfig>("update_computed", { overlayId, computed }));

export const deleteComputed = (overlayId: number, id: number) =>
  cfged(invoke<AppConfig>("delete_computed", { overlayId, id }));

// ---------- 排队队列 ----------

export const addQueue = () => cfged(invoke<AppConfig>("add_queue"));

/** 保存队列的直播间、正则、排序与弹窗样式（位置/显隐走独立命令） */
export const updateQueue = (queue: QueueConfig) =>
  cfged(invoke<AppConfig>("update_queue", { queue }));

export const setQueueEnabled = (id: number, value: boolean) =>
  cfged(invoke<AppConfig>("set_queue_enabled", { id, value }));

export const setQueueShown = (id: number, value: boolean) =>
  cfged(invoke<AppConfig>("set_queue_shown", { id, value }));

export const setQueueAlwaysTop = (id: number, value: boolean) =>
  cfged(invoke<AppConfig>("set_queue_always_top", { id, value }));

export const setQueueClickThrough = (id: number, value: boolean) =>
  cfged(invoke<AppConfig>("set_queue_click_through", { id, value }));

export const moveQueue = (id: number, x: number, y: number) =>
  cfged(invoke<AppConfig>("move_queue", { id, x, y }));

/** 拖到屏幕外时，把排队弹窗移回当前显示器中央 */
export const resetQueuePosition = (id: number) =>
  cfged(invoke<AppConfig>("reset_queue_position", { id }));

export const deleteQueue = (id: number) =>
  cfged(invoke<AppConfig>("delete_queue", { id }));

/** 排队弹窗量好内容尺寸后调用（仅 queue-* 窗口可调） */
export const setQueueSize = (width: number, height: number) =>
  invoke<void>("set_queue_size", { width, height });

/** 保存排队弹窗宽高：自动模式下作最小尺寸，固定模式下作实际尺寸；0/0 恢复自动 */
export const resizeQueue = (id: number, width: number, height: number) =>
  cfged(invoke<AppConfig>("resize_queue", { id, width, height }));

/** 切换排队弹窗自动宽高；关闭时 Rust 会用当前窗口尺寸固化宽高 */
export const setQueueAutoSize = (id: number, auto: boolean) =>
  cfged(invoke<AppConfig>("set_queue_auto_size", { id, auto }));

export const addQueueEntry = (queueId: number, nickname: string, count = 1, group = "") =>
  cfged(invoke<AppConfig>("add_queue_entry", { queueId, nickname, count, group }));

export const stepQueueEntry = (queueId: number, id: number, delta: number) =>
  cfged(invoke<AppConfig>("step_queue_entry", { queueId, id, delta }));

export const setQueueEntryCount = (queueId: number, id: number, count: number) =>
  cfged(invoke<AppConfig>("set_queue_entry_count", { queueId, id, count }));

export const renameQueueEntry = (queueId: number, id: number, nickname: string) =>
  cfged(invoke<AppConfig>("rename_queue_entry", { queueId, id, nickname }));

/** 修改成员分组；同一 uid + 分组只保留一行（目标分组已有同一成员时次数合并） */
export const setQueueEntryGroup = (queueId: number, id: number, group: string) =>
  cfged(invoke<AppConfig>("set_queue_entry_group", { queueId, id, group }));

export const deleteQueueEntry = (queueId: number, id: number) =>
  cfged(invoke<AppConfig>("delete_queue_entry", { queueId, id }));

export const clearQueueEntries = (queueId: number) =>
  cfged(invoke<AppConfig>("clear_queue_entries", { queueId }));

/** 主窗口定时查询各队列的弹幕连接状态 */
export const getQueueStatus = () => invoke<QueueStatus[]>("get_queue_status");

// ---------- 设置 / 备份同步 ----------

/** 读取应用设置（conf.toml：外观、日志等级、路径） */
export const getSettings = () => invoke<SettingsInfo>("get_settings");

/** 保存外观（auto / light / dark），立即写入 conf.toml */
export const saveTheme = (theme: ThemeMode) =>
  invoke<SettingsInfo>("set_theme", { theme });

/** 保存日志等级，立即生效并写入 conf.toml */
export const saveLogLevel = (level: LogLevel) =>
  invoke<SettingsInfo>("set_log_level", { level });

/** 保存 B 站登录 Cookie（解析 uid 后重启弹幕连接，用登录态拿真实 uid/昵称） */
export const saveBiliCookie = (cookie: string) =>
  invoke<SettingsInfo>("set_bili_cookie", { cookie });

/** 清除 B 站登录 Cookie，恢复游客连接 */
export const clearBiliCookie = () => invoke<SettingsInfo>("clear_bili_cookie");

/** 恢复默认设置（外观 auto + 日志 error；不影响 SQLite 数据和 B 站登录） */
export const resetSettings = () => invoke<SettingsInfo>("reset_settings");

/** 读取 WebDAV 连接信息（conf/auth.toml 的 [webdav] 段，含账号密码，仅本机可见） */
export const getWebdav = () => invoke<WebdavConfig>("get_webdav");

/** 保存 WebDAV 连接信息到 conf/auth.toml 的 [webdav] 段（不进 conf.toml / 备份） */
export const saveWebdav = (dav: WebdavConfig) => invoke<void>("set_webdav", { dav });

/** 打开 exe 同目录的 logs/（排查弹幕连接用），返回日志目录绝对路径 */
export const openLogDir = () => invoke<string>("open_log_dir");

/** 导出到本地路径（includeData=false 时只导出 conf.toml） */
export const exportLocal = (path: string, includeData: boolean) =>
  invoke<string>("export_local", { path, includeData });

/** 导出到 WebDAV（includeData=false 时上传 conf.toml，否则上传 JSON 备份） */
export const exportWebdav = (dav: WebdavConfig, includeData: boolean) =>
  invoke<string>("export_webdav", { dav, includeData });

/** 读取本地备份并暂存，返回摘要；真正导入走 applyBackup */
export const selectLocalBackup = (path: string) =>
  invoke<BackupPreview>("select_local_backup", { path });

/** 从 WebDAV 下载备份并暂存，返回摘要 */
export const selectWebdavBackup = (dav: WebdavConfig, remoteFile: string) =>
  invoke<BackupPreview>("select_webdav_backup", { dav, remoteFile });

/** 应用已暂存的备份：覆盖或增量；applySettings 控制是否导入外观/日志等级 */
export const applyBackup = (mode: "overwrite" | "merge", applySettings: boolean) =>
  invoke<ImportResult>("apply_backup", { mode, applySettings }).then((result) => {
    publish(result.config);
    return result;
  });

// ---------- 杂项 ----------

/** 主窗口置顶开关（主界面右上角按钮，只影响主窗口，不影响浮窗） */
export const setMainAlwaysOnTop = (value: boolean) =>
  invoke<boolean>("set_main_always_on_top", { value });

/** 用系统默认浏览器打开 http(s) 链接（「关于」里的仓库地址） */
export const openUrl = (url: string) => invoke<void>("open_url", { url });
