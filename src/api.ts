/** 类型化的 invoke 封装：所有命令都返回最新全量配置（单一数据源），前端直接采用 */
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, ComputedVar, OverlayConfig, UserVar } from "./types";

export const getConfig = () => invoke<AppConfig>("get_config");

export const setAccount = (account: string) =>
  invoke<AppConfig>("set_account", { account });

// ---------- 自定义变量 ----------

/** key 传 null 让后端自动分配（A1、A2…） */
export const addVariable = (name: string, key: string | null, value: number, step: number) =>
  invoke<AppConfig>("add_variable", { key, name, value, step });

export const updateVariable = (variable: UserVar) =>
  invoke<AppConfig>("update_variable", { variable });

export const stepVariable = (id: number, delta: number) =>
  invoke<AppConfig>("step_variable", { id, delta });

export const deleteVariable = (id: number) =>
  invoke<AppConfig>("delete_variable", { id });

// ---------- 计算变量 ----------

/** key 传 null 让后端自动分配（B1、B2…） */
export const addComputed = (
  name: string,
  key: string | null,
  expr: string,
  format: "plain" | "percent",
  decimals: number,
) => invoke<AppConfig>("add_computed", { key, name, expr, format, decimals });

export const updateComputed = (computed: ComputedVar) =>
  invoke<AppConfig>("update_computed", { computed });

export const deleteComputed = (id: number) =>
  invoke<AppConfig>("delete_computed", { id });

// ---------- 浮窗 ----------

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
