//! 单写者 actor：vault 内 markdown 文件的唯一写入通道（D17）。
//!
//! 为什么必须行级提交而非全文提交——光串行不解决丢写：
//!
//! ```text
//! t=0  磁盘：行1 笔记开头 / 行9 - [ ] 交周报
//! t=1  你在主编辑器改行1（还在缓冲里，未落盘）
//! t=2  便签勾选 → actor 把行9 改成 - [x]，已落盘
//! t=3  自动保存触发，主编辑器提交它手里的全文
//!      → 它那份快照里行9 还是 - [ ]
//!      → 便签那下勾选被静默吞掉
//! ```
//!
//! 根因是全文提交隐含了「我这份快照就是最新」的错误断言。
//!
//! 对应 spec：vault/file-write

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 一次写入请求。所有写者都提交这个，没有例外通道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    /// 相对 vault 根的路径，如 `inbox.md`。
    pub file_path: String,
    pub op: Operation,
    /// 入队时文件内容的 BLAKE3。`None` 表示该操作不关心基线（append/create）。
    pub base_hash: Option<String>,
}

/// 一处字符偏移编辑。`from`/`to` 是 UTF-16 code unit 偏移（与 CM6 一致）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    /// 追加到文件末尾。天然不与其他变更冲突，因此不校验基线。
    Append { content: String },
    /// 替换单行。`old_content` 用于基线失效后重新定位。
    ReplaceLine { line_number: usize, old_content: String, new_content: String },
    /// 新建文件。目标已存在则失败。
    Create { content: String },
    /// 字符偏移变更。偏移相对 ChangeSet 的严格基线哈希。
    ApplyEdits { edits: Vec<Edit> },
    /// 整文件替换。仅版本恢复可用——它的语义本就是整体回退。
    ReplaceFile { content: String },
}

impl Operation {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Append { .. } => "append",
            Self::ReplaceLine { .. } => "replace_line",
            Self::Create { .. } => "create",
            Self::ApplyEdits { .. } => "apply_edits",
            Self::ReplaceFile { .. } => "replace_file",
        }
    }
}

/// 写入失败的原因。区分「可重试」和「拒绝」——后者重试多少次都一样。
#[derive(Debug)]
pub enum WriteError {
    /// IO 层面的临时失败（文件被杀软锁定等），值得重试。
    Retryable(String),
    /// 语义上无法完成，重试无意义。必须让用户看到。
    Rejected(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retryable(m) | Self::Rejected(m) => write!(f, "{m}"),
        }
    }
}

pub fn hash(content: &str) -> String {
    blake3::hash(content.as_bytes()).to_hex().to_string()
}

/// 重试上限。超过则标记为失败，等用户处理。
const MAX_RETRIES: i64 = 3;
/// 重试前的等待。文件被杀软短暂锁定这类情况，隔一下就好了。
const RETRY_DELAY_MS: u64 = 200;

/// 投递给 actor 的请求。
pub enum Request {
    /// 入队一个变更。`reply` 收到的是「已入队」而非「已落盘」。
    Enqueue { cs: ChangeSet, reply: tokio::sync::oneshot::Sender<Result<i64, String>> },
    /// 唤醒处理循环（启动恢复、用户点重试后调用）。
    Drain,
}

/// actor 的句柄。前端命令通过它投递请求。
#[derive(Clone)]
pub struct Handle(tokio::sync::mpsc::UnboundedSender<Request>);

impl Handle {
    /// 入队并等待「已入队」确认。
    ///
    /// 刻意不等落盘：速记条要立刻关窗，不能卡在磁盘上。落盘结果通过
    /// `file:changed` / `write:failed` 事件通知。
    pub async fn enqueue(&self, cs: ChangeSet) -> Result<i64, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.0
            .send(Request::Enqueue { cs, reply: tx })
            // send 失败说明 actor task 已死。这时必须报错而不是假装成功，
            // 否则用户以为记下来了，实际什么都没发生。
            .map_err(|_| "写入服务已停止，请重启应用".to_string())?;
        rx.await.map_err(|_| "写入服务无响应".to_string())?
    }

    /// 唤醒处理循环（用户点重试后调用）。
    pub fn drain(&self) {
        let _ = self.0.send(Request::Drain);
    }
}

/// 一条放弃了的写入，供主窗口展示（spec：用户查看失败列表）。
#[derive(Debug, Clone, Serialize)]
pub struct FailedWrite {
    pub id: i64,
    pub file: String,
    pub op: String,
    pub error: String,
    /// 毫秒时间戳，前端自己格式化。
    pub at: i64,
}

/// 列出已放弃的写入（`retries < 0`）。
pub fn list_failed(db: &crate::db::Handle) -> Result<Vec<FailedWrite>, String> {
    db.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, file_path, operation, COALESCE(last_error, '未知原因'), updated_at
             FROM write_queue WHERE retries < 0 ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(FailedWrite {
                id: r.get(0)?,
                file: r.get(1)?,
                op: r.get(2)?,
                error: r.get(3)?,
                at: r.get(4)?,
            })
        })?;
        rows.collect()
    })
}

/// 把一条已放弃的记录重置为待处理，让它重新参与队列。
///
/// 限定 `retries < 0`：正在正常重试中的记录不该被外部打扰。
pub fn reset_failed(db: &crate::db::Handle, id: i64) -> Result<bool, String> {
    let now = crate::db::now_ms();
    db.with(|conn| {
        let n = conn.execute(
            "UPDATE write_queue SET retries = 0, last_error = NULL, updated_at = ?2
             WHERE id = ?1 AND retries < 0",
            rusqlite::params![id, now],
        )?;
        Ok(n > 0)
    })
}

/// 丢弃一条已放弃的记录。
///
/// 这是**用户主动放弃这次修改**，是唯一允许删除未落盘变更的路径，
/// 因此同样限定 `retries < 0`，避免误删正在重试的记录。
pub fn discard_failed(db: &crate::db::Handle, id: i64) -> Result<bool, String> {
    db.with(|conn| {
        let n = conn.execute("DELETE FROM write_queue WHERE id = ?1 AND retries < 0", [id])?;
        Ok(n > 0)
    })
}

/// 启动 actor。返回句柄，同时把遗留队列排空（崩溃恢复）。
pub fn spawn<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    vault_root: PathBuf,
    db: crate::db::Handle,
) -> Handle {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Request>();
    let handle = Handle(tx);

    tauri::async_runtime::spawn(async move {
        // 先消费上次没写完的，再接受新请求（崩溃恢复）。
        drain(&app, &db, &vault_root).await;

        while let Some(req) = rx.recv().await {
            match req {
                Request::Enqueue { cs, reply } => {
                    let result = enqueue_to_db(&db, &cs);
                    let ok = result.is_ok();
                    let _ = reply.send(result);
                    if ok {
                        drain(&app, &db, &vault_root).await;
                    }
                }
                Request::Drain => drain(&app, &db, &vault_root).await,
            }
        }
        eprintln!("[actor] 请求通道已关闭，写入服务停止");
    });

    handle
}

/// 入队。与 Tauri 无关，便于单测。
pub fn enqueue_to_db(db: &crate::db::Handle, cs: &ChangeSet) -> Result<i64, String> {
    let payload = serde_json::to_string(&cs.op).map_err(|e| format!("序列化失败: {e}"))?;
    let now = crate::db::now_ms();

    db.with(|conn| {
        conn.execute(
            "INSERT INTO write_queue (file_path, operation, payload, base_hash, retries, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
            rusqlite::params![cs.file_path, cs.op.name(), payload, cs.base_hash, now],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

/// 一条条处理队列，直到空或遇到失败。
///
/// 遇到失败就停：后续变更很可能针对同一文件，继续处理只会连环失败，
/// 而且会掩盖第一个错误。
async fn drain<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &crate::db::Handle,
    vault_root: &Path,
) {
    loop {
        match step(db, vault_root).await {
            Step::Empty => return,
            Step::Done(cs) => emit_changed(app, &cs),
            Step::Retrying => {}
            Step::Failed(cs, msg) => {
                emit_failed(app, &cs, &msg);
                return;
            }
        }
    }
}

/// 处理一条的结果。把「做了什么」与「怎么通知前端」分开，
/// 这样队列逻辑本身可以脱离 Tauri 单测。
pub enum Step {
    /// 队列空了。
    Empty,
    /// 成功落盘。
    Done(ChangeSet),
    /// 本次失败但会重试，已等待过。
    Retrying,
    /// 放弃，记录留在队列等用户处理。
    Failed(ChangeSet, String),
}

/// 处理队列里的一条。
pub async fn step(db: &crate::db::Handle, vault_root: &Path) -> Step {
    let Some(row) = next_pending(db) else { return Step::Empty };

    // 崩溃窗口：文件已写成功、但删队列记录之前进程死了。重启后重放，
    // append 会静默多出一行——四种操作里只有它是不幂等的。
    //
    // 另外三种重放的后果是可见的（内容已变→定位失败；文件已存在→拒绝），
    // 会变成一条用户看得懂、能丢弃的失败记录，不会悄悄改坏数据。
    if let Operation::Append { content } = &row.cs.op {
        if row.attempted {
            match already_appended(vault_root, &row.cs.file_path, content) {
                Ok(true) => {
                    // 上次其实写成功了，只是没来得及删记录。补删，不重写。
                    println!("[actor] {} 的追加已生效，跳过重放", row.cs.file_path);
                    delete_row(db, row.id);
                    return Step::Done(row.cs);
                }
                Ok(false) => {}
                // 读不了文件就当没写过：宁可重放一次（最多多一行），
                // 也不能因为读失败就丢掉用户的内容。
                Err(e) => eprintln!("[actor] 无法确认是否已追加，按未写过处理: {e}"),
            }
        } else {
            // 必须先记「已尝试」再动文件。顺序反了这层保护就不存在了。
            mark_attempted(db, row.id);
        }
    }

    // 单条记录的 panic 不能杀掉整个 actor task——否则一条坏数据会让
    // 应用彻底失去写入能力，且用户完全不知道为什么。
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(vault_root, &row.cs)))
            .unwrap_or_else(|_| {
                Err(WriteError::Rejected("处理该变更时发生内部错误（panic）".into()))
            });

    match outcome {
        Ok(()) => {
            delete_row(db, row.id);
            Step::Done(row.cs)
        }
        Err(WriteError::Retryable(msg)) if row.retries < MAX_RETRIES => {
            eprintln!("[actor] {} 写入失败（第 {} 次）: {msg}", row.cs.file_path, row.retries + 1);
            bump_retry(db, row.id, &msg);
            tokio::time::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS)).await;
            Step::Retrying
        }
        Err(e) => {
            let msg = e.to_string();
            eprintln!("[actor] {} 写入放弃: {msg}", row.cs.file_path);
            mark_failed(db, row.id, &msg);
            Step::Failed(row.cs, msg)
        }
    }
}

/// 追加内容是否已经在文件末尾了。
///
/// 判据是「文件末尾恰好是这段内容」。append 会补规范化换行，所以比对时
/// 也要按同样规则补一次，否则永远不匹配。
fn already_appended(vault_root: &Path, file_path: &str, content: &str) -> Result<bool, String> {
    let path = resolve(vault_root, file_path).map_err(|e| e.to_string())?;
    let current = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("读文件失败: {e}")),
    };

    let mut expected = content.to_string();
    if !expected.ends_with('\n') {
        expected.push('\n');
    }
    Ok(current.ends_with(&expected))
}

struct QueueRow {
    id: i64,
    cs: ChangeSet,
    retries: i64,
    /// 这条是否已经动过文件。用于区分「首次处理」和「崩溃后重放」。
    attempted: bool,
}

/// 取下一条待处理。`retries >= 0` 排除已放弃的（-1）。
fn next_pending(db: &crate::db::Handle) -> Option<QueueRow> {
    db.with(|conn| {
        conn.query_row(
            "SELECT id, file_path, payload, base_hash, retries, applied_marker
             FROM write_queue WHERE retries >= 0 ORDER BY id LIMIT 1",
            [],
            |r| {
                let payload: String = r.get(2)?;
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    payload,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    })
    .ok()
    .flatten()
    .and_then(|(id, file_path, payload, base_hash, retries, marker)| {
        match serde_json::from_str::<Operation>(&payload) {
            Ok(op) => Some(QueueRow {
                id,
                cs: ChangeSet { file_path, op, base_hash },
                retries,
                attempted: marker.is_some(),
            }),
            Err(e) => {
                // 队列里存了读不出来的东西，留着会卡死整个队列。
                eprintln!("[actor] 队列记录 {id} 无法解析，丢弃: {e}");
                delete_row(db, id);
                None
            }
        }
    })
}

fn delete_row(db: &crate::db::Handle, id: i64) {
    let _ = db.with(|conn| {
        conn.execute("DELETE FROM write_queue WHERE id = ?1", [id])?;
        Ok(())
    });
}

/// 标记「即将动文件」。必须在实际写盘之前调用。
fn mark_attempted(db: &crate::db::Handle, id: i64) {
    let now = crate::db::now_ms();
    let _ = db.with(|conn| {
        conn.execute(
            "UPDATE write_queue SET applied_marker = ?2, updated_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now],
        )?;
        Ok(())
    });
}

fn bump_retry(db: &crate::db::Handle, id: i64, err: &str) {
    let now = crate::db::now_ms();
    let _ = db.with(|conn| {
        conn.execute(
            "UPDATE write_queue SET retries = retries + 1, last_error = ?2, updated_at = ?3
             WHERE id = ?1",
            rusqlite::params![id, err, now],
        )?;
        Ok(())
    });
}

/// `retries = -1` 表示已放弃，不再自动重试，但记录留着等用户处理。
fn mark_failed(db: &crate::db::Handle, id: i64, err: &str) {
    let now = crate::db::now_ms();
    let _ = db.with(|conn| {
        conn.execute(
            "UPDATE write_queue SET retries = -1, last_error = ?2, updated_at = ?3
             WHERE id = ?1",
            rusqlite::params![id, err, now],
        )?;
        Ok(())
    });
}

fn emit_changed<R: tauri::Runtime>(app: &tauri::AppHandle<R>, cs: &ChangeSet) {
    use tauri::Emitter;
    let _ = app.emit(
        "file:changed",
        serde_json::json!({ "file": cs.file_path, "op": cs.op.name(), "change_set": cs }),
    );
}

fn emit_failed<R: tauri::Runtime>(app: &tauri::AppHandle<R>, cs: &ChangeSet, err: &str) {
    use tauri::Emitter;
    let _ = app.emit(
        "write:failed",
        serde_json::json!({ "file": cs.file_path, "op": cs.op.name(), "error": err }),
    );
}

/// 原子写盘：同目录临时文件 + fsync + rename。
///
/// 临时文件必须与目标同目录——跨卷 rename 会退化成复制，失去原子性。
fn write_atomic(path: &Path, content: &str) -> Result<(), WriteError> {
    use std::io::Write;

    let dir = path.parent().ok_or_else(|| WriteError::Rejected("目标路径没有父目录".into()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| WriteError::Retryable(format!("建目录失败: {e}")))?;

    let tmp = dir.join(format!(
        ".noteidea-tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        // fsync：不做的话 rename 可能先于数据落盘，断电后得到空文件。
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(WriteError::Retryable(format!("写临时文件失败: {e}")));
    }

    // Windows 上 rename 到已存在的目标会失败，必须用 ReplaceFile 语义。
    // std::fs::rename 在 Windows 实现里已使用 MoveFileEx(MOVEFILE_REPLACE_EXISTING)，
    // 因此可以直接覆盖；这一点在 4.3 里实测确认。
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(WriteError::Retryable(format!("替换目标文件失败: {e}")));
    }

    Ok(())
}

/// 把 ChangeSet 应用到磁盘。
///
/// 每次都重读磁盘当前内容——绝不信任提交方带来的快照。
pub fn apply(vault_root: &Path, cs: &ChangeSet) -> Result<(), WriteError> {
    let path = resolve(vault_root, &cs.file_path)?;

    match &cs.op {
        Operation::Append { content } => append(&path, content),
        Operation::Create { content } => {
            if path.exists() {
                return Err(WriteError::Rejected(format!("文件已存在: {}", cs.file_path)));
            }
            write_atomic(&path, content)
        }
        Operation::ReplaceFile { content } => write_atomic(&path, content),
        Operation::ReplaceLine { line_number, old_content, new_content } => {
            let current = read(&path)?;
            replace_line(&path, &current, cs, *line_number, old_content, new_content)
        }
        Operation::ApplyEdits { edits } => apply_edits(&path, cs, edits),
    }
}

/// 应用以基线内容为坐标系的编辑。偏移单位是 **UTF-16 code unit**——
/// 与前端 CodeMirror 的 `ChangeSet.iterChanges` 输出一致，前端不用再换算，
/// 含中文/ emoji 也不会错位。
///
/// 先完成全部校验，之后才拼出新内容并原子写盘。这样单个坏区间不会留下
/// 「前半批已写、后半批失败」的中间态。
fn apply_edits(path: &Path, cs: &ChangeSet, edits: &[Edit]) -> Result<(), WriteError> {
    let current = read(path)?;
    let Some(expected_hash) = cs.base_hash.as_deref() else {
        return Err(WriteError::Rejected("apply_edits 必须携带基线哈希".into()));
    };
    let actual_hash = hash(&current);
    if actual_hash != expected_hash {
        return Err(WriteError::Rejected(format!(
            "编辑冲突：磁盘内容已变化（当前哈希：{actual_hash}）"
        )));
    }

    let len_utf16: usize = current.chars().map(char::len_utf16).sum();

    let mut ordered = edits.to_vec();
    ordered.sort_by(|a, b| b.from.cmp(&a.from).then_with(|| b.to.cmp(&a.to)));

    let mut previous_from = len_utf16;
    let mut resolved: Vec<(usize, usize, &str)> = Vec::with_capacity(ordered.len());
    for edit in &ordered {
        if edit.from > edit.to || edit.to > len_utf16 {
            return Err(WriteError::Rejected("编辑区间超出文件边界".into()));
        }
        // 偏移必须正好落在 UTF-16 字符边界上（不能把 surrogate pair 拆开）。
        let from_byte = utf16_to_byte(&current, edit.from)
            .ok_or_else(|| WriteError::Rejected("编辑区间未对齐字符边界".into()))?;
        let to_byte = utf16_to_byte(&current, edit.to)
            .ok_or_else(|| WriteError::Rejected("编辑区间未对齐字符边界".into()))?;
        if edit.to > previous_from {
            return Err(WriteError::Rejected("编辑区间重叠".into()));
        }
        previous_from = edit.from;
        resolved.push((from_byte, to_byte, edit.insert.as_str()));
    }

    // 预转换的字节偏移按 from 降序应用：后面的编辑不影响前面（较小）偏移。
    let mut updated = current;
    for (from_byte, to_byte, insert) in resolved {
        updated.replace_range(from_byte..to_byte, insert);
    }
    write_atomic(path, &updated)
}

/// 把 UTF-16 偏移转成字节偏移。偏移不落在字符边界（surrogate 中间）时返回 None。
fn utf16_to_byte(s: &str, utf16: usize) -> Option<usize> {
    let mut seen = 0usize;
    for (byte, ch) in s.char_indices() {
        if seen == utf16 {
            return Some(byte);
        }
        seen += ch.len_utf16();
    }
    if seen == utf16 {
        Some(s.len())
    } else {
        None
    }
}

/// 把相对路径解析为 vault 内的绝对路径，并拒绝逃出 vault 的路径。
///
/// 这是个安全边界：ChangeSet 的 file_path 最终可能来自笔记内容或外部输入，
/// `../../` 之类不能放过去。
fn resolve(vault_root: &Path, rel: &str) -> Result<PathBuf, WriteError> {
    use std::path::Component;

    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(WriteError::Rejected(format!("只接受相对路径: {rel}")));
    }
    for c in rel_path.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(WriteError::Rejected(format!("路径不得包含 ..: {rel}")));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(WriteError::Rejected(format!("非法路径: {rel}")));
            }
        }
    }
    Ok(vault_root.join(rel_path))
}

fn read(path: &Path) -> Result<String, WriteError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(WriteError::Retryable(format!("读文件失败: {e}"))),
    }
}

/// 追加。走「读全文 + 补换行 + 原子写」而不是 O_APPEND，
/// 因为要保证上一行没有换行符时不会把两行黏在一起。
fn append(path: &Path, content: &str) -> Result<(), WriteError> {
    let mut current = read(path)?;
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    current.push_str(content);
    if !current.ends_with('\n') {
        current.push('\n');
    }
    write_atomic(path, &current)
}

/// 替换单行。只改那一行，其余字节逐字保留（FR-12）。
///
/// 快速路径：基线哈希匹配 → 直接按行号替换。
/// 慢速路径：基线已变 → 用 old_content 重新定位。
fn replace_line(
    path: &Path,
    current: &str,
    cs: &ChangeSet,
    line_number: usize,
    old_content: &str,
    new_content: &str,
) -> Result<(), WriteError> {
    let mut lines: Vec<&str> = current.split('\n').collect();
    // split('\n') 会在末尾换行处产生一个空元素，它不是真实行。
    let trailing_newline = matches!(lines.last(), Some(&"")) && !current.is_empty();
    if trailing_newline {
        lines.pop();
    }

    let base_matches = cs.base_hash.as_deref() == Some(hash(current).as_str());

    let target = if base_matches {
        // 基线没变，行号可信；仍然校验内容，防止调用方传错行号。
        let idx = line_number.checked_sub(1).ok_or_else(|| {
            WriteError::Rejected("行号从 1 开始，收到 0".into())
        })?;
        match lines.get(idx) {
            Some(l) if *l == old_content => idx,
            // 行号指向的行内容不对，说明行漂移了，退回内容定位。
            Some(_) => locate_by_content(&lines, old_content)?,
            None => {
                return Err(WriteError::Rejected(format!(
                    "行号 {line_number} 超出文件范围（共 {} 行）",
                    lines.len()
                )))
            }
        }
    } else {
        locate_by_content(&lines, old_content)?
    };

    lines[target] = new_content;

    let mut out = lines.join("\n");
    if trailing_newline || out.is_empty() {
        out.push('\n');
    }
    write_atomic(path, &out)
}

/// 按内容定位目标行。
///
/// 匹配到多行时**拒绝**而不是取第一个（design D4）：改错行是静默的数据
/// 损坏，而失败是可见的、用户能处理的。有 `~id` 的待办不会走到这里，
/// 因为 ID 本身就是唯一锚点。
fn locate_by_content(lines: &[&str], old_content: &str) -> Result<usize, WriteError> {
    let mut found = None;
    for (i, l) in lines.iter().enumerate() {
        if *l == old_content {
            if found.is_some() {
                return Err(WriteError::Rejected(format!(
                    "有多行内容相同，无法确定改哪一行：{old_content}"
                )));
            }
            found = Some(i);
        }
    }
    found.ok_or_else(|| {
        WriteError::Rejected(format!("目标行已不存在，可能已被改动：{old_content}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!(
                "noteidea-actor-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
        fn write(&self, rel: &str, content: &str) {
            std::fs::write(self.0.join(rel), content).unwrap();
        }
        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.0.join(rel)).unwrap()
        }
    }

    fn cs(file: &str, op: Operation, base: Option<String>) -> ChangeSet {
        ChangeSet { file_path: file.into(), op, base_hash: base }
    }

    #[test]
    fn apply_edits_applies_multiple_non_adjacent_changes() {
        let t = Tmp::new("apply-edits-multiple");
        let original = "第一行\n中间不变\n最后一行\n";
        t.write("note.md", original);
        // UTF-16 布局：第一行(3) \n 中间不变(4) \n 最后一行(4) \n
        // "第一行" = 0..3，"最后一行" = 9..13
        let edits = vec![
            Edit { from: 0, to: 3, insert: "标题".into() },
            Edit { from: 9, to: 13, insert: "结尾".into() },
        ];
        let changes = cs("note.md", Operation::ApplyEdits { edits }, Some(hash(original)));
        apply(&t.0, &changes).unwrap();
        assert_eq!(t.read("note.md"), "标题\n中间不变\n结尾\n");
    }

    #[test]
    fn apply_edits_uses_utf16_offsets_not_bytes() {
        let t = Tmp::new("apply-edits-utf16");
        let original = "中文\n";
        t.write("note.md", original);
        // "中文" 是 2 个 UTF-16 code unit（6 个 UTF-8 字节）。
        // 前端 CM6 传来的偏移是 UTF-16，替换 0..2 必须按字符边界处理。
        let changes = cs(
            "note.md",
            Operation::ApplyEdits {
                edits: vec![Edit { from: 0, to: 2, insert: "AB".into() }],
            },
            Some(hash(original)),
        );
        apply(&t.0, &changes).unwrap();
        assert_eq!(t.read("note.md"), "AB\n");
    }

    #[test]
    fn apply_edits_rejects_stale_base_without_touching_file() {
        let t = Tmp::new("apply-edits-conflict");
        let original = "原始内容\n";
        t.write("note.md", original);
        let changes = cs(
            "note.md",
            Operation::ApplyEdits {
                edits: vec![Edit { from: 0, to: 2, insert: "改".into() }],
            },
            Some(hash("不是当前内容")),
        );
        let error = apply(&t.0, &changes).unwrap_err().to_string();
        assert!(error.contains("编辑冲突"));
        assert!(error.contains(&hash(original)));
        assert_eq!(t.read("note.md"), original);
    }

    #[test]
    fn apply_edits_rejects_invalid_batch_before_writing() {
        let t = Tmp::new("apply-edits-invalid");
        let original = "0123456789";
        t.write("note.md", original);
        let changes = cs(
            "note.md",
            Operation::ApplyEdits {
                edits: vec![
                    Edit { from: 0, to: 1, insert: "a".into() },
                    Edit { from: 9, to: 20, insert: "b".into() },
                ],
            },
            Some(hash(original)),
        );
        assert!(apply(&t.0, &changes).is_err());
        assert_eq!(t.read("note.md"), original);
    }

    #[test]
    fn apply_edits_rejects_overlapping_and_reversed_ranges() {
        let t = Tmp::new("apply-edits-overlap");
        let original = "0123456789";
        t.write("note.md", original);
        for edits in [
            vec![
                Edit { from: 2, to: 6, insert: "a".into() },
                Edit { from: 4, to: 8, insert: "b".into() },
            ],
            vec![Edit { from: 8, to: 3, insert: "x".into() }],
        ] {
            let changes = cs("note.md", Operation::ApplyEdits { edits }, Some(hash(original)));
            assert!(apply(&t.0, &changes).is_err());
            assert_eq!(t.read("note.md"), original);
        }
    }

    #[test]
    fn apply_edits_rejects_missing_base_hash() {
        let t = Tmp::new("apply-edits-no-base");
        t.write("note.md", "内容");
        let changes = cs(
            "note.md",
            Operation::ApplyEdits { edits: Vec::new() },
            None,
        );
        assert!(apply(&t.0, &changes).is_err());
    }

    #[test]
    fn append_to_missing_file_creates_it() {
        let t = Tmp::new("append-new");
        apply(&t.0, &cs("inbox.md", Operation::Append { content: "- [ ] a".into() }, None)).unwrap();
        assert_eq!(t.read("inbox.md"), "- [ ] a\n");
    }

    #[test]
    fn append_inserts_newline_when_previous_line_unterminated() {
        let t = Tmp::new("append-nonl");
        t.write("inbox.md", "- [ ] first"); // 故意没有结尾换行
        apply(&t.0, &cs("inbox.md", Operation::Append { content: "- [ ] second".into() }, None))
            .unwrap();
        // 两行不能被黏成一行
        assert_eq!(t.read("inbox.md"), "- [ ] first\n- [ ] second\n");
    }

    #[test]
    fn replace_line_touches_only_that_line() {
        let t = Tmp::new("replace-exact");
        let original = "# 标题\n\n- [ ] 交周报 ~a3f9\n- [ ] 别的事\n\n正文*斜体*保持原样\n";
        t.write("n.md", original);
        apply(
            &t.0,
            &cs(
                "n.md",
                Operation::ReplaceLine {
                    line_number: 3,
                    old_content: "- [ ] 交周报 ~a3f9".into(),
                    new_content: "- [x] 交周报 ~a3f9".into(),
                },
                Some(hash(original)),
            ),
        )
        .unwrap();
        assert_eq!(
            t.read("n.md"),
            "# 标题\n\n- [x] 交周报 ~a3f9\n- [ ] 别的事\n\n正文*斜体*保持原样\n"
        );
    }

    #[test]
    fn replace_line_relocates_when_line_drifted() {
        let t = Tmp::new("replace-drift");
        let original = "- [ ] 目标\n";
        // 入队时基线是 original，但落盘前有人在前面插了两行
        t.write("n.md", "新增一行\n又一行\n- [ ] 目标\n");
        apply(
            &t.0,
            &cs(
                "n.md",
                Operation::ReplaceLine {
                    line_number: 1, // 已经过期的行号
                    old_content: "- [ ] 目标".into(),
                    new_content: "- [x] 目标".into(),
                },
                Some(hash(original)),
            ),
        )
        .unwrap();
        assert_eq!(t.read("n.md"), "新增一行\n又一行\n- [x] 目标\n");
    }

    #[test]
    fn replace_line_rejects_ambiguous_duplicate() {
        let t = Tmp::new("replace-dup");
        t.write("n.md", "- [ ] 买菜\n- [ ] 买菜\n");
        let err = apply(
            &t.0,
            &cs(
                "n.md",
                Operation::ReplaceLine {
                    line_number: 1,
                    old_content: "- [ ] 买菜".into(),
                    new_content: "- [x] 买菜".into(),
                },
                Some("过期的哈希".into()),
            ),
        )
        .unwrap_err();
        assert!(matches!(err, WriteError::Rejected(_)), "多行同内容必须拒绝，不能猜");
        // 拒绝时文件一个字节都不该动
        assert_eq!(t.read("n.md"), "- [ ] 买菜\n- [ ] 买菜\n");
    }

    #[test]
    fn replace_line_rejects_when_target_gone() {
        let t = Tmp::new("replace-gone");
        t.write("n.md", "- [ ] 别的\n");
        let err = apply(
            &t.0,
            &cs(
                "n.md",
                Operation::ReplaceLine {
                    line_number: 1,
                    old_content: "- [ ] 已被删掉的".into(),
                    new_content: "- [x] 已被删掉的".into(),
                },
                Some("过期".into()),
            ),
        )
        .unwrap_err();
        assert!(matches!(err, WriteError::Rejected(_)));
        assert_eq!(t.read("n.md"), "- [ ] 别的\n");
    }

    #[test]
    fn create_rejects_existing() {
        let t = Tmp::new("create-dup");
        t.write("n.md", "已有内容\n");
        let err =
            apply(&t.0, &cs("n.md", Operation::Create { content: "新".into() }, None)).unwrap_err();
        assert!(matches!(err, WriteError::Rejected(_)));
        assert_eq!(t.read("n.md"), "已有内容\n", "不得覆盖已有文件");
    }

    #[test]
    fn rejects_path_escaping_vault() {
        let t = Tmp::new("escape");
        for bad in ["../outside.md", "sub/../../outside.md"] {
            let err = apply(&t.0, &cs(bad, Operation::Append { content: "x".into() }, None))
                .unwrap_err();
            assert!(matches!(err, WriteError::Rejected(_)), "{bad} 应被拒绝");
        }
    }

    #[test]
    fn replace_file_overwrites_whole_content() {
        let t = Tmp::new("replace-file");
        t.write("n.md", "旧的全部内容\n");
        apply(&t.0, &cs("n.md", Operation::ReplaceFile { content: "恢复的内容\n".into() }, None))
            .unwrap();
        assert_eq!(t.read("n.md"), "恢复的内容\n");
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let t = Tmp::new("no-temp");
        apply(&t.0, &cs("inbox.md", Operation::Append { content: "a".into() }, None)).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&t.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件应已被 rename 掉");
    }

    // ---------- 队列逻辑 ----------

    fn mem_db() -> crate::db::Handle {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA_SQL).unwrap();
        crate::db::Handle::new(conn)
    }

    fn queue_len(db: &crate::db::Handle) -> i64 {
        db.with(|c| c.query_row("SELECT count(*) FROM write_queue", [], |r| r.get(0))).unwrap()
    }

    #[tokio::test]
    async fn queue_drains_in_fifo_order() {
        let t = Tmp::new("fifo");
        let db = mem_db();
        for i in 1..=5 {
            enqueue_to_db(
                &db,
                &cs("inbox.md", Operation::Append { content: format!("- [ ] {i}") }, None),
            )
            .unwrap();
        }
        assert_eq!(queue_len(&db), 5);

        while !matches!(step(&db, &t.0).await, Step::Empty) {}

        assert_eq!(queue_len(&db), 0, "全部处理完队列应为空");
        assert_eq!(t.read("inbox.md"), "- [ ] 1
- [ ] 2
- [ ] 3
- [ ] 4
- [ ] 5
");
    }

    #[tokio::test]
    async fn rejected_row_stays_in_queue_as_failed() {
        let t = Tmp::new("failed-stays");
        let db = mem_db();
        t.write("n.md", "已有
");
        // create 到已存在的文件 → Rejected
        enqueue_to_db(&db, &cs("n.md", Operation::Create { content: "新".into() }, None)).unwrap();

        let s = step(&db, &t.0).await;
        assert!(matches!(s, Step::Failed(..)), "应放弃而非重试");

        // 记录必须留着等用户处理，且 retries 标记为 -1
        let retries: i64 =
            db.with(|c| c.query_row("SELECT retries FROM write_queue", [], |r| r.get(0))).unwrap();
        assert_eq!(retries, -1);
        // 已放弃的不该再被取出，否则会无限循环
        assert!(matches!(step(&db, &t.0).await, Step::Empty));
        assert_eq!(t.read("n.md"), "已有
", "拒绝时文件不得改动");
    }

    #[tokio::test]
    async fn failed_row_does_not_block_later_ones_after_reset() {
        let t = Tmp::new("unblock");
        let db = mem_db();
        t.write("n.md", "已有
");
        enqueue_to_db(&db, &cs("n.md", Operation::Create { content: "x".into() }, None)).unwrap();
        enqueue_to_db(&db, &cs("inbox.md", Operation::Append { content: "- [ ] 后来的".into() }, None))
            .unwrap();

        assert!(matches!(step(&db, &t.0).await, Step::Failed(..)));
        // 第一条被标记 -1 后，第二条应能继续
        assert!(matches!(step(&db, &t.0).await, Step::Done(_)));
        assert_eq!(t.read("inbox.md"), "- [ ] 后来的
");
    }

    #[tokio::test]
    async fn unparseable_payload_is_discarded_not_stuck() {
        let t = Tmp::new("badjson");
        let db = mem_db();
        db.with(|c| {
            c.execute(
                "INSERT INTO write_queue (file_path, operation, payload, retries, created_at, updated_at)
                 VALUES ('n.md', 'append', '{not json', 0, 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        // 读不出来的记录必须被丢弃，否则整个队列卡死
        assert!(matches!(step(&db, &t.0).await, Step::Empty));
        assert_eq!(queue_len(&db), 0, "坏记录应被丢弃");
    }

    #[tokio::test]
    async fn recovers_leftover_queue_on_restart() {
        let t = Tmp::new("recover");
        let db = mem_db();
        // 模拟上次进程崩溃：记录已入队但没落盘
        enqueue_to_db(&db, &cs("inbox.md", Operation::Append { content: "- [ ] 崩溃前".into() }, None))
            .unwrap();
        assert!(!t.0.join("inbox.md").exists());

        while !matches!(step(&db, &t.0).await, Step::Empty) {}

        assert_eq!(t.read("inbox.md"), "- [ ] 崩溃前
", "重启后应补写");
        assert_eq!(queue_len(&db), 0);
    }

    // ---------- 失败列表 ----------

    /// 让一条记录进入「已放弃」状态，返回其 id。
    async fn make_failed(db: &crate::db::Handle, t: &Tmp) -> i64 {
        t.write("n.md", "已有
");
        let id =
            enqueue_to_db(db, &cs("n.md", Operation::Create { content: "x".into() }, None)).unwrap();
        assert!(matches!(step(db, &t.0).await, Step::Failed(..)));
        id
    }

    #[tokio::test]
    async fn failed_list_shows_reason_and_op() {
        let t = Tmp::new("list");
        let db = mem_db();
        make_failed(&db, &t).await;

        let rows = list_failed(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file, "n.md");
        assert_eq!(rows[0].op, "create");
        assert!(rows[0].error.contains("已存在"), "要能看到失败原因: {}", rows[0].error);
        assert!(rows[0].at > 0);
    }

    #[tokio::test]
    async fn retry_puts_row_back_in_queue() {
        let t = Tmp::new("retry");
        let db = mem_db();
        let id = make_failed(&db, &t).await;

        // 队列已空（那条被标记为放弃，不再取出）
        assert!(matches!(step(&db, &t.0).await, Step::Empty));

        assert!(reset_failed(&db, id).unwrap());
        assert!(list_failed(&db).unwrap().is_empty(), "重置后不该还在失败列表里");

        // 原因没修好（文件仍存在），所以会再次失败——这是对的
        assert!(matches!(step(&db, &t.0).await, Step::Failed(..)));

        // 修好原因后重试应当成功
        std::fs::remove_file(t.0.join("n.md")).unwrap();
        assert!(reset_failed(&db, id).unwrap());
        assert!(matches!(step(&db, &t.0).await, Step::Done(_)));
        assert_eq!(t.read("n.md"), "x");
        assert!(list_failed(&db).unwrap().is_empty());
    }

    #[tokio::test]
    async fn discard_removes_row() {
        let t = Tmp::new("discard");
        let db = mem_db();
        let id = make_failed(&db, &t).await;

        assert!(discard_failed(&db, id).unwrap());
        assert!(list_failed(&db).unwrap().is_empty());
        assert_eq!(queue_len(&db), 0);
        assert_eq!(t.read("n.md"), "已有
", "丢弃只影响队列，不该动文件");
    }

    #[tokio::test]
    async fn retry_and_discard_ignore_non_failed_rows() {
        let t = Tmp::new("guard");
        let db = mem_db();
        // 一条正常待处理的记录（retries = 0）
        let id =
            enqueue_to_db(&db, &cs("inbox.md", Operation::Append { content: "a".into() }, None))
                .unwrap();

        // 正在正常排队的记录不该被这两个操作碰到
        assert!(!reset_failed(&db, id).unwrap());
        assert!(!discard_failed(&db, id).unwrap());
        assert_eq!(queue_len(&db), 1, "记录必须还在");

        // 它仍应能正常落盘
        assert!(matches!(step(&db, &t.0).await, Step::Done(_)));
        assert_eq!(t.read("inbox.md"), "a
");
    }

    #[tokio::test]
    async fn operations_on_missing_id_report_false() {
        let db = mem_db();
        assert!(!reset_failed(&db, 999).unwrap());
        assert!(!discard_failed(&db, 999).unwrap());
    }

    // ---------- 崩溃重放保护 ----------

    fn marker_of(db: &crate::db::Handle, id: i64) -> Option<String> {
        db.with(|c| {
            c.query_row("SELECT applied_marker FROM write_queue WHERE id = ?1", [id], |r| r.get(0))
        })
        .unwrap()
    }

    fn insert_crashed_append(db: &crate::db::Handle, content: &str) {
        let payload =
            serde_json::to_string(&Operation::Append { content: content.into() }).unwrap();
        db.with(|c| {
            c.execute(
                "INSERT INTO write_queue
                 (file_path, operation, payload, retries, applied_marker, created_at, updated_at)
                 VALUES ('inbox.md', 'append', ?1, 0, '1', 1, 1)",
                rusqlite::params![payload],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn append_is_not_replayed_after_crash() {
        let t = Tmp::new("no-replay");
        let db = mem_db();
        t.write("inbox.md", "- [ ] 只该出现一次\n");
        insert_crashed_append(&db, "- [ ] 只该出现一次");

        while !matches!(step(&db, &t.0).await, Step::Empty) {}

        assert_eq!(t.read("inbox.md"), "- [ ] 只该出现一次\n", "崩溃重放不得产生重复行");
        assert_eq!(queue_len(&db), 0);
    }

    #[tokio::test]
    async fn append_is_replayed_when_write_did_not_happen() {
        let t = Tmp::new("do-replay");
        let db = mem_db();
        insert_crashed_append(&db, "- [ ] 没写成");

        while !matches!(step(&db, &t.0).await, Step::Empty) {}

        assert_eq!(t.read("inbox.md"), "- [ ] 没写成\n", "确实没写成的必须补写，不能丢");
    }

    #[tokio::test]
    async fn marker_is_set_before_touching_file() {
        let t = Tmp::new("marker-order");
        let db = mem_db();
        let id =
            enqueue_to_db(&db, &cs("inbox.md", Operation::Append { content: "x".into() }, None))
                .unwrap();
        assert!(marker_of(&db, id).is_none(), "入队时不该有 marker");

        // 让它失败在写盘阶段：把目标做成目录，怎么都写不进去
        std::fs::create_dir(t.0.join("inbox.md")).unwrap();
        let _ = step(&db, &t.0).await;

        // 即便写失败，marker 也必须已置上。顺序反了这层保护就不存在了。
        assert!(marker_of(&db, id).is_some(), "step 应在动文件之前记下 marker");
    }

    #[tokio::test]
    async fn legitimate_duplicate_append_still_works() {
        let t = Tmp::new("dup-ok");
        let db = mem_db();
        // 用户真的连着记了两条一样的，两条都得在
        for _ in 0..2 {
            enqueue_to_db(
                &db,
                &cs("inbox.md", Operation::Append { content: "- [ ] 买菜".into() }, None),
            )
            .unwrap();
        }

        while !matches!(step(&db, &t.0).await, Step::Empty) {}

        assert_eq!(
            t.read("inbox.md"),
            "- [ ] 买菜\n- [ ] 买菜\n",
            "重放保护只针对崩溃恢复，不该压掉用户真实的重复输入"
        );
    }
}
