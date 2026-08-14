//! 待办身份分配与管理（D6, D7, D8）
//!
//! 对应 spec：todo/identity

use rand::Rng;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
