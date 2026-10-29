//! pyncm 兼容的登录凭据解析（session dump / cookie 字符串）
//!
//! 支持三种输入（优先级从高到低）：
//! 1. `--session <file>` / `NCM_SESSION`：pyncm session dump
//!    - 新格式：`PYNCM` 魔数 + base64(zlib(json))，json 内 `cookies[]` 含 MUSIC_U/__csrf
//!    - legacy：大写 hex → AES-128-ECB(key `e82ckenh8dichen8`, PKCS7) →
//!      `pyncm-36cd479b6b5-{json}-36cd479b6b5-{md5}`，只解密不校验 md5
//! 2. `--cookie "MUSIC_U=xxx; __csrf=yyy"` / `NCM_COOKIE`：浏览器 F12 直接粘贴
//!
//! 持久化：`~/.config/ncm/sessions.json`（0600，原子写）按 profile 存
//! `{cookie, uid, name, updated}`——登录 whoami 成功后连同 uid/用户名一并写入；
//! `--profile` 支持档名 / uid / 用户名三种写法反查（歧义不猜）。
//! 旧版纯文本 `cookie`/`cookie.<名>` 读到即自动迁入 sessions.json 并删除原文件。
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

const PYNCM_MAGIC: &[u8] = b"PYNCM";
const EAPI_KEY: &[u8; 16] = b"e82ckenh8dichen8";
const LEGACY_MARK: &str = "36cd479b6b5";

/// 解析登录凭据，返回可直接放进 Cookie 头的字符串（不含 os=pc 等固定项）
///
/// 优先级：`--session` > `--cookie` > `NCM_SESSION`/`NCM_COOKIE`（调用方已合并到参数）>
/// `--user <profile>` 显式指定 > 板块绑定的 profile 凭据（`ncm config set profile.<板块> <名>`）>
/// 默认持久化文件
pub fn resolve_auth_for(
    section: &str,
    session: Option<&Path>,
    cookie: Option<&str>,
    user: Option<&str>,
) -> Result<Option<String>> {
    if let Some(path) = session {
        let raw = std::fs::read(path)
            .with_context(|| format!("读取 session 文件失败: {}", path.display()))?;
        let cookie = parse_session(&raw)
            .with_context(|| format!("解析 session 文件失败: {}", path.display()))?;
        return Ok(Some(cookie));
    }
    if let Some(c) = cookie {
        let c = c.trim();
        if !c.contains('=') {
            bail!("cookie 格式应为 \"MUSIC_U=xxx; __csrf=yyy\"");
        }
        return Ok(Some(c.to_string()));
    }
    // --user 显式指定 profile（支持档名 / uid / 用户名任一）
    if let Some(prof) = user {
        return load_profile(Some(prof)).map(Some).ok_or_else(|| {
            anyhow::anyhow!(
                "profile `{prof}` 不存在或凭据无效（可用档名/uid/用户名），\
                 先 ncm login --profile {prof} --cookie/--session"
            )
        });
    }
    // 板块 → profile 映射（如 download 绑 vip）→ 该 profile 凭据
    if let Some(prof) = crate::config::profile_for(section) {
        if let Some(c) = load_profile(Some(&prof)) {
            return Ok(Some(c));
        }
        // 绑定的 profile 没有凭据：显式报错，不能静默回退默认账号
        // （否则会用错账号继续跑，排查成本远高于失败本身）
        bail!(
            "板块 `{section}` 绑定的 profile `{prof}` 无凭据（profile.{section}={prof}），\
             先 `ncm login --profile {prof}`，或 ncm config set profile.{section} 换绑"
        );
    }
    // 回退：ncm login 持久化的默认凭据
    Ok(load_saved())
}

/// profile 名合法性：仅 [A-Za-z0-9._-] 且非 `.`/`..`、非空、≤64 字符
/// （新档名会与已有存档 key 比对，放行 `/`、`\`、`..` 等会污染存档语义）
fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 登录凭据存档路径（`sessions.json`，与 `config.json` 同目录同格式风格：
/// serde_json + 原子写 + 0600）
pub fn sessions_path() -> std::path::PathBuf {
    config_dir().join("sessions.json")
}

/// 读整个存档（缺/坏 → 空骨架）；原子写保证正常不损坏
fn load_sessions() -> Value {
    std::fs::read_to_string(sessions_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({ "version": 1, "profiles": {} }))
}

/// 原子落盘（临时文件 + rename，目录 0700、文件 0600，同 config.json 策略）
fn save_sessions(v: &Value) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建配置目录失败: {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = sessions_path();
    let mut os = path.clone().into_os_string();
    os.push(".tmp");
    let tmp = std::path::PathBuf::from(os);
    let payload = serde_json::to_string_pretty(v)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("写入登录存档失败: {}", tmp.display()))?;
        std::io::Write::write_all(&mut f, payload.as_bytes())
            .with_context(|| format!("写入登录存档失败: {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("写入登录存档失败: {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| {
            let _ = std::fs::remove_file(&tmp);
            format!("写入登录存档失败: {}", path.display())
        })?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, payload)
            .with_context(|| format!("写入登录存档失败: {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| {
            let _ = std::fs::remove_file(&tmp);
            format!("写入登录存档失败: {}", path.display())
        })?;
    }
    Ok(())
}

/// 规范化 profile spec → 存档 key。
/// 顺序：① 已存档同名 profile 直接命中；② 按 uid 或用户名唯一匹配（`--profile
/// 13437323299` / `--profile zZ丶KE` 也能选中，即创建时写入的 ID/用户名反查）；
/// ③ 合法新名原样返回（新档或旧纯文本 cookie 文件迁移）；④ 非法且无匹配 → None
/// （歧义匹配不猜账号，同 ④ 处理）
fn canonical_profile(spec: &str, sessions: &Value) -> Option<String> {
    if spec.is_empty() {
        return None;
    }
    let obj = sessions.get("profiles")?.as_object()?;
    if obj.contains_key(spec) {
        return Some(spec.to_string());
    }
    let mut hit: Option<&String> = None;
    for (k, v) in obj {
        let uid_match = v
            .get("uid")
            .and_then(Value::as_u64)
            .is_some_and(|u| u.to_string() == spec);
        let name_match = v.get("name").and_then(Value::as_str) == Some(spec);
        if uid_match || name_match {
            if hit.is_some_and(|h| h != k) {
                return None;
            }
            hit = Some(k);
        }
    }
    if let Some(k) = hit {
        return Some(k.to_string());
    }
    valid_profile_name(spec).then(|| spec.to_string())
}

/// 写入/覆盖一条 profile 档（cookie + 可选 uid/用户名元数据 + 更新时间）
fn set_entry(
    sessions: &mut Value,
    name: &str,
    cookie: &str,
    uid: Option<u64>,
    user_name: Option<&str>,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let obj = sessions
        .as_object_mut()
        .context("登录存档结构损坏（非对象）")?;
    let profiles = obj
        .entry("profiles")
        .or_insert_with(|| serde_json::json!({}));
    let p = profiles
        .as_object_mut()
        .context("登录存档 profiles 损坏（非对象）")?;
    let mut entry = serde_json::json!({
        "cookie": cookie,
        "uid": serde_json::Value::Null,
        "name": serde_json::Value::Null,
        "updated": now,
    });
    if let Some(u) = uid {
        entry["uid"] = serde_json::json!(u);
    }
    if let Some(n) = user_name {
        entry["name"] = serde_json::json!(n);
    }
    p.insert(name.to_string(), entry);
    Ok(())
}

/// 旧纯文本凭据路径（`cookie` / `cookie.<名>`，仅作自动迁移来源）
fn legacy_cookie_path(name: &str) -> std::path::PathBuf {
    if name == "default" {
        saved_cookie_path()
    } else {
        config_dir().join(format!("cookie.{name}"))
    }
}

/// 读指定 profile 凭据。
/// 先按 uid/用户名/档名规范化（`--profile`/`--user` 支持三者任一）；
/// 读 `sessions.json`，无该档则探测旧纯文本 `cookie[.<名>]` 并**自动迁移**
/// （写入 sessions.json 成功后删除旧文件，单一真源；迁移失败仍返回可用 cookie）
pub fn load_profile(profile: Option<&str>) -> Option<String> {
    let spec = profile.unwrap_or("default");
    let mut sessions = load_sessions();
    let name = canonical_profile(spec, &sessions)?;
    if let Some(c) = sessions["profiles"][name.as_str()]["cookie"].as_str() {
        let c = c.trim();
        if c.contains('=') {
            return Some(c.to_string());
        }
    }
    // 旧纯文本迁移（uid/用户名未知，留空待下次 ncm login 补全）
    let legacy = legacy_cookie_path(&name);
    let txt = std::fs::read_to_string(&legacy).ok()?;
    let c = txt.trim().to_string();
    if !c.contains('=') {
        return None;
    }
    if set_entry(&mut sessions, &name, &c, None, None).is_ok() && save_sessions(&sessions).is_ok() {
        let _ = std::fs::remove_file(&legacy);
    }
    Some(c)
}

/// 写入指定 profile 凭据（`sessions.json` 原子写 0600，与 config.json 对齐）。
/// `uid`/`user_name` 为登录时 whoami 查到的元数据，写入后 `--profile` 可按
/// 用户名或 ID 反查选中该档；同时清掉该档遗留的旧纯文本 cookie 文件
pub fn save_profile(
    profile: Option<&str>,
    cookie: &str,
    uid: Option<u64>,
    user_name: Option<&str>,
) -> Result<std::path::PathBuf> {
    let c = cookie.trim();
    if !c.contains('=') {
        bail!("cookie 格式应为 \"MUSIC_U=xxx; __csrf=yyy\"");
    }
    let spec = profile.unwrap_or("default");
    let mut sessions = load_sessions();
    // 新档必须是合法名（防路径穿越/怪名污染存档）；uid/用户名匹配可命中已存档
    let name = canonical_profile(spec, &sessions).with_context(|| {
        format!("非法 profile 名 \"{spec}\"（仅限字母/数字/._-），且无已存档的 uid/用户名匹配")
    })?;
    set_entry(&mut sessions, &name, c, uid, user_name)?;
    save_sessions(&sessions)?;
    // 该档旧纯文本一并清掉：避免旧 cookie 残留被后续读回造成双源不一致
    let _ = std::fs::remove_file(legacy_cookie_path(&name));
    Ok(sessions_path())
}

/// 删除指定 profile 凭据（sessions.json 条目 + 遗留旧纯文本）；返回是否确实存在
pub fn clear_profile(profile: Option<&str>) -> Result<bool> {
    let spec = profile.unwrap_or("default");
    let mut sessions = load_sessions();
    let name = canonical_profile(spec, &sessions).unwrap_or_else(|| spec.to_string());
    let mut had = false;
    let removed = sessions
        .get_mut("profiles")
        .and_then(Value::as_object_mut)
        .map(|p| p.remove(&name).is_some())
        .unwrap_or(false);
    if removed {
        had = true;
        save_sessions(&sessions)?;
    }
    let legacy = legacy_cookie_path(&name);
    if legacy.exists() {
        std::fs::remove_file(&legacy)
            .with_context(|| format!("删除凭据失败: {}", legacy.display()))?;
        had = true;
    }
    Ok(had)
}

/// 配置目录：`$XDG_CONFIG_HOME/ncm` 或 `~/.config/ncm`
pub fn config_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
        if !d.is_empty() {
            return std::path::PathBuf::from(d).join("ncm");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".config").join("ncm")
}

/// 持久化凭据文件路径
pub fn saved_cookie_path() -> std::path::PathBuf {
    config_dir().join("cookie")
}

/// 读取默认 profile 凭据（不存在或损坏返回 None）
pub fn load_saved() -> Option<String> {
    load_profile(None)
}

/// 列举全部已保存 profile 名（default 表示默认档）；按名排序。
/// 同时覆盖 sessions.json 档案与未迁移的旧纯文本（枚举即触发迁移）
pub fn list_profiles() -> Vec<String> {
    let sessions = load_sessions();
    let mut out: Vec<String> = sessions
        .get("profiles")
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    if load_saved().is_some() {
        out.push("default".to_string());
    }
    if let Ok(rd) = std::fs::read_dir(config_dir()) {
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            if let Some(name) = fname.strip_prefix("cookie.") {
                if !name.is_empty() && load_profile(Some(name)).is_some() {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 解析 session dump 字节流 → cookie 字符串
pub fn parse_session(data: &[u8]) -> Result<String> {
    if data.starts_with(PYNCM_MAGIC) {
        return parse_modern(&data[PYNCM_MAGIC.len()..]);
    }
    parse_legacy(data)
}

/// 新格式：base64(zlib(json))
fn parse_modern(payload: &[u8]) -> Result<String> {
    let b64 = std::str::from_utf8(payload).context("PYNCM 负载不是合法文本")?;
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .context("base64 解码失败")?;
    let mut json_bytes = Vec::with_capacity(compressed.len() * 4);
    flate2::read::ZlibDecoder::new(&compressed[..])
        .read_to_end(&mut json_bytes)
        .context("zlib 解压失败")?;
    let value: Value = serde_json::from_slice(&json_bytes).context("session JSON 解析失败")?;
    cookies_to_header(&value)
}

/// legacy 格式：hex → AES-128-ECB → `pyncm-{mark}-{json}-{mark}-{md5}`
fn parse_legacy(data: &[u8]) -> Result<String> {
    let hex = std::str::from_utf8(data)
        .context("legacy session 不是合法 UTF-8")?
        .trim();
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) || hex.len() % 32 != 0 || hex.is_empty() {
        bail!("无法识别的 session 格式（需要 PYNCM 魔数或 legacy hex）");
    }
    let cipher_bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<_, _>>()
        .context("hex 解码失败")?;

    use aes::cipher::{BlockDecrypt, KeyInit};
    let cipher = aes::Aes128::new(EAPI_KEY.into());
    let mut plain = Vec::with_capacity(cipher_bytes.len());
    for block in cipher_bytes.chunks(16) {
        if block.len() != 16 {
            bail!("legacy 密文长度不是 16 的倍数");
        }
        let mut blk = aes::cipher::generic_array::GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut blk);
        plain.extend_from_slice(blk.as_slice());
    }
    // 去 PKCS7 填充
    let pad = *plain.last().context("解密结果为空")? as usize;
    if pad == 0 || pad > 16 || plain.len() < pad {
        bail!("PKCS7 填充无效");
    }
    plain.truncate(plain.len() - pad);
    let text = String::from_utf8(plain).context("解密结果不是 UTF-8")?;

    // 提取 pyncm-{mark}-{json}-{mark}-{md5} 中的 json
    let marker = format!("-{LEGACY_MARK}-");
    let start = text
        .find(&marker)
        .map(|i| i + marker.len())
        .context("明文缺少 pyncm 标记")?;
    let json_str = &text[start..];
    let end = json_str.find(&marker).context("明文缺少 pyncm 结束标记")?;
    let value: Value = serde_json::from_slice(&json_str.as_bytes()[..end])
        .context("legacy session JSON 解析失败")?;
    cookies_to_header(&value)
}

/// session JSON → cookie 头。兼容 `cookies: [{name,value},...]` 与 `{名: 值}` 字典
fn cookies_to_header(value: &Value) -> Result<String> {
    let mut pairs: Vec<String> = Vec::new();
    if let Some(arr) = value["cookies"].as_array() {
        for c in arr {
            if let (Some(n), Some(v)) = (c["name"].as_str(), c["value"].as_str()) {
                pairs.push(format!("{n}={v}"));
            }
        }
    } else if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            if k == "cookies" {
                continue;
            }
            if let Some(s) = v.as_str() {
                pairs.push(format!("{k}={s}"));
            }
        }
    }
    if pairs.is_empty() {
        bail!("session 中没有可用的 cookie");
    }
    Ok(pairs.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_HEX: &str = "14802A8DCECD7B925B20A546D635E05937F0751884E9161B6FC030C323CE2990D6860B82E4B68EE18D17F0994E7F63D00AD95BA9133C108B3891FA2510A9BE1D8ADB06F3BC20BBB454421065092409C6B2808C9A11EDED250A382CC2E0CE1D5AF75A270FE9131C61451372DADDED7AEA05ABE0FA365F10864614CB9C579732C1";

    #[test]
    fn modern_pyncm_roundtrip() {
        let json =
            r#"{"cookies":[{"name":"MUSIC_U","value":"abc123"},{"name":"__csrf","value":"tok"}]}"#;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, json.as_bytes()).unwrap();
        let compressed = enc.finish().unwrap();
        let mut data = PYNCM_MAGIC.to_vec();
        data.extend_from_slice(
            base64::engine::general_purpose::STANDARD
                .encode(compressed)
                .as_bytes(),
        );
        let cookie = parse_session(&data).unwrap();
        assert_eq!(cookie, "MUSIC_U=abc123; __csrf=tok");
    }

    #[test]
    fn legacy_hex_roundtrip() {
        let cookie = parse_session(LEGACY_HEX.as_bytes()).unwrap();
        assert_eq!(cookie, "MUSIC_U=abc123; __csrf=tok");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_session(b"not-a-session").is_err());
    }
}

#[cfg(test)]
mod persist_tests {
    use super::*;

    #[test]
    fn saved_cookie_roundtrip() {
        // 与 config 测试共锁：都会临时改 XDG_CONFIG_HOME
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("ncm-sess-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = clear_profile(None);
        assert!(load_profile(None).is_none());
        assert!(save_profile(None, "MUSIC_U=abc; __csrf=t", None, None).is_ok());
        assert_eq!(load_profile(None).as_deref(), Some("MUSIC_U=abc; __csrf=t"));
        assert!(save_profile(None, "bad", None, None).is_err());
        assert!(clear_profile(None).unwrap());
        assert!(!clear_profile(None).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非法 profile 名拒绝写入/读取（路径穿越防护）
    #[test]
    fn rejects_path_traversal_profile_names() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("ncm-trav-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        for bad in ["../evil", "a/b", "a\\b", "..", ".", ""] {
            assert!(
                save_profile(Some(bad), "MUSIC_U=x; y=1", None, None).is_err(),
                "应拒绝 {bad:?}"
            );
            assert!(load_profile(Some(bad)).is_none(), "{bad:?} 不应读到内容");
        }
        assert!(valid_profile_name("vip-2_a.b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧纯文本 cookie 文件自动迁移到 sessions.json（转换后删旧文件，单一真源）
    #[test]
    fn legacy_cookie_auto_migration() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("ncm-migr-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = clear_profile(Some("old1"));
        let _ = clear_profile(None);
        // 手工造旧格式：cookie.old1 纯文本
        std::fs::create_dir_all(dir.join("ncm")).unwrap();
        std::fs::write(dir.join("ncm/cookie.old1"), "MUSIC_U=legacy; __csrf=z\n").unwrap();
        // 读取即迁移：得凭据、sessions.json 有档、旧文件已删
        assert_eq!(
            load_profile(Some("old1")).as_deref(),
            Some("MUSIC_U=legacy; __csrf=z")
        );
        let sessions = load_sessions();
        assert_eq!(
            sessions["profiles"]["old1"]["cookie"].as_str(),
            Some("MUSIC_U=legacy; __csrf=z")
        );
        assert!(!dir.join("ncm/cookie.old1").exists());
        // 迁移后再存（带元数据）不回退
        assert!(save_profile(Some("old1"), "MUSIC_U=new; k=1", Some(42), Some("旧档号")).is_ok());
        assert_eq!(
            load_profile(Some("old1")).as_deref(),
            Some("MUSIC_U=new; k=1")
        );
        let _ = clear_profile(Some("old1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// --profile 支持按 uid / 用户名反查已存档 profile
    #[test]
    fn resolve_profile_by_uid_and_name() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("ncm-uid-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = clear_profile(None);
        let _ = clear_profile(Some("acc"));
        assert!(save_profile(
            Some("acc"),
            "MUSIC_U=u1; t=1",
            Some(13437323299),
            Some("测试号")
        )
        .is_ok());
        // 按档名 / uid / 用户名三种方式都能读到同一凭据
        for spec in ["acc", "13437323299", "测试号"] {
            assert_eq!(
                load_profile(Some(spec)).as_deref(),
                Some("MUSIC_U=u1; t=1"),
                "spec={spec} 应命中 acc 档"
            );
        }
        // 不存在且无匹配的非法名 → None
        assert!(load_profile(Some("中文不存在")).is_none());
        let _ = clear_profile(Some("acc"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_paths_isolated() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("ncm-prof-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = clear_profile(None);
        let _ = clear_profile(Some("vip"));
        // 多 profile 互不覆盖；default 仍走旧 cookie 文件路径
        assert!(save_profile(None, "MUSIC_U=main; x=1", None, None).is_ok());
        assert!(save_profile(Some("vip"), "MUSIC_U=vip2; y=2", None, None).is_ok());
        assert!(save_profile(Some("default"), "MUSIC_U=alias; z=3", None, None).is_ok());
        assert_eq!(load_profile(None).as_deref(), Some("MUSIC_U=alias; z=3"));
        assert_eq!(
            load_profile(Some("vip")).as_deref(),
            Some("MUSIC_U=vip2; y=2")
        );
        // profile 回退 resolve：CLI/env 缺省时按板块映射取 cookie
        crate::config::set("profile.download", "vip").unwrap();
        let auth = resolve_auth_for("download", None, None, None).unwrap();
        assert_eq!(auth.as_deref(), Some("MUSIC_U=vip2; y=2"));
        let auth = resolve_auth_for("play", None, None, None).unwrap();
        assert_eq!(auth.as_deref(), Some("MUSIC_U=alias; z=3"));
        // CLI 显式参数优先于板块映射
        let auth = resolve_auth_for("download", None, Some("MUSIC_U=cli; a=1"), None).unwrap();
        assert_eq!(auth.as_deref(), Some("MUSIC_U=cli; a=1"));
        // --user 显式指定优先于板块绑定
        let auth = resolve_auth_for("download", None, None, Some("vip")).unwrap();
        assert_eq!(auth.as_deref(), Some("MUSIC_U=vip2; y=2"));
        // list_profiles 枚举
        let mut profs = list_profiles();
        profs.sort();
        assert!(profs.contains(&"default".to_string()));
        assert!(profs.contains(&"vip".to_string()));
        let _ = clear_profile(None);
        let _ = clear_profile(Some("vip"));
        let _ = crate::config::unset("profile.download");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
