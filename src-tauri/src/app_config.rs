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
/// 本地私有凭据（B 站登录 Cookie 等）统一存这里，绝不写进 conf.toml：
/// 备份导出/导入只处理 conf.toml，这样导出的配置不会把账号登录态带出去。
pub const AUTH_FILE: &str = "auth.toml";
/// 旧版单用途文件，仅用于启动时迁移到 auth.toml 后删除。
const LEGACY_BILI_COOKIE_FILE: &str = "bili_cookie.txt";

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
    /// B 站登录 uid。>0 来自 DedeUserID；0 可能是游客，也可能是只有 SESSDATA 的登录态，
    /// 判断是否登录请统一看 bili_logged_in。
    pub bili_uid: i64,
    /// 是否已配置有效的 B 站登录态（存在 SESSDATA）。界面用它显示登录状态。
    pub bili_logged_in: bool,
}

/// 本地私有凭据文件：conf/auth.toml 的内容。用 TOML 表结构，方便以后扩展
/// （其它不能导出的密钥加一个表即可），与 conf.toml 的备份链路完全隔离。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct AuthFile {
    #[serde(default)]
    bili: Option<AuthBili>,
}

/// B 站登录态在 auth.toml 里的 [bili] 段。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct AuthBili {
    uid: i64,
    cookie: String,
}

/// B 站登录态：从用户粘贴的 Cookie 中解析出 uid 与可用于 WebSocket 的 Cookie 头。
/// 只存在内存与 `conf/auth.toml`，不进 conf.toml、不写日志、不参与备份。
#[derive(Debug, Clone)]
pub struct BiliAuth {
    /// 登录账号 uid（来自 DedeUserID，仅用于界面展示/连接签名）
    pub uid: i64,
    /// 用户粘贴的完整 Cookie（原样存档，便于以后扩展）
    pub cookie: String,
    /// 从完整 Cookie 中提取的 SESSDATA 值。
    /// 弹幕 WebSocket 的 Cookie 头只发 `SESSDATA` 与 `buvid3` 两个字段（对齐 blivedm），
    /// 其余字段不发：sid / buvid4 等与鉴权包字段对不上时服务端会直接 RST。
    pub sessdata: String,
    /// 同一份 Cookie 里的 `buvid3`：与 SESSDATA 配套的设备标识，鉴权包的 `buvid`
    /// 必须用它（不能用另取的游客 buvid），缺失时鉴权包不带 buvid。
    pub buvid3: Option<String>,
}

pub struct LoadedConfig {
    pub settings: SettingsInfo,
    pub log_level: log::LevelFilter,
    pub path: PathBuf,
    pub warnings: Vec<String>,
    /// 启动时从 conf/auth.toml 读到的登录态（解析失败则为 None）
    pub bili_auth: Option<BiliAuth>,
}

/// 运行时配置状态：设置页读改写都走它，保证内存与 conf.toml 一致。
pub struct SettingsState {
    inner: Mutex<SettingsInfo>,
    /// 登录态单独存放：改 Cookie 不需要重写 conf.toml。
    bili_auth: Mutex<Option<BiliAuth>>,
}

impl SettingsState {
    pub fn new(settings: SettingsInfo, bili_auth: Option<BiliAuth>) -> Self {
        Self {
            inner: Mutex::new(settings),
            bili_auth: Mutex::new(bili_auth),
        }
    }

    pub fn get(&self) -> SettingsInfo {
        self.inner
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| fallback_settings())
    }

    /// 供弹幕连接读取当前登录态。绝不打印 cookie 内容。
    pub fn bili_auth(&self) -> Option<BiliAuth> {
        self.bili_auth
            .lock()
            .ok()
            .and_then(|auth| auth.clone())
    }

    /// 保存登录 Cookie：解析 uid 与 Cookie 头 → 写独立文件 → 更新内存。
    /// 返回的 SettingsInfo 里 bili_uid > 0 表示登录成功。
    pub fn set_bili_cookie(&self, raw: &str) -> Result<SettingsInfo, String> {
        let auth = parse_bili_cookie(raw)?;
        let path = auth_file_path(&self.get().conf_path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建配置目录失败：{e}"))?;
        }
        // 保留 auth.toml 里其它私有段（若以后扩展），只覆盖 [bili]。
        let mut file = read_auth_file(&path).unwrap_or_default();
        file.bili = Some(AuthBili {
            uid: auth.uid,
            cookie: auth.cookie.clone(),
        });
        write_auth_file(&path, &file)?;
        if let Ok(mut guard) = self.bili_auth.lock() {
            *guard = Some(auth.clone());
        }
        if let Ok(mut guard) = self.inner.lock() {
            guard.bili_uid = auth.uid;
            guard.bili_logged_in = true;
        }
        log::info!(
            "[B站登录] 已保存 Cookie：uid={}，字段={:?}，SESSDATA长度={}（不会记录 Cookie 明文），凭据文件={}",
            auth.uid,
            cookie_field_names(&auth.cookie),
            auth.sessdata.len(),
            path.display()
        );
        log::info!("[B站登录] 保存成功仅代表字段解析成功；弹幕重连后会请求账号接口核对 Cookie 是否仍有效");
        Ok(self.get())
    }

    /// 清除登录 Cookie：删文件 + 清空内存，队列回退到游客连接。
    pub fn clear_bili_cookie(&self) -> Result<SettingsInfo, String> {
        let path = auth_file_path(&self.get().conf_path)?;
        if path.exists() {
            // 只清掉 [bili] 段，保留 auth.toml 里其它私有凭据。
            let mut file = read_auth_file(&path).unwrap_or_default();
            file.bili = None;
            write_auth_file(&path, &file)?;
        }
        if let Ok(mut guard) = self.bili_auth.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.inner.lock() {
            guard.bili_uid = 0;
            guard.bili_logged_in = false;
        }
        log::info!("[B站登录] 已清除登录态，队列将使用游客连接");
        Ok(self.get())
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

    // B 站登录态：独立 auth.toml，解析失败只降级为游客，不让应用启动失败。
    // Cookie 内容绝不进日志，只记录成功/失败的 uid 或原因。
    let bili_auth = load_bili_auth(&path, &mut warnings);

    let settings = SettingsInfo {
        portable: raw.portable,
        theme,
        log_level: log_level.clone(),
        conf_path: path.display().to_string(),
        log_dir: log_dir.display().to_string(),
        bili_uid: bili_auth.as_ref().map(|a| a.uid).unwrap_or(0),
        bili_logged_in: bili_auth.is_some(),
    };

    LoadedConfig {
        log_level: level_filter(&log_level),
        settings,
        path,
        warnings,
        bili_auth,
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
        bili_uid: 0,
        bili_logged_in: false,
    }
}

/// conf.toml 同目录下的 auth.toml 路径。
fn auth_file_path(conf_path: &str) -> Result<PathBuf, String> {
    if conf_path.trim().is_empty() {
        return Err("无法确定配置目录，不能保存本地凭据".into());
    }
    Ok(Path::new(conf_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(AUTH_FILE))
}

/// 读 auth.toml；文件不存在 / 解析失败都当空处理（凭据坏了不该阻断启动）。
fn read_auth_file(path: &Path) -> Option<AuthFile> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str::<AuthFile>(&text).ok()
}

/// 写 auth.toml：带说明头，明确这是不会被备份的私有凭据。
fn write_auth_file(path: &Path, file: &AuthFile) -> Result<(), String> {
    let body = toml::to_string_pretty(file).map_err(|e| format!("序列化本地凭据失败：{e}"))?;
    let text = format!(
        "# 泉奈工具箱本地私有凭据\n\
         # 此文件不会被导出/备份，也不要发给任何人；Cookie 等同 B 站账号登录态。\n\
         # 可在设置页 → 开发者选项 → B 站登录 中修改。\n\n{body}"
    );
    std::fs::write(path, text).map_err(|e| format!("写入本地凭据失败：{e}"))
}

/// 启动时加载登录态，并顺手把旧版 bili_cookie.txt 迁移到 auth.toml 后删除。
fn load_bili_auth(conf_path: &Path, warnings: &mut Vec<String>) -> Option<BiliAuth> {
    let dir = conf_path.parent().unwrap_or_else(|| Path::new("."));
    let auth_path = dir.join(AUTH_FILE);
    let legacy_path = dir.join(LEGACY_BILI_COOKIE_FILE);

    // 1. 旧文件迁移：读入 → 写进 auth.toml 的 [bili] 段 → 删除旧文件。
    if legacy_path.is_file() && !auth_path.is_file() {
        if let Ok(text) = std::fs::read_to_string(&legacy_path) {
            if let Ok(auth) = parse_bili_cookie(&text) {
                let mut file = read_auth_file(&auth_path).unwrap_or_default();
                file.bili = Some(AuthBili {
                    uid: auth.uid,
                    cookie: auth.cookie.clone(),
                });
                if let Err(e) = write_auth_file(&auth_path, &file) {
                    warnings.push(format!("迁移旧版 B 站 Cookie 失败：{e}"));
                } else {
                    let _ = std::fs::remove_file(&legacy_path);
                    log::info!("[B站登录] 已从 bili_cookie.txt 迁移到 auth.toml");
                }
            }
        }
    }

    // 2. 从 auth.toml 的 [bili] 段恢复。
    let file = read_auth_file(&auth_path)?;
    let bili = file.bili?;
    // 文件里存的是完整 Cookie，仍走同一套校验，确保 uid/SESSDATA 有效。
    match parse_bili_cookie(&bili.cookie) {
        Ok(auth) => {
            log::info!("[B站登录] 已加载登录态 uid={}", auth.uid);
            Some(auth)
        }
        Err(e) => {
            warnings.push(format!("auth.toml 里的 B 站登录 Cookie 无效，使用游客连接：{e}"));
            None
        }
    }
}

/// 只提取 Cookie 字段名用于诊断；字段值（尤其 SESSDATA）绝不进入日志。
fn cookie_field_names(cookie: &str) -> Vec<&str> {
    let mut names: Vec<&str> = cookie
        .split(';')
        .filter_map(|pair| pair.split_once('=').map(|(key, _)| key.trim()))
        .filter(|key| !key.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// 从浏览器复制的 Cookie 字符串里解析出 uid 与弹幕连接可用的 Cookie 头。
///
/// 必需字段：
/// - `DedeUserID`：登录 uid（>0）；游客连接时服务端看到的 uid=0。
/// - `SESSDATA`：登录凭证；缺失说明不是登录态。
///
/// 整个 Cookie 原样保存（含 buvid3/buvid4/bili_jct/sid 等），因为 B 站各接口
/// 校验的字段会变，原样透传最稳妥。这里只做「是否登录」的最小校验。
fn parse_bili_cookie(raw: &str) -> Result<BiliAuth, String> {
    // 去掉首尾空白、BOM，以及用户连 "Cookie: " 一起复制时带的前缀。
    let mut cookie = raw.trim().trim_start_matches('\u{feff}').trim().to_string();
    if cookie.len() >= 7 && cookie[..7].eq_ignore_ascii_case("cookie:") {
        cookie = cookie[7..].trim().to_string();
    }
    if cookie.is_empty() {
        return Err("Cookie 为空，请从已登录的 B 站页面复制完整 Cookie".into());
    }
    let mut uid: Option<i64> = None;
    let mut sessdata = String::new();
    let mut buvid3: Option<String> = None;
    for pair in cookie.split(';') {
        let pair = pair.trim();
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        // 值可能带引号（从某些抓包工具复制时会出现），一并去掉。
        let value = value.trim().trim_matches('"').trim();
        match key.trim() {
            "DedeUserID" => {
                uid = value.parse::<i64>().ok().filter(|id| *id > 0);
            }
            "SESSDATA" => {
                if !value.is_empty() {
                    sessdata = value.to_string();
                }
            }
            "buvid3" => {
                if !value.is_empty() {
                    buvid3 = Some(value.to_string());
                }
            }
            _ => {}
        }
    }
    if sessdata.is_empty() {
        return Err("Cookie 中缺少 SESSDATA，请先登录 B 站再复制完整 Cookie".into());
    }
    // DedeUserID 只用于界面展示和连接签名，缺失也能登录：真实 uid 由服务端
    // 在鉴权成功后随弹幕下发，不依赖它。缺失时用 0（游客显示）不影响计数。
    let uid = uid.unwrap_or(0);
    Ok(BiliAuth {
        uid,
        cookie,
        sessdata,
        buvid3,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bili_cookie_extracts_uid_and_keeps_full_cookie() {
        let raw = "buvid3=ABC; DedeUserID=123456; SESSDATA=abc%2Fdef; bili_jct=token; sid=xyz";
        let auth = parse_bili_cookie(raw).unwrap();
        assert_eq!(auth.uid, 123456);
        assert_eq!(auth.sessdata, "abc%2Fdef");
        assert_eq!(auth.buvid3.as_deref(), Some("ABC"));
        assert_eq!(auth.cookie, raw);
    }

    #[test]
    fn parse_bili_cookie_rejects_guest_or_incomplete_cookie() {
        // 游客 Cookie：没有 SESSDATA
        assert!(parse_bili_cookie("buvid3=ABC; bili_jct=token").is_err());
        // 缺 SESSDATA
        assert!(parse_bili_cookie("DedeUserID=123").is_err());
        // 空
        assert!(parse_bili_cookie("   ").is_err());
    }

    #[test]
    fn parse_bili_cookie_without_dedeuserid_still_logs_in() {
        // 有些环境复制到的 Cookie 只有 SESSDATA，没有 DedeUserID；
        // 真实 uid 由服务端下发，本地 uid=0 只影响显示，不能因此拒绝登录。
        let auth = parse_bili_cookie("SESSDATA=only_sess; buvid3=ABC").unwrap();
        assert_eq!(auth.uid, 0);
        assert_eq!(auth.sessdata, "only_sess");
        // uid 字段非法时同样降级为 0，而不是报「缺少 DedeUserID」
        assert_eq!(
            parse_bili_cookie("DedeUserID=abc; SESSDATA=x")
                .unwrap()
                .uid,
            0
        );
    }

    #[test]
    fn parse_bili_cookie_tolerates_cookie_prefix_and_quotes() {
        // 用户从 DevTools 连 "Cookie: " 一起复制、值带引号的情况
        let raw = "Cookie: buvid3=ABC; DedeUserID=\"123456\"; SESSDATA=\"abc%2Fdef\"";
        let auth = parse_bili_cookie(raw).unwrap();
        assert_eq!(auth.uid, 123456);
        assert_eq!(auth.cookie, "buvid3=ABC; DedeUserID=\"123456\"; SESSDATA=\"abc%2Fdef\"");
        // 大小写前缀
        assert!(parse_bili_cookie("cookie: DedeUserID=1; SESSDATA=x").is_ok());
    }

    #[test]
    fn auth_toml_roundtrip_keeps_bili_section() {
        let dir = std::env::temp_dir().join(format!("izuna-auth-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(AUTH_FILE);
        let file = AuthFile {
            bili: Some(AuthBili {
                uid: 42,
                cookie: "DedeUserID=42; SESSDATA=abc".into(),
            }),
        };
        write_auth_file(&path, &file).unwrap();
        let loaded = read_auth_file(&path).unwrap();
        assert_eq!(loaded.bili.as_ref().map(|b| b.uid), Some(42));
        let recovered = load_bili_auth(&dir.join("conf.toml"), &mut Vec::new()).unwrap();
        assert_eq!(recovered.uid, 42);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
