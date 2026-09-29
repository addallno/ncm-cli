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
pub fn resolve_auth(session: Option<&Path>, cookie: Option<&str>) -> Result<Option<String>> {
    // 优先级：session > cookie（与 PLAN 一致）
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
    Ok(None)
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
        plain.extend_from_slice(&blk.as_slice());
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
    let value: Value = serde_json::from_slice(json_str[..end].as_bytes())
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
    use base64::Engine as _;

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
