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
import type { AppConfig, ComputedVar, OverlayConfig, UserVar } from "./types";

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

// ---------- 杂项 ----------

/** 主窗口置顶开关（主界面右上角按钮，只影响主窗口，不影响浮窗） */
export const setMainAlwaysOnTop = (value: boolean) =>
  invoke<boolean>("set_main_always_on_top", { value });

/** 用系统默认浏览器打开 http(s) 链接（「关于」里的仓库地址） */
export const openUrl = (url: string) => invoke<void>("open_url", { url });
