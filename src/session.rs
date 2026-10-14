//! pyncm 兼容的登录凭据解析（session dump / cookie 字符串）
//!
//! 支持三种输入（优先级从高到低）：
//! 1. `--session <file>` / `NCM_SESSION`：pyncm session dump
//!    - 新格式：`PYNCM` 魔数 + base64(zlib(json))，json 内 `cookies[]` 含 MUSIC_U/__csrf
//!    - legacy：大写 hex → AES-128-ECB(key `e82ckenh8dichen8`, PKCS7) →
//!      `pyncm-36cd479b6b5-{json}-36cd479b6b5-{md5}`，只解密不校验 md5
//! 2. `--cookie "MUSIC_U=xxx; __csrf=yyy"` / `NCM_COOKIE`：浏览器 F12 直接粘贴
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
    // --user 显式指定 profile
    if let Some(prof) = user {
        return load_profile(Some(prof)).map(Some).ok_or_else(|| {
            anyhow::anyhow!("profile `{prof}` 不存在或凭据无效，先 ncm login --profile {prof}")
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
/// （cookie.{name} 会拼进文件路径，放行 `/`、`\`、`..` 等可路径穿越出配置目录）
fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 某 profile 的凭据文件路径；`None`/`default` 即默认 profile（兼容旧 `cookie` 文件）
pub fn cookie_path(profile: Option<&str>) -> std::path::PathBuf {
    match profile {
        None | Some("default") => saved_cookie_path(),
        Some(name) => config_dir().join(format!("cookie.{name}")),
    }
}

/// 读取指定 profile 凭据（不存在、损坏或非法名返回 None）
pub fn load_profile(profile: Option<&str>) -> Option<String> {
    if let Some(name) = profile {
        if name != "default" && !valid_profile_name(name) {
            return None;
        }
    }
    let txt = std::fs::read_to_string(cookie_path(profile)).ok()?;
    let t = txt.trim();
    if t.contains('=') {
        Some(t.to_string())
    } else {
        None
    }
}

/// 写入指定 profile 凭据（目录 0700、文件 0600）
pub fn save_profile(profile: Option<&str>, cookie: &str) -> Result<std::path::PathBuf> {
    let c = cookie.trim();
    if !c.contains('=') {
        bail!("cookie 格式应为 \"MUSIC_U=xxx; __csrf=yyy\"");
    }
    if let Some(name) = profile {
        if name != "default" && !valid_profile_name(name) {
            bail!("非法 profile 名 \"{name}\"（仅限字母/数字/._-，不能含路径分隔符）");
        }
    }
    let dir = config_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建配置目录失败: {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = cookie_path(profile);
    // 新建文件直接以 0600 创建（避免"先写后 chmod"期间以默认权限裸奔）；
    // 已存在的旧文件再补一次 chmod 覆盖历史权限
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let payload = format!("{c}\n");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("写入凭据失败: {}", path.display()))?;
        std::io::Write::write_all(&mut f, payload.as_bytes())
            .with_context(|| format!("写入凭据失败: {}", path.display()))?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    std::fs::write(&path, format!("{c}\n"))
        .with_context(|| format!("写入凭据失败: {}", path.display()))?;
    Ok(path)
}

/// 删除指定 profile 凭据；返回是否确实存在
pub fn clear_profile(profile: Option<&str>) -> Result<bool> {
    let path = cookie_path(profile);
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("删除凭据失败: {}", path.display()))?;
        return Ok(true);
    }
    Ok(false)
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

/// 列举全部已保存 profile 名（default 表示默认 cookie 文件）；按名排序
pub fn list_profiles() -> Vec<String> {
    let mut out = Vec::new();
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
        assert!(save_profile(None, "MUSIC_U=abc; __csrf=t").is_ok());
        assert_eq!(load_profile(None).as_deref(), Some("MUSIC_U=abc; __csrf=t"));
        assert!(save_profile(None, "bad").is_err());
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
                save_profile(Some(bad), "MUSIC_U=x; y=1").is_err(),
                "应拒绝 {bad:?}"
            );
            assert!(load_profile(Some(bad)).is_none(), "{bad:?} 不应读到内容");
        }
        assert!(valid_profile_name("vip-2_a.b"));
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
        assert!(save_profile(None, "MUSIC_U=main; x=1").is_ok());
        assert!(save_profile(Some("vip"), "MUSIC_U=vip2; y=2").is_ok());
        assert!(save_profile(Some("default"), "MUSIC_U=alias; z=3").is_ok());
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
