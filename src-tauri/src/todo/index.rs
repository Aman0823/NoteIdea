//! 待办索引：扫描 vault 内 md 并构建 todos 表（对应 spec todo/index）
//!
//! 索引是可重建的派生数据（D9），md 为唯一真源。
//! 索引与 md 不一致的窗口客观存在（这期没有文件监听），
//! 用户在外部编辑器改了 md，索引是旧的 → 以 md 为准，提供手动重扫入口。

use crate::db;

/// 查询已有标签，按使用频次排序
pub fn list_tags(db: &db::Handle) -> Result<Vec<(String, u32)>, String> {
    db.with(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT tag, COUNT(*) as cnt FROM (
                    SELECT json_each.value as tag FROM todos, json_each(todos.tags)
                    WHERE todos.tags IS NOT NULL
                ) GROUP BY tag ORDER BY cnt DESC"
            )?;

        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;

        let mut tags = Vec::new();
        for row in rows {
            tags.push(row?);
        }

        Ok(tags)
    })
}
