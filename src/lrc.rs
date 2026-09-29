//! 歌词：多类型获取、多路径写入、yrc → ASS 转换
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::api::Ncm;

/// 歌词类型定义：内部名 -> (接口字段, 文件后缀, 中文说明)
pub const LRC_TYPES: [(&str, &str, &str, &str); 5] = [
    ("lrc", "lrc", "lrc", "源语言歌词"),
    ("tlyric", "tlyric", "tl.lrc", "翻译歌词"),
    ("romalrc", "romalrc", "rom.lrc", "罗马音歌词"),
    ("yrc", "yrc", "yrc", "逐词滚动歌词"),
    ("ass", "yrc", "ass", "逐词滚动(ASS)"),
];
pub const LRC_DEFAULT: &str = "lrc,tlyric,romalrc";

/// 解析 --lrcs：逗号/空格分隔、别名归一、去重；未知类型报错
pub fn parse_lrc_selection(text: &str) -> Result<Vec<&'static str>> {
    let mut wanted: Vec<&'static str> = Vec::new();
    for raw in text.split(|c: char| c == ',' || c.is_whitespace()) {
        let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() {
            continue;
        }
        let token = match token.as_str() {
            "tl" | "translate" => "tlyric",
            "roman" | "roma" => "romalrc",
            "word" | "rolling" => "yrc",
            other => other,
        };
        let Some(entry) = LRC_TYPES.iter().find(|(name, ..)| *name == token) else {
            let valid: Vec<&str> = LRC_TYPES.iter().map(|(n, ..)| *n).collect();
            bail!("未知歌词类型 \"{token}\"，可用: {}", valid.join(","));
        };
        if !wanted.contains(&entry.0) {
            wanted.push(entry.0);
        }
    }
    if wanted.is_empty() {
        bail!("--lrcs 至少指定一种类型");
    }
    Ok(wanted)
}

fn suffix_of(kind: &str) -> &'static str {
    LRC_TYPES
        .iter()
        .find(|(n, ..)| *n == kind)
        .map(|(_, _, suffix, _)| *suffix)
        .unwrap_or("lrc")
}

fn field_of(kind: &str) -> &'static str {
    LRC_TYPES
        .iter()
        .find(|(n, ..)| *n == kind)
        .map(|(_, field, _, _)| *field)
        .unwrap_or("lrc")
}

/// 拉取并筛选歌词
pub async fn fetch_lrcs(
    ncm: &Ncm,
    song_id: u64,
    wanted: &[&'static str],
) -> HashMap<&'static str, String> {
    let data = match ncm.lyric(song_id).await {
        Ok(d) => d,
        Err(_) => return HashMap::new(),
    };
    let mut got = HashMap::new();
    for kind in wanted {
        let field = field_of(kind);
        let text = data[field]["lyric"].as_str().unwrap_or("").trim();
        let placeholder = matches!(text, "暂无歌词" | "纯音乐，请欣赏" | "纯音乐，请欣赏。");
        if !text.is_empty() && !placeholder {
            got.insert(*kind, text.to_string());
        }
    }
    got
}

/// 把选中歌词写入每个目标目录（每类型独立文件），返回写出的路径
pub fn write_lrcs(
    got: &HashMap<&'static str, String>,
    base_path: &Path,
    lrc_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    let mut written = Vec::new();
    let base_name = base_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());
    for dir in lrc_dirs {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("  ✘ 创建歌词目录失败 {}: {e}", dir.display());
            continue;
        }
        for (kind, text) in got {
            let mut content = text.clone();
            if *kind == "ass" {
                content = yrc_to_ass(text);
                if content.trim().is_empty() {
                    continue;
                }
            }
            let path = dir.join(format!("{base_name}.{}", suffix_of(kind)));
            if std::fs::write(&path, format!("{}\n", content.trim_end())).is_ok() {
                written.push(path);
            }
        }
    }
    written
}

/// yrc 逐词歌词 → 简易 ASS 字幕（逐行一条 Dialogue）
pub fn yrc_to_ass(yrc: &str) -> String {
    fn parse_ts(token: &str) -> Option<f64> {
        let t = token.trim_matches(|c| c == '[' || c == ']');
        if let Some((min, sec)) = t.split_once(':') {
            let min: f64 = min.parse().ok()?;
            let sec: f64 = sec.parse().ok()?;
            Some(min * 60.0 + sec)
        } else if let Ok(ms) = t.parse::<u64>() {
            Some(ms as f64 / 1000.0)
        } else {
            None
        }
    }

    let mut lines: Vec<(f64, Option<f64>, String)> = Vec::new();
    for raw in yrc.lines() {
        let stamps = extract_brackets(raw);
        if stamps.is_empty() {
            continue;
        }
        let mut text = strip_time_tags(raw);
        text = strip_k_tags(&text).trim().to_string();
        if text.is_empty() {
            continue;
        }
        let start = parse_ts(&format!("[{}]", stamps[0]));
        let end = parse_ts(&format!("[{}]", stamps[1]));
        if let Some(start) = start {
            lines.push((start, end, text));
        }
    }

    let mut events = String::new();
    for (i, (start, end, text)) in lines.iter().enumerate() {
        let stop = end.unwrap_or_else(|| lines.get(i + 1).map(|(s, ..)| *s).unwrap_or(start + 3.0));
        events.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
            ass_time(*start),
            ass_time(stop),
            text
        ));
    }

    let header = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1280\nPlayResY: 720\n\n\
        [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, \
        OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, \
        Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
        Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H7F000000,0,0,0,0,100,100,0,0,1,2,1,2,20,20,30,1\n\n\
        [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
    format!("{header}{events}")
}

fn extract_brackets(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else {
            break;
        };
        let inner = &rest[start + 1..start + end];
        if inner
            .chars()
            .all(|c| c.is_ascii_digit() || c == ':' || c == '.')
            && !inner.is_empty()
        {
            out.push(inner.to_string());
        }
        rest = &rest[start + end + 1..];
    }
    out
}

fn strip_time_tags(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else {
            out.push_str(rest);
            return out;
        };
        let inner = &rest[start + 1..start + end];
        if !(inner
            .chars()
            .all(|c| c.is_ascii_digit() || c == ':' || c == '.')
            && !inner.is_empty())
        {
            out.push('[');
            out.push_str(inner);
            out.push(']');
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_k_tags(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else {
            out.push_str(rest);
            break;
        };
        let inner = &rest[start + 1..start + end];
        if !(inner.starts_with("k:") || inner.starts_with("t:")) {
            out.push('[');
            out.push_str(inner);
            out.push(']');
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

fn ass_time(seconds: f64) -> String {
    let hours = (seconds / 3600.0).floor() as u32;
    let minutes = ((seconds % 3600.0) / 60.0).floor() as u32;
    let secs = seconds % 60.0;
    format!("{hours}:{minutes:02}:{secs:05.2}")
}
