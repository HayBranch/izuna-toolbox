/** 类型化的 invoke 封装：所有命令都返回最新全量配置（单一数据源），前端直接采用。
 *  每个浮窗是一套独立配置，套内操作都带 overlayId。 */
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, ComputedVar, OverlayConfig, UserVar } from "./types";

export const getConfig = () => invoke<AppConfig>("get_config");

// ---------- 浮窗（套） ----------

export const addOverlay = () => invoke<AppConfig>("add_overlay");

export const updateOverlay = (overlay: OverlayConfig) =>
  invoke<AppConfig>("update_overlay", { overlay });

/** 主窗口直接修改浮窗位置（Rust 忽略 update_overlay 里的坐标） */
export const moveOverlay = (id: number, x: number, y: number) =>
  invoke<AppConfig>("move_overlay", { id, x, y });

export const setOverlayShown = (id: number, value: boolean) =>
  invoke<AppConfig>("set_overlay_shown", { id, value });

export const setOverlayAlwaysTop = (id: number, value: boolean) =>
  invoke<AppConfig>("set_overlay_always_top", { id, value });

export const setOverlayClickThrough = (id: number, value: boolean) =>
  invoke<AppConfig>("set_overlay_click_through", { id, value });

export const deleteOverlay = (id: number) =>
  invoke<AppConfig>("delete_overlay", { id });

/** 浮窗页面内容尺寸自适应后调用（仅浮窗可调） */
export const setOverlaySize = (width: number, height: number) =>
  invoke<void>("set_overlay_size", { width, height });

// ---------- 套内：账号 / 自定义变量 / 计算变量 ----------

export const setAccount = (overlayId: number, account: string) =>
  invoke<AppConfig>("set_account", { overlayId, account });

/** key 传 null 让后端自动分配（A1、A2…） */
export const addVariable = (overlayId: number, name: string, key: string | null, value: number, step: number) =>
  invoke<AppConfig>("add_variable", { overlayId, key, name, value, step });

export const updateVariable = (overlayId: number, variable: UserVar) =>
  invoke<AppConfig>("update_variable", { overlayId, variable });

export const stepVariable = (overlayId: number, id: number, delta: number) =>
  invoke<AppConfig>("step_variable", { overlayId, id, delta });

export const deleteVariable = (overlayId: number, id: number) =>
  invoke<AppConfig>("delete_variable", { overlayId, id });

/** key 传 null 让后端自动分配（B1、B2…） */
export const addComputed = (
  overlayId: number,
  name: string,
  key: string | null,
  expr: string,
  format: "plain" | "percent",
  decimals: number,
) => invoke<AppConfig>("add_computed", { overlayId, key, name, expr, format, decimals });

export const updateComputed = (overlayId: number, computed: ComputedVar) =>
  invoke<AppConfig>("update_computed", { overlayId, computed });

export const deleteComputed = (overlayId: number, id: number) =>
  invoke<AppConfig>("delete_computed", { overlayId, id });
