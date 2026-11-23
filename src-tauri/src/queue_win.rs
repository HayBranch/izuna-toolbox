//! 排队队列弹窗的窗口生命周期：创建、同步状态。
//!
//! 复用与悬浮字幕浮窗相同的窗口框架（无边框、透明、置顶、鼠标穿透、不抢焦点、
//! 可被 OBS 窗口捕获），只是内容由前端 `QueueWindow` 渲染。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::store::{QueueConfig, Store};

/// 已量好内容尺寸的队列弹窗 id 集合（显示门槛，同 overlay_win）。
pub struct QueueMeasuredSet(pub Mutex<HashSet<i64>>);

/// 队列弹窗窗口 label：`queue-{id}`，与 capabilities 里的 `queue-*` 对应。
pub const LABEL_PREFIX: &str = "queue-";

pub fn queue_label(id: i64) -> String {
    format!("{LABEL_PREFIX}{id}")
}

pub fn id_from_label(label: &str) -> Option<i64> {
    label.strip_prefix(LABEL_PREFIX)?.parse().ok()
}

/// `queue-moved` 事件负载（拖动弹窗后通知主窗口刷新 X/Y 输入框）。
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct QueueMovedEvent {
    pub id: i64,
    pub x: i32,
    pub y: i32,
}

/// 默认落点：主显示器右侧偏上，多个队列按索引错开，避免完全重叠。
pub fn default_queue_position(app: &AppHandle, index: usize) -> (i32, i32) {
    match app.primary_monitor() {
        Ok(Some(monitor)) => {
            let width = monitor.size().width as i32;
            let height = monitor.size().height as i32;
            let step = (index % 6) as i32;
            let x = (width - 400 - step * 18).max(0);
            let y = (100 + step * 70).min((height - 120).max(0));
            (x, y)
        }
        _ => (40, 120),
    }
}

/// 创建队列弹窗（若不存在）。保持隐藏，等页面量好尺寸后再显示。
pub fn create_queue_window(app: &AppHandle, cfg: &QueueConfig) -> Result<(), String> {
    let label = queue_label(cfg.id);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }

    let manual = cfg.width > 0 && cfg.height > 0;
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title(format!("泉奈排队 · {}", cfg.name))
        .visible(false)
        .inner_size(
            if manual { cfg.width as f64 } else { 240.0 },
            if manual { cfg.height as f64 } else { 120.0 },
        )
        .min_inner_size(60.0, 40.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(true)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .focused(false)
        .build()
        .map_err(|e| format!("创建排队弹窗失败：{e}"))?;

    apply_queue_window_state(app, &window, cfg, true)
}

pub fn mark_measured(app: &AppHandle, id: i64) {
    if let Some(set) = app.try_state::<QueueMeasuredSet>() {
        if let Ok(mut m) = set.0.lock() {
            m.insert(id);
        }
    }
}

fn is_measured(app: &AppHandle, id: i64) -> bool {
    app.try_state::<QueueMeasuredSet>()
        .map(|s| s.0.lock().map(|m| m.contains(&id)).unwrap_or(false))
        .unwrap_or(false)
}

/// 把配置同步到队列弹窗（置顶 / 穿透 / 位置 / 显隐）。
pub fn apply_queue_window_state(
    app: &AppHandle,
    window: &WebviewWindow,
    cfg: &QueueConfig,
    apply_position: bool,
) -> Result<(), String> {
    let main_focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    window
        .set_always_on_top(cfg.always_on_top && !main_focused)
        .map_err(warn("设置排队弹窗置顶失败"))?;
    window
        .set_ignore_cursor_events(cfg.click_through)
        .map_err(warn("设置排队弹窗鼠标穿透失败"))?;
    if apply_position {
        window
            .set_position(PhysicalPosition::new(cfg.x, cfg.y))
            .map_err(warn("设置排队弹窗位置失败"))?;
    }
    if cfg.shown {
        if is_measured(app, cfg.id) || window.is_visible().unwrap_or(false) {
            window.show().map_err(warn("显示排队弹窗失败"))?;
        }
    } else {
        window.hide().map_err(warn("隐藏排队弹窗失败"))?;
    }
    Ok(())
}

/// 保证队列弹窗存在并按配置同步状态；返回窗口句柄。
pub fn sync_queue_window(
    app: &AppHandle,
    cfg: &QueueConfig,
    apply_position: bool,
) -> Result<WebviewWindow, String> {
    let label = queue_label(cfg.id);
    if let Some(window) = app.get_webview_window(&label) {
        apply_queue_window_state(app, &window, cfg, apply_position)?;
        Ok(window)
    } else {
        create_queue_window(app, cfg)?;
        app.get_webview_window(&label)
            .ok_or_else(|| "创建排队弹窗后仍找不到窗口".into())
    }
}

/// 主窗口显隐/焦点变化后，统一刷新全部队列弹窗的层级与穿透状态。
pub fn sync_all_queue_windows(app: &AppHandle) {
    let store = app.state::<Arc<Store>>();
    for cfg in store.get().queues {
        if let Some(window) = app.get_webview_window(&queue_label(cfg.id)) {
            if let Err(e) = apply_queue_window_state(app, &window, &cfg, false) {
                eprintln!("同步排队弹窗 #{} 失败：{e}", cfg.id);
            }
        }
    }
}

fn warn<E: std::fmt::Display>(msg: &'static str) -> impl Fn(E) -> String {
    move |e| format!("{msg}：{e}")
}
