//! NoteIdea —— 笔记本 + 贴屏便签 + 定时提醒。
//!
//! 本文件只做装配：Tauri builder、command 注册、托盘与热键。
//! 具体逻辑分散在各模块：
//!   - `window`  窗口显示/隐藏、速记条延迟测量（NFR-2）
//!
//! 已落地的架构决策：D22（窗口预热）、D23（单实例）、FR-13（托盘常驻）、FR-21（热键失败提示）。

mod actor;
mod config;
mod db;
mod todo;
mod vault;
mod window;

use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

use config::{Config, VaultStatus};
use window::{HotkeyFailures, Timing, MAIN, QUICK};

/// 运行期共享的 vault 状态。用户可能中途重选，所以要可变。
pub struct VaultState(pub Mutex<VaultStatus>);

/// 追加一行到 inbox.md（FR-19 / D14）。
///
/// 走单写者 actor（D17）。返回代表「已入队」而非「已落盘」——落盘结果
/// 通过 `file:changed` / `write:failed` 事件通知，这样速记条能立刻关窗。
///
/// 这里无条件加 `- [ ]` 前缀是骨架期的简化，正式版应由 3.2 的语法解析决定。
#[tauri::command]
async fn capture(
    text: String,
    vault: State<'_, VaultState>,
    actor: State<'_, actor::Handle>,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }

    // degraded 状态下不静默丢弃用户刚敲的内容，而是明确告知原因。
    {
        let status = vault.0.lock().map_err(|_| "vault 状态锁失败")?;
        if let Some(reason) = status.reason() {
            return Err(reason);
        }
    }

    actor
        .enqueue(actor::ChangeSet {
            file_path: vault::INBOX.to_string(),
            op: actor::Operation::Append { content: format!("- [ ] {text}") },
            base_hash: None, // append 不关心基线
        })
        .await
        .map(|_| ())
}

/// vault 当前是否可用，以及不可用的原因。前端据此决定是否显示选择入口。
#[tauri::command]
fn vault_state(vault: State<'_, VaultState>) -> Result<Option<String>, String> {
    let status = vault.0.lock().map_err(|_| "vault 状态锁失败")?;
    Ok(status.reason())
}

/// 已放弃的写入列表（FR：写失败不得静默丢弃）。
#[tauri::command]
fn failed_writes(app: AppHandle) -> Result<Vec<actor::FailedWrite>, String> {
    // vault 不可用时没有 DB，此时没有失败列表可言，返回空而非报错。
    let Some(db) = app.try_state::<db::Handle>() else { return Ok(Vec::new()) };
    actor::list_failed(&db)
}

/// 重试一条失败的写入。
#[tauri::command]
fn retry_write(
    id: i64,
    db: State<'_, db::Handle>,
    actor: State<'_, actor::Handle>,
) -> Result<(), String> {
    if !actor::reset_failed(&db, id)? {
        return Err("该记录已不存在或不处于失败状态".into());
    }
    actor.drain();
    Ok(())
}

/// 丢弃一条失败的写入。用户主动放弃这次修改。
#[tauri::command]
fn discard_write(id: i64, db: State<'_, db::Handle>) -> Result<(), String> {
    if !actor::discard_failed(&db, id)? {
        return Err("该记录已不存在或不处于失败状态".into());
    }
    Ok(())
}

/// 解析待办行（D3：返回完整结构而非 UI 指令）
#[tauri::command]
fn parse_todo_line(text: String) -> Option<todo::syntax::TodoLine> {
    todo::syntax::parse(&text)
}

/// 查询 vault 内已有标签，按使用频次排序（供 # 弹层）
#[tauri::command]
fn list_tags(db: State<'_, db::Handle>) -> Result<Vec<(String, usize)>, String> {
    todo::index::list_tags(&db)
}

/// 手动触发全量重扫（任务 8.7）
#[tauri::command]
fn rescan_index(app: AppHandle, vault: State<'_, VaultState>) -> Result<(), String> {
    let status = vault.0.lock().map_err(|_| "vault 状态锁失败")?;

    if let config::VaultStatus::Ready(path) = &*status {
        todo::index::spawn_scan(app, path.clone());
        Ok(())
    } else {
        Err("vault 不可用".into())
    }
}

/// 为待办分配唯一 ID（D7：4-8 位十六进制，全库查重）
#[tauri::command]
fn allocate_todo_id(db: State<'_, db::Handle>) -> Result<String, String> {
    todo::identity::generate_id(|id| {
        todo::identity::id_exists(&db, id).unwrap_or(false)
    })
    .ok_or_else(|| "ID 分配失败：所有长度都已穷尽".into())
}

/// 将分配的 ID 写回 md 文件（D6：写盘失败则整个操作失败）
#[tauri::command]
async fn write_todo_id(
    file_path: String,
    line_number: usize,
    old_content: String,
    todo_id: String,
    vault: State<'_, VaultState>,
    actor: State<'_, actor::Handle>,
) -> Result<(), String> {
    {
        let status = vault.0.lock().map_err(|_| "vault 状态锁失败")?;
        if let Some(reason) = status.reason() {
            return Err(reason);
        }
    }

    let new_content = todo::syntax::write_marker_to_line(
        &old_content,
        &todo::syntax::MarkerValue::Id(todo_id),
    );

    actor
        .enqueue(actor::ChangeSet {
            file_path,
            op: actor::Operation::ReplaceLine {
                line_number,
                old_content,
                new_content,
            },
            base_hash: None, // D8：路径+行号+内容三重校验，已经够了
        })
        .await
        .map(|_| ())
}

/// 用户选定 vault 目录后调用：初始化结构、存配置、更新运行期状态。
#[tauri::command]
fn choose_vault(
    path: String,
    app: AppHandle,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    let dir = std::path::PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("不是一个目录：{path}"));
    }

    vault::init(&dir)?;

    let cfg = Config {
        vault_path: Some(dir.clone()),
        version: 1,
        default_reminder_time: "09:00".to_string(),
    };
    cfg.save()?;

    let status = cfg.vault_status();
    if let Some(reason) = status.reason() {
        return Err(reason);
    }

    *vault.0.lock().map_err(|_| "vault 状态锁失败")? = status;
    println!("[vault] 已选定 {}", dir.display());
    let _ = app.emit("vault:changed", ());
    Ok(())
}

/// 托盘（FR-13）：左键弹速记条，右键菜单可开主窗口 / 退出。
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let quick_item = MenuItem::with_id(app, "quick", "速记  Alt+Space", true, None::<&str>)?;
    let main_item = MenuItem::with_id(app, "main", "主窗口  Alt+N", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quick_item, &main_item, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("tray")
        .tooltip("NoteIdea")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quick" => window::show_quick(app),
            "main" => window::show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::show_quick(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    } else {
        eprintln!("[tray] 拿不到默认窗口图标，托盘将无图标显示");
    }

    builder.build(app)?;
    Ok(())
}

/// 注册全局热键（FR-19 / FR-20）。返回注册失败的键位描述（FR-21）。
fn setup_hotkeys(app: &AppHandle) -> Vec<String> {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

    let quick_key = Shortcut::new(Some(Modifiers::ALT), Code::Space);
    let main_key = Shortcut::new(Some(Modifiers::ALT), Code::KeyN);

    let plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            if shortcut == &quick_key {
                window::show_quick(app);
            } else if shortcut == &main_key {
                window::show_main(app);
            }
        })
        .build();

    if let Err(e) = app.plugin(plugin) {
        eprintln!("[hotkey] 全局热键插件初始化失败: {e}");
        return vec!["插件初始化失败".into()];
    }

    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let mut failed = Vec::new();
    for (key, label) in [(quick_key, "Alt+Space（速记条）"), (main_key, "Alt+N（主窗口）")] {
        if let Err(e) = app.global_shortcut().register(key) {
            eprintln!("[hotkey] {label} 注册失败: {e}");
            failed.push(label.to_string());
        }
    }
    failed
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // D23：单实例。第二个实例不开窗口，把「想记东西」的意图转交给已有实例。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            println!("[instance] 检测到第二个实例，聚焦已有实例并弹速记条");
            window::show_quick(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(Timing::default()))
        .invoke_handler(tauri::generate_handler![
            window::mark_ready,
            window::timings,
            window::hide_quick,
            window::quick_warmed,
            window::hotkey_failures,
            capture,
            vault_state,
            choose_vault,
            failed_writes,
            retry_write,
            discard_write,
            parse_todo_line,
            list_tags,
            rescan_index,
            allocate_todo_id,
            write_todo_id
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // vault 解析：路径失效时进入 degraded 状态而非静默重建目录。
            let cfg = Config::load();
            let mut status = cfg.vault_status();
            if let VaultStatus::Ready(p) = &status {
                let path = p.clone();
                vault::init(&path)?;
                println!("[vault] {}", path.display());

                // 数据库跟着 vault 走。打不开时降级为不可用，而不是让后续
                // 每次写入都失败——用户至少要知道为什么用不了。
                match db::open(&vault::db_path(&path)) {
                    Ok(conn) => {
                        app.manage(db::Handle::new(conn));
                        // actor 必须在 DB 就绪后启动：它启动时就会去排空遗留队列。
                        app.manage(actor::spawn(handle.clone(), path.clone()));

                        // 任务 8.5：在后台启动全量索引扫描，不阻塞窗口显示
                        todo::index::spawn_scan(handle.clone(), path.clone());
                    }
                    Err(e) => {
                        eprintln!("[db] 打开失败: {e}");
                        status = VaultStatus::NotWritable(path);
                    }
                }
            }
            if let Some(reason) = status.reason() {
                println!("[vault] 不可用（{reason}），进入 degraded 状态");
            }
            app.manage(VaultState(Mutex::new(status)));

            setup_tray(&handle)?;

            let failed = setup_hotkeys(&handle);
            app.manage(HotkeyFailures(failed.clone()));
            if !failed.is_empty() {
                let _ = handle.emit_to(MAIN, "hotkey:failed", failed);
            }

            // D22：速记条窗口在 conf 里已声明为 visible:false，
            // 进程启动时即完成 WebView 初始化，热键只做 show。
            if app.get_webview_window(QUICK).is_none() {
                eprintln!("[quick] 预热窗口未创建，NFR-2 无法达标");
            }

            Ok(())
        })
        .on_window_event(|win, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // FR-13：关闭主窗口只隐藏，进程留在托盘。
                api.prevent_close();
                let _ = win.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("启动 NoteIdea 失败");
}
