//! 待办索引：全量扫描 md 文件建立 todos 表（D10）
//!
//! 对应 spec：todo/index

use std::path::Path;
use tauri::AppHandle;

/// 在后台启动全量索引扫描（任务 8.5）
pub fn spawn_scan(_app: AppHandle, vault_root: impl AsRef<Path>) {
    let _vault = vault_root.as_ref().to_path_buf();
    tokio::spawn(async move {
        println!("[index] 开始后台全量扫描");

        // TODO: 实际扫描逻辑（任务组 8）
        // 1. 递归遍历 vault_root 下所有 .md 文件
        // 2. 逐行调用 syntax::parse()
        // 3. 写入 todos 表
        // 4. 统计并打印结果

        println!("[index] 扫描完成：<待实现>");
    });
}
