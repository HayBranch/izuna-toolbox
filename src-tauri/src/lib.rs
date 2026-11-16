//! 泉奈工具箱 · 应用装配
//!
//! 窗口结构：
//! - `main`：主窗口（配置界面：数据计数 + 悬浮字幕管理）
//! - `overlay-{id}`：N 个悬浮字幕浮窗，无边框透明置顶小窗，可被直播软件窗口捕获
//!
//! 数据流（单一真相在 Rust SQLite）：
//! 主窗口编辑 → invoke 命令 → SQLite 提交 → `emit("config-changed")` 广播全量配置
//! → 各浮窗页面按自己的 label 渲染模板；拖动浮窗 → `Moved` 事件回写位置。

mod commands;
mod overlay_win;
mod store;
mod tray;

use std::sync::Arc;

use tauri::Manager;
use store::Store;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 1. 数据存储：SQLite 数据库放在应用数据目录（Windows: %APPDATA%/com.izuna.toolbox/）
            //    首次运行建表并种入示例；发现旧版 config.json 会自动迁移数据。
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let store = Arc::new(Store::new(data_dir.join("izuna-toolbox.db"))?);

            // 2. 浮窗"已量尺寸"集合要先于浮窗创建注册（显示门槛依赖它）
            app.manage(overlay_win::MeasuredSet(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )));

            // 3. 为已有配置的浮窗创建窗口（保持隐藏，等浮窗页面量好尺寸再显示）
            for cfg in store.get().overlays {
                if let Err(e) = overlay_win::create_overlay_window(app.handle(), &cfg) {
                    eprintln!("启动浮窗 #{} 失败：{e}", cfg.id);
                }
            }

            // 4. 托盘 + 拖动事件节流时间戳
            app.manage(std::sync::Mutex::new(
                std::time::Instant::now() - std::time::Duration::from_secs(1),
            ));
            app.manage(store);
            tray::create_tray(app.handle())?;

            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                // 关闭主窗口 = 隐藏到托盘；关闭仍存在的浮窗（如 Alt+F4）= 等价"隐藏"；
                // 已被删除的浮窗窗口不拦截，让删除流程能真正关掉窗口。
                // 都不真正退出进程，托盘菜单"退出程序"才退出。
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if window.label() == "main" {
                        api.prevent_close();
                        let _ = window.hide();
                    } else if let Some(id) = overlay_win::id_from_label(window.label()) {
                        let app = window.app_handle();
                        let store = app.state::<Arc<Store>>();
                        let exists = store.get().overlays.iter().any(|o| o.id == id);
                        if exists {
                            api.prevent_close();
                            if store.set_flag(id, |o| o.shown = false).is_ok() {
                                if let Some(cfg) =
                                    store.get().overlays.into_iter().find(|o| o.id == id)
                                {
                                    let _ =
                                        overlay_win::sync_overlay_window(app, &cfg, false);
                                }
                                use tauri::Emitter;
                                let _ = app.emit(commands::CONFIG_CHANGED_EVENT, &store.get());
                            }
                        }
                    }
                }
                // 主窗口焦点变化：重应用所有浮窗状态（主窗口在前台时浮窗临时取消置顶，
                // 避免透明浮窗压住主窗口拦截点击；失焦后恢复置顶）
                tauri::WindowEvent::Focused(_) => {
                    if window.label() == "main" {
                        let app = window.app_handle();
                        let store = app.state::<Arc<Store>>();
                        for cfg in store.get().overlays {
                            if let Some(w) = app.get_webview_window(&overlay_win::overlay_label(cfg.id)) {
                                let _ = overlay_win::apply_window_state(app, &w, &cfg, false);
                            }
                        }
                    }
                }
                // 拖动浮窗：回写位置到数据库；同时节流广播给主窗口刷新坐标输入框
                tauri::WindowEvent::Moved(position) => {
                    if let Some(id) = overlay_win::id_from_label(window.label()) {
                        let store = window.app_handle().state::<Arc<Store>>();
                        store.set_position(id, position.x, position.y);
                        let throttle = window
                            .app_handle()
                            .state::<std::sync::Mutex<std::time::Instant>>();
                        let mut last = throttle.lock().unwrap();
                        if last.elapsed() >= std::time::Duration::from_millis(100) {
                            *last = std::time::Instant::now();
                            use tauri::Emitter;
                            let _ = window.app_handle().emit(
                                "overlay-moved",
                                overlay_win::MovedEvent {
                                    id,
                                    x: position.x,
                                    y: position.y,
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_account,
            commands::add_variable,
            commands::update_variable,
            commands::step_variable,
            commands::delete_variable,
            commands::add_computed,
            commands::update_computed,
            commands::delete_computed,
            commands::add_overlay,
            commands::update_overlay,
            commands::move_overlay,
            commands::set_overlay_shown,
            commands::set_overlay_always_top,
            commands::set_overlay_click_through,
            commands::delete_overlay,
            commands::set_overlay_size,
            commands::open_url
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用失败");
}
