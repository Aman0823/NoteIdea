//! 用户级配置：vault 路径的存放与解析。
//!
//! 配置放在用户配置目录而非 vault 内部——vault 内的 `.noteidea/` 只存该 vault 自身的
//! 派生状态，而「上次用的是哪个 vault」这种跨 vault 的信息必须在 vault 之外。
//!
//! 对应 spec：vault/config

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const APP_DIR: &str = "NoteIdea";
const CONFIG_FILE: &str = "config.json";
const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 选定的 vault 目录。`None` 表示用户还没选过，应用处于 degraded 状态。
    #[serde(default)]
    pub vault_path: Option<PathBuf>,
    #[serde(default = "default_version")]
    pub version: u32,
    /// 默认提醒时间（HH:MM 格式），用于只指定日期时补全。缺失或非法时回落 09:00。
    #[serde(default = "default_reminder_time")]
    pub default_reminder_time: String,
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

fn default_reminder_time() -> String {
    "09:00".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            vault_path: None,
            version: CURRENT_VERSION,
            default_reminder_time: default_reminder_time(),
        }
    }
}

/// vault 路径当前的可用状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultStatus {
    /// 可用。
    Ready(PathBuf),
    /// 用户从未选过 vault。
    NotChosen,
    /// 配置里记着路径，但目录已不存在（被删/改名/移动盘符未挂载）。
    Missing(PathBuf),
    /// 目录在，但写不进去。
    NotWritable(PathBuf),
}

impl VaultStatus {
    pub fn ready_path(&self) -> Option<&Path> {
        match self {
            Self::Ready(p) => Some(p),
            _ => None,
        }
    }

    /// 给前端看的原因说明。degraded 状态必须能讲清为什么不可用。
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Ready(_) => None,
            Self::NotChosen => Some("还没有选择笔记存放位置".into()),
            Self::Missing(p) => Some(format!("上次使用的笔记目录已不存在：{}", p.display())),
            Self::NotWritable(p) => Some(format!("笔记目录没有写入权限：{}", p.display())),
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR))
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(CONFIG_FILE))
}

impl Config {
    /// 读配置。文件缺失或损坏都不阻塞启动：
    /// 损坏的文件改名保留（便于用户自己找回内容），然后用默认值继续。
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            eprintln!("[config] 拿不到用户配置目录，使用默认配置");
            return Self::default();
        };

        let raw = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                eprintln!("[config] 读取失败，使用默认配置: {e}");
                return Self::default();
            }
        };

        match serde_json::from_str::<Self>(&raw) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("[config] 解析失败，保留损坏文件并使用默认配置: {e}");
                let backup = path.with_extension("json.corrupt");
                if let Err(e) = fs::rename(&path, &backup) {
                    eprintln!("[config] 损坏文件改名失败: {e}");
                } else {
                    eprintln!("[config] 损坏文件已保留为 {}", backup.display());
                }
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = config_dir().ok_or("拿不到用户配置目录")?;
        fs::create_dir_all(&dir).map_err(|e| format!("建配置目录失败: {e}"))?;
        let path = dir.join(CONFIG_FILE);
        let json = serde_json::to_string_pretty(self).map_err(|e| format!("序列化失败: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("写配置失败: {e}"))
    }

    /// 判定 vault 当前是否可用。
    ///
    /// 路径失效时**不**静默重建同名目录——那会让用户以为笔记丢了，
    /// 而实际可能只是移动硬盘没插。
    pub fn vault_status(&self) -> VaultStatus {
        let Some(path) = &self.vault_path else {
            return VaultStatus::NotChosen;
        };
        if !path.is_dir() {
            return VaultStatus::Missing(path.clone());
        }
        if is_writable(path) {
            VaultStatus::Ready(path.clone())
        } else {
            VaultStatus::NotWritable(path.clone())
        }
    }

    /// 解析默认提醒时间为 (小时, 分钟)，非法时回落 (9, 0)
    pub fn parse_default_reminder_time(&self) -> (u32, u32) {
        let parts: Vec<&str> = self.default_reminder_time.split(':').collect();
        if parts.len() != 2 {
            return (9, 0);
        }
        let Ok(hour) = parts[0].parse::<u32>() else { return (9, 0) };
        let Ok(minute) = parts[1].parse::<u32>() else { return (9, 0) };
        if hour > 23 || minute > 59 {
            return (9, 0);
        }
        (hour, minute)
    }
}

/// 实测写入探测。只看文件系统只读标志不够——Windows 上 ACL 才是决定性的。
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".noteidea-write-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
