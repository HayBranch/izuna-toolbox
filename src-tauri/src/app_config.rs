//! 应用启动配置：exe 同目录 `conf/conf.toml`。
//!
//! 配置项：
//! - `portable`：数据库放在 exe 同目录 `data/` 还是系统应用数据目录；
//! - `theme`：界面外观（auto / light / dark）；
//! - `log_level`：日志等级（off / error / warn / info / debug / trace）。
//!
//! 启动时读取一次并注册为 Tauri 状态；之后「设置」页通过命令修改并立即写回
//! 同一个文件，因此重启后仍然有效。旧的 WebView `localStorage` 主题偏好不再使用。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const CONF_DIR: &str = "conf";
pub const CONF_FILE: &str = "conf.toml";
pub const DATA_DIR: &str = "data";
pub const LOGS_DIR: &str = "logs";

/// 旧版便携标记，仅用于在首次生成 conf.toml 时推断默认值。
const LEGACY_PORTABLE_FLAG: &str = "portable.flag";

/// 默认日志等级：只记录错误，日常使用基本不输出日志。
pub const DEFAULT_LOG_LEVEL: &str = "error";
/// 默认外观：跟随系统。
pub const DEFAULT_THEME: &str = "auto";

#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
    #[serde(default)]
    portable: bool,
    #[serde(default = "default_log_level")]
    log_level: String,
    /// Option 用于区分「旧配置没有 theme」：读取后补写回文件。
    #[serde(default)]
    theme: Option<String>,
}

/// 前端「设置」页需要的配置信息（写回 conf.toml 的路径与日志目录也一并给出）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInfo {
    pub portable: bool,
    pub theme: String,
    pub log_level: String,
    pub conf_path: String,
    pub log_dir: String,
}

pub struct LoadedConfig {
    pub settings: SettingsInfo,
    pub log_level: log::LevelFilter,
    pub path: PathBuf,
    pub warnings: Vec<String>,
}

/// 运行时配置状态：设置页读改写都走它，保证内存与 conf.toml 一致。
pub struct SettingsState {
    inner: Mutex<SettingsInfo>,
}

impl SettingsState {
    pub fn new(settings: SettingsInfo) -> Self {
        Self {
            inner: Mutex::new(settings),
        }
    }

    pub fn get(&self) -> SettingsInfo {
        self.inner
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| fallback_settings())
    }

    pub fn set_theme(&self, theme: &str) -> Result<SettingsInfo, String> {
        self.update(Some(theme), None)
    }

    pub fn set_log_level(&self, level: &str) -> Result<SettingsInfo, String> {
        self.update(None, Some(level))
    }

    /// 恢复到默认设置（外观 + 日志等级），不改 `portable`（数据库位置不能悄悄变）。
    pub fn reset(&self) -> Result<SettingsInfo, String> {
        self.update(Some(DEFAULT_THEME), Some(DEFAULT_LOG_LEVEL))
    }

    /// 更新并立即写回 conf.toml；成功后同步内存与全局日志等级。
    pub fn update(
        &self,
        theme: Option<&str>,
        log_level: Option<&str>,
    ) -> Result<SettingsInfo, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        let mut next = guard.clone();
        if let Some(theme) = theme {
            next.theme = normalize_theme(theme)?;
        }
        if let Some(level) = log_level {
            next.log_level = normalize_log_level(level)?.to_string();
        }
        write_config(
            Path::new(&next.conf_path),
            next.portable,
            &next.theme,
            &next.log_level,
        )?;
        // 动态调整全局过滤等级：设置页改完立即生效，不需要重启。
        log::set_max_level(level_filter(&next.log_level));
        *guard = next.clone();
        Ok(next)
    }
}

/// 读取配置；任何异常都降级为默认值，并把提示收集起来等 logger 就绪后输出。
pub fn load(exe_dir: &Path) -> LoadedConfig {
    let path = exe_dir.join(CONF_DIR).join(CONF_FILE);
    let log_dir = exe_dir.join(LOGS_DIR);
    let mut warnings = Vec::new();
    let legacy_portable = exe_dir.join(LEGACY_PORTABLE_FLAG).is_file();

    // 解析失败 / 读取失败时也要回写一份可用配置，否则每次启动都会重复报警。
    let mut rewrite = false;
    let mut raw = if path.is_file() {
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<RawConfig>(&text) {
                Ok(raw) => raw,
                Err(e) => {
                    warnings.push(format!(
                        "配置文件 {} 解析失败：{e}；本次使用默认配置",
                        path.display()
                    ));
                    rewrite = true;
                    default_raw(legacy_portable)
                }
            },
            Err(e) => {
                warnings.push(format!(
                    "配置文件 {} 读取失败：{e}；本次使用默认配置",
                    path.display()
                ));
                rewrite = true;
                default_raw(legacy_portable)
            }
        }
    } else {
        rewrite = true;
        default_raw(legacy_portable)
    };

    // 外观：旧配置没有 theme 时补 auto，并回写文件让设置页有明确的值。
    let theme_missing = raw.theme.is_none();
    let theme = match raw.theme.as_deref() {
        Some(value) => match normalize_theme(value) {
            Ok(theme) => theme,
            Err(msg) => {
                warnings.push(format!("配置项 theme {msg}；本次使用 {DEFAULT_THEME}"));
                rewrite = true;
                DEFAULT_THEME.to_string()
            }
        },
        None => {
            rewrite = true;
            DEFAULT_THEME.to_string()
        }
    };
    raw.theme = Some(theme.clone());

    // v0.1.1 的默认日志等级是 info，且当时没有界面可改。升级到 v0.1.2 时
    // 把这份「旧默认值」同步成新的默认 error；手动改成其它等级的值保持不动。
    if theme_missing && raw.log_level.trim().eq_ignore_ascii_case("info") {
        raw.log_level = DEFAULT_LOG_LEVEL.to_string();
        rewrite = true;
    }

    // 日志等级：非法值降级为默认（error），并回写文件避免每次启动都警告。
    let log_level = match normalize_log_level(&raw.log_level) {
        Ok(level) => level.to_string(),
        Err(msg) => {
            warnings.push(format!(
                "配置项 log_level {msg}；本次使用 {DEFAULT_LOG_LEVEL}"
            ));
            rewrite = true;
            DEFAULT_LOG_LEVEL.to_string()
        }
    };
    raw.log_level = log_level.clone();

    if rewrite {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warnings.push(format!(
                    "无法创建配置目录 {}：{e}；本次使用默认配置",
                    parent.display()
                ));
            } else if let Err(e) = write_config(&path, raw.portable, &theme, &log_level) {
                warnings.push(format!("无法写入配置文件 {}：{e}", path.display()));
            }
        }
    }

    let settings = SettingsInfo {
        portable: raw.portable,
        theme,
        log_level: log_level.clone(),
        conf_path: path.display().to_string(),
        log_dir: log_dir.display().to_string(),
    };

    LoadedConfig {
        log_level: level_filter(&log_level),
        settings,
        path,
        warnings,
    }
}

/// 渲染完整的 conf.toml（导出配置也复用它，保证导出文件可直接拿来用）。
pub fn render_toml(portable: bool, theme: &str, log_level: &str) -> String {
    format!(
        r#"# 泉奈工具箱配置（exe 同目录 conf/conf.toml）
# 是否便携包：true = 数据库放在 exe 同目录 data/；false = 系统应用数据目录
portable = {portable}

# 界面外观：auto = 跟随系统 / light = 白色 / dark = 黑色
theme = "{theme}"

# 日志等级：off / error / warn / info / debug / trace
log_level = "{log_level}"
"#
    )
}

/// 解析导入的 conf.toml（只看外观与日志等级，忽略 portable）。
pub fn parse_import_toml(text: &str) -> Result<(String, String), String> {
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Err("conf.toml 内容为空".into());
    }
    // 空表 / 任意合法 TOML 都能被 RawConfig 的默认值解析成功，
    // 所以先确认它至少包含一个本程序的配置项，避免误导入无关文件。
    let value: toml::Value =
        toml::from_str(trimmed).map_err(|e| format!("conf.toml 解析失败：{e}"))?;
    let table = value
        .as_table()
        .ok_or_else(|| "conf.toml 格式不正确（顶层必须是配置项）".to_string())?;
    if !["theme", "log_level", "portable"]
        .iter()
        .any(|key| table.contains_key(*key))
    {
        return Err("不是泉奈工具箱的 conf.toml（缺少 theme / log_level）".into());
    }
    let raw: RawConfig = toml::from_str(trimmed).map_err(|e| format!("conf.toml 解析失败：{e}"))?;
    let theme = normalize_theme(raw.theme.as_deref().unwrap_or(DEFAULT_THEME))?;
    let log_level = normalize_log_level(&raw.log_level)?.to_string();
    Ok((theme, log_level))
}

fn fallback_settings() -> SettingsInfo {
    SettingsInfo {
        portable: false,
        theme: DEFAULT_THEME.to_string(),
        log_level: DEFAULT_LOG_LEVEL.to_string(),
        conf_path: String::new(),
        log_dir: String::new(),
    }
}

fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_string()
}

fn default_raw(portable: bool) -> RawConfig {
    RawConfig {
        portable,
        log_level: DEFAULT_LOG_LEVEL.to_string(),
        theme: Some(DEFAULT_THEME.to_string()),
    }
}

fn write_config(path: &Path, portable: bool, theme: &str, log_level: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建配置目录 {} 失败：{e}", parent.display()))?;
    }
    std::fs::write(path, render_toml(portable, theme, log_level))
        .map_err(|e| format!("写入配置文件 {} 失败：{e}", path.display()))
}

/// 校验并规范化外观取值。
pub fn normalize_theme(value: &str) -> Result<String, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok("auto".into()),
        "light" => Ok("light".into()),
        "dark" => Ok("dark".into()),
        other => Err(format!("取值无效：{other}（可用 auto/light/dark）")),
    }
}

/// 校验并规范化日志等级（`warning` 作为 `warn` 的别名保留）。
pub fn normalize_log_level(value: &str) -> Result<&'static str, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Ok("off"),
        "error" => Ok("error"),
        "warn" | "warning" => Ok("warn"),
        "info" => Ok("info"),
        "debug" => Ok("debug"),
        "trace" => Ok("trace"),
        other => Err(format!(
            "取值无效：{other}（可用 off/error/warn/info/debug/trace）"
        )),
    }
}

/// 规范化后的等级字符串 → `log::LevelFilter`。
pub fn level_filter(value: &str) -> log::LevelFilter {
    match value {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "info" => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Error,
    }
}
