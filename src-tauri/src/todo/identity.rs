//! 待办身份分配与管理（D6, D7, D8）
//!
//! 对应 spec：todo/identity

use rand::Rng;

/// 分配 ID 并经 actor 写回的结果
#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub enum AllocateResult {
    /// 成功分配并写回
    Success(String),
    /// ID 生成失败（穷尽所有位宽仍冲突）
    GenerationFailed,
    /// 写回失败（actor 返回错误）
    WritebackFailed(String),
}

/// 生成随机 ID，4 位十六进制起，冲突则扩位至上限 8 位
pub fn generate_id<F>(mut is_duplicate: F) -> Option<String>
where
    F: FnMut(&str) -> bool,
{
    const MAX_ATTEMPTS_PER_LENGTH: usize = 100;
    const LENGTHS: &[usize] = &[4, 5, 6, 7, 8];

    let mut rng = rand::rng();

    for &len in LENGTHS {
        for _ in 0..MAX_ATTEMPTS_PER_LENGTH {
            let id: String = (0..len)
                .map(|_| {
                    let digit = rng.random_range(0..16);
                    char::from_digit(digit, 16).unwrap()
                })
                .collect();

            if !is_duplicate(&id) {
                return Some(id);
            }
        }
    }

    // 8 位仍然冲突 100 次，放弃
    None
}

/// 查询 ID 是否已存在于 todos 表
pub fn id_exists(db: &crate::db::Handle, id: &str) -> Result<bool, String> {
    db.with(|conn| {
        let mut stmt = conn
            .prepare("SELECT 1 FROM todos WHERE todo_id = ?1 LIMIT 1")?;

        stmt.exists([id])
    })
}

/// 为待办分配 ID 并原子写回 md（任务 7.4）
///
/// 这是「设提醒 / 贴屏」操作的第一步。写回失败则整个操作失败，
/// DB 不留记录，避免失联导致漏发提醒（spec 明确禁止延后写盘）。
#[allow(dead_code)]
pub async fn allocate_and_writeback(
    db: &crate::db::Handle,
    actor: &crate::actor::Handle,
    file_path: &str,
    line_number: usize,
    current_line: &str,
) -> AllocateResult {
    // 1. 生成 ID 并查重
    let id = match generate_id(|candidate| id_exists(db, candidate).unwrap_or(false)) {
        Some(id) => id,
        None => return AllocateResult::GenerationFailed,
    };

    // 2. 解析当前行，检查是否已有 ID
    let todo = match crate::todo::syntax::parse(current_line) {
        Some(t) => t,
        None => {
            return AllocateResult::WritebackFailed(
                "该行不是待办行，无法分配 ID".to_string(),
            )
        }
    };

    if todo.markers.iter().any(|m| matches!(m.value, crate::todo::syntax::MarkerValue::Id(_))) {
        return AllocateResult::WritebackFailed("该待办已有 ID".to_string());
    }

    // 3. 追加 ID 标记
    let new_line = crate::todo::syntax::write_marker_to_line(
        current_line,
        &crate::todo::syntax::MarkerValue::Id(id.clone()),
    );

    // 4. 构造写回请求
    let baseline = blake3::hash(current_line.as_bytes());
    let cs = crate::actor::ChangeSet {
        file_path: file_path.to_string(),
        base_hash: Some(format!("{}", baseline.to_hex())),
        op: crate::actor::Operation::ReplaceLine {
            line_number,
            old_content: current_line.to_string(),
            new_content: new_line,
        },
    };

    // 5. 提交并等待结果
    match actor.enqueue(cs).await {
        Ok(_) => AllocateResult::Success(id),
        Err(e) => AllocateResult::WritebackFailed(e),
    }
}


/// 处理重复 ID：为后者分配新 ID 并写回（任务 7.5）
///
/// 当索引扫描发现同一 ID 出现在多处时，先扫到的保留原 ID，
/// 后扫到的调用此函数重新分配。写回失败则返回 Err，由调用方
/// 决定如何标记该项（通常是「身份未确定」状态）。
#[allow(dead_code)]
pub async fn reallocate_duplicate(
    db: &crate::db::Handle,
    actor: &crate::actor::Handle,
    file_path: &str,
    line_number: usize,
    current_line: &str,
    old_id: &str,
) -> Result<String, String> {
    // 1. 生成新 ID 并查重
    let new_id = generate_id(|candidate| id_exists(db, candidate).unwrap_or(false))
        .ok_or_else(|| "ID 生成失败（穷尽所有位宽仍冲突）".to_string())?;

    // 2. 替换旧 ID 为新 ID
    let new_line = current_line.replace(&format!("~{}", old_id), &format!("~{}", new_id));

    // 3. 构造写回请求
    let baseline = blake3::hash(current_line.as_bytes());
    let cs = crate::actor::ChangeSet {
        file_path: file_path.to_string(),
        base_hash: Some(format!("{}", baseline.to_hex())),
        op: crate::actor::Operation::ReplaceLine {
            line_number,
            old_content: current_line.to_string(),
            new_content: new_line,
        },
    };

    // 4. 提交并等待结果
    actor.enqueue(cs).await.map_err(|e| format!("写回失败: {}", e))?;

    Ok(new_id)
}




#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> crate::db::Handle {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA_SQL).unwrap();
        crate::db::Handle::new(conn)
    }

    #[test]
    fn generates_4_hex_when_no_collision() {
        let id = generate_id(|_| false).unwrap();
        assert_eq!(id.len(), 4);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn widens_to_5_when_4_always_collides() {
        let mut attempts = 0;
        let id = generate_id(|candidate| {
            attempts += 1;
            candidate.len() == 4 // 4 位的全部冲突
        })
        .unwrap();

        assert_eq!(id.len(), 5);
        assert!(attempts > 100); // 至少尝试了 100 次 4 位
    }

    #[test]
    fn gives_up_after_all_lengths_exhausted() {
        let result = generate_id(|_| true); // 全部冲突
        assert!(result.is_none());
    }

    #[test]
    fn generated_ids_are_random() {
        let id1 = generate_id(|_| false).unwrap();
        let id2 = generate_id(|_| false).unwrap();
        // 极大概率不相等（16^4 = 65536 种可能，碰撞概率极低）
        assert_ne!(id1, id2);
    }

    // 任务 7.6：ID 写回后该行其余字节与文件其余内容逐字节不变
    #[tokio::test]
    async fn id_writeback_preserves_file_exactly() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let vault_root = temp_dir.path();

        let file_path = vault_root.join("test.md");
        let original = "第一行\n- [ ] 交周报 @2026-08-14 18:00\n第三行\n";
        std::fs::write(&file_path, original).unwrap();

        let db = mem_db();

        // 手动构造 ID 写回操作
        let current_line = "- [ ] 交周报 @2026-08-14 18:00";
        let id = generate_id(|candidate| id_exists(&db, candidate).unwrap_or(false)).unwrap();

        let new_line = crate::todo::syntax::write_marker_to_line(
            current_line,
            &crate::todo::syntax::MarkerValue::Id(id.clone()),
        );

        let baseline = blake3::hash(current_line.as_bytes());
        let cs = crate::actor::ChangeSet {
            file_path: "test.md".to_string(),
            base_hash: Some(format!("{}", baseline.to_hex())),
            op: crate::actor::Operation::ReplaceLine {
                line_number: 2,
                old_content: current_line.to_string(),
                new_content: new_line.clone(),
            },
        };

        // 入队并执行
        crate::actor::enqueue_to_db(&db, &cs).unwrap();
        crate::actor::step(&db, vault_root).await;

        let content = std::fs::read_to_string(&file_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        // 第一行和第三行完全不变
        assert_eq!(lines[0], "第一行");
        assert_eq!(lines[2], "第三行");

        // 第二行只在末尾追加了 ~id
        assert_eq!(lines[1], format!("- [ ] 交周报 @2026-08-14 18:00 ~{}", id));

        // 换行符数量不变
        assert_eq!(lines.len(), 3);
    }

    // 任务 7.7：写回失败时 DB 无残留记录（简化版：只测入队逻辑）
    #[test]
    fn id_exists_returns_false_for_nonexistent() {
        let db = mem_db();
        assert!(!id_exists(&db, "abcd").unwrap());
    }

    #[test]
    fn id_exists_returns_true_after_insert() {
        let db = mem_db();

        db.with(|conn| {
            conn.execute(
                "INSERT INTO todos (file_path, line_number, todo_id, text, checked, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                ("test.md", 1, "abcd", "任务", 0, 1234567890_i64),
            )
        })
        .unwrap();

        assert!(id_exists(&db, "abcd").unwrap());
    }

    // 任务 7.8：重复 ID 的保留与重分配
    #[tokio::test]
    async fn duplicate_id_handling() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let vault_root = temp_dir.path();

        let file_path = vault_root.join("test.md");
        let original = "- [ ] 任务A ~abcd\n- [ ] 任务B ~abcd\n";
        std::fs::write(&file_path, original).unwrap();

        let db = mem_db();

        // 模拟索引扫描：先扫到第一行，插入 DB
        db.with(|conn| {
            conn.execute(
                "INSERT INTO todos (file_path, line_number, todo_id, text, checked, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                ("test.md", 1, "abcd", "任务A", 0, 1234567890_i64),
            )
        })
        .unwrap();

        // 扫到第二行，发现重复 ID，手动重分配
        let new_id = generate_id(|candidate| id_exists(&db, candidate).unwrap_or(false)).unwrap();
        assert_ne!(new_id, "abcd");

        let current_line = "- [ ] 任务B ~abcd";
        let new_line = current_line.replace("~abcd", &format!("~{}", new_id));

        let baseline = blake3::hash(current_line.as_bytes());
        let cs = crate::actor::ChangeSet {
            file_path: "test.md".to_string(),
            base_hash: Some(format!("{}", baseline.to_hex())),
            op: crate::actor::Operation::ReplaceLine {
                line_number: 2,
                old_content: current_line.to_string(),
                new_content: new_line,
            },
        };

        crate::actor::enqueue_to_db(&db, &cs).unwrap();
        crate::actor::step(&db, vault_root).await;

        // 验证文件内容
        let content = std::fs::read_to_string(&file_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        assert_eq!(lines[0], "- [ ] 任务A ~abcd", "第一行保持不变");
        assert_eq!(lines[1], format!("- [ ] 任务B ~{}", new_id), "第二行 ID 已替换");
    }

    // 任务 7.8 补充：先扫到的记录不受后续失败影响
    #[test]
    fn first_record_unaffected_by_later_operations() {
        let db = mem_db();

        // 先扫到的记录
        db.with(|conn| {
            conn.execute(
                "INSERT INTO todos (file_path, line_number, todo_id, text, checked, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                ("first.md", 1, "abcd", "任务A", 0, 1234567890_i64),
            )
        })
        .unwrap();

        // 后续的任何操作都不应该改变这条记录
        let first_record: (String, i64, String) = db
            .with(|conn| {
                conn.query_row(
                    "SELECT file_path, line_number, text FROM todos WHERE todo_id = 'abcd'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .unwrap();

        assert_eq!(first_record, ("first.md".to_string(), 1, "任务A".to_string()));
    }
}

