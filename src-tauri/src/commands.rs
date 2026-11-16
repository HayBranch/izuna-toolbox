//! 所有 `#[tauri::command]`：主窗口和浮窗页面通过 `invoke` 调用。
//!
//! 统一模式：改 Store（SQLite）→ `emit("config-changed")` 广播全量配置，
//! 所有窗口（主窗口 + 各浮窗）监听同一事件保持一致，前端不维护第二份真相。
//!
//! 数据模型：每个浮窗 = 一套独立配置（自己的账号/变量/计算变量/模板）。
//! 凡是操作"套"内数据的命令都带 `overlay_id`；从浮窗发起的调用只允许
//! 操作浮窗自己（id 与调用窗口一致），且浮窗无权改动删除类/新建类操作。

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

/// 浮窗页面无权做新建/删除这类"全局"操作。
fn require_main(window: &WebviewWindow) -> Result<(), String> {
    if id_from_label(window.label()).is_some() {
        return Err("浮窗无权执行该操作，请在主窗口操作".into());
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

// ---------- 配置 ----------

#[tauri::command]
pub fn get_config(store: State<'_, Arc<Store>>) -> Config {
    store.get()
}

// ---------- 浮窗（套） ----------

/// 新建浮窗：生成配置（默认位置 = 主显示器顶部居中）→ 落库 → 种入示例变量 → 建窗广播。
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
    // 新套自带示例变量（A1-A4 + B1-B4，初始为 0），否则模板里的
    // {A1}/{B1} 会以原文显示，看起来像坏了
    store.seed_example_variables(id, false)?;
    if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == id) {
        overlay_win::sync_overlay_window(&app, &cfg, true)?;
    }
    changed(&app, &store)
}

/// 更新浮窗的名称/账号/模板/样式（不动位置；位置走 move_overlay 或拖动回写）。
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
    sync_and_changed(&app, &store, overlay_id, false)
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
    // 必须应用位置（true）——这里曾经误用 false，X/Y 输入框点了不生效
    sync_and_changed(&app, &store, id, true)
}

/// 浮窗页面内容自适应尺寸后调用：把窗口调成内容的逻辑尺寸。
/// 首次量到尺寸且配置为"显示"时才真正 show —— 浮窗在量好尺寸前绝不可见，
/// 否则一块透明窗口会挡住底下所有窗口的鼠标点击。
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
    overlay_win::mark_measured(&app, id);

    // 量好尺寸后走统一的窗口状态同步（含置顶策略）再决定显隐：
    // 不能直接 win.show()——那会让浮窗跳过"主窗口在前台时取消置顶"的判断，
    // 一显示就可能以置顶态盖住主窗口、拦截点击。
    if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == id) {
        overlay_win::apply_window_state(&app, &win, &cfg, false)?;
    }
    Ok(())
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
    sync_and_changed(app, store, id, false)
}

/// 删除浮窗：连同它那套变量一起删；窗口真正关闭
/// （CloseRequested 只拦截"仍存在于数据库"的浮窗，见 lib.rs）。
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

// ---------- 套内变量 ----------

#[tauri::command]
pub fn set_account(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    account: String,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    if !store.overlay_exists(overlay_id) {
        return Err(format!("浮窗 #{overlay_id} 不存在"));
    }
    // 账号只是浮窗的一个字段，走 update_overlay 的字段级更新
    store.update_account(overlay_id, &account)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn add_variable(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    key: Option<String>,
    name: String,
    value: f64,
    step: f64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.add_variable(overlay_id, key, &name, value, step)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn update_variable(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    variable: UserVar,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.update_variable(overlay_id, &variable)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn step_variable(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    id: i64,
    delta: f64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.step_variable(overlay_id, id, delta)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn delete_variable(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.delete_variable(overlay_id, id)?;
    changed(&app, &store)
}

// ---------- 套内计算变量 ----------

#[tauri::command]
pub fn add_computed(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    key: Option<String>,
    name: String,
    expr: String,
    format: String,
    decimals: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.add_computed(overlay_id, key, &name, &expr, &format, decimals)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn update_computed(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    computed: ComputedVar,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.update_computed(overlay_id, &computed)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn delete_computed(
    app: AppHandle,
    window: WebviewWindow,
    overlay_id: i64,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.delete_computed(overlay_id, id)?;
    changed(&app, &store)
}

/// 同步某个浮窗的窗口状态并广播全量配置。
fn sync_and_changed(
    app: &AppHandle,
    store: &State<'_, Arc<Store>>,
    overlay_id: i64,
    apply_position: bool,
) -> Result<Config, String> {
    if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == overlay_id) {
        overlay_win::sync_overlay_window(app, &cfg, apply_position)?;
    }
    changed(app, store)
}

/// 用系统默认浏览器打开 http(s) 链接（供「关于」里的仓库地址点击）。
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("只允许打开 http(s) 链接".into());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("打开链接失败：{e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("当前平台暂不支持直接打开链接".into())
    }
}
