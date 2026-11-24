//! 备份文件：导出 / 解析 conf.toml 与 SQLite 数据。
//!
//! 两种导出格式：
//! - **只导出配置**：直接写一份可读的 `conf.toml`（外观 + 日志等级 + portable）；
//! - **同时导出数据**：写一个 JSON 备份，包含配置和全部浮窗 / 队列 / 名单。
//!
//! 导入时按内容自动识别：`{` 开头按 JSON，否则按 conf.toml 解析。
//! 解析结果会先放进 `PendingBackup`，前端看过摘要、选择「覆盖 / 增量」后才真正写入。

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::app_config::{self, SettingsInfo};
use crate::store::{now_ms, Config};

pub const BACKUP_FORMAT: &str = "izuna-toolbox-backup";
pub const BACKUP_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    /// 格式标记与版本必须有：缺失时按默认值补齐会把任意 JSON 误当成合法备份。
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub exported_at: i64,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub settings: Option<BackupSettings>,
    #[serde(default)]
    pub data: Option<Config>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupSettings {
    pub theme: Option<String>,
    pub log_level: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedBackup {
    pub source: String,
    pub exported_at: i64,
    pub app_version: String,
    pub theme: Option<String>,
    pub log_level: Option<String>,
    pub data: Option<Config>,
}

impl ParsedBackup {
    pub fn preview(&self) -> BackupPreview {
        let (overlay_count, queue_count, entry_count) = match &self.data {
            Some(data) => (
                data.overlays.len(),
                data.queues.len(),
                data.queues.iter().map(|q| q.entries.len()).sum(),
            ),
            None => (0, 0, 0),
        };
        BackupPreview {
            source: self.source.clone(),
            exported_at: self.exported_at,
            app_version: self.app_version.clone(),
            has_settings: self.theme.is_some() || self.log_level.is_some(),
            theme: self.theme.clone(),
            log_level: self.log_level.clone(),
            has_data: self.data.is_some(),
            overlay_count,
            queue_count,
            entry_count,
        }
    }
}

/// 给前端的导入预览（不包含真实数据，避免把整份名单传回界面）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPreview {
    pub source: String,
    pub exported_at: i64,
    pub app_version: String,
    pub has_settings: bool,
    pub theme: Option<String>,
    pub log_level: Option<String>,
    pub has_data: bool,
    pub overlay_count: usize,
    pub queue_count: usize,
    pub entry_count: usize,
}

/// 已读取、等待用户确认的备份。
#[derive(Default)]
pub struct PendingBackup(pub Mutex<Option<ParsedBackup>>);

/// 生成导出内容：`include_data = false` 时输出 conf.toml，否则输出 JSON 备份。
pub fn build_content(
    settings: &SettingsInfo,
    data: Option<Config>,
    app_version: &str,
) -> Result<String, String> {
    match data {
        None => Ok(app_config::render_toml(
            settings.portable,
            &settings.theme,
            &settings.log_level,
        )),
        Some(data) => {
            let file = BackupFile {
                format: BACKUP_FORMAT.into(),
                version: BACKUP_VERSION,
                exported_at: now_ms(),
                app_version: app_version.to_string(),
                settings: Some(BackupSettings {
                    theme: Some(settings.theme.clone()),
                    log_level: Some(settings.log_level.clone()),
                }),
                data: Some(data),
            };
            serde_json::to_string_pretty(&file).map_err(|e| format!("生成备份失败：{e}"))
        }
    }
}

/// 解析备份文本：自动识别 conf.toml 与 JSON。
pub fn parse_content(source: &str, text: &str) -> Result<ParsedBackup, String> {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with('{') {
        parse_json(source, trimmed)
    } else {
        let (theme, log_level) = app_config::parse_import_toml(text)?;
        Ok(ParsedBackup {
            source: source.to_string(),
            exported_at: 0,
            app_version: String::new(),
            theme: Some(theme),
            log_level: Some(log_level),
            data: None,
        })
    }
}

fn parse_json(source: &str, text: &str) -> Result<ParsedBackup, String> {
    let file: BackupFile =
        serde_json::from_str(text).map_err(|e| format!("备份 JSON 解析失败：{e}"))?;
    if file.format != BACKUP_FORMAT {
        return Err("不是泉奈工具箱的备份文件（format 标记不匹配）".into());
    }
    if file.version > BACKUP_VERSION {
        return Err(format!(
            "备份版本 {} 高于当前程序支持的 {BACKUP_VERSION}，请先升级程序",
            file.version
        ));
    }
    if file.settings.is_none() && file.data.is_none() {
        return Err("备份文件不包含配置或数据".into());
    }
    Ok(ParsedBackup {
        source: source.to_string(),
        exported_at: file.exported_at,
        app_version: file.app_version,
        theme: file.settings.as_ref().and_then(|s| s.theme.clone()),
        log_level: file.settings.as_ref().and_then(|s| s.log_level.clone()),
        data: file.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exported_conf_toml() {
        let text = "# 泉奈工具箱配置\nportable = false\ntheme = \"dark\"\nlog_level = \"warn\"\n";
        let parsed = parse_content("conf.toml", text).unwrap();
        assert_eq!(parsed.theme.as_deref(), Some("dark"));
        assert_eq!(parsed.log_level.as_deref(), Some("warn"));
        assert!(parsed.data.is_none());
        assert!(parsed.preview().has_settings);
    }

    #[test]
    fn rejects_empty_or_unrelated_files() {
        assert!(parse_content("empty", "   ").is_err());
        assert!(parse_content("other", "title = \"hello\"").is_err());
        assert!(parse_content("other.json", "{}").is_err());
    }

    #[test]
    fn rejects_backup_without_settings_or_data() {
        let json = r#"{"format":"izuna-toolbox-backup","version":1}"#;
        assert!(parse_content("empty.json", json).is_err());
    }
}
