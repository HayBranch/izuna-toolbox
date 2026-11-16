//! 悬浮字幕浮窗的窗口生命周期：创建、同步配置到窗口状态。
//!
//! 为什么"能被直播软件截取"：
//! - 浮窗是一个普通的顶层无边框窗口（`decorations(false)` + `transparent(true)`），
//!   OBS / 直播伴侣的"窗口捕获"（WGC / BitBlt）都能正常抓取；
//! - `shadow(false)` 关掉无边框窗口的 DWM 阴影，避免捕获出黑边/圆角发虚；
//! - `skip_taskbar(true)` 只影响任务栏图标，不影响捕获；
//! - 鼠标穿透（`set_ignore_cursor_events`）只改变鼠标事件的去向，不改变画面。

use std::collections::HashSet;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::store::OverlayConfig;

/// 已量好内容尺寸的浮窗 id 集合（运行时状态，见 `apply_window_state` 的显示门槛）。
pub struct MeasuredSet(pub Mutex<HashSet<i64>>);

/// 浮窗窗口 label：`overlay-{id}`，与 capabilities 里的 `overlay-*` 对应。
pub const LABEL_PREFIX: &str = "overlay-";

pub fn overlay_label(id: i64) -> String {
    format!("{LABEL_PREFIX}{id}")
}

/// 从窗口 label 反解浮窗 id；非浮窗窗口返回 None。
pub fn id_from_label(label: &str) -> Option<i64> {
    label.strip_prefix(LABEL_PREFIX)?.parse().ok()
}

/// `overlay-moved` 事件负载（拖动浮窗后通知主窗口刷新 X/Y 输入框）。
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct MovedEvent {
    pub id: i64,
    pub x: i32,
    pub y: i32,
}

/// 默认落点：主显示器顶部水平居中（宽度按 800 物理像素估算，
/// 加载后浮窗会自适应真实尺寸；y 固定 40 留一点边距）。
pub fn default_top_center(app: &AppHandle) -> (i32, i32) {
    match app.primary_monitor() {
        Ok(Some(monitor)) => {
            let x = (((monitor.size().width as f64) - 800.0) / 2.0).max(0.0) as i32;
            (x, 40)
        }
        _ => (0, 40),
    }
}

/// 创建浮窗窗口（若不存在）。创建后保持隐藏，
/// 等浮窗页面量好内容尺寸（`set_overlay_size`）且 `shown = true` 时才显示，
/// 避免出现"先闪一下默认大小再缩放"的观感。
pub fn create_overlay_window(app: &AppHandle, cfg: &OverlayConfig) -> Result<(), String> {
    let label = overlay_label(cfg.id);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title(format!("泉奈浮窗 · {}", cfg.name))
        .visible(false) // 先隐藏，防止加载时闪烁
        .inner_size(160.0, 44.0) // 极小起步：显示门槛由"量好尺寸"把关（见下）
        .decorations(false) // 无边框
        .transparent(true) // 背景透明，圆角/半透明背景由页面 CSS 画出
        .shadow(false) // 关键：透明窗口必须关阴影，否则直播捕获会有黑边
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .focused(false) // 出现/显示时绝不抢焦点，避免把游戏切到后台
        .build()
        .map_err(|e| format!("创建浮窗失败：{e}"))?;

    apply_window_state(app, &window, cfg, true)
}

/// 透明浮窗的显示门槛：**没量好内容尺寸的浮窗绝不允许显示**。
/// 原因：透明窗口的整块矩形（包括看不见的部分）都会拦截鼠标点击，
/// 若用默认大小（800x600 级别）提前显示，会像一块玻璃盖住主窗口，
/// 用户表现为"主窗口点什么都没反应"。
pub fn mark_measured(app: &AppHandle, id: i64) {
    if let Some(set) = app.try_state::<MeasuredSet>() {
        if let Ok(mut m) = set.0.lock() {
            m.insert(id);
        }
    }
}

fn is_measured(app: &AppHandle, id: i64) -> bool {
    app.try_state::<MeasuredSet>()
        .map(|s| s.0.lock().map(|m| m.contains(&id)).unwrap_or(false))
        .unwrap_or(false)
}

/// 把配置同步到浮窗窗口状态（置顶 / 鼠标穿透 / 位置 / 显隐）。
/// `apply_position = false` 时跳过移动，用于"样式微调"命令，
/// 防止主窗口表单里过期的坐标把用户刚拖好的位置弹回去。
pub fn apply_window_state(
    app: &AppHandle,
    window: &WebviewWindow,
    cfg: &OverlayConfig,
    apply_position: bool,
) -> Result<(), String> {
    // 主窗口在前台时临时取消浮窗置顶：浮窗的矩形（含透明部分）会拦截鼠标，
    // 常驻置顶的话会压在自家主窗口上，表现为"主窗口按钮点了没反应"。
    // 主窗口失焦（回到游戏/桌面）后恢复配置里的置顶。
    let main_focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    window
        .set_always_on_top(cfg.always_on_top && !main_focused)
        .map_err(warn("设置置顶失败"))?;
    window
        .set_ignore_cursor_events(cfg.click_through)
        .map_err(warn("设置鼠标穿透失败"))?;
    if apply_position {
        window
            .set_position(PhysicalPosition::new(cfg.x, cfg.y))
            .map_err(warn("设置浮窗位置失败"))?;
    }
    // show() 不会抢焦点（区别于 set_focus），适合直播场景；
    // 但只在量好尺寸后才 show，未量好之前保持隐藏
    if cfg.shown {
        if is_measured(app, cfg.id) || window.is_visible().unwrap_or(false) {
            window.show().map_err(warn("显示浮窗失败"))?;
        }
    } else {
        window.hide().map_err(warn("隐藏浮窗失败"))?;
    }
    Ok(())
}

/// 保证浮窗窗口存在并按配置同步状态；返回窗口句柄。
pub fn sync_overlay_window(
    app: &AppHandle,
    cfg: &OverlayConfig,
    apply_position: bool,
) -> Result<WebviewWindow, String> {
    let label = overlay_label(cfg.id);
    if let Some(window) = app.get_webview_window(&label) {
        apply_window_state(app, &window, cfg, apply_position)?;
        Ok(window)
    } else {
        create_overlay_window(app, cfg)?;
        app.get_webview_window(&label)
            .ok_or_else(|| "创建浮窗后仍找不到窗口".into())
    }
}

/// 错误转字符串的小工具（窗口状态设置失败只转成消息，不 panic）。
fn warn<E: std::fmt::Display>(msg: &'static str) -> impl Fn(E) -> String {
    move |e| format!("{msg}：{e}")
}
