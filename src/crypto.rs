//! weapi/eapi 加密（对齐 pyncm 1.8.1 实测可用参数）
//!
//! weapi：双重 AES-128-CBC + 教科书 RSA（无填充）；用于歌单曲目增删、改名
//! eapi：单次 AES-128-ECB + md5 摘要；用于新建/删除歌单
//! AES 块加密用 aes crate，CBC/ECB 链式层手写；secKey 优先 /dev/urandom，失败退时间熵
use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use num_bigint::BigInt;
use num_bigint::Sign;

const WEAPI_AES_KEY: &[u8; 16] = b"0CoJUm6Qyw8W8jud";
const WEAPI_AES_IV: &[u8; 16] = b"0102030405060708";
const EAPI_AES_KEY: &[u8; 16] = b"e82ckenh8dichen8";
/// 教科书 RSA 公钥（网易固定）
const WEAPI_RSA_N: &str = "00e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7";
const WEAPI_RSA_E: u32 = 0x10001;

/// 时间熵 xorshift64（secKey/requestId 够用，非密码学强度）
fn next_rand(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn time_seed() -> u64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_nanos() as u64) ^ ((std::process::id() as u64) << 32)
}

/// PKCS7 填充到 16 字节倍数
fn pkcs7_pad(data: &[u8]) -> Vec<u8> {
    let pad = 16 - data.len() % 16;
    let mut out = Vec::with_capacity(data.len() + pad);
    out.extend_from_slice(data);
    out.resize(data.len() + pad, pad as u8);
    out
}

/// AES-128-CBC + PKCS7，返回密文字节
fn aes_cbc_encrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let buf = pkcs7_pad(data);
    let mut prev = *iv;
    let mut out = Vec::with_capacity(buf.len());
    for block in buf.as_chunks::<16>().0 {
        let mut x = [0u8; 16];
        for i in 0..16 {
            x[i] = block[i] ^ prev[i];
        }
        let mut ga = GenericArray::clone_from_slice(&x);
        cipher.encrypt_block(&mut ga);
        out.extend_from_slice(&ga);
        prev.copy_from_slice(&ga);
    }
    out
}

/// AES-128-ECB + PKCS7，返回密文字节
fn aes_ecb_encrypt(data: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let buf = pkcs7_pad(data);
    let mut out = Vec::with_capacity(buf.len());
    for block in buf.as_chunks::<16>().0 {
        let mut ga = GenericArray::clone_from_slice(block);
        cipher.encrypt_block(&mut ga);
        out.extend_from_slice(&ga);
    }
    out
}

/// eapi 响应解密（AES-128-ECB + PKCS7）；非密文返回 None。
/// pyncm 注释 "hack: data isn't padded"——去填充失败时回退按原始明文返回
pub fn eapi_decrypt(data: &[u8]) -> Option<String> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return None;
    }
    let cipher = Aes128::new(EAPI_AES_KEY.into());
    let mut out = Vec::with_capacity(data.len());
    for block in data.as_chunks::<16>().0 {
        let mut ga = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut ga);
        out.extend_from_slice(&ga);
    }
    // PKCS7 去填充；校验失败视为未填充数据，直接用原文（对齐 pyncm hack）
    let pad = *out.last().unwrap_or(&0) as usize;
    if (1..=16).contains(&pad)
        && pad <= out.len()
        && out[out.len() - pad..].iter().all(|&b| b == pad as u8)
    {
        out.truncate(out.len() - pad);
    }
    String::from_utf8(out).ok()
}

/// 16 位随机 secKey（字母数字）；getrandom OS 熵，失败退时间熵 xorshift
fn random_sec_key() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut seed = {
        let mut buf = [0u8; 16];
        match getrandom::getrandom(&mut buf) {
            Ok(()) => {
                u64::from_le_bytes(buf[0..8].try_into().expect("8 字节"))
                    ^ u64::from_le_bytes(buf[8..16].try_into().expect("8 字节")).rotate_left(32)
            }
            Err(_) => time_seed(),
        }
    };
    (0..16)
        .map(|_| CHARS[(next_rand(&mut seed) % CHARS.len() as u64) as usize] as char)
        .collect()
}

/// 教科书 RSA：反转 secKey → UTF-8 字节当大整数 → m^e mod n → 256 位 hex
fn rsa_encrypt(sec_key: &str) -> String {
    let rev: String = sec_key.chars().rev().collect();
    let m = BigInt::from_bytes_be(Sign::Plus, rev.as_bytes());
    let n = BigInt::parse_bytes(WEAPI_RSA_N.as_bytes(), 16).expect("硬编码 RSA 模数合法");
    let r = m.modpow(&BigInt::from(WEAPI_RSA_E), &n);
    format!("{r:0>256x}")
}

/// weapi 加密；payload_json 为含 csrf_token 的完整参数 JSON
/// 返回 (params, encSecKey)，POST form 字段名同名
pub fn weapi_encrypt(payload_json: &str) -> (String, String) {
    let sec_key = random_sec_key();
    let p1 = B64.encode(aes_cbc_encrypt(
        payload_json.as_bytes(),
        WEAPI_AES_KEY,
        WEAPI_AES_IV,
    ));
    let params = B64.encode(aes_cbc_encrypt(
        p1.as_bytes(),
        sec_key.as_bytes().try_into().expect("secKey 恰 16 字节"),
        WEAPI_AES_IV,
    ));
    (params, rsa_encrypt(&sec_key))
}

/// eapi 加密；api_path 为 /api/... 形式（分隔串里用），payload_json 为完整参数（含 header 字段）
/// 返回 hex 密文，POST 字段名 params
pub fn eapi_encrypt(api_path: &str, payload_json: &str) -> String {
    let digest = md5::compute(format!("nobody{api_path}use{payload_json}md5forencrypt"));
    let plain = format!("{api_path}-36cd479b6b5-{payload_json}-36cd479b6b5-{digest:x}");
    aes_ecb_encrypt(plain.as_bytes(), EAPI_AES_KEY)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// eapi 内嵌 header（客户端指纹），requestId 为随机 8 位数
pub fn eapi_header_json() -> String {
    let mut seed = time_seed().wrapping_add(0x9e37_79b9);
    let request_id = 20_000_000 + (next_rand(&mut seed) % 10_000_000);
    format!(
        r#"{{"os":"iPhone OS","appver":"10.0.0","osver":"16.2","channel":"distribution","deviceId":"pyncm!","requestId":"{request_id}"}}"#
    )
}
