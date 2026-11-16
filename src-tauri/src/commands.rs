//! 所有 `#[tauri::command]`：主窗口和浮窗页面通过 `invoke` 调用。
//!
//! 统一模式：改 Store（SQLite）→ `emit("config-changed")` 广播全量配置，
//! 所有窗口（主窗口 + 各浮窗）监听同一事件保持一致，前端不维护第二份真相。
//!
//! 权限收敛：凡是从浮窗发起的调用，只允许操作浮窗自己（id 与调用窗口一致），
//! 且浮窗无权改动变量/账号等数据。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::overlay_win::{self, id_from_label};
use crate::store::{ComputedVar, Config, OverlayConfig, Store, UserVar};

/// 全量配置变化事件名（Rust → 所有窗口广播）。
pub const CONFIG_CHANGED_EVENT: &str = "config-changed";

fn broadcast(app: &AppHandle, config: &Config) {
    if let Err(e) = app.emit(CONFIG_CHANGED_EVENT, config) {
        eprintln!("广播配置失败：{e}");
    }
}

/// 改完数据库 → 重新组装 → 广播。
fn changed(app: &AppHandle, store: &Store) -> Result<Config, String> {
    let config = store.get();
    broadcast(app, &config);
    Ok(config)
}

/// 浮窗页面无权改数据（只能改自己的显示状态）。
fn require_main(window: &WebviewWindow) -> Result<(), String> {
    if id_from_label(window.label()).is_some() {
        return Err("浮窗无权修改数据，请在主窗口操作".into());
    }
    Ok(())
}

/// 调用者如果是浮窗，则强制只能操作自己；主窗口可以操作任意浮窗。
fn check_permission(window: &WebviewWindow, id: i64) -> Result<(), String> {
    if let Some(caller) = id_from_label(window.label()) {
        if caller != id {
            return Err(format!("浮窗只能操作自己（{caller}），不能操作 #{id}"));
        }
    }
    Ok(())
}

// ---------- 配置 / 账号 ----------

#[tauri::command]
pub fn get_config(store: State<'_, Arc<Store>>) -> Config {
    store.get()
}

#[tauri::command]
pub fn set_account(
    app: AppHandle,
    window: WebviewWindow,
    account: String,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.set_account(&account)?;
    changed(&app, &store)
}

// ---------- 自定义变量 ----------

#[tauri::command]
pub fn add_variable(
    app: AppHandle,
    window: WebviewWindow,
    key: Option<String>,
    name: String,
    value: f64,
    step: f64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.add_variable(key, &name, value, step)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn update_variable(
    app: AppHandle,
    window: WebviewWindow,
    variable: UserVar,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.update_variable(&variable)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn step_variable(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    delta: f64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.step_variable(id, delta)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn delete_variable(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.delete_variable(id)?;
    changed(&app, &store)
}

// ---------- 计算变量 ----------

#[tauri::command]
pub fn add_computed(
    app: AppHandle,
    window: WebviewWindow,
    key: Option<String>,
    name: String,
    expr: String,
    format: String,
    decimals: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.add_computed(key, &name, &expr, &format, decimals)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn update_computed(
    app: AppHandle,
    window: WebviewWindow,
    computed: ComputedVar,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.update_computed(&computed)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn delete_computed(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.delete_computed(id)?;
    changed(&app, &store)
}

// ---------- 浮窗 ----------

/// 新建浮窗：生成配置（默认位置 = 主显示器顶部居中）→ 落库广播 → 创建窗口。
#[tauri::command]
pub fn add_overlay(
    app: AppHandle,
    window: WebviewWindow,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;

    let id = store.next_overlay_id();
    let (x, y) = overlay_win::default_top_center(&app);
    let mut cfg = crate::store::default_overlay(id, crate::store::now_ms());
    cfg.x = x;
    cfg.y = y;

    store.push_overlay(&cfg)?;
    overlay_win::sync_overlay_window(&app, &cfg, true)?;
    changed(&app, &store)
}

/// 更新浮窗的名称/模板/样式（不动位置；位置走 move_overlay 或拖动回写）。
#[tauri::command]
pub fn update_overlay(
    app: AppHandle,
    window: WebviewWindow,
    overlay: OverlayConfig,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    check_permission(&window, overlay.id)?;
    let overlay_id = overlay.id;
    store.update_overlay(&overlay)?;
    if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == overlay_id) {
        overlay_win::sync_overlay_window(&app, &cfg, false)?;
    }
    changed(&app, &store)
}

/// 主窗口直接修改浮窗位置（X/Y 输入框），立即生效。
#[tauri::command]
pub fn move_overlay(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    x: i32,
    y: i32,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    check_permission(&window, id)?;
    store.move_overlay(id, x, y)?;
    if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == id) {
        overlay_win::sync_overlay_window(&app, &cfg, true)?;
    }
    changed(&app, &store)
}

// 下面三个开关命令独立出来：浮窗自身的悬停小工具条和主窗口的开关都只改一个布尔值。

#[tauri::command]
pub fn set_overlay_shown(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_flag_command(&app, &window, &store, id, |o| o.shown = value)
}

#[tauri::command]
pub fn set_overlay_always_top(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_flag_command(&app, &window, &store, id, |o| o.always_on_top = value)
}

#[tauri::command]
pub fn set_overlay_click_through(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_flag_command(&app, &window, &store, id, |o| o.click_through = value)
}

fn set_flag_command(
    app: &AppHandle,
    window: &WebviewWindow,
    store: &State<'_, Arc<Store>>,
    id: i64,
    apply: impl FnOnce(&mut OverlayConfig),
) -> Result<Config, String> {
    check_permission(window, id)?;
    store.set_flag(id, apply)?;
    if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == id) {
        overlay_win::sync_overlay_window(app, &cfg, false)?;
    }
    changed(app, store)
}

#[tauri::command]
pub fn delete_overlay(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.delete_overlay(id)?;
    if let Some(w) = app.get_webview_window(&overlay_win::overlay_label(id)) {
        let _ = w.close();
    }
    changed(&app, &store)
}

/// 浮窗页面内容自适应尺寸后调用：把窗口调成内容的逻辑尺寸。
/// 首次量到尺寸且配置为"显示"时才真正 show，避免默认大小闪一下。
#[tauri::command]
pub fn set_overlay_size(
    app: AppHandle,
    window: WebviewWindow,
    width: f64,
    height: f64,
    store: State<'_, Arc<Store>>,
) -> Result<(), String> {
    let id = id_from_label(window.label()).ok_or("只有浮窗可以调用 set_overlay_size")?;
    let win = app
        .get_webview_window(&overlay_win::overlay_label(id))
        .ok_or("浮窗窗口不存在")?;

    let mut w = width.round().max(40.0);
    let h = height.round().max(28.0);
    // 不允许浮窗比所在显示器还宽
    if let Ok(Some(monitor)) = win.current_monitor() {
        let scale = monitor.scale_factor();
        let max_logical = monitor.size().width as f64 / scale;
        if max_logical > 0.0 {
            w = w.min(max_logical);
        }
    }

    win.set_size(tauri::LogicalSize::new(w, h))
        .map_err(|e| format!("调整浮窗尺寸失败：{e}"))?;

    // 首次显示逻辑：窗口创建时保持隐藏，这里量好尺寸了才亮相
    if let Some(cfg) = store
        .get()
        .overlays
        .into_iter()
        .find(|o| o.id == id)
    {
        if cfg.shown && !win.is_visible().unwrap_or(true) {
            let _ = win.show();
        }
    }
    Ok(())
}
