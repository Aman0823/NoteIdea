//! 待办索引：启动时全量扫描 vault 内 md，解析待办并写入 SQLite。
//!
//! 职责边界（对应 spec：todo/index）——
//!   - 索引是纯派生数据，完全来自 md 解析，随时可删可重建
//!   - md 与索引不一致时一律以 md 为准
//!   - 单个文件失败不中断整体扫描，失败可见地列出
//!
//! 对应 spec：todo/index

use std::fs;
use std::path::{Path, PathBuf};

use crate::db;
use crate::todo::syntax;

/// 全量扫描结果
#[derive(Debug)]
#[allow(dead_code)] // 等后台扫描任务与 command 接入时使用
pub struct ScanResult {
    pub scanned: usize,
    pub todos_found: usize,
    pub skipped_files: Vec<(PathBuf, String)>, // (路径, 失败原因)
}

/// 全量扫描 vault 内所有 md 文件，解析待办并 upsert 到 `todos` 表
#[allow(dead_code)] // 等后台扫描任务接入时使用
pub fn scan_vault(vault_root: &Path, db: &db::Handle) -> Result<ScanResult, String> {
    let mut scanned = 0;
    let mut todos_found = 0;
    let mut skipped_files = Vec::new();

    // 清空旧索引（全量重建）
    db.with(|conn| {
        conn.execute("DELETE FROM todos", [])?;
        Ok(())
    })?;

    // 递归遍历 vault，找出所有 .md 文件
    let md_files = collect_markdown_files(vault_root)?;

    for file_path in md_files {
        match scan_file(&file_path, vault_root, db) {
            Ok(count) => {
                scanned += 1;
                todos_found += count;
            }
            Err(e) => {
                let rel_path = file_path
                    .strip_prefix(vault_root)
                    .unwrap_or(&file_path)
                    .to_path_buf();
                skipped_files.push((rel_path, e));
            }
        }
    }

    Ok(ScanResult {
        scanned,
        todos_found,
        skipped_files,
    })
}

/// 递归收集 vault 内所有 .md 文件，跳过 `.noteidea/` 和 `assets/`
#[allow(dead_code)]
fn collect_markdown_files(vault_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut result = Vec::new();
    visit_dir(vault_root, &mut result)?;
    Ok(result)
}

#[allow(dead_code)]
fn visit_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("读目录失败 {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("读目录项失败: {e}"))?;
        let path = entry.path();

        // 跳过 .noteidea/ 和 assets/
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name == ".noteidea" || name == "assets" {
                continue;
            }
        }

        if path.is_dir() {
            visit_dir(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }

    Ok(())
}

/// 扫描单个文件，返回找到的待办数量
#[allow(dead_code)]
fn scan_file(file_path: &Path, vault_root: &Path, db: &db::Handle) -> Result<usize, String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("读文件失败: {e}"))?;

    let rel_path = file_path
        .strip_prefix(vault_root)
        .map_err(|_| "文件不在 vault 内".to_string())?
        .to_str()
        .ok_or("路径包含非 UTF-8 字符")?;

    let lines: Vec<&str> = content.lines().collect();
    let todos = parse_todos(&lines);

    let scanned_at = db::now_ms();

    // 批量写入
    db.with(|conn| {
        let tx = conn.transaction()?;
        for todo in &todos {
            tx.execute(
                r#"
                INSERT INTO todos (file_path, line_number, todo_id, text, checked, time_expr, recurrence, intensity, tags, scanned_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT (file_path, line_number) DO UPDATE SET
                    todo_id = excluded.todo_id,
                    text = excluded.text,
                    checked = excluded.checked,
                    time_expr = excluded.time_expr,
                    recurrence = excluded.recurrence,
                    intensity = excluded.intensity,
                    tags = excluded.tags,
                    scanned_at = excluded.scanned_at
                "#,
                rusqlite::params![
                    rel_path,
                    todo.line_number as i64,
                    todo.id.as_deref(),
                    &todo.text,
                    if todo.checked { 1 } else { 0 },
                    todo.time_expr.as_deref(),
                    todo.recurrence.as_deref(),
                    todo.intensity.as_deref(),
                    todo.tags.as_deref(),
                    scanned_at,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    })?;

    Ok(todos.len())
}

#[derive(Debug)]
#[allow(dead_code)]
struct TodoItem {
    line_number: usize,
    id: Option<String>,
    text: String,
    checked: bool,
    time_expr: Option<String>,  // JSON
    recurrence: Option<String>,
    intensity: Option<String>,
    tags: Option<String>,       // JSON array
}

/// 解析文件中的所有待办，跳过代码块内的内容
#[allow(dead_code)]
fn parse_todos(lines: &[&str]) -> Vec<TodoItem> {
    let mut result = Vec::new();
    let mut in_code_block = false;

    for (idx, line) in lines.iter().enumerate() {
        // 检测代码块边界
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            continue;
        }

        // 尝试解析为待办
        if let Some(todo_line) = syntax::parse(line) {
            let id = todo_line.markers.iter()
                .find_map(|m| match &m.value {
                    syntax::MarkerValue::Id(id) => Some(id.clone()),
                    _ => None,
                });

            let time_marker = todo_line.markers.iter()
                .find(|m| matches!(m.value, syntax::MarkerValue::Time(_)));

            let time_expr = time_marker.map(|m| {
                if let syntax::MarkerValue::Time(ref te) = m.value {
                    serde_json::to_string(te).unwrap_or_default()
                } else {
                    String::new()
                }
            });

            let recurrence = todo_line.markers.iter()
                .find_map(|m| match &m.value {
                    syntax::MarkerValue::Repeat(r) => Some(format!("{:?}", r).to_lowercase()),
                    _ => None,
                });

            let intensity = todo_line.markers.iter()
                .find_map(|m| match &m.value {
                    syntax::MarkerValue::Intensity(i) => Some(format!("{:?}", i).to_lowercase()),
                    _ => None,
                });

            let tags: Vec<String> = todo_line.markers.iter()
                .filter_map(|m| match &m.value {
                    syntax::MarkerValue::Tag(t) => Some(t.clone()),
                    _ => None,
                })
                .collect();

            let tags_json = if tags.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&tags).unwrap_or_default())
            };

            let content_text = &line[todo_line.content.start..todo_line.content.end];

            result.push(TodoItem {
                line_number: idx + 1, // 1-based
                id,
                text: content_text.to_string(),
                checked: todo_line.checked,
                time_expr,
                recurrence,
                intensity,
                tags: tags_json,
            });
        }
    }

    result
}

/// 列出所有已使用的标签及其出现次数，按频次降序
pub fn list_tags(db: &db::Handle) -> Result<Vec<(String, usize)>, String> {
    db.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT tags FROM todos WHERE tags IS NOT NULL"
        )?;

        let mut tag_counts = std::collections::HashMap::new();

        let rows = stmt.query_map([], |row| {
            let tags_json: String = row.get(0)?;
            Ok(tags_json)
        })?;

        for row in rows {
            let tags_json = row?;
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(&tags_json) {
                for tag in tags {
                    *tag_counts.entry(tag).or_insert(0) += 1;
                }
            }
        }

        let mut result: Vec<_> = tag_counts.into_iter().collect();
        result.sort_by_key(|b| std::cmp::Reverse(b.1)); // 按频次降序

        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn skips_code_blocks() {
        let lines = vec![
            "- [ ] 真待办",
            "```",
            "- [ ] 代码块内的伪待办",
            "```",
            "- [x] 另一个真待办",
        ];

        let todos = parse_todos(&lines);
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].text, "真待办");
        assert_eq!(todos[1].text, "另一个真待办");
    }

    #[test]
    fn parses_markers() {
        let lines = vec![
            "- [ ] 交周报 @2026-08-14 18:00 !weekly #工作 ^ring ~a3f9",
        ];

        let todos = parse_todos(&lines);
        assert_eq!(todos.len(), 1);
        let todo = &todos[0];
        assert_eq!(todo.text, "交周报 "); // content_end 指向第一个标记前，包含尾部空格
        assert_eq!(todo.id.as_deref(), Some("a3f9"));
        assert!(todo.time_expr.is_some());
        assert_eq!(todo.recurrence.as_deref(), Some("weekly"));
        assert_eq!(todo.intensity.as_deref(), Some("ring"));
        assert!(todo.tags.is_some());
    }

    #[test]
    fn full_scan_recreates_index() {
        let temp = TempDir::new().unwrap();
        let vault_root = temp.path();

        fs::write(vault_root.join("a.md"), "- [ ] 任务A\n").unwrap();
        fs::write(vault_root.join("b.md"), "- [x] 任务B\n").unwrap();

        let db_path = vault_root.join(".noteidea").join("test.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = crate::db::open(&db_path).unwrap();
        let db = crate::db::Handle::new(conn);

        let result = scan_vault(vault_root, &db).unwrap();
        assert_eq!(result.scanned, 2);
        assert_eq!(result.todos_found, 2);
        assert!(result.skipped_files.is_empty());

        // 验证数据库内容
        let count: i64 = db.with(|conn| {
            conn.query_row("SELECT COUNT(*) FROM todos", [], |r| r.get(0))
        }).unwrap();
        assert_eq!(count, 2);
    }
}
