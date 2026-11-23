//! 应用启动配置：exe 同目录 `conf/conf.toml`。
//!
//! 只在启动时读取一次。文件缺失时按默认值生成；默认便携值参考旧的
//! `portable.flag`，保证老便携包升级后仍能找到原来的 `data/` 数据库。
//! 之后一律以配置文件为准。

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const CONF_DIR: &str = "conf";
pub const CONF_FILE: &str = "conf.toml";
pub const DATA_DIR: &str = "data";
pub const LOGS_DIR: &str = "logs";

/// 旧版便携标记，仅用于在首次生成 conf.toml 时推断默认值。
const LEGACY_PORTABLE_FLAG: &str = "portable.flag";
const DEFAULT_LOG_LEVEL: &str = "info";

#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
    #[serde(default)]
    portable: bool,
    #[serde(default = "default_log_level")]
    log_level: String,
}

pub struct LoadedConfig {
    pub portable: bool,
    pub log_level: log::LevelFilter,
    pub path: PathBuf,
    pub warnings: Vec<String>,
}

/// 读取配置；任何异常都降级为默认值，并把提示收集起来等 logger 就绪后输出。
pub fn load(exe_dir: &Path) -> LoadedConfig {
    let path = exe_dir.join(CONF_DIR).join(CONF_FILE);
    let mut warnings = Vec::new();
    let legacy_portable = exe_dir.join(LEGACY_PORTABLE_FLAG).is_file();

    let raw = if path.is_file() {
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<RawConfig>(&text) {
                Ok(raw) => raw,
                Err(e) => {
                    warnings.push(format!(
                        "配置文件 {} 解析失败：{e}；本次使用默认配置",
                        path.display()
                    ));
                    default_raw(legacy_portable)
                }
            },
            Err(e) => {
                warnings.push(format!(
                    "配置文件 {} 读取失败：{e}；本次使用默认配置",
                    path.display()
                ));
                default_raw(legacy_portable)
            }
        }
    } else {
        let raw = default_raw(legacy_portable);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warnings.push(format!(
                    "无法创建配置目录 {}：{e}；本次使用默认配置",
                    parent.display()
                ));
            } else if let Err(e) = std::fs::write(&path, default_toml(raw.portable)) {
                warnings.push(format!(
                    "无法写入默认配置文件 {}：{e}；本次使用默认配置",
                    path.display()
                ));
            }
        }
        raw
    };

    let log_level = match parse_level(&raw.log_level) {
        Ok(level) => level,
        Err(msg) => {
            warnings.push(format!(
                "配置项 log_level {msg}；本次使用 {DEFAULT_LOG_LEVEL}"
            ));
            log::LevelFilter::Info
        }
    };

    LoadedConfig {
        portable: raw.portable,
        log_level,
        path,
        warnings,
    }
}

fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_string()
}

fn default_raw(portable: bool) -> RawConfig {
    RawConfig {
        portable,
        log_level: DEFAULT_LOG_LEVEL.to_string(),
    }
}

fn default_toml(portable: bool) -> String {
    format!(
        r#"# 泉奈工具箱配置（exe 同目录 conf/conf.toml）
# 是否便携包：true = 数据库放在 exe 同目录 data/；false = 系统应用数据目录
portable = {portable}

# 日志等级：off / error / warn / info / debug / trace
log_level = "{default}"
"#,
        portable = portable,
        default = DEFAULT_LOG_LEVEL,
    )
}

fn parse_level(value: &str) -> Result<log::LevelFilter, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Ok(log::LevelFilter::Off),
        "error" => Ok(log::LevelFilter::Error),
        "warn" | "warning" => Ok(log::LevelFilter::Warn),
        "info" => Ok(log::LevelFilter::Info),
        "debug" => Ok(log::LevelFilter::Debug),
        "trace" => Ok(log::LevelFilter::Trace),
        other => Err(format!(
            "取值无效：{other}（可用 off/error/warn/info/debug/trace）"
        )),
    }
}
