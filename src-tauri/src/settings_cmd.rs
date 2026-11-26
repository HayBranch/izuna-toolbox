//! 「设置」页命令：外观、日志等级、恢复默认，以及配置/数据的导出与导入。
//!
//! 为什么单独成模块：`commands.rs` 已经承载浮窗/队列的全部业务命令，
//! 这里只处理应用级配置（conf.toml）与备份文件，职责分开更好维护。
//!
//! 备份导入是两阶段：
//! 1. `select_*_backup` 只读取并解析，放进 `PendingBackup`，返回摘要；
//! 2. 前端让用户选择「覆盖 / 增量」后，`apply_backup` 才真正写数据库和 conf.toml。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::app_config::{SettingsInfo, SettingsState};
use crate::backup::{self, BackupPreview, PendingBackup};
use crate::danmaku;
use crate::overlay_win;
use crate::queue_win;
use crate::store::{Config, ImportStats, Store};
use crate::webdav::{self, WebdavConfig};

#[tauri::command]
pub fn get_settings(settings: State<'_, SettingsState>) -> SettingsInfo {
    settings.get()
}

#[tauri::command]
pub fn set_theme(
    settings: State<'_, SettingsState>,
    theme: String,
) -> Result<SettingsInfo, String> {
    settings.set_theme(&theme)
}

#[tauri::command]
pub fn set_log_level(
    settings: State<'_, SettingsState>,
    level: String,
) -> Result<SettingsInfo, String> {
    settings.set_log_level(&level)
}

/// 排查弹幕连接时，用系统文件管理器打开 exe 同目录的 logs/，并把目录路径
/// 回给前端展示。日志等级默认 error，连接过程日志基本不可见；设置页的
/// 「弹幕连接排查」会先把等级调到 debug 再调这个命令，让用户直接拿到当天日志。
#[tauri::command]
pub fn open_log_dir(settings: State<'_, SettingsState>) -> Result<String, String> {
    let dir = PathBuf::from(settings.get().log_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败：{e}"))?;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("打开日志目录失败：{e}"))?;
    }
    Ok(dir.display().to_string())
}

/// 保存 B 站登录 Cookie：解析 uid 后写入 conf/bili_cookie.txt（不进 conf.toml、
/// 不进备份）。成功后重启所有弹幕连接，让新登录态立即生效——服务端只在
/// 登录连接里下发真实 uid 和未打码昵称，游客连接一律 uid=0 + 打码昵称。
#[tauri::command]
pub fn set_bili_cookie(
    app: AppHandle,
    cookie: String,
    settings: State<'_, SettingsState>,
) -> Result<SettingsInfo, String> {
    let info = settings.set_bili_cookie(&cookie)?;
    danmaku::reconcile(&app);
    Ok(info)
}

/// 清除 B 站登录 Cookie，恢复游客连接；同样重启弹幕连接。
#[tauri::command]
pub fn clear_bili_cookie(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<SettingsInfo, String> {
    let info = settings.clear_bili_cookie()?;
    danmaku::reconcile(&app);
    Ok(info)
}

#[tauri::command]
pub fn reset_settings(settings: State<'_, SettingsState>) -> Result<SettingsInfo, String> {
    settings.reset()
}

// ---------- 导出 ----------

fn export_content(
    app: &AppHandle,
    settings: &SettingsInfo,
    store: &Store,
    include_data: bool,
) -> Result<String, String> {
    let data = if include_data { Some(store.get()) } else { None };
    backup::build_content(settings, data, &app.package_info().version.to_string())
}

/// 导出到本地文件（路径由前端的系统另存对话框提供）。
#[tauri::command]
pub fn export_local(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    store: State<'_, Arc<Store>>,
    path: String,
    include_data: bool,
) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("保存路径不能为空".into());
    }
    let content = export_content(&app, &settings.get(), &store, include_data)?;
    std::fs::write(trimmed, content).map_err(|e| format!("写入备份文件失败：{e}"))?;
    Ok(trimmed.to_string())
}

const WEBDAV_FULL_BACKUP: &str = "izuna-backup.json";
const WEBDAV_CONFIG: &str = "conf.toml";

/// 导出到 WebDAV：含数据时上传 JSON 备份，否则上传 conf.toml。
#[tauri::command]
pub async fn export_webdav(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    store: State<'_, Arc<Store>>,
    dav: WebdavConfig,
    include_data: bool,
) -> Result<String, String> {
    let (content, file_name) = {
        let text = export_content(&app, &settings.get(), &store, include_data)?;
        let name = if include_data {
            WEBDAV_FULL_BACKUP
        } else {
            WEBDAV_CONFIG
        };
        (text, name)
    };
    webdav::put(&dav, file_name, content.into_bytes()).await
}

// ---------- 导入（先预览） ----------

#[tauri::command]
pub fn select_local_backup(
    pending: State<'_, PendingBackup>,
    path: String,
) -> Result<BackupPreview, String> {
    let text =
        std::fs::read_to_string(path.trim()).map_err(|e| format!("读取备份文件失败：{e}"))?;
    let parsed = backup::parse_content(path.trim(), &text)?;
    let preview = parsed.preview();
    *pending
        .0
        .lock()
        .map_err(|e| e.to_string())? = Some(parsed);
    Ok(preview)
}

#[tauri::command]
pub async fn select_webdav_backup(
    app: AppHandle,
    dav: WebdavConfig,
    remote_file: String,
) -> Result<BackupPreview, String> {
    let text = webdav::get(&dav, &remote_file).await?;
    let source = format!("WebDAV · {}", remote_file.trim());
    let parsed = backup::parse_content(&source, &text)?;
    let preview = parsed.preview();
    let pending = app.state::<PendingBackup>();
    *pending
        .0
        .lock()
        .map_err(|e| e.to_string())? = Some(parsed);
    Ok(preview)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub settings: SettingsInfo,
    pub config: Config,
    pub stats: ImportStats,
    pub mode: String,
}

/// 应用待导入备份。
/// - `mode = overwrite`：清空现有浮窗/队列后完全恢复备份；
/// - `mode = merge`：只新增本地没有的同名浮窗/队列，同名队列合并名单；
/// - `apply_settings`：是否同时把备份里的外观 / 日志等级写回 conf.toml。
#[tauri::command]
pub fn apply_backup(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    settings: State<'_, SettingsState>,
    pending: State<'_, PendingBackup>,
    mode: String,
    apply_settings: bool,
) -> Result<ImportResult, String> {
    let parsed = {
        let guard = pending.0.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| "没有待导入的备份，请重新选择文件".to_string())?
    };

    let mode = mode.trim().to_ascii_lowercase();
    if mode != "overwrite" && mode != "merge" {
        return Err(format!("未知的导入方式：{mode}"));
    }

    if apply_settings && (parsed.theme.is_some() || parsed.log_level.is_some()) {
        settings.update(parsed.theme.as_deref(), parsed.log_level.as_deref())?;
    }

    let stats = match &parsed.data {
        Some(data) if mode == "overwrite" => store.replace_all(data)?,
        Some(data) => store.merge_from(data)?,
        None => ImportStats::default(),
    };

    // 导入成功后清掉待处理备份，避免重复点「确认导入」
    if let Ok(mut guard) = pending.0.lock() {
        *guard = None;
    }

    sync_windows(&app);

    let config = store.get();
    if let Err(e) = app.emit(crate::commands::CONFIG_CHANGED_EVENT, &config) {
        log::error!("导入后广播配置失败：{e}");
    }

    Ok(ImportResult {
        settings: settings.get(),
        config,
        stats,
        mode,
    })
}

/// 导入后让窗口与数据库对齐：关掉已删除的弹窗、为新增配置建窗、重启弹幕任务。
fn sync_windows(app: &AppHandle) {
    let store = app.state::<Arc<Store>>();
    let config = store.get();
    let overlay_ids: HashSet<i64> = config.overlays.iter().map(|o| o.id).collect();
    let queue_ids: HashSet<i64> = config.queues.iter().map(|q| q.id).collect();

    for (label, window) in app.webview_windows() {
        if let Some(id) = overlay_win::id_from_label(&label) {
            if !overlay_ids.contains(&id) {
                let _ = window.close();
            }
        } else if let Some(id) = queue_win::id_from_label(&label) {
            if !queue_ids.contains(&id) {
                let _ = window.close();
            }
        }
    }

    for cfg in &config.overlays {
        if let Err(e) = overlay_win::sync_overlay_window(app, cfg, true) {
            log::error!("导入后同步浮窗 #{} 失败：{e}", cfg.id);
        }
    }
    for cfg in &config.queues {
        if let Err(e) = queue_win::sync_queue_window(app, cfg, true) {
            log::error!("导入后同步排队弹窗 #{} 失败：{e}", cfg.id);
        }
    }

    // 队列可能被删除/暂停/换直播间，统一让弹幕任务重新对齐
    danmaku::reconcile(app);
}
