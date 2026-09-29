//! 下载、探测与完整性校验（语义对齐 Python 版实测通过的防线）
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api::{Ncm, BASE, UA};

/// Range 探测：状态码 / Content-Type / 总长（等价 musicdl 的 AudioLinkTester）
pub async fn probe_link(ncm: &Ncm, url: &str) -> Result<(u64, String), String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RANGE, "bytes=0-1".parse().unwrap());
    let resp = match ncm
        .insecure
        .get(url)
        .headers(headers)
        .timeout(Duration::from_secs(15))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Err(format!("探测失败: {e}")),
    };
    let status = resp.status().as_u16();
    if status != 200 && status != 206 {
        return Err(format!("HTTP {status}"));
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if ctype.starts_with("text/") || ctype == "application/json" || ctype == "application/xhtml+xml"
    {
        return Err(format!("非音频类型 {ctype}"));
    }
    let mut total = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if let Some(cr) = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
    {
        if let Some((_, tail)) = cr.rsplit_once('/') {
            if let Ok(n) = tail.parse::<u64>() {
                total = n;
            }
        }
    }
    // 读 1 字节确认真的有数据
    let mut stream = resp.bytes_stream();
    match futures_util::StreamExt::next(&mut stream).await {
        Some(Ok(chunk)) if !chunk.is_empty() => {}
        _ => return Err("无法读取内容".into()),
    }
    if total == 0 {
        return Err("无法获取文件长度".into());
    }
    Ok((total, ctype))
}

/// 文件头嗅探
pub fn sniff_head(path: &Path) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    match std::fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            match f.read(&mut buf) {
                Ok(n) => {
                    buf.truncate(n);
                    buf
                }
                Err(_) => Vec::new(),
            }
        }
        Err(_) => Vec::new(),
    }
}

pub fn sniff_flac(path: &Path) -> bool {
    sniff_head(path).starts_with(b"fLaC")
}

pub fn sniff_ext(path: &Path) -> Option<&'static str> {
    let head = sniff_head(path);
    if head.starts_with(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        return Some("mp3");
    }
    if head.starts_with(b"fLaC") {
        return Some("flac");
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some("m4a");
    }
    if head.starts_with(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WAVE" {
        return Some("wav");
    }
    if head.starts_with(b"OggS") {
        return Some("ogg");
    }
    None
}

/// 解析 fLaC STREAMINFO：返回 (总采样数, 声道数, 位深, 采样率)
pub fn flac_stream_info(path: &Path) -> Option<(u64, u32, u32, u32)> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 42 || &data[0..4] != b"fLaC" {
        return None;
    }
    // 块头 4 字节（最后一块 + 类型 + 长度），STREAMINFO 类型必须为 0、长度 34
    let block_type = data[4] & 0x7F;
    if block_type != 0 {
        return None;
    }
    let si = &data[8..42];
    // [10..18] 8 字节：sample_rate(20) | channels-1(3) | bits-1(5) | total_samples(36)
    let packed = u64::from_be_bytes([
        si[10], si[11], si[12], si[13], si[14], si[15], si[16], si[17],
    ]);
    let sample_rate = (packed >> 44) as u32;
    let channels = (((packed >> 41) & 0b111) + 1) as u32;
    let bits = (((packed >> 36) & 0b1_1111) + 1) as u32;
    let total_samples = packed & 0xF_FFFF_FFFF;
    Some((total_samples, channels, bits, sample_rate))
}

/// 音频内容校验（魔数 → 时长 → 体积 → FLAC 下界），返回 Err(原因)
pub fn verify_audio(
    path: &Path,
    _expect_ms: u64,
    expect_br: Option<u64>,
    size: u64,
) -> Result<(), String> {
    if size == 0 {
        return Err("文件为空".into());
    }
    if sniff_head(path).is_empty() {
        return Err("文件头不是音频格式".into());
    }
    let is_flac = sniff_flac(path);

    // FLAC 截断防线：STREAMINFO 在头部可读出完整时长，改用未压缩 PCM 量反推体积下界
    if is_flac {
        if let Some((samples, channels, bits, _)) = flac_stream_info(path) {
            let raw = samples * channels as u64 * bits.div_ceil(8) as u64;
            if raw > 0 && size < (raw as f64 * 0.35) as u64 {
                return Err(format!("FLAC 体积异常（疑似截断）{size}B / 未压缩 {raw}B"));
            }
            return Ok(());
        }
    }

    let _ = expect_br;
    Ok(())
}

/// 带时长交叉校验的版本（调用方传入接口声明时长）
pub fn verify_with_duration(
    path: &Path,
    expect_ms: u64,
    expect_br: Option<u64>,
    size: u64,
    actual_ms: Option<u64>,
) -> Result<(), String> {
    let is_flac = sniff_flac(path);
    if let (Some(actual), true) = (actual_ms, expect_ms > 0) {
        let diff = actual.abs_diff(expect_ms);
        if diff > 5_000 && diff * 100 > expect_ms * 3 {
            return Err(format!(
                "时长不符 实际{:.1}s vs 预期{:.1}s",
                actual as f64 / 1000.0,
                expect_ms as f64 / 1000.0
            ));
        }
    }
    if let (Some(br), true) = (expect_br, !is_flac) {
        if let Some(actual) = actual_ms {
            if actual > 0 && size * 8 < br * (actual / 1000) * 3 / 4 {
                return Err(format!("体积过小 {size}B @ {br}bps"));
            }
        }
    }
    Ok(())
}

/// 下载单曲：探测 → 流式写 .part → 校验 → 落正式文件
/// 返回 (最终路径, 实际时长 ms)
pub async fn download_audio(
    ncm: &Ncm,
    url: &str,
    dest_base: &Path,
    expect_ms: u64,
    expect_br: Option<u64>,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(PathBuf, u64), String> {
    let (total, _) = probe_link(ncm, url).await?;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::REFERER,
        reqwest::header::HeaderValue::from_static(BASE),
    );
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(UA),
    );

    let resp = ncm
        .insecure
        .get(url)
        .headers(headers)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("下载异常: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载异常: {e}"))?;

    let declared = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let total = if total > 0 { total } else { declared };

    let tmp = dest_base.with_extension("part");
    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut done: u64 = 0;
    use std::io::Write;

    let result: Result<(), String> = async {
        while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
            let chunk = chunk.map_err(|e| format!("下载异常: {e}"))?;
            file.write_all(&chunk)
                .map_err(|e| format!("写盘失败: {e}"))?;
            done += chunk.len() as u64;
            on_progress(done, total);
        }
        file.flush().map_err(|e| format!("写盘失败: {e}"))?;
        Ok(())
    }
    .await;
    drop(file);

    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if declared != 0 && done != declared {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("下载不完整 {done}/{declared} 字节"));
    }
    if total != 0 && done != total {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("下载不完整 {done}/{total} 字节"));
    }
    on_progress(done, total);

    // 扩展名以真实文件头为准（第三方源不返回 type）
    let ext = if sniff_flac(&tmp) {
        "flac"
    } else {
        sniff_ext(&tmp).unwrap_or("mp3")
    };
    let is_flac = ext == "flac";

    // 读实际时长（lofty），用于交叉校验
    let actual_ms = read_duration_ms(&tmp);

    if let Err(e) = verify_audio(
        &tmp,
        expect_ms,
        if is_flac { None } else { expect_br },
        done,
    ) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("校验失败: {e}"));
    }
    if let Err(e) = verify_with_duration(
        &tmp,
        expect_ms,
        if is_flac { None } else { expect_br },
        done,
        actual_ms,
    ) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("校验失败: {e}"));
    }

    let final_path = dest_base.with_extension(ext);
    std::fs::rename(&tmp, &final_path).map_err(|e| format!("落盘失败: {e}"))?;
    Ok((final_path, actual_ms.unwrap_or(expect_ms)))
}

/// lofty 读取时长（失败返回 None）
pub fn read_duration_ms(path: &Path) -> Option<u64> {
    use lofty::prelude::*;
    let tagged = lofty::read_from_path(path).ok()?;
    let props = tagged.properties();
    Some((props.duration().as_secs_f64() * 1000.0) as u64)
}
