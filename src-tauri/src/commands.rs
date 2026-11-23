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

use crate::danmaku::{self, DanmakuManager, QueueStatus};
use crate::overlay_win::{self, id_from_label};
use crate::queue_win;
use crate::store::{ComputedVar, Config, OverlayConfig, QueueConfig, Store, UserVar};

/// 全量配置变化事件名（Rust → 所有窗口广播）。
pub const CONFIG_CHANGED_EVENT: &str = "config-changed";

fn broadcast(app: &AppHandle, config: &Config) {
    if let Err(e) = app.emit(CONFIG_CHANGED_EVENT, config) {
        log::error!("广播配置失败：{e}");
    }
}

/// 改完数据库 → 重新组装 → 广播。
fn changed(app: &AppHandle, store: &Store) -> Result<Config, String> {
    let config = store.get();
    broadcast(app, &config);
    Ok(config)
}

/// 把弹窗移回当前显示器的可见中央；拿不到显示器信息时退回主屏 1920×1080 估算。
fn centered_position(window: &WebviewWindow) -> (i32, i32) {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let (mx, my, mw, mh) = match monitor {
        Some(m) => (
            m.position().x,
            m.position().y,
            m.size().width as i32,
            m.size().height as i32,
        ),
        None => (0, 0, 1920, 1080),
    };
    let (w, h) = window
        .outer_size()
        .map(|s| (s.width as i32, s.height as i32))
        .unwrap_or((320, 120));
    (mx + (mw - w).max(0) / 2, my + (mh - h).max(0) / 2)
}

/// 浮窗 / 排队弹窗无权做新建删除、编辑名单这类"全局"操作。
fn require_main(window: &WebviewWindow) -> Result<(), String> {
    if id_from_label(window.label()).is_some()
        || queue_win::id_from_label(window.label()).is_some()
    {
        return Err("弹窗无权执行该操作，请在主窗口操作".into());
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

/// 排队弹窗同样只能操作自己的窗口状态和尺寸。
fn check_queue_permission(window: &WebviewWindow, id: i64) -> Result<(), String> {
    if id_from_label(window.label()).is_some() {
        return Err("悬浮字幕浮窗不能操作排队弹窗".into());
    }
    if let Some(caller) = queue_win::id_from_label(window.label()) {
        if caller != id {
            return Err(format!("排队弹窗只能操作自己（{caller}），不能操作 #{id}"));
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
///
/// `#[tauri::command(async)]` 很关键：同步命令会占用主线程，而创建 WebviewWindow
/// 又需要主线程的事件循环。两者相撞时命令永远不返回、新窗口停在 about:blank，
/// 表现就是「删除所有浮窗后再添加没反应」。
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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

/// 重置浮窗位置：拖到屏幕外点不到的情况下，一键移回当前显示器中央。
#[tauri::command(async)]
pub fn reset_overlay_position(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    check_permission(&window, id)?;
    // 用目标浮窗自己所在的显示器和尺寸居中；主窗口调用时不能拿主窗口的尺寸算。
    let (x, y) = app
        .get_webview_window(&overlay_win::overlay_label(id))
        .map(|w| centered_position(&w))
        .unwrap_or_else(|| centered_position(&window));
    store.move_overlay(id, x, y)?;
    log::info!("浮窗 #{id} 位置重置为 ({x}, {y})");
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

    let cfg = store.get().overlays.into_iter().find(|o| o.id == id);
    // 手动尺寸优先：浮窗被拖动过宽高后，内容自适应不再覆盖它
    let manual = cfg
        .as_ref()
        .map(|c| c.width > 0 && c.height > 0)
        .unwrap_or(false);
    let mut w = if manual {
        cfg.as_ref().map(|c| c.width as f64).unwrap_or(40.0)
    } else {
        width.round().max(40.0)
    };
    let h = if manual {
        cfg.as_ref().map(|c| c.height as f64).unwrap_or(28.0)
    } else {
        height.round().max(28.0)
    };
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
    if let Some(cfg) = cfg {
        overlay_win::apply_window_state(&app, &win, &cfg, false)?;
    }
    Ok(())
}

/// 保存浮窗的手动宽高（逻辑像素）。浮窗拖动边缘后回写；主窗口传 0/0 可恢复自动。
#[tauri::command(async)]
pub fn resize_overlay(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    width: i64,
    height: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    check_permission(&window, id)?;
    // 0/0 = 自动按内容；只要有一个为 0 也按自动处理，避免出现半手动状态
    let (w, h) = if width > 0 && height > 0 {
        (width.max(40), height.max(28))
    } else {
        (0, 0)
    };
    store.set_size(id, w, h)?;

    if let Some(win) = app.get_webview_window(&overlay_win::overlay_label(id)) {
        if w > 0 && h > 0 {
            win.set_size(tauri::LogicalSize::new(w as f64, h as f64))
                .map_err(|e| format!("调整浮窗尺寸失败：{e}"))?;
            overlay_win::mark_measured(&app, id);
        }
        // 恢复自动时不主动改大小：浮窗页面收到 config-changed 后会重新量内容。
        if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == id) {
            overlay_win::apply_window_state(&app, &win, &cfg, false)?;
        }
    }
    changed(&app, &store)
}

// 下面三个开关命令独立出来：浮窗自身的悬停小工具条和主窗口的开关都只改一个布尔值。

#[tauri::command(async)]
pub fn set_overlay_shown(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_flag_command(&app, &window, &store, id, |o| o.shown = value)
}

#[tauri::command(async)]
pub fn set_overlay_always_top(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_flag_command(&app, &window, &store, id, |o| o.always_on_top = value)
}

#[tauri::command(async)]
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
#[tauri::command(async)]
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
// （账号只是浮窗的一个字段，update_overlay / store.update_account 已覆盖，
//  不再单独提供命令）

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

// ---------- 排队队列 ----------

/// 新建排队队列：默认规则「排队」、3 秒刷新、弹窗位于屏幕右侧。
/// 与 add_overlay 一样必须是 async 命令：里面要创建 WebviewWindow。
#[tauri::command(async)]
pub fn add_queue(
    app: AppHandle,
    window: WebviewWindow,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    let id = store.next_queue_id();
    let index = store.get().queues.len();
    let (x, y) = queue_win::default_queue_position(&app, index);
    let mut cfg = crate::store::default_queue(id, crate::store::now_ms());
    if index > 0 {
        cfg.name = format!("排队队列 {}", index + 1);
    }
    cfg.x = x;
    cfg.y = y;

    store.push_queue(&cfg)?;
    if let Some(cfg) = store.get().queues.into_iter().find(|q| q.id == id) {
        queue_win::sync_queue_window(&app, &cfg, true)?;
    }
    danmaku::reconcile(&app);
    changed(&app, &store)
}

/// 更新队列的直播间、轮询间隔、正则、排序与弹窗样式。
#[tauri::command(async)]
pub fn update_queue(
    app: AppHandle,
    window: WebviewWindow,
    queue: QueueConfig,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    let id = queue.id;
    store.update_queue(&queue)?;
    if let Some(cfg) = store.get().queues.into_iter().find(|q| q.id == id) {
        queue_win::sync_queue_window(&app, &cfg, false)?;
    }
    danmaku::reconcile(&app);
    changed(&app, &store)
}

#[tauri::command(async)]
pub fn set_queue_enabled(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.set_queue_enabled(id, value)?;
    danmaku::reconcile(&app);
    changed(&app, &store)
}

#[tauri::command(async)]
pub fn set_queue_shown(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_queue_flag_command(&app, &window, &store, id, |q| q.shown = value)
}

#[tauri::command(async)]
pub fn set_queue_always_top(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_queue_flag_command(&app, &window, &store, id, |q| q.always_on_top = value)
}

#[tauri::command(async)]
pub fn set_queue_click_through(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    value: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    set_queue_flag_command(&app, &window, &store, id, |q| q.click_through = value)
}

fn set_queue_flag_command(
    app: &AppHandle,
    window: &WebviewWindow,
    store: &State<'_, Arc<Store>>,
    id: i64,
    apply: impl FnOnce(&mut QueueConfig),
) -> Result<Config, String> {
    check_queue_permission(window, id)?;
    store.set_queue_flag(id, apply)?;
    sync_queue_and_changed(app, store, id, false)
}

#[tauri::command(async)]
pub fn move_queue(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    x: i32,
    y: i32,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    check_queue_permission(&window, id)?;
    store.move_queue(id, x, y)?;
    sync_queue_and_changed(&app, &store, id, true)
}

/// 重置排队弹窗位置：一键移回当前显示器中央。
#[tauri::command(async)]
pub fn reset_queue_position(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    check_queue_permission(&window, id)?;
    // 同上：按目标弹窗自己的显示器和尺寸居中。
    let (x, y) = app
        .get_webview_window(&queue_win::queue_label(id))
        .map(|w| centered_position(&w))
        .unwrap_or_else(|| centered_position(&window));
    store.move_queue(id, x, y)?;
    log::info!("排队弹窗 #{id} 位置重置为 ({x}, {y})");
    sync_queue_and_changed(&app, &store, id, true)
}

/// 队列弹窗量好内容尺寸后调用（只有 queue-* 窗口可以调）。
#[tauri::command]
pub fn set_queue_size(
    app: AppHandle,
    window: WebviewWindow,
    width: f64,
    height: f64,
    store: State<'_, Arc<Store>>,
) -> Result<(), String> {
    let id = queue_win::id_from_label(window.label())
        .ok_or("只有排队弹窗可以调用 set_queue_size")?;
    let win = app
        .get_webview_window(&queue_win::queue_label(id))
        .ok_or("排队弹窗窗口不存在")?;

    let cfg = store.get().queues.into_iter().find(|q| q.id == id);
    let manual = cfg
        .as_ref()
        .map(|c| c.width > 0 && c.height > 0)
        .unwrap_or(false);
    let mut w = if manual {
        cfg.as_ref().map(|c| c.width as f64).unwrap_or(60.0)
    } else {
        width.round().max(60.0)
    };
    let h = if manual {
        cfg.as_ref().map(|c| c.height as f64).unwrap_or(40.0)
    } else {
        height.round().max(40.0)
    };
    if let Ok(Some(monitor)) = win.current_monitor() {
        let scale = monitor.scale_factor();
        let max_logical = monitor.size().width as f64 / scale;
        if max_logical > 0.0 {
            w = w.min(max_logical);
        }
    }

    win.set_size(tauri::LogicalSize::new(w, h))
        .map_err(|e| format!("调整排队弹窗尺寸失败：{e}"))?;
    queue_win::mark_measured(&app, id);
    if let Some(cfg) = cfg {
        queue_win::apply_queue_window_state(&app, &win, &cfg, false)?;
    }
    Ok(())
}

/// 保存队列弹窗的手动宽高；主窗口传 0/0 恢复自动按内容。
#[tauri::command(async)]
pub fn resize_queue(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    width: i64,
    height: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    check_queue_permission(&window, id)?;
    let (w, h) = if width > 0 && height > 0 {
        (width.max(60), height.max(40))
    } else {
        (0, 0)
    };
    store.set_queue_size(id, w, h)?;
    if let Some(win) = app.get_webview_window(&queue_win::queue_label(id)) {
        if w > 0 && h > 0 {
            win.set_size(tauri::LogicalSize::new(w as f64, h as f64))
                .map_err(|e| format!("调整排队弹窗尺寸失败：{e}"))?;
            queue_win::mark_measured(&app, id);
        }
        if let Some(cfg) = store.get().queues.into_iter().find(|q| q.id == id) {
            queue_win::apply_queue_window_state(&app, &win, &cfg, false)?;
        }
    }
    changed(&app, &store)
}

#[tauri::command(async)]
pub fn delete_queue(
    app: AppHandle,
    window: WebviewWindow,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.delete_queue(id)?;
    if let Some(w) = app.get_webview_window(&queue_win::queue_label(id)) {
        let _ = w.close();
    }
    danmaku::reconcile(&app);
    changed(&app, &store)
}

#[tauri::command]
pub fn add_queue_entry(
    app: AppHandle,
    window: WebviewWindow,
    queue_id: i64,
    nickname: String,
    count: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.add_queue_entry(queue_id, &nickname, count)?;
    changed(&app, &store)
}

/// 原子 +1 / -1，和弹幕实时累加不会互相覆盖。
#[tauri::command]
pub fn step_queue_entry(
    app: AppHandle,
    window: WebviewWindow,
    queue_id: i64,
    id: i64,
    delta: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.step_queue_entry(queue_id, id, delta)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn set_queue_entry_count(
    app: AppHandle,
    window: WebviewWindow,
    queue_id: i64,
    id: i64,
    count: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.set_queue_entry_count(queue_id, id, count)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn rename_queue_entry(
    app: AppHandle,
    window: WebviewWindow,
    queue_id: i64,
    id: i64,
    nickname: String,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.rename_queue_entry(queue_id, id, &nickname)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn delete_queue_entry(
    app: AppHandle,
    window: WebviewWindow,
    queue_id: i64,
    id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.delete_queue_entry(queue_id, id)?;
    changed(&app, &store)
}

#[tauri::command]
pub fn clear_queue_entries(
    app: AppHandle,
    window: WebviewWindow,
    queue_id: i64,
    store: State<'_, Arc<Store>>,
) -> Result<Config, String> {
    require_main(&window)?;
    store.clear_queue_entries(queue_id)?;
    changed(&app, &store)
}

/// 主窗口定时拉取的连接状态（只在内存，不写数据库）。
#[tauri::command]
pub fn get_queue_status(manager: State<'_, DanmakuManager>) -> Vec<QueueStatus> {
    manager.statuses()
}

fn sync_queue_and_changed(
    app: &AppHandle,
    store: &State<'_, Arc<Store>>,
    queue_id: i64,
    apply_position: bool,
) -> Result<Config, String> {
    if let Some(cfg) = store.get().queues.into_iter().find(|q| q.id == queue_id) {
        queue_win::sync_queue_window(app, &cfg, apply_position)?;
    }
    changed(app, store)
}

/// 设置主窗口是否置顶（主界面右上角的「窗口置顶」按钮）。
/// 只允许主窗口调用，浮窗的置顶仍走 set_overlay_always_top。
#[tauri::command]
pub fn set_main_always_on_top(window: WebviewWindow, value: bool) -> Result<bool, String> {
    if window.label() != "main" {
        return Err("只有主窗口可以设置窗口置顶".into());
    }
    window
        .set_always_on_top(value)
        .map_err(|e| format!("设置主窗口置顶失败：{e}"))?;
    Ok(value)
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
