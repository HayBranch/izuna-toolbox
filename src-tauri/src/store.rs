//! 持久化：SQLite（`rusqlite` bundled，SQLite 源码静态编译进程序）。
//!
//! 数据模型：**每个浮窗 = 一套完全独立的配置**——
//! 自己的账号、自定义变量、计算变量、模板与样式，互不影响。
//!
//! 表结构：
//! - `overlays`  浮窗（套）队列：可多个，含名字/创建时间等基本信息 + 每套自己的账号
//! - `variables` 自定义变量，归属某个浮窗（overlay_id；follow_id 表示跟随本套哪个变量）
//! - `computed`  计算变量，归属某个浮窗（overlay_id）
//! - `queues`    排队队列：直播间号、正则、排序方式、弹窗样式等
//! - `queue_entries` 排队成员，归属某个队列（queue_id；uid 为 0 表示手动添加）
//!
//! 并发模型：`Connection` 非 `Sync`，用 `Mutex` 包一层挂进 Tauri 状态。
//! 每次变更返回最新全量 `Config`，由命令层广播给所有窗口（单一数据源）。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 新规则没有指定颜色时的默认分组色（与前端色板第一项一致）。
pub const DEFAULT_GROUP_COLOR: &str = "#3b82f6";

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
    /// 手动尺寸（逻辑像素；0 = 自动按内容，>0 时固定窗口大小）
    pub width: i64,
    pub height: i64,
    /// 创建时间（毫秒时间戳）
    pub created_at: i64,
    /// 最后修改时间（毫秒时间戳；改名称/账号/模板/样式或开关时更新，拖动位置不算）
    pub updated_at: i64,
    /// 本套自己的自定义变量
    pub variables: Vec<UserVar>,
    /// 本套自己的计算变量
    pub computed: Vec<ComputedVar>,
}

/// 一条匹配规则：正则 + 所属分组 + 分组颜色。
///
/// 兼容旧数据：反序列化时既接受 `"排队"` 这样的纯字符串（v0.1.5 及更早的
/// 数据库 / 备份），也接受 `{ "pattern": ..., "group": ..., "color": ... }`。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueRule {
    pub pattern: String,
    /// 命中这条规则时给成员打的组名（空 = 不分组）
    pub group: String,
    /// 分组色块颜色（#RRGGBB）
    pub color: String,
}

impl<'de> Deserialize<'de> for QueueRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Text(String),
            Rule {
                #[serde(default)]
                pattern: String,
                #[serde(default)]
                group: String,
                #[serde(default)]
                color: String,
            },
        }
        match Repr::deserialize(deserializer)? {
            Repr::Text(pattern) => Ok(QueueRule {
                pattern,
                group: String::new(),
                color: String::new(),
            }),
            Repr::Rule {
                pattern,
                group,
                color,
            } => Ok(QueueRule {
                pattern,
                group,
                color,
            }),
        }
    }
}

/// 排队队列里的一个成员：同一个 uid 累计发送次数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueEntry {
    pub id: i64,
    /// B 站 uid；0 表示主窗口手动添加的成员
    pub uid: i64,
    pub nickname: String,
    /// 匹配到的弹幕条数（或手动调整后的值）
    pub count: i64,
    /// 首次排队时间（毫秒时间戳）
    pub first_at: i64,
    /// 最近一次排队时间（毫秒时间戳）
    pub last_at: i64,
    /// 最近一次命中规则所属的分组（空 = 未分组）
    pub group: String,
}

/// 一个排队队列 = 自己独立的直播间、正则、排序与弹窗样式。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueConfig {
    pub id: i64,
    pub name: String,
    /// 直播间号（支持短号；0 = 未设置）
    pub room_id: i64,
    /// 弹幕批量写入 / 界面刷新间隔（秒）
    pub poll_secs: u32,
    /// 匹配弹幕文本的规则（任一命中即计数；每条规则带分组与颜色）
    ///
    /// 反序列化时兼容旧字段名 `patterns`（纯字符串数组）。
    #[serde(alias = "patterns")]
    pub rules: Vec<QueueRule>,
    /// `time` 按首次发送时间（先到先得）| `count` 按发送次数（多到少）
    pub sort_mode: String,
    /// 是否监听弹幕
    pub enabled: bool,
    /// 弹窗最多显示多少人
    pub max_entries: u32,
    /// 弹窗里是否在昵称后显示分组色块
    pub show_group: bool,
    /// 弹窗里是否对昵称做脱敏（保留首尾字符）
    pub mask_nickname: bool,
    /// 新人入队特效：none / fade / slide / zoom / pop
    pub enter_effect: String,
    /// 数字 +1 特效：none / bump / zoom / flash / glow / rise
    pub count_effect: String,
    /// 分组色块字号（px；0 = 按整体字号自动计算）
    pub group_font_size: u32,
    /// 是否显示弹窗（隐藏 ≠ 删除）
    pub shown: bool,
    pub always_on_top: bool,
    pub click_through: bool,
    /// 弹窗位置（物理像素，拖动时回写）
    pub x: i32,
    pub y: i32,
    pub font_size: u32,
    /// #RRGGBB
    pub font_color: String,
    pub bold: bool,
    pub shadow: bool,
    /// #RRGGBB
    pub bg_color: String,
    /// 背景不透明度 0-100
    pub bg_opacity: u32,
    /// 自动宽度的最大宽度（逻辑像素）
    pub max_width: i64,
    /// 手动尺寸（0 = 自动按内容）
    pub width: i64,
    pub height: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// 队列成员（按数据库 id 返回，展示顺序由 sortMode 在前端决定）
    pub entries: Vec<QueueEntry>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            room_id: 0,
            poll_secs: 3,
            rules: vec![QueueRule {
                pattern: "排队".into(),
                group: String::new(),
                color: DEFAULT_GROUP_COLOR.into(),
            }],
            sort_mode: "time".into(),
            enabled: true,
            max_entries: 20,
            show_group: false,
            mask_nickname: false,
            enter_effect: "none".into(),
            count_effect: "none".into(),
            group_font_size: 0,
            shown: true,
            always_on_top: true,
            click_through: false,
            x: 0,
            y: 0,
            font_size: 18,
            font_color: "#ffffff".into(),
            bold: true,
            shadow: true,
            bg_color: "#0f172a".into(),
            bg_opacity: 55,
            max_width: 360,
            width: 0,
            height: 0,
            created_at: 0,
            updated_at: 0,
            entries: vec![],
        }
    }
}

/// 全量配置：浮窗（套）列表 + 排队队列列表。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub overlays: Vec<OverlayConfig>,
    pub queues: Vec<QueueConfig>,
}

/// 导入备份后的统计（「增量」模式下会跳过同名浮窗、合并同名队列名单）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStats {
    pub overlays_added: usize,
    pub overlays_skipped: usize,
    pub queues_added: usize,
    pub queues_merged: usize,
    pub entries_added: usize,
    pub entries_merged: usize,
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
    width          INTEGER NOT NULL DEFAULT 0,
    height         INTEGER NOT NULL DEFAULT 0,
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
CREATE TABLE IF NOT EXISTS queues (
    id             INTEGER PRIMARY KEY,
    name           TEXT NOT NULL,
    room_id        INTEGER NOT NULL DEFAULT 0,
    poll_secs      INTEGER NOT NULL DEFAULT 3,
    patterns       TEXT NOT NULL DEFAULT '[\"排队\"]',
    sort_mode      TEXT NOT NULL DEFAULT 'time',
    enabled        INTEGER NOT NULL DEFAULT 1,
    max_entries    INTEGER NOT NULL DEFAULT 20,
    shown          INTEGER NOT NULL DEFAULT 1,
    always_on_top  INTEGER NOT NULL DEFAULT 1,
    click_through  INTEGER NOT NULL DEFAULT 0,
    x              INTEGER NOT NULL DEFAULT 0,
    y              INTEGER NOT NULL DEFAULT 0,
    font_size      INTEGER NOT NULL DEFAULT 18,
    font_color     TEXT NOT NULL DEFAULT '#ffffff',
    bold           INTEGER NOT NULL DEFAULT 1,
    shadow         INTEGER NOT NULL DEFAULT 1,
    bg_color       TEXT NOT NULL DEFAULT '#0f172a',
    bg_opacity     INTEGER NOT NULL DEFAULT 55,
    max_width      INTEGER NOT NULL DEFAULT 360,
    width          INTEGER NOT NULL DEFAULT 0,
    height         INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL DEFAULT 0,
    show_group     INTEGER NOT NULL DEFAULT 0,
    mask_nickname  INTEGER NOT NULL DEFAULT 0,
    enter_effect   TEXT NOT NULL DEFAULT 'none',
    count_effect   TEXT NOT NULL DEFAULT 'none',
    group_font_size INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS queue_entries (
    id          INTEGER PRIMARY KEY,
    queue_id    INTEGER NOT NULL,
    uid         INTEGER NOT NULL DEFAULT 0,
    nickname    TEXT NOT NULL,
    count       INTEGER NOT NULL DEFAULT 1,
    first_at    INTEGER NOT NULL,
    last_at     INTEGER NOT NULL,
    group_name  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_variables_overlay ON variables(overlay_id);
CREATE INDEX IF NOT EXISTS idx_computed_overlay  ON computed(overlay_id);
CREATE INDEX IF NOT EXISTS idx_queue_entries_queue ON queue_entries(queue_id);
";

/// 新人入队特效白名单（与前端下拉框一致）。
const ENTER_EFFECTS: &[&str] = &["none", "fade", "slide", "zoom", "pop"];
/// 数字 +1 特效白名单。
const COUNT_EFFECTS: &[&str] = &["none", "bump", "zoom", "flash", "glow", "rise"];

/// 特效名不在白名单里时回退 `none`，避免前端传入任意 CSS 类名。
fn normalize_effect(value: &str, allowed: &[&str]) -> String {
    let value = value.trim();
    if allowed.contains(&value) {
        value.to_string()
    } else {
        "none".to_string()
    }
}

/// 分组颜色统一成小写 `#rrggbb`；非法值回退默认色。
fn normalize_color(color: &str) -> String {
    let color = color.trim();
    if color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        color.to_ascii_lowercase()
    } else {
        DEFAULT_GROUP_COLOR.to_string()
    }
}

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

        // 轻量迁移：v0.1.0 起支持在浮窗上直接拖动宽高。老库补列，默认 0 = 自动。
        for column in ["width", "height"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('overlays') WHERE name = ?1",
                    params![column],
                    |r| r.get(0),
                )
                .map_err(|e| format!("检查表结构失败：{e}"))?;
            if exists == 0 {
                conn.execute_batch(&format!(
                    "ALTER TABLE overlays ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0;"
                ))
                .map_err(|e| format!("迁移 overlays.{column} 失败：{e}"))?;
            }
        }

        // 轻量迁移：v0.1.6 起排队规则带分组/颜色，弹窗可显示分组、脱敏昵称与入队特效。
        // 老库补列（新库建表时已带列，这里跳过）。
        let queue_columns = [
            ("queues", "show_group", "ALTER TABLE queues ADD COLUMN show_group INTEGER NOT NULL DEFAULT 0"),
            ("queues", "mask_nickname", "ALTER TABLE queues ADD COLUMN mask_nickname INTEGER NOT NULL DEFAULT 0"),
            ("queues", "enter_effect", "ALTER TABLE queues ADD COLUMN enter_effect TEXT NOT NULL DEFAULT 'none'"),
            ("queues", "count_effect", "ALTER TABLE queues ADD COLUMN count_effect TEXT NOT NULL DEFAULT 'none'"),
            ("queue_entries", "group_name", "ALTER TABLE queue_entries ADD COLUMN group_name TEXT NOT NULL DEFAULT ''"),
            ("queues", "group_font_size", "ALTER TABLE queues ADD COLUMN group_font_size INTEGER NOT NULL DEFAULT 0"),
        ];
        for (table, column, ddl) in queue_columns {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                    params![table, column],
                    |r| r.get(0),
                )
                .map_err(|e| format!("检查表结构失败：{e}"))?;
            if exists == 0 {
                conn.execute_batch(ddl)
                    .map_err(|e| format!("迁移 {table}.{column} 失败：{e}"))?;
            }
        }

        // 轻量迁移：v0.1.6 起成员唯一键从「uid」改为「uid + 分组」——同一个人进入
        // 不同分组时各占一行。旧索引会阻止同一 uid 出现在多个分组，必须先删；
        // 新索引必须等 group_name 补列之后再建（老库在 CREATE_SQL 阶段还没有这列）。
        // 手动添加的成员 uid = 0，按昵称区分，不参与唯一索引。
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_queue_entries_uid;
             CREATE UNIQUE INDEX IF NOT EXISTS idx_queue_entries_uid_group
                 ON queue_entries(queue_id, uid, group_name) WHERE uid <> 0;",
        )
        .map_err(|e| format!("迁移 queue_entries 唯一索引失败：{e}"))?;

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
                log::error!("数据库加锁失败：{e}");
                return Config::default();
            }
        };
        let mut overlays = Vec::new();
        let mut stmt = match conn.prepare(
            "SELECT id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, wrap, max_width, width, height, created_at, updated_at FROM overlays ORDER BY id",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::error!("查询 overlays 失败：{e}");
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
                width: row.get(17)?,
                height: row.get(18)?,
                created_at: row.get(19)?,
                updated_at: row.get(20)?,
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
        let queues = load_queues(&conn);
        Config { overlays, queues }
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

    /// 保存浮窗的手动尺寸（逻辑像素；0/0 = 恢复自动按内容）。
    pub fn set_size(&self, id: i64, width: i64, height: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE overlays SET width = ?1, height = ?2, updated_at = ?3 WHERE id = ?4",
                params![width, height, now_ms(), id],
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

    // ---------- 排队队列 ----------

    /// 新队列 id：毫秒时间戳与现有最大 id+1 取大，保证单调唯一。
    pub fn next_queue_id(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        let max: i64 = conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM queues", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        now_ms().max(max + 1)
    }

    pub fn push_queue(&self, cfg: &QueueConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        insert_queue(&conn, cfg)
    }

    pub fn queue_exists(&self, id: i64) -> bool {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return false,
        };
        conn.query_row("SELECT 1 FROM queues WHERE id = ?1", params![id], |_| {
            Ok(())
        })
        .is_ok()
    }

    /// 供弹幕工作线程读取最新配置（不含成员，避免每次轮询都查一遍名单）。
    pub fn get_queue(&self, id: i64) -> Option<QueueConfig> {
        let conn = self.conn.lock().ok()?;
        let mut cfg = conn
            .query_row(
                "SELECT id, name, room_id, poll_secs, patterns, sort_mode, enabled, max_entries, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, max_width, width, height, created_at, updated_at, show_group, mask_nickname, enter_effect, count_effect, group_font_size FROM queues WHERE id = ?1",
                params![id],
                map_queue_row,
            )
            .optional()
            .ok()??;
        cfg.entries = vec![];
        Some(cfg)
    }

    /// 更新队列的直播间、规则、排序与弹窗样式（不动位置与显隐开关）。
    pub fn update_queue(&self, cfg: &QueueConfig) -> Result<(), String> {
        let rules = Self::validate_rules(&cfg.rules)?;
        let rules_json = serde_json::to_string(&rules).map_err(|e| e.to_string())?;
        let sort_mode = if cfg.sort_mode == "count" { "count" } else { "time" };
        let room_id = cfg.room_id.max(0);
        let poll_secs = cfg.poll_secs.clamp(1, 60) as i64;
        let max_entries = cfg.max_entries.clamp(1, 100) as i64;
        let font_size = cfg.font_size.clamp(10, 72) as i64;
        let bg_opacity = cfg.bg_opacity.clamp(0, 100) as i64;
        let max_width = if cfg.max_width <= 0 {
            360
        } else {
            cfg.max_width.clamp(80, 3000)
        };
        let enter_effect = normalize_effect(&cfg.enter_effect, ENTER_EFFECTS);
        let count_effect = normalize_effect(&cfg.count_effect, COUNT_EFFECTS);
        let group_font_size = cfg.group_font_size.clamp(0, 72) as i64;
        let name = cfg.name.trim();

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE queues SET name = ?1, room_id = ?2, poll_secs = ?3, patterns = ?4, sort_mode = ?5, enabled = ?6, max_entries = ?7, font_size = ?8, font_color = ?9, bold = ?10, shadow = ?11, bg_color = ?12, bg_opacity = ?13, max_width = ?14, show_group = ?15, mask_nickname = ?16, enter_effect = ?17, count_effect = ?18, group_font_size = ?19, updated_at = ?20 WHERE id = ?21",
                params![
                    name,
                    room_id,
                    poll_secs,
                    rules_json,
                    sort_mode,
                    cfg.enabled as i64,
                    max_entries,
                    font_size,
                    cfg.font_color,
                    cfg.bold as i64,
                    cfg.shadow as i64,
                    cfg.bg_color,
                    bg_opacity,
                    max_width,
                    cfg.show_group as i64,
                    cfg.mask_nickname as i64,
                    enter_effect,
                    count_effect,
                    group_font_size,
                    now_ms(),
                    cfg.id
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("队列 #{} 不存在", cfg.id));
        }
        Ok(())
    }

    /// 校验并清洗规则：去掉空正则行，全部编译一遍；至少保留一条。
    /// 颜色统一成小写 `#rrggbb`，非法值回退默认色。
    pub fn validate_rules(rules: &[QueueRule]) -> Result<Vec<QueueRule>, String> {
        let mut cleaned = Vec::new();
        for (i, r) in rules.iter().enumerate() {
            let pattern = r.pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            Regex::new(pattern).map_err(|e| format!("第 {} 条正则不合法：{e}", i + 1))?;
            cleaned.push(QueueRule {
                pattern: pattern.to_string(),
                group: r.group.trim().to_string(),
                color: normalize_color(&r.color),
            });
        }
        if cleaned.is_empty() {
            return Err("至少保留一个正则表达式（例如：排队）".into());
        }
        Ok(cleaned)
    }

    pub fn set_queue_enabled(&self, id: i64, value: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE queues SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                params![value as i64, now_ms(), id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("队列 #{id} 不存在"));
        }
        Ok(())
    }

    pub fn set_queue_flag(
        &self,
        id: i64,
        apply: impl FnOnce(&mut QueueConfig),
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let (shown, top, through): (i64, i64, i64) = conn
            .query_row(
                "SELECT shown, always_on_top, click_through FROM queues WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| format!("队列 #{id} 不存在"))?;
        let mut probe = QueueConfig {
            shown: shown != 0,
            always_on_top: top != 0,
            click_through: through != 0,
            ..Default::default()
        };
        apply(&mut probe);
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE queues SET shown = ?2, always_on_top = ?3, click_through = ?4, updated_at = ?5 WHERE id = ?1",
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

    pub fn move_queue(&self, id: i64, x: i32, y: i32) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE queues SET x = ?1, y = ?2 WHERE id = ?3",
                params![x, y, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("队列 #{id} 不存在"));
        }
        Ok(())
    }

    /// 拖动弹窗时由 Moved 事件回写位置（不刷新修改时间）。
    pub fn set_queue_position(&self, id: i64, x: i32, y: i32) {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return,
        };
        let _ = conn.execute(
            "UPDATE queues SET x = ?1, y = ?2 WHERE id = ?3",
            params![x, y, id],
        );
    }

    /// 保存队列弹窗的手动宽高（0/0 = 恢复自动按内容）。
    pub fn set_queue_size(&self, id: i64, width: i64, height: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE queues SET width = ?1, height = ?2, updated_at = ?3 WHERE id = ?4",
                params![width, height, now_ms(), id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("队列 #{id} 不存在"));
        }
        Ok(())
    }

    pub fn delete_queue(&self, id: i64) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM queue_entries WHERE queue_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM queues WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 主窗口手动添加成员：同一「昵称 + 分组」已有成员则累加次数，否则新建（uid = 0）。
    pub fn add_queue_entry(
        &self,
        queue_id: i64,
        nickname: &str,
        count: i64,
        group: &str,
    ) -> Result<(), String> {
        let nickname = nickname.trim();
        if nickname.is_empty() {
            return Err("昵称不能为空".into());
        }
        if !self.queue_exists(queue_id) {
            return Err("队列不存在，先创建队列".into());
        }
        let group = group.trim();
        let add = count.clamp(1, 999_999);
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM queue_entries WHERE queue_id = ?1 AND uid = 0 AND nickname = ?2 AND group_name = ?3 ORDER BY id LIMIT 1",
                params![queue_id, nickname, group],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let now = now_ms();
        if let Some(id) = existing {
            tx.execute(
                "UPDATE queue_entries SET count = count + ?1, last_at = ?2 WHERE id = ?3",
                params![add, now, id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            tx.execute(
                "INSERT INTO queue_entries (queue_id, uid, nickname, count, first_at, last_at, group_name) VALUES (?1, 0, ?2, ?3, ?4, ?4, ?5)",
                params![queue_id, nickname, add, now, group],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 原子增减次数（下限 0），避免和弹幕实时累加互相覆盖。
    pub fn step_queue_entry(&self, queue_id: i64, id: i64, delta: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE queue_entries SET count = MAX(0, count + ?1), last_at = ?2 WHERE id = ?3 AND queue_id = ?4",
                params![delta, now_ms(), id, queue_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("成员 #{id} 不存在（或不属于该队列）"));
        }
        Ok(())
    }

    /// 直接指定次数（用于输入框精确修改）。
    pub fn set_queue_entry_count(
        &self,
        queue_id: i64,
        id: i64,
        count: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE queue_entries SET count = ?1, last_at = ?2 WHERE id = ?3 AND queue_id = ?4",
                params![count.clamp(0, 999_999), now_ms(), id, queue_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("成员 #{id} 不存在（或不属于该队列）"));
        }
        Ok(())
    }

    pub fn rename_queue_entry(
        &self,
        queue_id: i64,
        id: i64,
        nickname: &str,
    ) -> Result<(), String> {
        let nickname = nickname.trim();
        if nickname.is_empty() {
            return Err("昵称不能为空".into());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE queue_entries SET nickname = ?1 WHERE id = ?2 AND queue_id = ?3",
                params![nickname, id, queue_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("成员 #{id} 不存在（或不属于该队列）"));
        }
        Ok(())
    }

    /// 修改成员所属分组。唯一键是「uid + 分组」：目标分组里若已有同一个人，
    /// 就把次数合并过去（次数相加、时间取并集）并删除当前行，避免同键两条记录。
    /// 手动添加的成员（uid = 0）按昵称判断是否同一人。
    pub fn set_queue_entry_group(
        &self,
        queue_id: i64,
        id: i64,
        group: &str,
    ) -> Result<(), String> {
        let group = group.trim();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let row: Option<(i64, String, i64, i64, i64)> = tx
            .query_row(
                "SELECT uid, nickname, count, first_at, last_at FROM queue_entries WHERE id = ?1 AND queue_id = ?2",
                params![id, queue_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((uid, nickname, count, first_at, last_at)) = row else {
            return Err(format!("成员 #{id} 不存在（或不属于该队列）"));
        };
        let conflict: Option<i64> = if uid != 0 {
            tx.query_row(
                "SELECT id FROM queue_entries WHERE queue_id = ?1 AND uid = ?2 AND group_name = ?3 AND id <> ?4",
                params![queue_id, uid, group, id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        } else {
            tx.query_row(
                "SELECT id FROM queue_entries WHERE queue_id = ?1 AND uid = 0 AND nickname = ?2 AND group_name = ?3 AND id <> ?4",
                params![queue_id, nickname, group, id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        };
        if let Some(target) = conflict {
            tx.execute(
                "UPDATE queue_entries SET count = MIN(count + ?1, 999999), first_at = MIN(first_at, ?2), last_at = MAX(last_at, ?3) WHERE id = ?4",
                params![count, first_at, last_at, target],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM queue_entries WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        } else {
            tx.execute(
                "UPDATE queue_entries SET group_name = ?1 WHERE id = ?2 AND queue_id = ?3",
                params![group, id, queue_id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_queue_entry(&self, queue_id: i64, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM queue_entries WHERE id = ?1 AND queue_id = ?2",
            params![id, queue_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear_queue_entries(&self, queue_id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM queue_entries WHERE queue_id = ?1",
            params![queue_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 弹幕工作线程匹配到命中后调用：同一 uid 累加次数，新用户插入。
    /// `hits` 每项是 `(uid, 昵称, 弹幕时间, 命中规则的分组)`；分组为空表示不分组。
    /// 返回实际写入的命中条数（0 = 队列已在其它窗口被删除，忽略）。
    pub fn record_queue_matches(
        &self,
        queue_id: i64,
        hits: &[(i64, String, i64, String)],
    ) -> Result<usize, String> {
        if hits.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let exists: Option<i64> = tx
            .query_row("SELECT 1 FROM queues WHERE id = ?1", params![queue_id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_none() {
            return Ok(0);
        }
        let now = now_ms();
        let mut changed = 0usize;
        for (uid, nickname, at, group) in hits {
            // uid==0 是「手动添加成员」的占位值，不接受弹幕写入；
            // 负数 uid 是弹幕层为游客（B 站返回 uid=0）按昵称生成的虚拟 UID，
            // 必须放行，否则游客发的弹幕永远不 +1。
            if *uid == 0 {
                continue;
            }
            let nickname = nickname.trim();
            if nickname.is_empty() {
                continue;
            }
            let group = group.trim();
            let at = if *at > 0 { *at } else { now };
            // 唯一键是「uid + 分组」：同一个人进入不同分组时各占一行、各自累计。
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM queue_entries WHERE queue_id = ?1 AND uid = ?2 AND group_name = ?3",
                    params![queue_id, uid, group],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some(id) = existing {
                tx.execute(
                    "UPDATE queue_entries SET count = count + 1, nickname = ?1, last_at = MAX(last_at, ?2), first_at = MIN(first_at, ?2) WHERE id = ?3",
                    params![nickname, at, id],
                )
                .map_err(|e| e.to_string())?;
            } else {
                tx.execute(
                    "INSERT INTO queue_entries (queue_id, uid, nickname, count, first_at, last_at, group_name) VALUES (?1, ?2, ?3, 1, ?4, ?4, ?5)",
                    params![queue_id, uid, nickname, at, group],
                )
                .map_err(|e| e.to_string())?;
            }
            changed += 1;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed)
    }

    // ---------- 备份导入 ----------

    /// 覆盖导入：清空现有浮窗/变量/计算变量/队列/名单，再按备份重建。
    /// 所有记录都重新分配 id，避免备份里的旧 id 与库内其它数据撞车。
    pub fn replace_all(&self, config: &Config) -> Result<ImportStats, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM variables", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM computed", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM queue_entries", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM overlays", [])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM queues", [])
            .map_err(|e| e.to_string())?;

        let mut stats = ImportStats::default();
        let mut overlay_id = next_id(&tx, "overlays")?;
        for cfg in &config.overlays {
            insert_overlay_full(&tx, cfg, overlay_id)?;
            overlay_id += 1;
            stats.overlays_added += 1;
        }
        let mut queue_id = next_id(&tx, "queues")?;
        for cfg in &config.queues {
            insert_queue_full(&tx, cfg, queue_id)?;
            queue_id += 1;
            stats.queues_added += 1;
            stats.entries_added += cfg.entries.len();
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(stats)
    }

    /// 增量导入：保留现有数据，只新增本地没有的同名浮窗 / 队列。
    /// - 同名浮窗：跳过（不覆盖现有配置）；
    /// - 同名队列：把备份名单合并进去（同 UID 次数取较大值，时间取并集）；
    /// - 新队列：整条新增，名单一并导入。
    pub fn merge_from(&self, config: &Config) -> Result<ImportStats, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut stats = ImportStats::default();

        // 现有浮窗名（去首尾空白）→ 同名视为同一套，增量模式不覆盖
        let mut overlay_names: HashSet<String> = {
            let mut stmt = tx
                .prepare("SELECT name FROM overlays")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok())
                .map(|n| n.trim().to_string())
                .collect()
        };

        let mut overlay_id = next_id(&tx, "overlays")?;
        for cfg in &config.overlays {
            let name = cfg.name.trim().to_string();
            if name.is_empty() || overlay_names.contains(&name) {
                stats.overlays_skipped += 1;
                continue;
            }
            insert_overlay_full(&tx, cfg, overlay_id)?;
            overlay_id += 1;
            overlay_names.insert(name);
            stats.overlays_added += 1;
        }

        // 现有队列名 → id，同名队列只合并名单
        let mut queue_ids: HashMap<String, i64> = {
            let mut stmt = tx
                .prepare("SELECT id, name FROM queues")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok())
                .map(|(id, name)| (name.trim().to_string(), id))
                .collect()
        };

        let mut queue_id = next_id(&tx, "queues")?;
        for cfg in &config.queues {
            let name = cfg.name.trim().to_string();
            if let Some(existing) = queue_ids.get(&name) {
                let (added, merged) = merge_entries(&tx, *existing, &cfg.entries)?;
                stats.entries_added += added;
                stats.entries_merged += merged;
                stats.queues_merged += 1;
                continue;
            }
            insert_queue_full(&tx, cfg, queue_id)?;
            queue_id += 1;
            if !name.is_empty() {
                queue_ids.insert(name, queue_id - 1);
            }
            stats.queues_added += 1;
            stats.entries_added += cfg.entries.len();
        }

        tx.commit().map_err(|e| e.to_string())?;
        Ok(stats)
    }
}

/// 下一个可用 id：当前时间戳与现有最大 id + 1 取大（与新建浮窗/队列一致）。
fn next_id(conn: &Connection, table: &str) -> Result<i64, String> {
    let max: i64 = conn
        .query_row(&format!("SELECT COALESCE(MAX(id), 0) FROM {table}"), [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    Ok(now_ms().max(max + 1))
}

/// 写入一套浮窗及其变量/计算变量（导入共用，id 由调用方分配）。
fn insert_overlay_full(conn: &Connection, cfg: &OverlayConfig, new_id: i64) -> Result<(), String> {
    let mut overlay = cfg.clone();
    overlay.id = new_id;
    insert_overlay(conn, &overlay)?;

    // 变量 id 会重新分配，联动关系要按旧 id → 新 id 映射后回填
    let mut id_map: HashMap<i64, i64> = HashMap::new();
    let fallback_created = now_ms();
    for (i, v) in cfg.variables.iter().enumerate() {
        conn.execute(
            "INSERT INTO variables (overlay_id, key, name, value, step, follow_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            params![
                new_id,
                v.key,
                v.name,
                v.value,
                v.step,
                if v.created_at > 0 {
                    v.created_at
                } else {
                    fallback_created + i as i64
                }
            ],
        )
        .map_err(|e| e.to_string())?;
        id_map.insert(v.id, conn.last_insert_rowid());
    }
    for v in &cfg.variables {
        if v.follow_id == 0 {
            continue;
        }
        let Some(new_follow) = id_map.get(&v.follow_id) else {
            continue;
        };
        conn.execute(
            "UPDATE variables SET follow_id = ?1 WHERE overlay_id = ?2 AND key = ?3",
            params![new_follow, new_id, v.key],
        )
        .map_err(|e| e.to_string())?;
    }

    for c in &cfg.computed {
        conn.execute(
            "INSERT INTO computed (overlay_id, key, name, expr, format, decimals, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                new_id,
                c.key,
                c.name,
                c.expr,
                c.format,
                c.decimals,
                if c.created_at > 0 {
                    c.created_at
                } else {
                    fallback_created
                }
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 写入一条队列及其全部名单（导入共用）。
fn insert_queue_full(conn: &Connection, cfg: &QueueConfig, new_id: i64) -> Result<(), String> {
    let mut queue = cfg.clone();
    queue.id = new_id;
    insert_queue(conn, &queue)?;
    merge_entries(conn, new_id, &cfg.entries)?;
    Ok(())
}

/// 把备份名单合并进队列：同 UID 保留较大次数、时间取并集；手动成员按昵称匹配。
/// 返回（新增条数，合并条数）。
fn merge_entries(
    conn: &Connection,
    queue_id: i64,
    entries: &[QueueEntry],
) -> Result<(usize, usize), String> {
    let mut added = 0usize;
    let mut merged = 0usize;
    for e in entries {
        let nickname = e.nickname.trim();
        if nickname.is_empty() {
            continue;
        }
        let group = e.group.trim();
        // 唯一键是「uid + 分组」；手动成员（uid = 0）按昵称 + 分组匹配。
        // 游客的虚拟 UID 为负数，导入时按手动成员处理（与旧行为一致）。
        let existing: Option<(i64, i64, i64)> = if e.uid > 0 {
            conn.query_row(
                "SELECT id, count, first_at FROM queue_entries WHERE queue_id = ?1 AND uid = ?2 AND group_name = ?3",
                params![queue_id, e.uid, group],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|err| err.to_string())?
        } else {
            conn.query_row(
                "SELECT id, count, first_at FROM queue_entries WHERE queue_id = ?1 AND uid = 0 AND nickname = ?2 AND group_name = ?3",
                params![queue_id, nickname, group],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|err| err.to_string())?
        };
        let first_at = if e.first_at > 0 { e.first_at } else { now_ms() };
        let last_at = if e.last_at > 0 { e.last_at } else { first_at };
        if let Some((id, _, _)) = existing {
            conn.execute(
                "UPDATE queue_entries SET count = MAX(count, ?1), first_at = MIN(first_at, ?2), last_at = MAX(last_at, ?3) WHERE id = ?4",
                params![e.count.max(1), first_at, last_at, id],
            )
            .map_err(|err| err.to_string())?;
            merged += 1;
        } else {
            conn.execute(
                "INSERT INTO queue_entries (queue_id, uid, nickname, count, first_at, last_at, group_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    queue_id,
                    e.uid.max(0),
                    nickname,
                    e.count.max(1),
                    first_at,
                    last_at,
                    group
                ],
            )
            .map_err(|err| err.to_string())?;
            added += 1;
        }
    }
    Ok((added, merged))
}

/// 写入一条浮窗记录（新浮窗与空库初始化共用）。
fn insert_overlay(conn: &Connection, cfg: &OverlayConfig) -> Result<(), String> {
    conn.execute(
        "INSERT INTO overlays (id, name, account, template, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, wrap, max_width, width, height, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
        params![
            cfg.id, cfg.name, cfg.account, cfg.template,
            cfg.shown as i64, cfg.always_on_top as i64, cfg.click_through as i64,
            cfg.x, cfg.y, cfg.font_size as i64, cfg.font_color,
            cfg.bold as i64, cfg.shadow as i64, cfg.bg_color, cfg.bg_opacity as i64,
            cfg.wrap as i64, cfg.max_width, cfg.width, cfg.height, cfg.created_at, cfg.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 写入一条排队队列记录。
fn insert_queue(conn: &Connection, cfg: &QueueConfig) -> Result<(), String> {
    // 导入旧备份时规则可能缺颜色 / 带空白：这里做一次宽松清洗，保证落库格式统一。
    let mut rules: Vec<QueueRule> = cfg
        .rules
        .iter()
        .map(|r| QueueRule {
            pattern: r.pattern.trim().to_string(),
            group: r.group.trim().to_string(),
            color: normalize_color(&r.color),
        })
        .filter(|r| !r.pattern.is_empty())
        .collect();
    if rules.is_empty() {
        rules.push(QueueRule {
            pattern: "排队".into(),
            group: String::new(),
            color: DEFAULT_GROUP_COLOR.into(),
        });
    }
    let rules = serde_json::to_string(&rules).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO queues (id, name, room_id, poll_secs, patterns, sort_mode, enabled, max_entries, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, max_width, width, height, created_at, updated_at, show_group, mask_nickname, enter_effect, count_effect, group_font_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)",
        params![
            cfg.id, cfg.name, cfg.room_id, cfg.poll_secs as i64, rules, cfg.sort_mode,
            cfg.enabled as i64, cfg.max_entries as i64, cfg.shown as i64,
            cfg.always_on_top as i64, cfg.click_through as i64, cfg.x, cfg.y,
            cfg.font_size as i64, cfg.font_color, cfg.bold as i64, cfg.shadow as i64,
            cfg.bg_color, cfg.bg_opacity as i64, cfg.max_width, cfg.width, cfg.height,
            cfg.created_at, cfg.updated_at, cfg.show_group as i64, cfg.mask_nickname as i64,
            normalize_effect(&cfg.enter_effect, ENTER_EFFECTS),
            normalize_effect(&cfg.count_effect, COUNT_EFFECTS),
            cfg.group_font_size.clamp(0, 72) as i64
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn map_queue_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueConfig> {
    let patterns: String = row.get(4)?;
    Ok(QueueConfig {
        id: row.get(0)?,
        name: row.get(1)?,
        room_id: row.get(2)?,
        poll_secs: row.get::<_, i64>(3)?.max(1) as u32,
        rules: serde_json::from_str(&patterns).unwrap_or_else(|_| {
            vec![QueueRule {
                pattern: "排队".into(),
                group: String::new(),
                color: DEFAULT_GROUP_COLOR.into(),
            }]
        }),
        sort_mode: row.get(5)?,
        enabled: row.get::<_, i64>(6)? != 0,
        max_entries: row.get::<_, i64>(7)?.max(1) as u32,
        shown: row.get::<_, i64>(8)? != 0,
        always_on_top: row.get::<_, i64>(9)? != 0,
        click_through: row.get::<_, i64>(10)? != 0,
        x: row.get(11)?,
        y: row.get(12)?,
        font_size: row.get::<_, i64>(13)?.max(10) as u32,
        font_color: row.get(14)?,
        bold: row.get::<_, i64>(15)? != 0,
        shadow: row.get::<_, i64>(16)? != 0,
        bg_color: row.get(17)?,
        bg_opacity: row.get::<_, i64>(18)?.clamp(0, 100) as u32,
        max_width: row.get(19)?,
        width: row.get(20)?,
        height: row.get(21)?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
        show_group: row.get::<_, i64>(24)? != 0,
        mask_nickname: row.get::<_, i64>(25)? != 0,
        enter_effect: row.get(26)?,
        count_effect: row.get(27)?,
        group_font_size: row.get::<_, i64>(28)?.clamp(0, 72) as u32,
        entries: vec![],
    })
}

/// 加载所有排队队列（含成员）。
fn load_queues(conn: &Connection) -> Vec<QueueConfig> {
    let mut stmt = match conn.prepare(
        "SELECT id, name, room_id, poll_secs, patterns, sort_mode, enabled, max_entries, shown, always_on_top, click_through, x, y, font_size, font_color, bold, shadow, bg_color, bg_opacity, max_width, width, height, created_at, updated_at, show_group, mask_nickname, enter_effect, count_effect, group_font_size FROM queues ORDER BY id",
    ) {
        Ok(s) => s,
        Err(e) => {
            log::error!("查询 queues 失败：{e}");
            return vec![];
        }
    };
    let rows = stmt.query_map([], map_queue_row);
    match rows {
        Ok(rows) => rows
            .filter_map(|r| r.ok())
            .map(|mut cfg| {
                cfg.entries = load_queue_entries(conn, cfg.id);
                cfg
            })
            .collect(),
        Err(e) => {
            log::error!("读取 queues 失败：{e}");
            vec![]
        }
    }
}

fn load_queue_entries(conn: &Connection, queue_id: i64) -> Vec<QueueEntry> {
    let mut stmt = match conn.prepare(
        "SELECT id, uid, nickname, count, first_at, last_at, group_name FROM queue_entries WHERE queue_id = ?1 ORDER BY id",
    ) {
        Ok(s) => s,
        Err(e) => {
            log::error!("查询 queue_entries 失败：{e}");
            return vec![];
        }
    };
    stmt.query_map(params![queue_id], |row| {
        Ok(QueueEntry {
            id: row.get(0)?,
            uid: row.get(1)?,
            nickname: row.get(2)?,
            count: row.get(3)?,
            first_at: row.get(4)?,
            last_at: row.get(5)?,
            group: row.get(6)?,
        })
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_else(|e| {
        log::error!("读取 queue_entries 失败：{e}");
        vec![]
    })
}

/// 新排队队列的默认配置（位置由命令层根据主显示器计算后填入）。
pub fn default_queue(id: i64, created_at: i64) -> QueueConfig {
    QueueConfig {
        id,
        name: "排队队列".into(),
        room_id: 0,
        poll_secs: 3,
        rules: vec![QueueRule {
            pattern: "排队".into(),
            group: String::new(),
            color: DEFAULT_GROUP_COLOR.into(),
        }],
        sort_mode: "time".into(),
        enabled: true,
        max_entries: 20,
        show_group: false,
        mask_nickname: false,
        enter_effect: "none".into(),
        count_effect: "none".into(),
        group_font_size: 0,
        shown: true,
        always_on_top: true,
        click_through: false,
        x: 0,
        y: 0,
        font_size: 18,
        font_color: "#ffffff".into(),
        bold: true,
        shadow: true,
        bg_color: "#0f172a".into(),
        bg_opacity: 55,
        max_width: 360,
        width: 0,
        height: 0,
        created_at,
        updated_at: created_at,
        entries: vec![],
    }
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
        width: 0,
        height: 0,
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
            log::error!("查询 variables 失败：{e}");
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
        log::error!("读取 variables 失败：{e}");
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
            log::error!("查询 computed 失败：{e}");
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
        log::error!("读取 computed 失败：{e}");
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
    fn replace_all_rebuilds_overlays_and_queues() {
        let (store, dir) = test_store();
        let mut snapshot = store.get();
        snapshot.overlays[0].name = "导入浮窗".into();
        snapshot.overlays[0].variables[0].value = 12.0;
        let qid = store.next_queue_id();
        let mut queue = default_queue(qid, now_ms());
        queue.name = "导入队列".into();
        queue.entries = vec![QueueEntry {
            uid: 1001,
            nickname: "阿一".into(),
            count: 3,
            first_at: now_ms(),
            last_at: now_ms(),
            ..Default::default()
        }];
        snapshot.queues.push(queue);

        let stats = store.replace_all(&snapshot).unwrap();
        assert_eq!(stats.overlays_added, 1);
        assert_eq!(stats.queues_added, 1);

        let after = store.get();
        assert_eq!(after.overlays.len(), 1);
        assert_eq!(after.overlays[0].name, "导入浮窗");
        assert_eq!(after.overlays[0].variables[0].value, 12.0);
        assert_eq!(after.queues.len(), 1);
        assert_eq!(after.queues[0].entries.len(), 1);
        cleanup(store, dir);
    }

    #[test]
    fn merge_from_skips_same_overlay_and_merges_queue_entries() {
        let (store, dir) = test_store();
        let existing_name = store.get().overlays[0].name.clone();

        let qid = store.next_queue_id();
        let mut queue = default_queue(qid, now_ms());
        queue.name = "直播排队".into();
        store.push_queue(&queue).unwrap();
        store
            .record_queue_matches(qid, &[(1001, "阿一".into(), now_ms(), String::new())])
            .unwrap();

        let mut backup_overlay = store.get().overlays[0].clone();
        backup_overlay.name = existing_name;
        let mut backup_queue = queue.clone();
        backup_queue.entries = vec![
            QueueEntry {
                uid: 1001,
                nickname: "阿一".into(),
                count: 5,
                first_at: 0,
                last_at: 0,
                ..Default::default()
            },
            QueueEntry {
                uid: 1002,
                nickname: "阿二".into(),
                count: 2,
                first_at: 0,
                last_at: 0,
                ..Default::default()
            },
        ];
        let backup = Config {
            overlays: vec![backup_overlay],
            queues: vec![backup_queue],
        };

        let stats = store.merge_from(&backup).unwrap();
        assert_eq!(stats.overlays_added, 0);
        assert_eq!(stats.overlays_skipped, 1);
        assert_eq!(stats.queues_merged, 1);
        assert_eq!(stats.entries_merged, 1);
        assert_eq!(stats.entries_added, 1);

        let after = store.get();
        assert_eq!(after.overlays.len(), 1);
        assert_eq!(after.queues.len(), 1);
        let merged = after.queues.iter().find(|q| q.name == "直播排队").unwrap();
        let one = merged.entries.iter().find(|e| e.uid == 1001).unwrap();
        assert_eq!(one.count, 5);
        assert!(merged.entries.iter().any(|e| e.uid == 1002 && e.count == 2));
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

#[cfg(test)]
mod queue_tests {
    use super::*;

    /// 回归：v0.1.5 及更早的备份 / 数据库里规则是纯字符串数组，
    /// 新版本必须能原样读入（分组、颜色留空），不能因为格式变化丢规则。
    #[test]
    fn legacy_patterns_json_is_accepted() {
        let legacy = r#"{"id":1,"name":"旧队列","patterns":["排队","^上车$"],"entries":[]}"#;
        let cfg: QueueConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(cfg.rules.len(), 2);
        assert_eq!(cfg.rules[0].pattern, "排队");
        assert!(cfg.rules[0].group.is_empty());
        assert!(!cfg.show_group);
        assert_eq!(cfg.enter_effect, "none");
        assert_eq!(cfg.group_font_size, 0, "旧备份缺字段时字号应为 0（自动）");

        let entry: QueueEntry = serde_json::from_str(
            r#"{"id":1,"uid":1001,"nickname":"阿一","count":1,"firstAt":0,"lastAt":0}"#,
        )
        .unwrap();
        assert!(entry.group.is_empty());
    }

    #[test]
    fn queue_crud_roundtrip() {
        let dir = std::env::temp_dir().join(format!("izuna_queue_test_{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let store = Store::new(db.clone()).unwrap();
        let id = store.next_queue_id();
        let cfg = default_queue(id, now_ms());
        store.push_queue(&cfg).unwrap();
        assert!(store.queue_exists(id));

        let base = now_ms() - 10_000;
        store
            .record_queue_matches(
                id,
                &[(1001, "阿一".into(), base, String::new()), (1002, "阿二".into(), base + 1000, String::new())],
            )
            .unwrap();
        store
            .record_queue_matches(id, &[(1001, "阿一改名".into(), base + 2000, "一组".into())])
            .unwrap();

        let q = store.get_queue(id).unwrap();
        assert_eq!(q.rules.len(), 1);
        assert_eq!(q.rules[0].pattern, "排队");
        let all = store.get().queues;
        let q = all.iter().find(|q| q.id == id).unwrap();
        // uid + 分组是累计键：同一个人进入「一组」后单独占一行，旧行仍保留。
        assert_eq!(q.entries.len(), 3);
        let one = q.entries.iter().find(|e| e.uid == 1001 && e.group.is_empty()).unwrap();
        assert_eq!(one.count, 1);
        assert_eq!(one.nickname, "阿一");
        assert_eq!(one.first_at, base);
        assert_eq!(one.last_at, base);
        let grouped = q.entries.iter().find(|e| e.uid == 1001 && e.group == "一组").unwrap();
        assert_eq!((grouped.count, grouped.nickname.as_str()), (1, "阿一改名"));
        assert_eq!((grouped.first_at, grouped.last_at), (base + 2000, base + 2000));

        store.step_queue_entry(id, one.id, -1).unwrap();
        store.set_queue_entry_count(id, one.id, 7).unwrap();
        store.rename_queue_entry(id, one.id, "新名字").unwrap();
        let q = store.get().queues.into_iter().find(|q| q.id == id).unwrap();
        let one = q.entries.iter().find(|e| e.uid == 1001).unwrap();
        assert_eq!((one.count, one.nickname.as_str()), (7, "新名字"));

        let mut edit = q.clone();
        edit.rules = vec![
            QueueRule {
                pattern: "排队".into(),
                group: "候车".into(),
                color: "#3b82f6".into(),
            },
            QueueRule {
                pattern: "^上车$".into(),
                group: "上车".into(),
                color: "#ef4444".into(),
            },
        ];
        edit.sort_mode = "count".into();
        edit.name = "测试队列".into();
        edit.group_font_size = 22;
        store.update_queue(&edit).unwrap();
        let q = store.get_queue(id).unwrap();
        assert_eq!(q.name, "测试队列");
        assert_eq!(q.rules.len(), 2);
        assert_eq!(q.rules[1].group, "上车");
        assert_eq!(q.sort_mode, "count");
        assert_eq!(q.group_font_size, 22);

        store.add_queue_entry(id, "手动成员", 3, "").unwrap();
        store.add_queue_entry(id, "手动成员", 2, "").unwrap();
        let q = store.get().queues.into_iter().find(|q| q.id == id).unwrap();
        let manual = q.entries.iter().find(|e| e.nickname == "手动成员").unwrap();
        assert_eq!(manual.count, 5);

        store.delete_queue_entry(id, one.id).unwrap();
        store.clear_queue_entries(id).unwrap();
        let q = store.get().queues.into_iter().find(|q| q.id == id).unwrap();
        assert!(q.entries.is_empty());

        store.delete_queue(id).unwrap();
        assert!(!store.queue_exists(id));
        drop(store);
        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_file(db.with_extension("db-wal"));
        let _ = std::fs::remove_file(db.with_extension("db-shm"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：一条弹幕命中多条规则时，弹幕线程会为每条命中规则各提交一条 hit。
    /// 同分组累加到同一行，不同分组各占一行且都 +1。
    #[test]
    fn multiple_rule_hits_are_all_counted() {
        let dir = std::env::temp_dir().join(format!("izuna_multi_hit_test_{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let store = Store::new(db.clone()).unwrap();
        let id = store.next_queue_id();
        store.push_queue(&default_queue(id, now_ms())).unwrap();
        let base = now_ms() - 1_000;

        // 模拟一条弹幕同时命中两条「一组」规则和一条「二组」规则
        store
            .record_queue_matches(
                id,
                &[
                    (1001, "阿一".into(), base, "一组".into()),
                    (1001, "阿一".into(), base, "一组".into()),
                    (1001, "阿一".into(), base, "二组".into()),
                ],
            )
            .unwrap();

        let q = store.get().queues.into_iter().find(|q| q.id == id).unwrap();
        assert_eq!(q.entries.len(), 2);
        let first = q.entries.iter().find(|e| e.group == "一组").unwrap();
        let second = q.entries.iter().find(|e| e.group == "二组").unwrap();
        assert_eq!((first.count, second.count), (2, 1));
        assert_eq!(first.uid, 1001);
        assert_eq!(second.uid, 1001);

        store.delete_queue(id).unwrap();
        drop(store);
        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_file(db.with_extension("db-wal"));
        let _ = std::fs::remove_file(db.with_extension("db-shm"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：多个队列同时监听时，命中写入必须严格按 queue_id 隔离，
    /// 不能出现「A 的弹幕记到 B」「一个队列停住另一个也不计数」这类串扰。
    #[test]
    fn multi_queue_matches_are_isolated() {
        let dir = std::env::temp_dir().join(format!("izuna_multi_queue_test_{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let store = Store::new(db.clone()).unwrap();

        let id_a = store.next_queue_id();
        store.push_queue(&default_queue(id_a, now_ms())).unwrap();
        let id_b = store.next_queue_id();
        store.push_queue(&default_queue(id_b, now_ms())).unwrap();
        assert_ne!(id_a, id_b);

        let base = now_ms() - 5_000;
        // 两个队列各自写入，且共用同一个 uid：计数与昵称都应各自独立
        assert_eq!(
            store
                .record_queue_matches(id_a, &[(1001, "阿一".into(), base, String::new())])
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .record_queue_matches(id_b, &[(1001, "阿一".into(), base, String::new())])
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .record_queue_matches(id_a, &[(1001, "阿一改名".into(), base + 1, String::new())])
                .unwrap(),
            1
        );

        let all = store.get().queues;
        let qa = all.iter().find(|q| q.id == id_a).unwrap();
        let qb = all.iter().find(|q| q.id == id_b).unwrap();
        assert_eq!(qa.entries.len(), 1);
        assert_eq!(qb.entries.len(), 1);
        let ea = qa.entries.iter().find(|e| e.uid == 1001).unwrap();
        let eb = qb.entries.iter().find(|e| e.uid == 1001).unwrap();
        assert_eq!(ea.count, 2);
        assert_eq!(eb.count, 1, "队列 B 不应被队列 A 的写入影响");
        assert_eq!(ea.nickname, "阿一改名");
        assert_eq!(eb.nickname, "阿一", "队列 B 的昵称不应被队列 A 覆盖");

        // 删除其中一个队列，另一个仍应正常计数（模拟用户「删除重开」）
        store.delete_queue(id_a).unwrap();
        assert_eq!(
            store
                .record_queue_matches(id_a, &[(1002, "已删队列".into(), base, String::new())])
                .unwrap(),
            0,
            "已删除队列的命中应被安全忽略"
        );
        assert_eq!(
            store
                .record_queue_matches(id_b, &[(1001, "阿一".into(), base + 2, String::new())])
                .unwrap(),
            1
        );
        let qb = store.get_queue(id_b).unwrap();
        let all = store.get().queues;
        let qb_entries = &all.iter().find(|q| q.id == id_b).unwrap().entries;
        assert_eq!(qb.id, id_b);
        assert_eq!(qb_entries.iter().find(|e| e.uid == 1001).unwrap().count, 2);

        drop(store);
        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_file(db.with_extension("db-wal"));
        let _ = std::fs::remove_file(db.with_extension("db-shm"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：B 站游客弹幕 uid=0，弹幕层会换成负数虚拟 UID。
    /// 写入层必须接受负数 UID 并正常累加，同时仍拒绝 uid=0 的手动占位值。
    #[test]
    fn guest_virtual_uid_is_recorded() {
        let dir = std::env::temp_dir().join(format!("izuna_guest_uid_test_{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let store = Store::new(db.clone()).unwrap();
        let id = store.next_queue_id();
        store.push_queue(&default_queue(id, now_ms())).unwrap();

        let base = now_ms();
        // 负数虚拟 UID（游客「泉***」）：两次应累加成一条、count=2
        assert_eq!(
            store
                .record_queue_matches(id, &[(-1234567, "泉***".into(), base, String::new())])
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .record_queue_matches(id, &[(-1234567, "泉***".into(), base + 1, String::new())])
                .unwrap(),
            1
        );
        // uid=0 是手动添加占位，仍拒绝
        assert_eq!(
            store
                .record_queue_matches(id, &[(0, "手动添加".into(), base, String::new())])
                .unwrap(),
            0
        );

        let q = store.get().queues.into_iter().find(|q| q.id == id).unwrap();
        assert_eq!(q.entries.len(), 1);
        let guest = q.entries.iter().find(|e| e.uid == -1234567).unwrap();
        assert_eq!(guest.count, 2);
        assert_eq!(guest.nickname, "泉***");

        drop(store);
        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_file(db.with_extension("db-wal"));
        let _ = std::fs::remove_file(db.with_extension("db-shm"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
