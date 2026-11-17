//! 持久化：SQLite（`rusqlite` bundled，SQLite 源码静态编译进程序）。
//!
//! 数据模型：**每个浮窗 = 一套完全独立的配置**——
//! 自己的账号、自定义变量、计算变量、模板与样式，互不影响。
//!
//! 表结构：
//! - `overlays`  浮窗（套）队列：可多个，含名字/创建时间等基本信息 + 每套自己的账号
//! - `variables` 自定义变量，归属某个浮窗（overlay_id）
//! - `computed`  计算变量，归属某个浮窗（overlay_id）
//! - `meta`      杂项键值（保留；旧版全局账号迁移后不再使用）
//!
//! 旧库自动迁移：v1（全局变量、无 account 列）→ 当前结构；
//! 更早的 config.json（JSON 存储）也会在首次启动时自动导入。
//!
//! 并发模型：`Connection` 非 `Sync`，用 `Mutex` 包一层挂进 Tauri 状态。
//! 每次变更返回最新全量 `Config`，由命令层广播给所有窗口（单一数据源）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 用户自定义变量（手动维护数值的"框框"），属于某一浮窗。
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

/// 计算变量：由表达式对**同一浮窗内**的其它变量求值得到，只读。
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

/// 单个浮窗的配置 = 一套独立的数据（含自己的变量）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlayConfig {
    pub id: i64,
    pub name: String,
    /// 本套浮窗自己的账号（模板里的 {account}）
    pub account: String,
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
    /// 自动换行：开启后长行按 `max_width` 折行（模板里的回车始终换行）
    pub wrap: bool,
    /// 自动换行最大宽度（逻辑像素；<=0 表示不限制）
    pub max_width: i64,
    /// 创建时间（毫秒时间戳）
    pub created_at: i64,
    /// 本套自己的自定义变量
    pub variables: Vec<UserVar>,
    /// 本套自己的计算变量
    pub computed: Vec<ComputedVar>,
}

/// 全量配置：浮窗（套）列表。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub overlays: Vec<OverlayConfig>,
}

/// 默认模板：多行排版示范变量引用与计算变量的用法，可随意改（`\n` 即换行）。
pub const DEFAULT_TEMPLATE: &str = "今日{today} · 账号：{account}\n总场次：{B1}（人机 {A1}｜真人 {A2}）\n胜场：{B2}（人机 {A3}｜真人 {A4}）\n总胜率：{B3}｜真实胜率：{B4}";

/// 旧版（第一版全局变量）的变量名 → 新名字。长的先替换，避免前缀误伤。
fn rewrite_legacy_template(template: &str) -> String {
    template
        .replace("{totalWinRatePct}", "{B3}")
        .replace("{realWinRatePct}", "{B4}")
        .replace("{totalWinRate}", "{B3}")
        .replace("{realWinRate}", "{B4}")
        .replace("{totalGames}", "{B1}")
        .replace("{totalWins}", "{B2}")
        .replace("{botGames}", "{A1}")
        .replace("{realGames}", "{A2}")
        .replace("{botWins}", "{A3}")
        .replace("{realWins}", "{A4}")
}

pub struct Store {
    conn: Mutex<Connection>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

const CREATE_SQL: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS overlays (
    id             INTEGER PRIMARY KEY,
    name           TEXT NOT NULL,
    account        TEXT NOT NULL DEFAULT '',
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
    wrap           INTEGER NOT NULL DEFAULT 1,
    max_width      INTEGER NOT NULL DEFAULT 460,
    created_at     INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS variables (
    id          INTEGER PRIMARY KEY,
    overlay_id  INTEGER NOT NULL,
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,
    value       REAL NOT NULL DEFAULT 0,
    step        REAL NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    UNIQUE (overlay_id, key)
);
CREATE TABLE IF NOT EXISTS computed (
    id          INTEGER PRIMARY KEY,
    overlay_id  INTEGER NOT NULL,
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,
    expr        TEXT NOT NULL,
    format      TEXT NOT NULL DEFAULT 'plain',
    decimals    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    UNIQUE (overlay_id, key)
);
CREATE INDEX IF NOT EXISTS idx_variables_overlay ON variables(overlay_id);
CREATE INDEX IF NOT EXISTS idx_computed_overlay  ON computed(overlay_id);
";

/// 旧版 SQLite 结构（v1：全局变量表、overlays 无 account 列）→ 当前结构。
/// 返回是否发生了迁移。全新库（还没有 variables 表）直接跳过。
fn migrate_v1(conn: &Connection) -> Result<bool, String> {
    let has_variables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'variables'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if has_variables == 0 {
        return Ok(false); // 全新库，直接走 CREATE_SQL
    }
    let has_overlay_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('variables') WHERE name = 'overlay_id'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if has_overlay_id > 0 {
        return Ok(false); // 已是当前结构
    }
    // variables 表存在但缺 overlay_id → v1 结构。旧表改名让位。
    conn.execute_batch(
        "ALTER TABLE variables RENAME TO variables_legacy;
         ALTER TABLE computed RENAME TO computed_legacy;",
    )
    .map_err(|e| format!("迁移旧表失败：{e}"))?;
    Ok(true)
}

/// overlays 表补 account 列（v1 → 当前）。
fn ensure_account_column(conn: &Connection) -> Result<(), String> {
    let has_account: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('overlays') WHERE name = 'account'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if has_account == 0 {
        conn.execute("ALTER TABLE overlays ADD COLUMN account TEXT NOT NULL DEFAULT ''", [])
            .map_err(|e| format!("补 account 列失败：{e}"))?;
    }
    Ok(())
}

/// overlays 表补 wrap / max_width 列（换行显示，v2 → 当前）。
/// 已有列则跳过（全新库由 CREATE_SQL 直接建出，迁移库走 ALTER）。
fn ensure_wrap_columns(conn: &Connection) -> Result<(), String> {
    let has_wrap: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('overlays') WHERE name = 'wrap'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("检查 wrap 列失败：{e}"))?;
    if has_wrap == 0 {
        conn.execute("ALTER TABLE overlays ADD COLUMN wrap INTEGER NOT NULL DEFAULT 1", [])
            .map_err(|e| format!("补 wrap 列失败：{e}"))?;
        conn.execute("ALTER TABLE overlays ADD COLUMN max_width INTEGER NOT NULL DEFAULT 460", [])
            .map_err(|e| format!("补 max_width 列失败：{e}"))?;
    }
    Ok(())
}

impl Store {
    /// 打开数据库、建表；首次使用时种入示例数据，并尝试从旧版数据迁移。
    pub fn new(db_path: PathBuf) -> Result<Self, String> {
        let conn = Connection::open(&db_path).map_err(|e| format!("打开数据库失败：{e}"))?;
        // WAL：写入更快、读写不互斥，桌面应用常规配置
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;

        // 顺序很重要：先做"旧表改名"迁移（此时变量表还是 v1 结构），
        // 再 CREATE 建出当前结构的表（IF NOT EXISTS 对已存在的旧表无操作，
        // 被改名让位的 variables/computed 会在这里重建），最后补缺失的列。
        // 直接建表 → 补列的顺序在全新库上会失败（表还不存在就 ALTER）。
        let was_v1 = migrate_v1(&conn)?;
        conn.execute_batch(CREATE_SQL).map_err(|e| format!("建表失败：{e}"))?;
        ensure_account_column(&conn)?;
        ensure_wrap_columns(&conn)?;

        let store = Self { conn: Mutex::new(conn) };
        if was_v1 {
            store.copy_v1_data()?;
        } else {
            store.import_legacy_json(&db_path)?;
        }
        store.repair_missing_computed()?;
        store.seed_if_fresh()?;
        Ok(store)
    }

    /// v1 数据搬迁：全局变量挂到"第一个显示中的浮窗"（没有就建一个默认浮窗），
    /// 旧模板里的旧变量名全部改写，旧全局账号写入每个浮窗。
    fn copy_v1_data(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let meta_account: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'account'", [], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
            .unwrap_or_default();

        // 变量与计算变量的归宿：第一个显示中的浮窗，否则 id 最小的浮窗
        let target: Option<i64> = conn
            .query_row(
                "SELECT id FROM overlays ORDER BY shown = 0, id LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let target = match target {
            Some(id) => id,
            None => {
                conn.execute(
                    "INSERT INTO overlays (name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, created_at)
                     VALUES ('战绩浮窗', ?1, ?2, 1, 1, 0, 0, 40, 20, '#ffffff', 0, 1, '#0f172a', 55, ?3)",
                    params![meta_account, DEFAULT_TEMPLATE, now_ms()],
                )
                .map_err(|e| e.to_string())?;
                conn.last_insert_rowid()
            }
        };

        conn.execute(
            "INSERT INTO variables (id, overlay_id, key, name, value, step, created_at)
             SELECT id, ?1, key, name, value, step, created_at FROM variables_legacy",
            params![target],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO computed (id, overlay_id, key, name, expr, format, decimals, created_at)
             SELECT id, ?1, key, name, expr, format, decimals, created_at FROM computed_legacy",
            params![target],
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch("DROP TABLE variables_legacy; DROP TABLE computed_legacy;")
            .map_err(|e| e.to_string())?;

        // 旧全局账号 → 每个还没账号的浮窗；旧变量名 → 新变量名
        conn.execute(
            "UPDATE overlays SET account = ?1 WHERE account = ''",
            params![meta_account],
        )
        .map_err(|e| e.to_string())?;
        let ids: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, template FROM overlays")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for (id, template) in ids {
            let rewritten = rewrite_legacy_template(&template);
            if rewritten != template {
                conn.execute(
                    "UPDATE overlays SET template = ?1 WHERE id = ?2",
                    params![rewritten, id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// 修复：模板引用了变量但一套都没有（旧版迁移/删除变量后留下的空缺）→ 补种子。
    fn repair_missing_computed(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let ids: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT id FROM overlays")
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for id in ids {
            let vcount: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM variables WHERE overlay_id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let ccount: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM computed WHERE overlay_id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let (refs_a, refs_b): (i64, i64) = conn
                .query_row(
                    "SELECT COALESCE(SUM(template LIKE '%{A%'), 0), COALESCE(SUM(template LIKE '%{B%'), 0) FROM overlays WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| e.to_string())?;
            if vcount == 0 && ccount == 0 && (refs_a + refs_b) > 0 {
                // 整套变量都缺 → 补一套示范（数值 0，不冒充真实数据）
                insert_seed_vars(&conn, id, now_ms(), false)?;
            } else if ccount == 0 && refs_b > 0 {
                insert_seed_computed(&conn, id, now_ms())?;
            }
        }
        Ok(())
    }

    /// 给某浮窗种入示例变量（A1-A4 + B1-B4）。`demo_values = true` 时带示范数值。
    pub fn seed_example_variables(&self, overlay_id: i64, demo_values: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        insert_seed_vars(&conn, overlay_id, now_ms(), demo_values)
    }

    /// 更早版本（JSON 文件存储）的一次性导入：成功返回 true。
    fn import_legacy_json(&self, db_path: &Path) -> Result<bool, String> {
        let dir = match db_path.parent() {
            Some(d) => d,
            None => return Ok(false),
        };
        let legacy = dir.join("config.json");
        let Ok(text) = std::fs::read_to_string(&legacy) else {
            return Ok(false);
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Ok(false);
        };
        let (Some(counters), Some(overlays)) = (v.get("counters"), v.get("overlays")) else {
            return Ok(false);
        };

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = now_ms();
        let account = v.get("account").and_then(|x| x.as_str()).unwrap_or("");

        // 变量
        let seed = [
            ("A1", "人机场次", "botGames"),
            ("A2", "真人场次", "realGames"),
            ("A3", "人机胜场", "botWins"),
            ("A4", "真人胜场", "realWins"),
        ];
        for (i, (key, name, old_field)) in seed.iter().enumerate() {
            let value = counters.get(*old_field).and_then(|x| x.as_f64()).unwrap_or(0.0);
            conn.execute(
                "INSERT INTO variables (overlay_id, key, name, value, step, created_at) VALUES (0, ?1, ?2, ?3, 1, ?4)",
                params![key, name, value, now + i as i64],
            )
            .map_err(|e| e.to_string())?;
        }
        // 浮窗（变量先挂 overlay_id = 0，落到第一个浮窗头上）
        let mut first_id: Option<i64> = None;
        for (i, o) in overlays.as_array().unwrap_or(&vec![]).iter().enumerate() {
            let get_str = |f: &str| o.get(f).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let get_bool = |f: &str| o.get(f).and_then(|x| x.as_bool()).unwrap_or(false);
            let get_i64 = |f: &str| o.get(f).and_then(|x| x.as_i64()).unwrap_or(0);
            conn.execute(
                "INSERT INTO overlays (id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    get_i64("id").max((i + 1) as i64), get_str("name"), account,
                    rewrite_legacy_template(&get_str("template")),
                    get_bool("shown") as i64, get_bool("alwaysOnTop") as i64, get_bool("clickThrough") as i64,
                    get_i64("x"), get_i64("y"), get_i64("fontSize"), get_str("fontColor"),
                    get_bool("bold") as i64, get_bool("shadow") as i64, get_str("bgColor"), get_i64("bgOpacity"),
                    now + i as i64
                ],
            )
            .map_err(|e| e.to_string())?;
            if first_id.is_none() {
                first_id = Some(get_i64("id").max((i + 1) as i64));
            }
        }
        let target = first_id.unwrap_or(0);
        conn.execute(
            "UPDATE variables SET overlay_id = ?1 WHERE overlay_id = 0",
            params![target],
        )
        .map_err(|e| e.to_string())?;
        // 模板引用了 B 变量就补一套种子计算变量
        let refs_b: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM overlays WHERE id = ?1 AND template LIKE '%{B%'",
                params![target],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if refs_b > 0 {
            insert_seed_computed(&conn, target, now)?;
        }
        drop(conn);
        let _ = std::fs::rename(&legacy, dir.join("config.json.migrated"));
        Ok(true)
    }

    /// 全新数据库：种入一套示例浮窗（对应"战绩"场景）。
    fn seed_if_fresh(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let overlays: i64 = conn
            .query_row("SELECT COUNT(*) FROM overlays", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if overlays > 0 {
            return Ok(());
        }
        let now = now_ms();
        conn.execute(
            "INSERT INTO overlays (id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, created_at)
             VALUES (1, '战绩浮窗', '', ?1, 1, 1, 0, 0, 40, 20, '#ffffff', 0, 1, '#0f172a', 55, ?2)",
            params![DEFAULT_TEMPLATE, now],
        )
        .map_err(|e| e.to_string())?;
        // 示范数值（6/2/6/1），方便第一次打开就看到效果
        insert_seed_vars(&conn, 1, now, true)?;
        Ok(())
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
        let mut overlays = Vec::new();
        let mut stmt = match conn.prepare(
            "SELECT id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, wrap, max_width, created_at FROM overlays ORDER BY id",
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("查询 overlays 失败：{e}");
                return Config::default();
            }
        };
        let rows = stmt.query_map([], |row| {
            Ok(OverlayConfig {
                id: row.get(0)?,
                name: row.get(1)?,
                account: row.get(2)?,
                template: row.get(3)?,
                shown: row.get::<_, i64>(4)? != 0,
                always_on_top: row.get::<_, i64>(5)? != 0,
                click_through: row.get::<_, i64>(6)? != 0,
                x: row.get(7)?,
                y: row.get(8)?,
                font_size: row.get::<_, i64>(9)? as u32,
                font_color: row.get(10)?,
                bold: row.get::<_, i64>(11)? != 0,
                shadow: row.get::<_, i64>(12)? != 0,
                bg_color: row.get(13)?,
                bg_opacity: row.get::<_, i64>(14)? as u32,
                wrap: row.get::<_, i64>(15)? != 0,
                max_width: row.get(16)?,
                created_at: row.get(17)?,
                variables: vec![],
                computed: vec![],
            })
        });
        if let Ok(rows) = rows {
            for cfg in rows.filter_map(|r| r.ok()) {
                let mut cfg = cfg;
                cfg.variables = load_variables(&conn, cfg.id);
                cfg.computed = load_computed(&conn, cfg.id);
                overlays.push(cfg);
            }
        }
        Config { overlays }
    }

    pub fn overlay_exists(&self, id: i64) -> bool {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return false,
        };
        conn.query_row("SELECT 1 FROM overlays WHERE id = ?1", params![id], |_| Ok(()))
            .is_ok()
    }

    // ---------- 浮窗（套） ----------

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
            "INSERT INTO overlays (id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, wrap, max_width, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                cfg.id, cfg.name, cfg.account, cfg.template,
                cfg.shown as i64, cfg.always_on_top as i64, cfg.click_through as i64,
                cfg.x, cfg.y, cfg.font_size as i64, cfg.font_color,
                cfg.bold as i64, cfg.shadow as i64, cfg.bg_color, cfg.bg_opacity as i64,
                cfg.wrap as i64, cfg.max_width, cfg.created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 更新浮窗的名称/账号/模板/样式（**有意不更新 x/y**——位置只由拖动回写和
    /// move_overlay 维护，避免主窗口表单里过期的坐标把用户刚拖好的位置弹回去）。
    /// 变量有独立命令，这里不碰。
    pub fn update_overlay(&self, cfg: &OverlayConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE overlays SET name = ?1, account = ?2, template = ?3, font_size = ?4, font_color = ?5, bold = ?6, shadow = ?7, bg_color = ?8, bg_opacity = ?9, wrap = ?10, max_width = ?11 WHERE id = ?12",
                params![
                    cfg.name, cfg.account.trim(), cfg.template, cfg.font_size as i64, cfg.font_color,
                    cfg.bold as i64, cfg.shadow as i64, cfg.bg_color, cfg.bg_opacity as i64,
                    cfg.wrap as i64, cfg.max_width, cfg.id
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
        // 读出当前三个开关 → apply 只改其中一个 → 写回
        let (shown, top, through): (i64, i64, i64) = conn
            .query_row(
                "SELECT shown, always_on_top, click_through FROM overlays WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| format!("浮窗 #{id} 不存在"))?;
        let mut probe = OverlayConfig {
            shown: shown != 0,
            always_on_top: top != 0,
            click_through: through != 0,
            ..Default::default()
        };
        apply(&mut probe);
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE overlays SET shown = ?2, always_on_top = ?3, click_through = ?4 WHERE id = ?1",
            params![id, probe.shown as i64, probe.always_on_top as i64, probe.click_through as i64],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_overlay(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM variables WHERE overlay_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM computed WHERE overlay_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM overlays WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 拖动浮窗时由 Moved 事件回写位置。
    pub fn set_position(&self, id: i64, x: i32, y: i32) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("UPDATE overlays SET x = ?1, y = ?2 WHERE id = ?3", params![x, y, id]);
    }

    // ---------- 套内变量 ----------

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

    /// 生成下一可用 key：扫描**本浮窗**现有 `A{n}` 取最大 n+1（computed 用 B 前缀）。
    fn next_key(conn: &Connection, overlay_id: i64, prefix: &str) -> Result<String, String> {
        let table = if prefix == "A" { "variables" } else { "computed" };
        let mut stmt = conn
            .prepare(&format!("SELECT key FROM {table} WHERE overlay_id = ?1"))
            .map_err(|e| e.to_string())?;
        let keys: Vec<String> = stmt
            .query_map(params![overlay_id], |r| r.get::<_, String>(0))
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
        overlay_id: i64,
        key: Option<String>,
        name: &str,
        value: f64,
        step: f64,
    ) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("变量显示名不能为空".into());
        }
        if !self.overlay_exists(overlay_id) {
            return Err("浮窗不存在，先创建浮窗".into());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let key = match key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => {
                Self::validate_key(k)?;
                k.to_string()
            }
            None => Self::next_key(&conn, overlay_id, "A")?,
        };
        conn.execute(
            "INSERT INTO variables (overlay_id, key, name, value, step, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![overlay_id, key, name, value, if step > 0.0 { step } else { 1.0 }, now_ms()],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                format!("这套浮窗里变量名 `{key}` 已存在")
            } else {
                e.to_string()
            }
        })?;
        Ok(())
    }

    /// 更新变量的名称/引用名/步长。数值只能由 step_variable 原子增减：
    /// 若这里把编辑草稿里的旧 value 一起写回，防抖保存就可能抵消刚点的＋/−。
    pub fn update_variable(&self, overlay_id: i64, var: &UserVar) -> Result<(), String> {
        Self::validate_key(&var.key)?;
        let name = var.name.trim();
        if name.is_empty() {
            return Err("变量显示名不能为空".into());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE variables SET key = ?1, name = ?2, step = ?3 WHERE id = ?4 AND overlay_id = ?5",
                params![var.key, name, if var.step > 0.0 { var.step } else { 1.0 }, var.id, overlay_id],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    format!("这套浮窗里变量名 `{}` 已存在", var.key)
                } else {
                    e.to_string()
                }
            })?;
        if changed == 0 {
            return Err(format!("变量 #{} 不存在（或不属于该浮窗）", var.id));
        }
        Ok(())
    }

    /// 计数器语义：不允许减到负数。
    pub fn step_variable(&self, overlay_id: i64, id: i64, delta: f64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE variables SET value = MAX(0, value + ?1) WHERE id = ?2 AND overlay_id = ?3",
                params![delta, id, overlay_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("变量 #{id} 不存在（或不属于该浮窗）"));
        }
        Ok(())
    }

    pub fn delete_variable(&self, overlay_id: i64, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM variables WHERE id = ?1 AND overlay_id = ?2",
            params![id, overlay_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---------- 套内计算变量 ----------

    pub fn add_computed(
        &self,
        overlay_id: i64,
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
        if !self.overlay_exists(overlay_id) {
            return Err("浮窗不存在，先创建浮窗".into());
        }
        let format = if format == "percent" { "percent" } else { "plain" };
        let decimals = decimals.clamp(0, 6);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let key = match key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => {
                Self::validate_key(k)?;
                k.to_string()
            }
            None => Self::next_key(&conn, overlay_id, "B")?,
        };
        conn.execute(
            "INSERT INTO computed (overlay_id, key, name, expr, format, decimals, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![overlay_id, key, name, expr, format, decimals, now_ms()],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                format!("这套浮窗里变量名 `{key}` 已存在")
            } else {
                e.to_string()
            }
        })?;
        Ok(())
    }

    pub fn update_computed(&self, overlay_id: i64, var: &ComputedVar) -> Result<(), String> {
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
                "UPDATE computed SET key = ?1, name = ?2, expr = ?3, format = ?4, decimals = ?5 WHERE id = ?6 AND overlay_id = ?7",
                params![var.key, name, expr, format, var.decimals.clamp(0, 6), var.id, overlay_id],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    format!("这套浮窗里变量名 `{}` 已存在", var.key)
                } else {
                    e.to_string()
                }
            })?;
        if changed == 0 {
            return Err(format!("计算变量 #{} 不存在（或不属于该浮窗）", var.id));
        }
        Ok(())
    }

    pub fn delete_computed(&self, overlay_id: i64, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM computed WHERE id = ?1 AND overlay_id = ?2",
            params![id, overlay_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// 给某浮窗补一套示范变量：A1-A4（人机/真人 场次与胜场）+ B1-B4 计算变量。
/// `demo_values = true` 时填入示范数值 6/2/6/1，否则全部从 0 开始。
fn insert_seed_vars(
    conn: &Connection,
    overlay_id: i64,
    now: i64,
    demo_values: bool,
) -> Result<(), String> {
    let v = |demo: f64| if demo_values { demo } else { 0.0 };
    let vars = [
        ("A1", "人机场次", v(6.0)),
        ("A2", "真人场次", v(2.0)),
        ("A3", "人机胜场", v(6.0)),
        ("A4", "真人胜场", v(1.0)),
    ];
    for (i, (key, name, value)) in vars.iter().enumerate() {
        conn.execute(
            "INSERT INTO variables (overlay_id, key, name, value, step, created_at) VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            params![overlay_id, key, name, value, now + i as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    insert_seed_computed(conn, overlay_id, now)
}

/// 给某浮窗补一套种子计算变量（B1-B4，对应"战绩"场景）。
fn insert_seed_computed(conn: &Connection, overlay_id: i64, now: i64) -> Result<(), String> {
    let seed = [
        ("B1", "总场次", "A1 + A2", "plain", 0),
        ("B2", "总胜场", "A3 + A4", "plain", 0),
        ("B3", "总胜率", "B2 / B1", "percent", 1),
        ("B4", "真实胜率", "A4 / B1", "percent", 1),
    ];
    for (i, (key, name, expr, format, decimals)) in seed.iter().enumerate() {
        conn.execute(
            "INSERT INTO computed (overlay_id, key, name, expr, format, decimals, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![overlay_id, key, name, expr, format, decimals, now + i as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 新浮窗的默认配置（位置 x/y 由命令层根据主显示器计算后填入）。
pub fn default_overlay(id: i64, created_at: i64) -> OverlayConfig {
    OverlayConfig {
        id,
        name: "战绩浮窗".into(),
        account: String::new(),
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
        wrap: true,
        max_width: 460,
        created_at,
        variables: vec![],
        computed: vec![],
    }
}

/// 加载某浮窗的自定义变量。
fn load_variables(conn: &Connection, overlay_id: i64) -> Vec<UserVar> {
    let mut stmt = match conn
        .prepare("SELECT id, key, name, value, step, created_at FROM variables WHERE overlay_id = ?1 ORDER BY id")
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("查询 variables 失败：{e}");
            return vec![];
        }
    };
    stmt.query_map(params![overlay_id], |row| {
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
    .unwrap_or_else(|e| {
        eprintln!("读取 variables 失败：{e}");
        vec![]
    })
}

/// 加载某浮窗的计算变量。
fn load_computed(conn: &Connection, overlay_id: i64) -> Vec<ComputedVar> {
    let mut stmt = match conn.prepare(
        "SELECT id, key, name, expr, format, decimals, created_at FROM computed WHERE overlay_id = ?1 ORDER BY id",
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("查询 computed 失败：{e}");
            return vec![];
        }
    };
    stmt.query_map(params![overlay_id], |row| {
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
    .unwrap_or_else(|e| {
        eprintln!("读取 computed 失败：{e}");
        vec![]
    })
}
