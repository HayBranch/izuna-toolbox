//! 泉奈工具箱 · 应用装配
//!
//! 窗口结构：
//! - `main`：主窗口（配置界面：悬浮字幕 + 排队队列）
//! - `overlay-{id}`：N 个悬浮字幕浮窗，无边框透明置顶小窗，可被直播软件窗口捕获
//! - `queue-{id}`：N 个排队队列弹窗，复用同款透明窗口框架
//!
//! 数据流（单一真相在 Rust SQLite）：
//! 主窗口编辑 → invoke 命令 → SQLite 提交 → `emit("config-changed")` 广播全量配置
//! → 各浮窗页面按自己的 label 渲染内容；拖动浮窗 → `Moved` 事件回写位置。
//! B 站弹幕由 `danmaku` 模块的长连接任务实时接收，按队列配置的间隔批量写入。

mod app_config;
mod backup;
mod commands;
mod danmaku;
mod overlay_win;
mod queue_win;
mod settings_cmd;
mod store;
mod tray;
mod updater;
mod webdav;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use store::Store;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 启动配置：exe 同目录 conf/conf.toml（便携模式 + 日志等级）。
    // 任何读取异常都降级为默认值，提示先攒着，等 logger 注册后再写进日志。
    let exe_result = std::env::current_exe();
    let exe_dir = exe_result
        .as_ref()
        .ok()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut loaded = app_config::load(&exe_dir);
    if exe_result.is_err() {
        loaded.warnings.push(
            "无法确定 exe 所在目录，配置与日志暂时按当前工作目录处理".into(),
        );
    }

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    // 单实例插件必须最先注册：再次启动程序时不再开新进程，
    // 而是把已经运行的实例显示出来并聚焦。
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
                overlay_win::sync_all_overlay_windows(app);
                queue_win::sync_all_queue_windows(app);
            }
        }));
    }
    // 日志插件：dev 终端 + WebView 控制台 + exe 同目录 logs/ 三路输出。
    // 插件内部先按 Trace 接收全部记录，真正的过滤等级由 `log::set_max_level`
    // 在 setup 里按 conf.toml 设置；这样设置页改日志等级可以立即生效，无需重启。
    builder = builder.plugin(
        tauri_plugin_log::Builder::new()
            .level(log::LevelFilter::Trace)
            .targets([
                tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Webview),
                tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                    path: exe_dir.join(app_config::LOGS_DIR),
                    file_name: None,
                }),
            ])
            .build(),
    );
    // 系统文件选择 / 另存对话框（导出、导入备份用）
    builder = builder.plugin(tauri_plugin_dialog::init());

    builder
        .setup(move |app| {
            // 恢复 conf.toml 里保存的日志等级（默认 error：基本不打印日志）。
            log::set_max_level(loaded.log_level);

            // logger 已就绪，补记启动配置阶段的提示。
            for warning in &loaded.warnings {
                log::warn!("{warning}");
            }

            // 1. 数据存储仍是 SQLite；便携模式由 conf/conf.toml 决定：
            //    true  → exe 同目录 data/（整包可拷走）
            //    false → 系统应用数据目录 %APPDATA%/com.izuna.toolbox/
            let data_dir = if loaded.settings.portable {
                exe_dir.join(app_config::DATA_DIR)
            } else {
                app.path().app_data_dir()?
            };
            std::fs::create_dir_all(&data_dir)?;

            log::info!("配置文件：{}", loaded.path.display());
            log::info!("数据库目录：{}", data_dir.display());
            log::info!("日志目录：{}", exe_dir.join(app_config::LOGS_DIR).display());

            let store = Arc::new(Store::new(data_dir.join("izuna-toolbox.db"))?);

            // 2. 先注册所有状态，再创建任何窗口。
            //    浮窗页面一加载就会调用 get_config；若 store 到窗口创建之后才 manage，
            //    启动瞬间会报 "state not managed for field store on command get_config"。
            app.manage(store.clone());
            // 应用设置（外观 / 日志等级）与待导入备份：设置页与导入命令共用。
            // 必须在任何窗口创建前注册，否则页面首次 get_settings 会报 state not managed。
            app.manage(app_config::SettingsState::new(loaded.settings.clone()));
            app.manage(backup::PendingBackup::default());
            app.manage(overlay_win::MeasuredSet(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )));
            app.manage(queue_win::QueueMeasuredSet(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )));
            app.manage(std::sync::Mutex::new(
                std::time::Instant::now() - std::time::Duration::from_secs(1),
            ));
            // 自动更新的取消标志（download_update / cancel_update 共用）
            app.manage(updater::UpdateState::default());
            // B 站弹幕任务管理器（队列连接状态 + 取消标志）
            app.manage(danmaku::DanmakuManager::default());

            let config = store.get();

            // 3. 为已有配置的浮窗 / 排队弹窗创建窗口
            //    （保持隐藏，等页面量好尺寸再显示）
            for cfg in &config.overlays {
                if let Err(e) = overlay_win::create_overlay_window(app.handle(), cfg) {
                    log::error!("启动浮窗 #{} 失败：{e}", cfg.id);
                }
            }
            for cfg in &config.queues {
                if let Err(e) = queue_win::create_queue_window(app.handle(), cfg) {
                    log::error!("启动排队弹窗 #{} 失败：{e}", cfg.id);
                }
            }

            // 4. 启动所有启用中的排队队列弹幕监听
            danmaku::reconcile(app.handle());

            // 5. 托盘
            tray::create_tray(app.handle())?;

            log::info!("应用启动完成，日志系统就绪");
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
                        overlay_win::sync_all_overlay_windows(window.app_handle());
                        queue_win::sync_all_queue_windows(window.app_handle());
                    } else if let Some(id) = overlay_win::id_from_label(window.label()) {
                        close_popup_like(window, api, id, false);
                    } else if let Some(id) = queue_win::id_from_label(window.label()) {
                        close_popup_like(window, api, id, true);
                    }
                }
                // 主窗口焦点变化：重应用所有浮窗状态（主窗口聚焦时临时取消置顶，
                // 失焦后恢复浮窗自己的置顶配置；不强制鼠标穿透，浮窗仍可拖动）。
                tauri::WindowEvent::Focused(_) => {
                    if window.label() == "main" {
                        overlay_win::sync_all_overlay_windows(window.app_handle());
                        queue_win::sync_all_queue_windows(window.app_handle());
                    }
                }
                // 拖动浮窗：回写位置到数据库；同时节流广播给主窗口刷新坐标输入框
                tauri::WindowEvent::Moved(position) => {
                    if let Some(id) = overlay_win::id_from_label(window.label()) {
                        let store = window.app_handle().state::<Arc<Store>>();
                        store.set_position(id, position.x, position.y);
                        emit_moved(
                            window.app_handle(),
                            "overlay-moved",
                            overlay_win::MovedEvent {
                                id,
                                x: position.x,
                                y: position.y,
                            },
                        );
                    } else if let Some(id) = queue_win::id_from_label(window.label()) {
                        let store = window.app_handle().state::<Arc<Store>>();
                        store.set_queue_position(id, position.x, position.y);
                        emit_moved(
                            window.app_handle(),
                            "queue-moved",
                            queue_win::QueueMovedEvent {
                                id,
                                x: position.x,
                                y: position.y,
                            },
                        );
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
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
            commands::reset_overlay_position,
            commands::set_overlay_shown,
            commands::set_overlay_always_top,
            commands::set_overlay_click_through,
            commands::delete_overlay,
            commands::set_overlay_size,
            commands::resize_overlay,
            commands::add_queue,
            commands::update_queue,
            commands::set_queue_enabled,
            commands::set_queue_shown,
            commands::set_queue_always_top,
            commands::set_queue_click_through,
            commands::move_queue,
            commands::reset_queue_position,
            commands::set_queue_size,
            commands::resize_queue,
            commands::delete_queue,
            commands::add_queue_entry,
            commands::step_queue_entry,
            commands::set_queue_entry_count,
            commands::rename_queue_entry,
            commands::delete_queue_entry,
            commands::clear_queue_entries,
            commands::get_queue_status,
            commands::set_main_always_on_top,
            commands::open_url,
            settings_cmd::get_settings,
            settings_cmd::set_theme,
            settings_cmd::set_log_level,
            settings_cmd::reset_settings,
            settings_cmd::export_local,
            settings_cmd::export_webdav,
            settings_cmd::select_local_backup,
            settings_cmd::select_webdav_backup,
            settings_cmd::apply_backup,
            updater::download_update,
            updater::cancel_update,
            updater::install_update,
            updater::fetch_mirror_list
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用失败");
}

/// 关闭浮窗 / 排队弹窗的默认行为：仍存在于数据库时改为「隐藏」，
/// 只有删除流程（记录已删）才能真正关掉窗口。
fn close_popup_like(
    window: &tauri::Window,
    api: &tauri::CloseRequestApi,
    id: i64,
    is_queue: bool,
) {
    let app = window.app_handle();
    let store = app.state::<Arc<Store>>();
    let config = store.get();
    if is_queue {
        if !config.queues.iter().any(|q| q.id == id) {
            return;
        }
        api.prevent_close();
        if store.set_queue_flag(id, |q| q.shown = false).is_ok() {
            if let Some(cfg) = store.get().queues.into_iter().find(|q| q.id == id) {
                let _ = queue_win::sync_queue_window(app, &cfg, false);
            }
            broadcast_config(app, &store);
        }
    } else {
        if !config.overlays.iter().any(|o| o.id == id) {
            return;
        }
        api.prevent_close();
        if store.set_flag(id, |o| o.shown = false).is_ok() {
            if let Some(cfg) = store.get().overlays.into_iter().find(|o| o.id == id) {
                let _ = overlay_win::sync_overlay_window(app, &cfg, false);
            }
            broadcast_config(app, &store);
        }
    }
}

fn broadcast_config(app: &tauri::AppHandle, store: &Arc<Store>) {
    use tauri::Emitter;
    let _ = app.emit(commands::CONFIG_CHANGED_EVENT, store.get());
}

/// 拖动弹窗时广播位置（节流 100ms），主窗口据此刷新 X/Y 输入框。
fn emit_moved<P: Serialize + Clone>(
    app: &tauri::AppHandle,
    event: &str,
    payload: P,
) {
    let throttle = app.state::<std::sync::Mutex<std::time::Instant>>();
    let mut last = throttle.lock().unwrap();
    if last.elapsed() >= std::time::Duration::from_millis(100) {
        *last = std::time::Instant::now();
        use tauri::Emitter;
        let _ = app.emit(event, payload);
    }
}
