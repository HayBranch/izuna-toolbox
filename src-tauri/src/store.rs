//! 持久化：SQLite（`rusqlite` bundled，SQLite 源码静态编译进程序）。
//!
//! 表结构：
//! - `meta`      键值表（账号等杂项）
//! - `variables` 用户自定义变量（A1、A2…）：名字 + 数值 + 步长，主界面可 −/＋
//! - `computed`  计算变量（B1、B2…）：表达式（如 `A1+A2`）+ 格式（普通数/百分比）+ 小数位
//! - `overlays`  悬浮字幕配置队列：可多个，含名字、创建时间等基本信息
//!
//! 并发模型：`Connection` 是 `Send` 但非 `Sync`，用 `Mutex` 包一层挂进 Tauri 状态。
//! 每次变更返回最新全量 `Config`，由命令层广播给所有窗口（单一数据源）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 用户自定义变量（手动维护数值的"框框"）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserVar {
    pub id: i64,
    /// 模板/表达式里引用的名字，如 `A1`
    pub key: String,
    /// 界面显示名，如"人机场次"
    pub name: String,
    pub value: f64,
    /// 每按一次 −/＋ 增减多少
    pub step: f64,
    pub created_at: i64,
}

/// 计算变量：由表达式对其它变量求值得到，只读。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComputedVar {
    pub id: i64,
    pub key: String,
    pub name: String,
    /// 如 `A1 + A2`、`A4 / (A1 + A2)`
    pub expr: String,
    /// `plain` 普通数 | `percent` 百分比（0.8 → "80%"）
    pub format: String,
    /// 小数点位数 0-6
    pub decimals: i64,
    pub created_at: i64,
}

/// 单个悬浮字幕浮窗的配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlayConfig {
    pub id: i64,
    pub name: String,
    /// 模板文本，`{today}` `{A1}` 这类占位符由前端渲染时替换
    pub template: String,
    /// 是否显示（隐藏 ≠ 删除，配置保留）
    pub shown: bool,
    /// 窗口置顶
    pub always_on_top: bool,
    /// 鼠标穿透：开启后浮窗不接收任何鼠标事件，游戏点击不会被打断
    pub click_through: bool,
    /// 窗口位置（物理像素，拖动时由 Moved 事件回写）
    pub x: i32,
    pub y: i32,
    pub font_size: u32,
    /// #RRGGBB
    pub font_color: String,
    pub bold: bool,
    /// 文字描边阴影（直播场景下更易读）
    pub shadow: bool,
    /// #RRGGBB
    pub bg_color: String,
    /// 背景不透明度 0-100（0 = 全透明）
    pub bg_opacity: u32,
    /// 创建时间（毫秒时间戳）
    pub created_at: i64,
}

/// 全量配置（对前端 `src/types.ts` 里的 `AppConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub account: String,
    pub variables: Vec<UserVar>,
    pub computed: Vec<ComputedVar>,
    pub overlays: Vec<OverlayConfig>,
}

/// 默认模板：示范变量引用与计算变量的用法，可随意改。
pub const DEFAULT_TEMPLATE: &str = "今日{today}，账号：{account}，总场次：{B1}，人机：{A1}，真人：{A2}，人机胜：{A3}，真人胜：{A4}，总胜率：{B3}，真实胜率：{B4}";

pub struct Store {
    conn: Mutex<Connection>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

impl Store {
    /// 打开数据库、建表；首次使用时种入示例数据，并尝试从旧版 config.json 迁移。
    pub fn new(db_path: PathBuf) -> Result<Self, String> {
        let conn = Connection::open(&db_path).map_err(|e| format!("打开数据库失败：{e}"))?;
        // WAL：写入更快、读写不互斥，桌面应用常规配置
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS variables (
                id         INTEGER PRIMARY KEY,
                key        TEXT NOT NULL UNIQUE,
                name       TEXT NOT NULL,
                value      REAL NOT NULL DEFAULT 0,
                step       REAL NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS computed (
                id         INTEGER PRIMARY KEY,
                key        TEXT NOT NULL UNIQUE,
                name       TEXT NOT NULL,
                expr       TEXT NOT NULL,
                format     TEXT NOT NULL DEFAULT 'plain',
                decimals   INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS overlays (
                id             INTEGER PRIMARY KEY,
                name           TEXT NOT NULL,
                template       TEXT NOT NULL,
                shown          INTEGER NOT NULL DEFAULT 1,
                always_on_top  INTEGER NOT NULL DEFAULT 1,
                click_through  INTEGER NOT NULL DEFAULT 0,
                x              INTEGER NOT NULL DEFAULT 0,
                y              INTEGER NOT NULL DEFAULT 0,
                font_size      INTEGER NOT NULL DEFAULT 20,
                font_color     TEXT NOT NULL DEFAULT '#ffffff',
                bold           INTEGER NOT NULL DEFAULT 0,
                shadow         INTEGER NOT NULL DEFAULT 1,
                bg_color       TEXT NOT NULL DEFAULT '#0f172a',
                bg_opacity     INTEGER NOT NULL DEFAULT 55,
                created_at     INTEGER NOT NULL
            );",
        )
        .map_err(|e| format!("建表失败：{e}"))?;

        let store = Self { conn: Mutex::new(conn) };
        store.seed_or_migrate(&db_path)?;
        Ok(store)
    }

    /// 数据库为空时：先尝试从旧版 config.json 迁移；没有旧数据就种入示例。
    fn seed_or_migrate(&self, db_path: &Path) -> Result<(), String> {
        let fresh = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            let vars: i64 = conn
                .query_row("SELECT COUNT(*) FROM variables", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let overlays: i64 = conn
                .query_row("SELECT COUNT(*) FROM overlays", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            vars == 0 && overlays == 0
        };
        if !fresh {
            return Ok(());
        }

        let data_dir = db_path.parent();
        if let Some(dir) = data_dir {
            if self.migrate_legacy_json(dir)? {
                return Ok(());
            }
        }

        // ---------- 首次运行的示例数据（对应用户给的例子） ----------
        let now = now_ms();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let seed_vars = [
            ("A1", "人机场次", 6.0),
            ("A2", "真人场次", 2.0),
            ("A3", "人机胜场", 6.0),
            ("A4", "真人胜场", 1.0),
        ];
        for (i, (key, name, value)) in seed_vars.iter().enumerate() {
            conn.execute(
                "INSERT INTO variables (key, name, value, step, created_at) VALUES (?1, ?2, ?3, 1, ?4)",
                params![key, name, value, now + i as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        let seed_computed = [
            ("B1", "总场次", "A1 + A2", "plain", 0),
            ("B2", "总胜场", "A3 + A4", "plain", 0),
            ("B3", "总胜率", "B2 / B1", "percent", 1),
            ("B4", "真实胜率", "A4 / B1", "percent", 1),
        ];
        for (i, (key, name, expr, format, decimals)) in seed_computed.iter().enumerate() {
            conn.execute(
                "INSERT INTO computed (key, name, expr, format, decimals, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![key, name, expr, format, decimals, now + i as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        let overlay = default_overlay(1, now);
        conn.execute(
            "INSERT INTO overlays (id, name, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                overlay.id, overlay.name, overlay.template,
                overlay.shown as i64, overlay.always_on_top as i64, overlay.click_through as i64,
                overlay.x, overlay.y, overlay.font_size as i64, overlay.font_color,
                overlay.bold as i64, overlay.shadow as i64, overlay.bg_color, overlay.bg_opacity as i64,
                overlay.created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 旧版（JSON 存储）一次性迁移：成功返回 true。数据搬完把旧文件改名备份。
    fn migrate_legacy_json(&self, dir: &Path) -> Result<bool, String> {
        let legacy = dir.join("config.json");
        let Ok(text) = fs::read_to_string(&legacy) else {
            return Ok(false);
        };
        // 旧 schema 的最小解析（缺字段就放弃迁移，走示例种子）
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Ok(false);
        };
        let (Some(counters), Some(overlays)) = (v.get("counters"), v.get("overlays")) else {
            return Ok(false);
        };

        let now = now_ms();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let seed = [
            ("A1", "人机场次", "botGames"),
            ("A2", "真人场次", "realGames"),
            ("A3", "人机胜场", "botWins"),
            ("A4", "真人胜场", "realWins"),
        ];
        for (i, (key, name, old_field)) in seed.iter().enumerate() {
            let value = counters.get(*old_field).and_then(|x| x.as_f64()).unwrap_or(0.0);
            conn.execute(
                "INSERT INTO variables (key, name, value, step, created_at) VALUES (?1, ?2, ?3, 1, ?4)",
                params![key, name, value, now + i as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        for (i, o) in overlays.as_array().unwrap_or(&vec![]).iter().enumerate() {
            let get_str = |f: &str| o.get(f).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let get_bool = |f: &str| o.get(f).and_then(|x| x.as_bool()).unwrap_or(false);
            let get_i64 = |f: &str| o.get(f).and_then(|x| x.as_i64()).unwrap_or(0);
            conn.execute(
                "INSERT INTO overlays (id, name, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    get_i64("id").max((i + 1) as i64), get_str("name"), get_str("template"),
                    get_bool("shown") as i64, get_bool("alwaysOnTop") as i64, get_bool("clickThrough") as i64,
                    get_i64("x"), get_i64("y"), get_i64("fontSize"), get_str("fontColor"),
                    get_bool("bold") as i64, get_bool("shadow") as i64, get_str("bgColor"), get_i64("bgOpacity"),
                    now + i as i64
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        if let Some(account) = v.get("account").and_then(|x| x.as_str()) {
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('account', ?1)",
                params![account],
            )
            .map_err(|e| e.to_string())?;
        }
        drop(conn);
        let _ = std::fs::rename(&legacy, dir.join("config.json.migrated"));
        Ok(true)
    }

    // ---------- 组装全量配置 ----------

    pub fn get(&self) -> Config {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("数据库加锁失败：{e}");
                return Config::default();
            }
        };
        let account = conn
            .query_row("SELECT value FROM meta WHERE key = 'account'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .ok()
            .flatten()
            .unwrap_or_default();

        let variables = Self::load_variables(&conn);
        let computed = Self::load_computed(&conn);
        let overlays = Self::load_overlays(&conn);
        Config { account, variables, computed, overlays }
    }

    fn load_variables(conn: &Connection) -> Vec<UserVar> {
        let mut stmt = match conn.prepare("SELECT id, key, name, value, step, created_at FROM variables ORDER BY id") {
            Ok(s) => s,
            Err(e) => { eprintln!("查询 variables 失败：{e}"); return vec![]; }
        };
        stmt.query_map([], |row| {
            Ok(UserVar {
                id: row.get(0)?,
                key: row.get(1)?,
                name: row.get(2)?,
                value: row.get(3)?,
                step: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_else(|e| { eprintln!("读取 variables 失败：{e}"); vec![] })
    }

    fn load_computed(conn: &Connection) -> Vec<ComputedVar> {
        let mut stmt = match conn.prepare("SELECT id, key, name, expr, format, decimals, created_at FROM computed ORDER BY id") {
            Ok(s) => s,
            Err(e) => { eprintln!("查询 computed 失败：{e}"); return vec![]; }
        };
        stmt.query_map([], |row| {
            Ok(ComputedVar {
                id: row.get(0)?,
                key: row.get(1)?,
                name: row.get(2)?,
                expr: row.get(3)?,
                format: row.get(4)?,
                decimals: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_else(|e| { eprintln!("读取 computed 失败：{e}"); vec![] })
    }

    fn load_overlays(conn: &Connection) -> Vec<OverlayConfig> {
        let mut stmt = match conn.prepare(
            "SELECT id, name, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, created_at FROM overlays ORDER BY id",
        ) {
            Ok(s) => s,
            Err(e) => { eprintln!("查询 overlays 失败：{e}"); return vec![]; }
        };
        stmt.query_map([], |row| {
            Ok(OverlayConfig {
                id: row.get(0)?,
                name: row.get(1)?,
                template: row.get(2)?,
                shown: row.get::<_, i64>(3)? != 0,
                always_on_top: row.get::<_, i64>(4)? != 0,
                click_through: row.get::<_, i64>(5)? != 0,
                x: row.get(6)?,
                y: row.get(7)?,
                font_size: row.get::<_, i64>(8)? as u32,
                font_color: row.get(9)?,
                bold: row.get::<_, i64>(10)? != 0,
                shadow: row.get::<_, i64>(11)? != 0,
                bg_color: row.get(12)?,
                bg_opacity: row.get::<_, i64>(13)? as u32,
                created_at: row.get(14)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_else(|e| { eprintln!("读取 overlays 失败：{e}"); vec![] })
    }

    // ---------- 账号 ----------

    pub fn set_account(&self, account: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('account', ?1)",
            params![account.trim()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---------- 自定义变量 ----------

    /// 校验变量 key：字母开头，只含字母/数字/下划线，且不能撞内置变量。
    pub fn validate_key(key: &str) -> Result<(), String> {
        let mut chars = key.chars();
        let ok = key.len() >= 1
            && key.len() <= 32
            && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !matches!(key, "today" | "date" | "time" | "account");
        if ok {
            Ok(())
        } else {
            Err(format!("变量名 `{key}` 不合法：字母开头，只含字母/数字/下划线，且不能用 today/date/time/account"))
        }
    }

    /// 生成下一个可用 key：扫描现有 `A{n}` 取最大 n+1（computed 用 B 前缀）。
    fn next_key(conn: &Connection, prefix: &str) -> Result<String, String> {
        let mut stmt = conn
            .prepare(&format!("SELECT key FROM {table}", table = if prefix == "A" { "variables" } else { "computed" }))
            .map_err(|e| e.to_string())?;
        let keys: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        let max = keys
            .iter()
            .filter_map(|k| k.strip_prefix(prefix))
            .filter_map(|s| s.parse::<i64>().ok())
            .max()
            .unwrap_or(0);
        Ok(format!("{prefix}{}", max + 1))
    }

    pub fn add_variable(
        &self,
        key: Option<String>,
        name: &str,
        value: f64,
        step: f64,
    ) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("变量显示名不能为空".into());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let key = match key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => {
                Self::validate_key(k)?;
                k.to_string()
            }
            None => Self::next_key(&conn, "A")?,
        };
        conn.execute(
            "INSERT INTO variables (key, name, value, step, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, name.trim(), value, if step > 0.0 { step } else { 1.0 }, now_ms()],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                format!("变量名 `{key}` 已存在")
            } else {
                e.to_string()
            }
        })?;
        Ok(())
    }

    pub fn update_variable(&self, var: &UserVar) -> Result<(), String> {
        Self::validate_key(&var.key)?;
        let name = var.name.trim();
        if name.is_empty() {
            return Err("变量显示名不能为空".into());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE variables SET key = ?1, name = ?2, value = ?3, step = ?4 WHERE id = ?5",
                params![var.key, name, var.value, if var.step > 0.0 { var.step } else { 1.0 }, var.id],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    format!("变量名 `{}` 已存在", var.key)
                } else {
                    e.to_string()
                }
            })?;
        if changed == 0 {
            return Err(format!("变量 #{} 不存在", var.id));
        }
        Ok(())
    }

    /// 计数器语义：不允许减到负数。
    pub fn step_variable(&self, id: i64, delta: f64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE variables SET value = MAX(0, value + ?1) WHERE id = ?2",
                params![delta, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("变量 #{id} 不存在"));
        }
        Ok(())
    }

    pub fn delete_variable(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM variables WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---------- 计算变量 ----------

    pub fn add_computed(
        &self,
        key: Option<String>,
        name: &str,
        expr: &str,
        format: &str,
        decimals: i64,
    ) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("名称不能为空".into());
        }
        let expr = expr.trim();
        if expr.is_empty() {
            return Err("表达式不能为空".into());
        }
        let format = if format == "percent" { "percent" } else { "plain" };
        let decimals = decimals.clamp(0, 6);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let key = match key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => {
                Self::validate_key(k)?;
                k.to_string()
            }
            None => Self::next_key(&conn, "B")?,
        };
        conn.execute(
            "INSERT INTO computed (key, name, expr, format, decimals, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![key, name, expr, format, decimals, now_ms()],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                format!("变量名 `{key}` 已存在")
            } else {
                e.to_string()
            }
        })?;
        Ok(())
    }

    pub fn update_computed(&self, var: &ComputedVar) -> Result<(), String> {
        Self::validate_key(&var.key)?;
        let name = var.name.trim();
        if name.is_empty() {
            return Err("名称不能为空".into());
        }
        let expr = var.expr.trim();
        if expr.is_empty() {
            return Err("表达式不能为空".into());
        }
        let format = if var.format == "percent" { "percent" } else { "plain" };
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE computed SET key = ?1, name = ?2, expr = ?3, format = ?4, decimals = ?5 WHERE id = ?6",
                params![var.key, name, expr, format, var.decimals.clamp(0, 6), var.id],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    format!("变量名 `{}` 已存在", var.key)
                } else {
                    e.to_string()
                }
            })?;
        if changed == 0 {
            return Err(format!("计算变量 #{} 不存在", var.id));
        }
        Ok(())
    }

    pub fn delete_computed(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM computed WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---------- 浮窗 ----------

    /// 新浮窗 id：毫秒时间戳与现有最大 id+1 取大，保证单调唯一。
    pub fn next_overlay_id(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        let max: i64 = conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM overlays", [], |r| r.get(0))
            .unwrap_or(0);
        now_ms().max(max + 1)
    }

    pub fn push_overlay(&self, cfg: &OverlayConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO overlays (id, name, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                cfg.id, cfg.name, cfg.template,
                cfg.shown as i64, cfg.always_on_top as i64, cfg.click_through as i64,
                cfg.x, cfg.y, cfg.font_size as i64, cfg.font_color,
                cfg.bold as i64, cfg.shadow as i64, cfg.bg_color, cfg.bg_opacity as i64,
                cfg.created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 更新浮窗的名称/模板/样式（**有意不更新 x/y**——位置只由拖动回写和 move_overlay 维护，
    /// 避免主窗口表单里过期的坐标把用户刚拖好的位置弹回去）。
    pub fn update_overlay(&self, cfg: &OverlayConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE overlays SET name = ?1, template = ?2, font_size = ?3, font_color = ?4, bold = ?5, shadow = ?6, bg_color = ?7, bg_opacity = ?8 WHERE id = ?9",
                params![
                    cfg.name, cfg.template, cfg.font_size as i64, cfg.font_color,
                    cfg.bold as i64, cfg.shadow as i64, cfg.bg_color, cfg.bg_opacity as i64, cfg.id
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("浮窗 #{} 不存在", cfg.id));
        }
        Ok(())
    }

    pub fn move_overlay(&self, id: i64, x: i32, y: i32) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute("UPDATE overlays SET x = ?1, y = ?2 WHERE id = ?3", params![x, y, id])
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("浮窗 #{id} 不存在"));
        }
        Ok(())
    }

    pub fn set_flag(&self, id: i64, apply: impl FnOnce(&mut OverlayConfig)) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let overlays = Self::load_overlays(&conn);
        let mut cfg = overlays
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| format!("浮窗 #{id} 不存在"))?;
        apply(&mut cfg);
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE overlays SET shown = ?1, always_on_top = ?2, click_through = ?3 WHERE id = ?4",
            params![cfg.shown as i64, cfg.always_on_top as i64, cfg.click_through as i64, id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_overlay(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM overlays WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 拖动浮窗时由 Moved 事件回写位置。
    pub fn set_position(&self, id: i64, x: i32, y: i32) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("UPDATE overlays SET x = ?1, y = ?2 WHERE id = ?3", params![x, y, id]);
    }
}

/// 新浮窗的默认配置（位置 x/y 由命令层根据主显示器计算后填入）。
pub fn default_overlay(id: i64, created_at: i64) -> OverlayConfig {
    OverlayConfig {
        id,
        name: "战绩浮窗".into(),
        template: DEFAULT_TEMPLATE.into(),
        shown: true,
        always_on_top: true,
        click_through: false,
        x: 0,
        y: 0,
        font_size: 20,
        font_color: "#ffffff".into(),
        bold: false,
        shadow: true,
        bg_color: "#0f172a".into(),
        bg_opacity: 55,
        created_at,
    }
}
