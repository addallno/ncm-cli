//! 持久化配置（`~/.config/ncm/config.json`）：`ncm config` 子命令读写
//!
//! 纯字符串键值 + 键白名单，避免手改配置文件：
//! - `download-dir`：默认下载目录（`download -o` / `play -o` 未指定时回退）
//! - `play.download-key`：播放时快捷下载的 mpv 按键（默认 `d`）
//! - `profile.<板块>`：板块 → 登录会话 profile 名（`login --profile <名>` 保存的凭据）
use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// 可映射会话 profile 的板块名
pub const SECTIONS: &[&str] = &[
    "default",
    "download",
    "play",
    "recommend",
    "roam",
    "search",
    "list",
];

/// 配置文件路径
pub fn config_path() -> std::path::PathBuf {
    crate::session::config_dir().join("config.json")
}

/// 读全部配置（不存在或损坏按空配置处理）
pub fn load_all() -> Map<String, Value> {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// 落盘（目录 0700、文件 0600，同凭据文件）
fn save(m: &Map<String, Value>) -> Result<()> {
    let dir = crate::session::config_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建配置目录失败: {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = config_path();
    std::fs::write(&path, serde_json::to_string_pretty(m)?)
        .with_context(|| format!("写入配置失败: {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 键白名单校验
fn validate_key(key: &str) -> Result<()> {
    match key {
        "download-dir" | "play.download-key" | "play.like-key" => Ok(()),
        _ => {
            if let Some(sec) = key.strip_prefix("profile.") {
                if SECTIONS.contains(&sec) {
                    return Ok(());
                }
            }
            bail!(
                "未知配置键 {key}；可用: download-dir, play.download-key, play.like-key, profile.<{}>",
                SECTIONS.join("|")
            )
        }
    }
}

/// 写入配置（拒绝空值；按键拒绝含空白）
pub fn set(key: &str, value: &str) -> Result<()> {
    validate_key(key)?;
    let v = value.trim();
    if v.is_empty() {
        bail!("配置值不能为空");
    }
    if key == "play.download-key" && v.chars().any(char::is_whitespace) {
        bail!("按键名不能含空白");
    }
    if key == "play.like-key" && v.chars().any(char::is_whitespace) {
        bail!("按键名不能含空白");
    }
    let mut m = load_all();
    m.insert(key.to_string(), Value::String(v.to_string()));
    save(&m)
}

/// 读单键（键非法直接报错）
pub fn get(key: &str) -> Result<Option<String>> {
    validate_key(key)?;
    Ok(load_all()
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// 删除单键；返回是否确实存在
pub fn unset(key: &str) -> Result<bool> {
    validate_key(key)?;
    let mut m = load_all();
    let had = m.remove(key).is_some();
    if had {
        save(&m)?;
    }
    Ok(had)
}

/// 列出全部已配置项（按键名排序）
pub fn list() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = load_all()
        .into_iter()
        .filter_map(|(k, val)| val.as_str().map(|s| (k, s.to_string())))
        .collect();
    v.sort();
    v
}

/// 默认下载目录（未配置返回 None）
pub fn download_dir() -> Option<PathBuf> {
    get("download-dir").ok().flatten().map(PathBuf::from)
}

/// 播放快捷下载键（未配置默认 `d`，播放期由 ncm 键盘接管消费，不与 mpv 键冲突）
pub fn play_download_key() -> String {
    get("play.download-key")
        .ok()
        .flatten()
        .unwrap_or_else(|| "d".to_string())
}

/// 播放快捷收藏键（未配置默认 `l`，播放期由 ncm 键盘接管消费，不与 mpv 键冲突）
pub fn play_like_key() -> String {
    get("play.like-key")
        .ok()
        .flatten()
        .unwrap_or_else(|| "l".to_string())
}

/// 板块绑定的会话 profile 名（未绑定返回 None → 回退默认凭据）
pub fn profile_for(section: &str) -> Option<String> {
    get(&format!("profile.{section}")).ok().flatten()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 持久化测试共用进程级锁：各测试都改 XDG_CONFIG_HOME，并行会互踩
    pub(crate) static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 各测试用独立目录（同进程内不共用）
    fn fresh(name: &str) {
        let dir = std::env::temp_dir().join(format!("ncm-conf-{name}-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_file(config_path());
    }

    #[test]
    fn set_get_unset_roundtrip() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        fresh("roundtrip");
        assert_eq!(get("download-dir").unwrap(), None);
        set("download-dir", "/tmp/music").unwrap();
        assert_eq!(get("download-dir").unwrap().as_deref(), Some("/tmp/music"));
        assert!(unset("download-dir").unwrap());
        assert!(!unset("download-dir").unwrap());
    }

    #[test]
    fn rejects_unknown_key_and_empty_value() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        fresh("reject");
        assert!(set("bogus-key", "x").is_err());
        assert!(set("profile.bogus", "x").is_err());
        assert!(set("download-dir", "  ").is_err());
        set("play.download-key", "x").unwrap();
        assert!(set("play.download-key", "a b").is_err());
        set("play.like-key", "x").unwrap();
        assert!(set("play.like-key", "a b").is_err());
    }

    #[test]
    fn defaults() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        fresh("defaults");
        assert_eq!(play_download_key(), "d");
        assert!(download_dir().is_none());
        assert!(profile_for("download").is_none());
        set("profile.download", "vip").unwrap();
        assert_eq!(profile_for("download").as_deref(), Some("vip"));
        let mut all = list();
        assert_eq!(all.len(), 1);
        assert_eq!(all.remove(0).0, "profile.download");
    }
}
