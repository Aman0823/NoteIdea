//! SQLite 本地状态库。
//!
//! 职责边界（对应 spec：vault/persistence）——只准存三类数据：
//!   1. 索引缓存：待办正文、提醒时间、标签、所属文件与行号
//!   2. 运行状态：提醒的上次触发时间、推迟至、写队列
//!   3. 窗口状态：便签位置/尺寸/颜色
//!
//! 硬约束：**不得存任何"只有 DB 里有"的用户数据**。判据是随时删库重扫，
//! 用户不该感到丢了任何自己写下的东西。因此损坏时的处理是直接重建，
//! 而不是想办法抢救。

use std::fs;
use std::path::Path;

use rusqlite::Connection;

/// schema 版本。加表或改表结构时递增，并在 `migrate` 里加对应分支。
const SCHEMA_VERSION: i64 = 1;

/// 打开数据库。文件不存在、损坏、版本不匹配一律静默重建，不阻塞启动。
///
/// 单实例插件已保证正常情况下只有一个进程，但插件失效或用户用两个 vault
/// 指向同一目录时仍可能撞车。SQLite 自身的锁会拒绝并发写，`busy_timeout`
/// 让瞬时冲突自行化解，持续冲突则报错——这比静默写坏数据好。
pub fn open(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("建数据库目录失败: {e}"))?;
    }

    match try_open(db_path) {
        Ok(conn) => Ok(conn),
        // 被别的进程占用时**绝不能**重建：那会把对方正在用的库改名掉。
        // 直接向上报错，让调用方进入不可用状态。
        Err(OpenError::Locked(reason)) => Err(reason),
        Err(OpenError::Rebuild(reason)) => {
            eprintln!("[db] {reason}，重建数据库");
            quarantine(db_path);
            let conn = Connection::open(db_path).map_err(|e| format!("创建数据库失败: {e}"))?;
            configure(&conn)?;
            create_schema(&conn)?;
            Ok(conn)
        }
    }
}

/// 打开失败的两种性质：可以重建的，和绝不能重建的。
enum OpenError {
    /// 库损坏/版本不认识/首次运行——重建是安全且正确的。
    Rebuild(String),
    /// 库被另一个进程持有——重建会破坏对方数据。
    Locked(String),
}

fn try_open(db_path: &Path) -> Result<Connection, OpenError> {
    use OpenError::{Locked, Rebuild};

    if !db_path.exists() {
        // 不是错误，只是首次运行。走重建路径把 schema 建起来。
        return Err(Rebuild("数据库不存在".into()));
    }

    let conn = Connection::open(db_path).map_err(|e| Rebuild(format!("打开失败: {e}")))?;
    configure(&conn).map_err(Rebuild)?;

    // integrity_check 对损坏文件会返回错误或非 "ok" 字符串。
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| Rebuild(format!("完整性检查失败: {e}")))?;
    if integrity != "ok" {
        return Err(Rebuild(format!("完整性检查未通过: {integrity}")));
    }

    // 确认真能拿到写锁。只 open 成功不代表可写——另一个进程可能正持有锁，
    // 那种情况下要立刻发现，而不是等第一次写队列时才炸。
    conn.execute_batch("BEGIN IMMEDIATE; COMMIT;")
        .map_err(|e| Locked(format!("数据库被占用，可能有另一个实例正在运行: {e}")))?;

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| Rebuild(format!("读 schema 版本失败: {e}")))?;

    if version == 0 {
        // 空库（有文件但没建过表），直接建 schema。
        create_schema(&conn).map_err(Rebuild)?;
        return Ok(conn);
    }
    if version > SCHEMA_VERSION {
        // 用旧版应用打开了新版库，字段可能已不兼容，不冒险读。
        return Err(Rebuild(format!("schema 版本 {version} 高于当前支持的 {SCHEMA_VERSION}")));
    }
    if version < SCHEMA_VERSION {
        migrate(&conn, version).map_err(Rebuild)?;
    }

    Ok(conn)
}

/// 把损坏/不兼容的库改名保留，而不是删掉——万一里面有能人工抢救的东西。
fn quarantine(db_path: &Path) {
    if !db_path.exists() {
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = db_path.with_extension(format!("db.corrupt.{stamp}"));
    match fs::rename(db_path, &backup) {
        Ok(()) => eprintln!("[db] 原文件已保留为 {}", backup.display()),
        Err(e) => eprintln!("[db] 原文件改名失败，将直接覆盖: {e}"),
    }
    // WAL / SHM 残留会让新库读到旧数据，一并清掉。
    for ext in ["db-wal", "db-shm"] {
        let _ = fs::remove_file(db_path.with_extension(ext));
    }
}

fn configure(conn: &Connection) -> Result<(), String> {
    // WAL：读写并发更好，且崩溃恢复能力强于默认的 journal 模式。
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("设置 WAL 失败: {e}"))?;
    // NORMAL 在 WAL 下已能保证崩溃不损坏，比 FULL 快得多。
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| format!("设置 synchronous 失败: {e}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("开启外键失败: {e}"))?;
    // 单实例已保证只有一个进程写，这里的超时只为容忍 WAL checkpoint 的瞬时锁。
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(())
}

fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(SCHEMA_SQL).map_err(|e| format!("建表失败: {e}"))?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|e| format!("写 schema 版本失败: {e}"))?;
    println!("[db] schema v{SCHEMA_VERSION} 已就绪");
    Ok(())
}

/// 迁移失败按损坏处理（由调用方重建），不 panic。
fn migrate(conn: &Connection, from: i64) -> Result<(), String> {
    println!("[db] 迁移 schema v{from} → v{SCHEMA_VERSION}");
    // 目前只有 v1，没有历史版本需要迁移。
    // 将来加版本时在此按 from 逐级 ALTER TABLE。
    let _ = conn;
    Err(format!("没有从 v{from} 出发的迁移路径"))
}

/// 共享的数据库连接。
///
/// 用 `Mutex` 而非连接池：单写者 actor 是唯一的写方，读方也少，
/// 池化只会增加复杂度。真需要并发读时再改。
pub struct Handle(std::sync::Mutex<Connection>);

impl Handle {
    pub fn new(conn: Connection) -> Self {
        Self(std::sync::Mutex::new(conn))
    }

    /// 借用连接执行一段操作。锁中毒（持锁线程 panic）时报错而不是继续用，
    /// 因为那意味着上一次操作可能只做了一半。
    pub fn with<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, String> {
        let mut guard = self.0.lock().map_err(|_| "数据库锁已中毒".to_string())?;
        f(&mut guard).map_err(|e| e.to_string())
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) const SCHEMA_SQL: &str = r#"
-- 写队列：未完成的 ChangeSet。落库是为了进程崩溃后能继续处理。
CREATE TABLE IF NOT EXISTS write_queue (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path   TEXT    NOT NULL,          -- 相对 vault 根，如 "inbox.md"
    operation   TEXT    NOT NULL,          -- append | replace_line | create | replace_file
    payload     TEXT    NOT NULL,          -- JSON，字段随 operation 而异
    base_hash   TEXT,                      -- 入队时文件内容的 BLAKE3；append 为 NULL
    applied_marker TEXT,                   -- append 专用：已追加内容的指纹，防崩溃后重放
    retries     INTEGER NOT NULL DEFAULT 0, -- -1 表示重试耗尽、等用户处理
    last_error  TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_queue_pending ON write_queue(retries, id);
CREATE INDEX IF NOT EXISTS idx_queue_file    ON write_queue(file_path);

-- 提醒运行状态。提醒该怎么设写在 md 里（真源），这里只记"跑到哪了"。
CREATE TABLE IF NOT EXISTS reminders (
    todo_id       TEXT PRIMARY KEY,        -- md 行尾的 ~id，如 "a3f9"
    file_path     TEXT    NOT NULL,
    line_number   INTEGER NOT NULL,        -- 会漂移，仅作定位提示，须配合内容校验
    line_content  TEXT    NOT NULL,        -- 用于校验行号是否还指向同一行
    anchor_time   INTEGER NOT NULL,        -- @ 标记解析出的首次触发时间（不可变锚点）
    recurrence    TEXT,                    -- daily/weekly/... ，NULL = once
    intensity     TEXT    NOT NULL DEFAULT 'toast',
    fired_at      INTEGER,                 -- 已弹出（D21 第一阶段）
    acked_at      INTEGER,                 -- 用户已确认（D21 第二阶段）
    snoozed_until INTEGER,
    updated_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_reminders_due  ON reminders(anchor_time);
CREATE INDEX IF NOT EXISTS idx_reminders_file ON reminders(file_path);

-- 重复待办的每次发生是否完成。md 里恒为 - [ ]，完成状态只在这里（3.8）。
CREATE TABLE IF NOT EXISTS occurrences (
    todo_id      TEXT    NOT NULL,
    occurred_at  INTEGER NOT NULL,         -- 该次发生的应触发时间
    completed_at INTEGER,
    PRIMARY KEY (todo_id, occurred_at),
    FOREIGN KEY (todo_id) REFERENCES reminders(todo_id) ON DELETE CASCADE
);

-- 便签窗口状态（FR-10）。
CREATE TABLE IF NOT EXISTS stickies (
    todo_id    TEXT PRIMARY KEY,
    x          INTEGER,
    y          INTEGER,
    width      INTEGER,
    height     INTEGER,
    opacity    REAL,
    color      TEXT,
    monitor    TEXT,                       -- 所属显示器标识，断开时回落主屏
    updated_at INTEGER NOT NULL
);

-- 待办索引缓存（FR-7 聚合视图 / FR-5 搜索的基础）。
-- 全部可从 md 重扫重建，删表无损。
CREATE TABLE IF NOT EXISTS todos (
    file_path   TEXT    NOT NULL,
    line_number INTEGER NOT NULL,
    todo_id     TEXT,                      -- 无提醒且未贴屏的待办没有 ID
    text        TEXT    NOT NULL,
    checked     INTEGER NOT NULL DEFAULT 0,
    tags        TEXT,                      -- JSON 数组
    scanned_at  INTEGER NOT NULL,
    PRIMARY KEY (file_path, line_number)
);

CREATE INDEX IF NOT EXISTS idx_todos_id ON todos(todo_id);

-- 通用 KV，存"上次扫描时间"这类零散状态。
CREATE TABLE IF NOT EXISTS app_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("noteidea-test-{}-{}", name, now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("local.db")
    }

    #[test]
    fn creates_schema_on_first_open() {
        let p = tmp("fresh");
        let conn = open(&p).expect("首次打开应成功");
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // 六张表都得在
        for t in ["write_queue", "reminders", "occurrences", "stickies", "todos", "app_state"] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "缺表 {t}");
        }
    }

    #[test]
    fn reopens_existing_without_rebuild() {
        let p = tmp("reopen");
        {
            let conn = open(&p).unwrap();
            conn.execute("INSERT INTO app_state (key, value) VALUES ('k', 'v')", []).unwrap();
        }
        let conn = open(&p).unwrap();
        let got: String =
            conn.query_row("SELECT value FROM app_state WHERE key='k'", [], |r| r.get(0)).unwrap();
        assert_eq!(got, "v", "重开不应清空已有数据");
    }

    #[test]
    fn rebuilds_when_corrupt() {
        let p = tmp("corrupt");
        std::fs::write(&p, b"this is definitely not a sqlite file").unwrap();
        let conn = open(&p).expect("损坏应重建而非报错");
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // 损坏文件应被改名保留，而不是直接删掉
        let kept = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("corrupt"));
        assert!(kept, "损坏文件应改名保留");
    }

    #[test]
    fn rebuilds_when_version_too_new() {
        let p = tmp("future");
        {
            let conn = open(&p).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 99).unwrap();
            conn.execute("INSERT INTO app_state (key, value) VALUES ('old', '1')", []).unwrap();
        }
        let conn = open(&p).expect("未来版本应重建");
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let n: i64 =
            conn.query_row("SELECT count(*) FROM app_state WHERE key='old'", [], |r| r.get(0))
                .unwrap();
        assert_eq!(n, 0, "重建后应是空库");
    }
}
