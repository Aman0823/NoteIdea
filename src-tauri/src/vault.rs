//! vault 目录初始化。
//!
//! 原则：只补齐缺失项，绝不覆盖或移动用户已有的任何文件。用户选中的目录里可能
//! 本来就有别的东西（甚至是个已有的 Obsidian 库），我们无权清理。
//!
//! 对应 spec：vault/config 的「vault 目录初始化」「派生数据与用户数据分离」

use std::fs;
use std::path::{Path, PathBuf};

pub const STATE_DIR: &str = ".noteidea";
pub const DB_FILE: &str = "local.db";
pub const INBOX: &str = "inbox.md";
pub const ASSETS: &str = "assets";

/// `.noteidea/` 里的一切都是可丢弃的派生数据，不该进版本库。
const GITIGNORE_ENTRY: &str = ".noteidea/";

pub fn state_dir(vault: &Path) -> PathBuf {
    vault.join(STATE_DIR)
}

pub fn db_path(vault: &Path) -> PathBuf {
    state_dir(vault).join(DB_FILE)
}

pub fn inbox_path(vault: &Path) -> PathBuf {
    vault.join(INBOX)
}

/// 确保 vault 基础结构存在。幂等：重复调用不产生副作用。
pub fn init(vault: &Path) -> Result<(), String> {
    fs::create_dir_all(state_dir(vault)).map_err(|e| format!("建 .noteidea 目录失败: {e}"))?;
    fs::create_dir_all(vault.join(ASSETS)).map_err(|e| format!("建 assets 目录失败: {e}"))?;

    // 缺了就建空文件；已存在则一个字节都不动。
    let inbox = inbox_path(vault);
    if !inbox.exists() {
        fs::write(&inbox, "").map_err(|e| format!("建 inbox.md 失败: {e}"))?;
        println!("[vault] 已创建 {}", inbox.display());
    }

    ensure_gitignore(vault)?;
    Ok(())
}

/// 往 vault 的 .gitignore 追加 `.noteidea/`，已有则不重复写。
///
/// 只追加、不重写整个文件——用户自己的忽略规则必须原样保留。
fn ensure_gitignore(vault: &Path) -> Result<(), String> {
    let path = vault.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let already = existing
        .lines()
        .any(|l| matches!(l.trim(), ".noteidea/" | ".noteidea" | "/.noteidea/" | "/.noteidea"));
    if already {
        return Ok(());
    }

    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(GITIGNORE_ENTRY);
    out.push('\n');

    fs::write(&path, out).map_err(|e| format!("写 .gitignore 失败: {e}"))
}
