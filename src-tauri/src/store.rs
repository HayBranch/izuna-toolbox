//! 持久化：SQLite（`rusqlite` bundled，SQLite 源码静态编译进程序）。
//!
//! 数据模型：**每个浮窗 = 一套完全独立的配置**——
//! 自己的账号、自定义变量、计算变量、模板与样式，互不影响。
//!
//! 表结构：
//! - `overlays`  浮窗（套）队列：可多个，含名字/创建时间等基本信息 + 每套自己的账号
//! - `variables` 自定义变量，归属某个浮窗（overlay_id；follow_id 表示跟随本套哪个变量）
//! - `computed`  计算变量，归属某个浮窗（overlay_id）
//!
//! 并发模型：`Connection` 非 `Sync`，用 `Mutex` 包一层挂进 Tauri 状态。
//! 每次变更返回最新全量 `Config`，由命令层广播给所有窗口（单一数据源）。

use std::path::PathBuf;
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
    /// 联动：跟随本套哪个变量（variables.id；0 = 不跟随）。
    /// 被跟随变量每次 −/＋，本变量也同步同样的实际增减幅度。
    pub follow_id: i64,
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
    /// 最后修改时间（毫秒时间戳；改名称/账号/模板/样式或开关时更新，拖动位置不算）
    pub updated_at: i64,
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

// ---------- 空库 / 新浮窗的默认数据（要改初始配置，只改这一段） ----------
//
// - `DEFAULT_TEMPLATE` / `DEFAULT_OVERLAY_NAME`：默认模板与浮窗名
// - `SEED_VARIABLES`：默认自定义变量（含示范值、步长、联动）
// - `SEED_COMPUTED`：默认计算变量
//
// 空库初始化（第一次启动）用 `SEED_VARIABLES` 里的示范数值；
// 之后新建浮窗、变量被删光后的补种，都用同一套定义但数值从 0 开始。

/// 默认模板：多行排版示范变量引用与计算变量的用法，可随意改（`\n` 即换行）。
pub const DEFAULT_TEMPLATE: &str = "今日{today} · 账号：{account}\n总场次：{B1}（人机 {A1}｜真人 {A2}）\n胜场：{B2}（人机 {A3}｜真人 {A4}）\n总胜率：{B3}｜真实胜率：{B4}";

/// 默认浮窗名。
pub const DEFAULT_OVERLAY_NAME: &str = "战绩浮窗";

/// 一个默认自定义变量的定义。
struct SeedVar {
    /// 引用名：模板/表达式里用 `{A1}` 这样引用
    key: &'static str,
    /// 界面显示名
    name: &'static str,
    /// 空库初始化时的示范数值（补种时不使用，一律 0）
    value: f64,
    /// 每次 −/＋ 的步长
    step: f64,
    /// 默认联动：跟随本表里的哪个 key（None = 不跟随）。
    /// 被跟随变量每次 −/＋，本变量同步同样的实际幅度（单向）。
    follow: Option<&'static str>,
}

/// 默认自定义变量（顺序即界面顺序，key 必须是 A + 数字）。
const SEED_VARIABLES: &[SeedVar] = &[
    // 场次跟随胜场：记一场胜场时，对应场次自动 +1
    SeedVar {
        key: "A1",
        name: "人机场次",
        value: 0.0,
        step: 1.0,
        follow: Some("A3"),
    },
    SeedVar {
        key: "A2",
        name: "真人场次",
        value: 0.0,
        step: 1.0,
        follow: Some("A4"),
    },
    SeedVar {
        key: "A3",
        name: "人机胜场",
        value: 0.0,
        step: 1.0,
        follow: None,
    },
    SeedVar {
        key: "A4",
        name: "真人胜场",
        value: 0.0,
        step: 1.0,
        follow: None,
    },
];

/// 一个默认计算变量的定义。
struct SeedComputed {
    key: &'static str,
    name: &'static str,
    /// 表达式，可引用本表/上面的变量
    expr: &'static str,
    /// `plain` 普通数 | `percent` 百分比
    format: &'static str,
    /// 小数点位数 0-6
    decimals: i64,
}

/// 默认计算变量（只读，由表达式求值）。
const SEED_COMPUTED: &[SeedComputed] = &[
    SeedComputed {
        key: "B1",
        name: "总场次",
        expr: "A1 + A2",
        format: "plain",
        decimals: 0,
    },
    SeedComputed {
        key: "B2",
        name: "总胜场",
        expr: "A3 + A4",
        format: "plain",
        decimals: 0,
    },
    SeedComputed {
        key: "B3",
        name: "总胜率",
        expr: "B2 / B1",
        format: "percent",
        decimals: 1,
    },
    SeedComputed {
        key: "B4",
        name: "真实胜率",
        expr: "A4 / A2",
        format: "percent",
        decimals: 1,
    },
];

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
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS variables (
    id          INTEGER PRIMARY KEY,
    overlay_id  INTEGER NOT NULL,
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,
    value       REAL NOT NULL DEFAULT 0,
    step        REAL NOT NULL DEFAULT 1,
    follow_id   INTEGER NOT NULL DEFAULT 0,
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

impl Store {
    /// 打开数据库、建表；首次使用时种入示例数据。
    pub fn new(db_path: PathBuf) -> Result<Self, String> {
        let conn = Connection::open(&db_path).map_err(|e| format!("打开数据库失败：{e}"))?;
        // WAL：写入更快、读写不互斥，桌面应用常规配置
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;
        conn.execute_batch(CREATE_SQL)
            .map_err(|e| format!("建表失败：{e}"))?;

        // 轻量迁移：v0.0.9 起记录最后修改时间。老库补列并把已有记录回填为
        // 创建时间（比 0 更有意义）；新库建表时已带列，这里跳过。
        let has_updated_at: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('overlays') WHERE name = 'updated_at'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("检查表结构失败：{e}"))?;
        if has_updated_at == 0 {
            conn.execute_batch(
                "ALTER TABLE overlays ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0;
                 UPDATE overlays SET updated_at = created_at WHERE updated_at = 0;",
            )
            .map_err(|e| format!("迁移 overlays.updated_at 失败：{e}"))?;
        }

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.repair_missing_computed()?;
        store.seed_if_fresh()?;
        Ok(store)
    }

    /// 修复：模板引用了变量但变量被删光（或计算变量缺失）→ 补种子。
    /// 补种数值一律为 0，不冒充真实数据。
    fn repair_missing_computed(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let ids: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT id FROM overlays")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get(0))
                .map_err(|e| e.to_string())?;
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
            let now = now_ms();
            // 自定义变量被删光（但模板还引用着变量）→ 只补变量，计算变量已存在的保留
            if vcount == 0 && (refs_a + refs_b) > 0 {
                insert_seed_variables(&conn, id, now, false)?;
            }
            // 计算变量缺失但模板引用着 {B..} → 只补计算变量
            if ccount == 0 && refs_b > 0 {
                insert_seed_computed(&conn, id, now)?;
            }
        }
        Ok(())
    }

    /// 给某浮窗种入默认变量（`SEED_VARIABLES` + `SEED_COMPUTED`）。
    /// `demo_values = true` 时带示范数值（空库初始化），否则全部从 0 开始。
    pub fn seed_example_variables(&self, overlay_id: i64, demo_values: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        insert_seed_vars(&conn, overlay_id, now_ms(), demo_values)
    }

    /// 全新数据库：种入一套默认浮窗（对应"战绩"场景）。
    fn seed_if_fresh(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let overlays: i64 = conn
            .query_row("SELECT COUNT(*) FROM overlays", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if overlays > 0 {
            return Ok(());
        }
        let now = now_ms();
        // 默认浮窗：名字/模板/样式见 default_overlay，位置略下移避免贴屏幕顶边
        let mut cfg = default_overlay(1, now);
        cfg.y = 40;
        insert_overlay(&conn, &cfg)?;
        // 示范数值 + 默认联动，第一次打开就能看到完整效果
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
            "SELECT id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, wrap, max_width, created_at, updated_at FROM overlays ORDER BY id",
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
                updated_at: row.get(18)?,
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
        conn.query_row("SELECT 1 FROM overlays WHERE id = ?1", params![id], |_| {
            Ok(())
        })
        .is_ok()
    }

    // ---------- 浮窗（套） ----------

    /// 新浮窗 id：毫秒时间戳与现有最大 id+1 取大，保证单调唯一。
    pub fn next_overlay_id(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        let max: i64 = conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM overlays", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        now_ms().max(max + 1)
    }

    pub fn push_overlay(&self, cfg: &OverlayConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        insert_overlay(&conn, cfg)
    }

    /// 更新浮窗的名称/账号/模板/样式（**有意不更新 x/y**——位置只由拖动回写和
    /// move_overlay 维护，避免主窗口表单里过期的坐标把用户刚拖好的位置弹回去）。
    /// 变量有独立命令，这里不碰。同时刷新最后修改时间。
    pub fn update_overlay(&self, cfg: &OverlayConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE overlays SET name = ?1, account = ?2, template = ?3, font_size = ?4, font_color = ?5, bold = ?6, shadow = ?7, bg_color = ?8, bg_opacity = ?9, wrap = ?10, max_width = ?11, updated_at = ?12 WHERE id = ?13",
                params![
                    cfg.name, cfg.account.trim(), cfg.template, cfg.font_size as i64, cfg.font_color,
                    cfg.bold as i64, cfg.shadow as i64, cfg.bg_color, cfg.bg_opacity as i64,
                    cfg.wrap as i64, cfg.max_width, now_ms(), cfg.id
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
            .execute(
                "UPDATE overlays SET x = ?1, y = ?2 WHERE id = ?3",
                params![x, y, id],
            )
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
            "UPDATE overlays SET shown = ?2, always_on_top = ?3, click_through = ?4, updated_at = ?5 WHERE id = ?1",
            params![
                id,
                probe.shown as i64,
                probe.always_on_top as i64,
                probe.click_through as i64,
                now_ms()
            ],
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
        let _ = conn.execute(
            "UPDATE overlays SET x = ?1, y = ?2 WHERE id = ?3",
            params![x, y, id],
        );
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
        let table = if prefix == "A" {
            "variables"
        } else {
            "computed"
        };
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

    /// 校验联动目标：必须属于同一套、不能是自己、不能形成环。
    fn validate_follow(
        conn: &Connection,
        overlay_id: i64,
        id: i64,
        follow_id: i64,
    ) -> Result<(), String> {
        if follow_id == 0 {
            return Ok(());
        }
        if follow_id == id {
            return Err("变量不能跟随自己".into());
        }
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM variables WHERE id = ?1 AND overlay_id = ?2",
                params![follow_id, overlay_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_none() {
            return Err("要跟随的变量不存在（或不属于该浮窗）".into());
        }
        // 从目标出发沿"它跟随谁"往上走，若绕回自己就会成环
        let mut cur = follow_id;
        let mut seen = std::collections::HashSet::new();
        while cur != 0 {
            if cur == id {
                return Err("联动会形成循环，请换一个变量".into());
            }
            if !seen.insert(cur) {
                return Err("联动配置里已有循环，请重新选择".into());
            }
            cur = conn
                .query_row(
                    "SELECT follow_id FROM variables WHERE id = ?1 AND overlay_id = ?2",
                    params![cur, overlay_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?
                .unwrap_or(0);
        }
        Ok(())
    }

    /// 更新变量的名称/引用名/步长/联动目标。数值只能由 step_variable 原子增减：
    /// 若这里把编辑草稿里的旧 value 一起写回，防抖保存就可能抵消刚点的＋/−。
    pub fn update_variable(&self, overlay_id: i64, var: &UserVar) -> Result<(), String> {
        Self::validate_key(&var.key)?;
        let name = var.name.trim();
        if name.is_empty() {
            return Err("变量显示名不能为空".into());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        Self::validate_follow(&conn, overlay_id, var.id, var.follow_id)?;
        let changed = conn
            .execute(
                "UPDATE variables SET key = ?1, name = ?2, step = ?3, follow_id = ?4 WHERE id = ?5 AND overlay_id = ?6",
                params![
                    var.key,
                    name,
                    if var.step > 0.0 { var.step } else { 1.0 },
                    var.follow_id,
                    var.id,
                    overlay_id
                ],
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
    /// 联动：变量变化后，所有（直接/间接）跟随它的变量同步同样的实际幅度。
    pub fn step_variable(&self, overlay_id: i64, id: i64, delta: f64) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let applied = apply_step(&tx, overlay_id, id, delta)?;
        propagate_followers(&tx, overlay_id, id, applied)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_variable(&self, overlay_id: i64, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        // 被删掉的变量不能继续被跟随：把指向它的联动清空
        conn.execute(
            "UPDATE variables SET follow_id = 0 WHERE overlay_id = ?1 AND follow_id = ?2",
            params![overlay_id, id],
        )
        .map_err(|e| e.to_string())?;
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
        let format = if format == "percent" {
            "percent"
        } else {
            "plain"
        };
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
        let format = if var.format == "percent" {
            "percent"
        } else {
            "plain"
        };
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

/// 写入一条浮窗记录（新浮窗与空库初始化共用）。
fn insert_overlay(conn: &Connection, cfg: &OverlayConfig) -> Result<(), String> {
    conn.execute(
        "INSERT INTO overlays (id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, wrap, max_width, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            cfg.id, cfg.name, cfg.account, cfg.template,
            cfg.shown as i64, cfg.always_on_top as i64, cfg.click_through as i64,
            cfg.x, cfg.y, cfg.font_size as i64, cfg.font_color,
            cfg.bold as i64, cfg.shadow as i64, cfg.bg_color, cfg.bg_opacity as i64,
            cfg.wrap as i64, cfg.max_width, cfg.created_at, cfg.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 给单个变量加减 delta（下限 0），返回实际变化量。
fn apply_step(
    tx: &rusqlite::Transaction<'_>,
    overlay_id: i64,
    id: i64,
    delta: f64,
) -> Result<f64, String> {
    let old: f64 = tx
        .query_row(
            "SELECT value FROM variables WHERE id = ?1 AND overlay_id = ?2",
            params![id, overlay_id],
            |r| r.get(0),
        )
        .map_err(|_| format!("变量 #{id} 不存在（或不属于该浮窗）"))?;
    tx.execute(
        "UPDATE variables SET value = MAX(0, value + ?1) WHERE id = ?2 AND overlay_id = ?3",
        params![delta, id, overlay_id],
    )
    .map_err(|e| e.to_string())?;
    let new: f64 = tx
        .query_row(
            "SELECT value FROM variables WHERE id = ?1 AND overlay_id = ?2",
            params![id, overlay_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(new - old)
}

/// 从被操作的变量出发，按"谁跟随它"的边向下传播实际变化量。
/// 每个变量一次操作最多更新一次（visited），即使旧数据里有环也不会死循环。
fn propagate_followers(
    tx: &rusqlite::Transaction<'_>,
    overlay_id: i64,
    source_id: i64,
    delta: f64,
) -> Result<(), String> {
    if delta == 0.0 {
        return Ok(());
    }
    let mut stack = vec![(source_id, delta)];
    let mut visited = std::collections::HashSet::new();
    visited.insert(source_id);
    while let Some((src, d)) = stack.pop() {
        let followers: Vec<i64> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id FROM variables WHERE overlay_id = ?1 AND follow_id = ?2 ORDER BY id",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![overlay_id, src], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for fid in followers {
            if !visited.insert(fid) {
                continue;
            }
            let applied = apply_step(tx, overlay_id, fid, d)?;
            if applied != 0.0 {
                stack.push((fid, applied));
            }
        }
    }
    Ok(())
}

/// 给某浮窗种入默认变量（`SEED_VARIABLES` + `SEED_COMPUTED`），含默认联动。
/// `demo_values = true` 时使用示范数值（空库初始化），否则全部从 0 开始。
fn insert_seed_vars(
    conn: &Connection,
    overlay_id: i64,
    now: i64,
    demo_values: bool,
) -> Result<(), String> {
    // 变量 + 联动 + 计算变量一次性提交，避免补种到一半中断
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    insert_seed_variables(&tx, overlay_id, now, demo_values)?;
    insert_seed_computed(&tx, overlay_id, now)?;
    tx.commit().map_err(|e| e.to_string())
}

/// 只种自定义变量（含默认联动），不动计算变量。
/// 自增 id 插入后才知道，所以联动按 key 第二遍回填。
fn insert_seed_variables(
    conn: &Connection,
    overlay_id: i64,
    now: i64,
    demo_values: bool,
) -> Result<(), String> {
    for (i, v) in SEED_VARIABLES.iter().enumerate() {
        let value = if demo_values { v.value } else { 0.0 };
        conn.execute(
            "INSERT INTO variables (overlay_id, key, name, value, step, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![overlay_id, v.key, v.name, value, v.step, now + i as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    for v in SEED_VARIABLES {
        let Some(target) = v.follow else { continue };
        conn.execute(
            "UPDATE variables SET follow_id = (SELECT id FROM variables WHERE overlay_id = ?1 AND key = ?2)
             WHERE overlay_id = ?3 AND key = ?4",
            params![overlay_id, target, overlay_id, v.key],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 给某浮窗补一套默认计算变量（`SEED_COMPUTED`）。
fn insert_seed_computed(conn: &Connection, overlay_id: i64, now: i64) -> Result<(), String> {
    for (i, c) in SEED_COMPUTED.iter().enumerate() {
        conn.execute(
            "INSERT INTO computed (overlay_id, key, name, expr, format, decimals, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![overlay_id, c.key, c.name, c.expr, c.format, c.decimals, now + i as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 新浮窗的默认配置（位置 x/y 由命令层根据主显示器计算后填入）。
pub fn default_overlay(id: i64, created_at: i64) -> OverlayConfig {
    OverlayConfig {
        id,
        name: DEFAULT_OVERLAY_NAME.into(),
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
        updated_at: created_at,
        variables: vec![],
        computed: vec![],
    }
}

/// 加载某浮窗的自定义变量。
fn load_variables(conn: &Connection, overlay_id: i64) -> Vec<UserVar> {
    let mut stmt = match conn.prepare(
        "SELECT id, key, name, value, step, follow_id, created_at FROM variables WHERE overlay_id = ?1 ORDER BY id",
    ) {
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
            follow_id: row.get(5)?,
            created_at: row.get(6)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "izuna-toolbox-test-{}-{}-{}",
            std::process::id(),
            now_ms(),
            n
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_store() -> (Store, PathBuf) {
        let dir = test_dir();
        let store = Store::new(dir.join("test.db")).unwrap();
        (store, dir)
    }

    fn var(store: &Store, key: &str) -> UserVar {
        store
            .get()
            .overlays
            .into_iter()
            .find(|o| o.id == 1)
            .and_then(|o| o.variables.into_iter().find(|v| v.key == key))
            .unwrap_or_else(|| panic!("变量 {key} 不存在"))
    }

    fn set_follow(store: &Store, key: &str, source: &str) {
        let mut v = var(store, key);
        v.follow_id = var(store, source).id;
        store.update_variable(1, &v).unwrap();
    }

    fn cleanup(store: Store, dir: PathBuf) {
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn follow_mirrors_steps_and_rejects_cycle() {
        let (store, dir) = test_store();
        let a1 = var(&store, "A1");
        let a3 = var(&store, "A3");
        set_follow(&store, "A1", "A3");

        store.step_variable(1, a3.id, 1.0).unwrap();
        assert_eq!(var(&store, "A1").value, a1.value + 1.0);
        assert_eq!(var(&store, "A3").value, a3.value + 1.0);

        store.step_variable(1, a3.id, -1.0).unwrap();
        assert_eq!(var(&store, "A1").value, a1.value);
        assert_eq!(var(&store, "A3").value, a3.value);

        // 反向跟随会成环，应被拒绝
        let mut back = var(&store, "A3");
        back.follow_id = var(&store, "A1").id;
        assert!(store.update_variable(1, &back).is_err());

        cleanup(store, dir);
    }

    #[test]
    fn follow_supports_chain_and_cleans_up_on_delete() {
        let (store, dir) = test_store();
        let a1 = var(&store, "A1");
        let a2 = var(&store, "A2");
        let a3 = var(&store, "A3");
        set_follow(&store, "A1", "A3");
        set_follow(&store, "A2", "A1");

        store.step_variable(1, a3.id, 1.0).unwrap();
        assert_eq!(var(&store, "A3").value, a3.value + 1.0);
        assert_eq!(var(&store, "A1").value, a1.value + 1.0);
        assert_eq!(var(&store, "A2").value, a2.value + 1.0);

        store.delete_variable(1, a3.id).unwrap();
        assert_eq!(var(&store, "A1").follow_id, 0);
        // A2 仍然跟随 A1，只是 A1 不再跟随已删除的 A3
        assert_eq!(var(&store, "A2").follow_id, var(&store, "A1").id);

        cleanup(store, dir);
    }

    #[test]
    fn follow_stops_at_zero() {
        let (store, dir) = test_store();
        set_follow(&store, "A1", "A3");
        let a3 = var(&store, "A3");
        for _ in 0..8 {
            store.step_variable(1, a3.id, -1.0).unwrap();
        }
        assert_eq!(var(&store, "A3").value, 0.0);
        assert_eq!(var(&store, "A1").value, 0.0);
        cleanup(store, dir);
    }

    #[test]
    fn fresh_db_seeds_demo_values_and_follow_links() {
        let (store, dir) = test_store();

        // 空库初始化：每个变量的示范值/步长/默认联动都应与 SEED_VARIABLES 一致
        for seed in SEED_VARIABLES {
            let seeded = var(&store, seed.key);
            assert_eq!(seeded.value, seed.value, "变量 {} 初始值", seed.key);
            assert_eq!(seeded.step, seed.step, "变量 {} 步长", seed.key);
            let want_follow = seed
                .follow
                .map(|target| var(&store, target).id)
                .unwrap_or(0);
            assert_eq!(seeded.follow_id, want_follow, "变量 {} 默认联动", seed.key);
        }

        // 联动确实生效：被跟随变量 +1 → 跟随者同步 +1
        let follower = SEED_VARIABLES
            .iter()
            .find(|v| v.follow.is_some())
            .expect("默认配置里至少应有一个联动");
        let target = follower.follow.unwrap();
        let follower_before = var(&store, follower.key).value;
        let target_before = var(&store, target).value;
        store.step_variable(1, var(&store, target).id, 1.0).unwrap();
        assert_eq!(var(&store, target).value, target_before + 1.0);
        assert_eq!(var(&store, follower.key).value, follower_before + 1.0);
        cleanup(store, dir);
    }

    #[test]
    fn deleted_variables_are_reseeded_zero_with_follow_links() {
        let dir = test_dir();
        let path = dir.join("test.db");
        {
            let store = Store::new(path.clone()).unwrap();
            for v in store.get().overlays[0].variables.clone() {
                store.delete_variable(1, v.id).unwrap();
            }
            assert!(store.get().overlays[0].variables.is_empty());
        }
        // 重新打开 = 模拟重启，触发补种
        let store = Store::new(path).unwrap();
        for seed in SEED_VARIABLES {
            let seeded = var(&store, seed.key);
            assert_eq!(seeded.value, 0.0, "补种不应带示范数值：{}", seed.key);
            assert_eq!(
                seeded.follow_id,
                seed.follow
                    .map(|target| var(&store, target).id)
                    .unwrap_or(0),
                "补种应保留默认联动：{}",
                seed.key
            );
        }
        // 计算变量还在，不应重复插入
        assert_eq!(store.get().overlays[0].computed.len(), SEED_COMPUTED.len());
        cleanup(store, dir);
    }
}
