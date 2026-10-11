//! 歌词：多类型获取、多路径写入、yrc → ASS 转换
use anyhow::{bail, Context, Result};
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

/// 拉取并筛选歌词；接口失败（网络耗尽/HTTP错误）上抛，不再静默丢弃
pub async fn fetch_lrcs(
    ncm: &Ncm,
    song_id: u64,
    wanted: &[&'static str],
) -> Result<HashMap<&'static str, String>> {
    let data = match ncm.lyric(song_id).await {
        Ok(d) => d,
        Err(e) => {
            // 限频/抖动常见：get_json 内层重试后仍失败，退避一次再试
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            ncm.lyric(song_id)
                .await
                .with_context(|| format!("歌词接口失败(id={song_id}): {e}"))?
        }
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
    Ok(got)
}

/// 选嵌入主歌词：按 lrc → tlyric → romalrc → yrc 固定优先级
/// （HashMap 遍历顺序随机，不能用 values().next()，否则可能嵌入翻译/罗马音）
pub fn pick_embed(got: &HashMap<&'static str, String>) -> Option<String> {
    ["lrc", "tlyric", "romalrc", "yrc"]
        .into_iter()
        .find_map(|k| got.get(k).cloned())
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

/// 解析 LRC 时间轴 → (秒, 文本) 列表（按时间升序，同一秒多行保持原序）
///
/// 支持 `[mm:ss.xx]` / `[mm:ss.xxx]` 多时间标签、`[mm:ss]`；
/// 过滤 `[ar:]` 等元数据行与空文本行
pub fn parse_lrc_timeline(text: &str) -> Vec<(f64, String)> {
    let mut out: Vec<(f64, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.first() != Some(&b'[') {
            continue;
        }
        // 收集行首连续的 [...] 时间标签；元数据（非时间）整体跳过
        let mut times: Vec<f64> = Vec::new();
        let mut rest = line;
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[1..end];
            match parse_timestamp(tag) {
                Some(t) => times.push(t),
                None => {
                    if !times.is_empty() {
                        break; // 时间标签后跟 [ar:] 类元数据：以已收集的时间为准
                    }
                    times.clear();
                    break;
                }
            }
            rest = &rest[end + 1..];
        }
        if times.is_empty() {
            continue; // 元数据行 [ti:..] / [offset:..]
        }
        let content = rest.trim();
        if content.is_empty() {
            continue;
        }
        for t in times {
            out.push((t, content.to_string()));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// 把译文行并入原文时间轴：译文插到同刻原文之后且**沿用原文时间戳**
/// （player::current_line 靠 `lines[i-1].0 == lines[i].0` 精确相等判配对）
///
/// 容错：先按时间戳匹配（偏差 ≤0.5s 对齐最近原文，覆盖 tlyric 精度/整体偏移）；
/// 若几乎全失败（匹配率 <20%）且行数与原文一致，按序配对兜底；
/// 仍找不到归属的译文行丢弃，避免错位成主行
pub fn merge_translation(
    orig: Vec<(f64, String)>,
    trans: Vec<(f64, String)>,
) -> Vec<(f64, String)> {
    if trans.is_empty() {
        return orig;
    }
    // 每个译文行找 |Δt| 最小且 ≤0.5s 的原文
    let match_idx = |t: f64, lines: &[(f64, String)]| -> Option<usize> {
        lines
            .iter()
            .enumerate()
            .filter(|(_, (ot, _))| (ot - t).abs() <= 0.5)
            .min_by(|a, b| {
                (a.1 .0 - t)
                    .abs()
                    .partial_cmp(&(b.1 .0 - t).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    };
    let matched: Vec<(usize, f64, String)> = trans
        .iter()
        .filter_map(|(t, text)| match_idx(*t, &orig).map(|i| (i, *t, text.clone())))
        .collect();
    // ② 时间戳几乎全错位（如 tlyric 时间戳全为 0 或乱序）且行数一致 → 按序配对
    if matched.len() * 5 < trans.len() && orig.len() == trans.len() {
        let mut out = Vec::with_capacity(orig.len() * 2);
        for ((ts, otext), (_, ttext)) in orig.into_iter().zip(trans) {
            out.push((ts, otext));
            out.push((ts, ttext));
        }
        return out;
    }
    // ① 沿用原文时间戳，插到对应原文之后（后插不干扰先插的相邻判定）
    let mut out = orig;
    for (i, _t, text) in matched.into_iter().rev() {
        let ts = out[i].0;
        out.insert(i + 1, (ts, text));
    }
    out
}

/// /api/song/lyric 响应 → 播放时间轴（原文 + 同刻译文分行所需的成对条目）
pub fn timeline_from_lyric(v: &serde_json::Value) -> Vec<(f64, String)> {
    let Some(text) = v["lrc"]["lyric"].as_str() else {
        return Vec::new();
    };
    let mut lines = parse_lrc_timeline(text);
    if let Some(tl) = v["tlyric"]["lyric"].as_str() {
        lines = merge_translation(lines, parse_lrc_timeline(tl));
    }
    lines
}

/// `[mm:ss]` / `[mm:ss.xx]` / `[mm:ss.xxx]` → 秒；非时间标签返回 None
fn parse_timestamp(tag: &str) -> Option<f64> {
    let (mm, ss) = tag.split_once(':')?;
    let m: f64 = mm.trim().parse().ok()?;
    let (sec, frac) = match ss.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (ss, None),
    };
    let s: f64 = sec.trim().parse().ok()?;
    let frac_v = match frac {
        Some(f) if !f.is_empty() => {
            let digits: String = f.chars().take(3).collect();
            let v: f64 = digits.parse().ok()?;
            let scale = 10f64.powi(digits.len() as i32);
            v / scale
        }
        _ => 0.0,
    };
    Some(m * 60.0 + s + frac_v)
}

#[cfg(test)]
mod timeline_tests {
    use super::*;

    #[test]
    fn parse_basic_lrc() {
        let lrc = "[ti:晴天]\n[ar:周杰伦]\n[00:00.00]前奏\n[00:12.50]故事的小黄花\n[00:17.300]从出生那年就飘着\n";
        let v = parse_lrc_timeline(lrc);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], (0.0, "前奏".into()));
        assert_eq!(v[1], (12.5, "故事的小黄花".into()));
        assert_eq!(v[2], (17.3, "从出生那年就飘着".into()));
    }

    #[test]
    fn parse_multi_tag_and_sort() {
        let lrc = "[00:05.00][00:20.10]重复段\n[01:02]C\n";
        let v = parse_lrc_timeline(lrc);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], (5.0, "重复段".into()));
        assert_eq!(v[1], (20.1, "重复段".into()));
        assert_eq!(v[2], (62.0, "C".into()));
    }

    #[test]
    fn parse_offset_tag_ignored() {
        let lrc = "[offset:500]\n[00:01]词\n";
        let v = parse_lrc_timeline(lrc);
        assert_eq!(v, vec![(1.0, "词".to_string())]);
    }

    /// 译文紧跟同刻原文之后且时间戳完全相等（player 配对判定依赖精确相等）
    #[test]
    fn merge_translation_pairs_same_timestamp() {
        let orig = vec![
            (8.272, "I been running".into()),
            (11.991, "Trippin over".into()),
        ];
        let trans = vec![(8.272, "大脑回旋".into()), (99.0, "孤儿译文".into())];
        let m = merge_translation(orig, trans);
        assert_eq!(
            m,
            vec![
                (8.272, "I been running".into()),
                (8.272, "大脑回旋".into()),
                (11.991, "Trippin over".into()),
            ],
            "同刻插入 + 无匹配译文丢弃"
        );
        // 时间戳精确相等 → player::current_line 可判成对
        assert_eq!(m[0].0, m[1].0);
    }

    /// tlyric 时间戳与原文有微小偏差（≤0.5s）时对齐到最近的原文时间戳
    #[test]
    fn merge_translation_snaps_slight_offset() {
        let orig = vec![(12.5, "原文".into())];
        let trans = vec![(12.9, "译文".into())];
        let m = merge_translation(orig, trans);
        assert_eq!(m, vec![(12.5, "原文".into()), (12.5, "译文".into())]);
    }

    /// tlyric 时间戳整体错位（全 >0.5s）但行数一致 → 按序配对兜底
    #[test]
    fn merge_translation_index_fallback_when_all_misaligned() {
        let orig = vec![(1.0, "A".into()), (5.0, "B".into()), (9.0, "C".into())];
        let trans = vec![(3.0, "甲".into()), (7.0, "乙".into()), (11.0, "丙".into())];
        let m = merge_translation(orig, trans);
        assert_eq!(
            m,
            vec![
                (1.0, "A".into()),
                (1.0, "甲".into()),
                (5.0, "B".into()),
                (5.0, "乙".into()),
                (9.0, "C".into()),
                (9.0, "丙".into()),
            ]
        );
    }

    /// 大部分可按时间戳匹配时不用按序兜底（避免孤儿译文错位成主行）
    #[test]
    fn merge_translation_prefers_timestamp_when_mostly_matched() {
        let orig = vec![(1.0, "A".into()), (5.0, "B".into()), (9.0, "C".into())];
        // 2/3 匹配，1 行孤儿
        let trans = vec![
            (1.1, "甲".into()),
            (5.05, "乙".into()),
            (99.0, "孤儿".into()),
        ];
        let m = merge_translation(orig, trans);
        assert_eq!(
            m,
            vec![
                (1.0, "A".into()),
                (1.0, "甲".into()),
                (5.0, "B".into()),
                (5.0, "乙".into()),
                (9.0, "C".into()),
            ],
            "孤儿译文丢弃，不按序错位"
        );
    }

    /// 无 tlyric 字段时按纯原文返回
    #[test]
    fn timeline_from_lyric_without_tlyric() {
        let v: serde_json::Value = serde_json::json!({"lrc": {"lyric": "[00:01]词\n"}});
        assert_eq!(timeline_from_lyric(&v), vec![(1.0, "词".into())]);
    }

    /// 有 tlyric 时译文并入成对
    #[test]
    fn timeline_from_lyric_merges_tlyric() {
        let v: serde_json::Value = serde_json::json!({
            "lrc": {"lyric": "[00:08.272]I been running\n[00:11.991]Trippin over\n"},
            "tlyric": {"lyric": "[by:x]\n[00:08.272]大脑回旋着\n"}
        });
        let lines = timeline_from_lyric(&v);
        assert_eq!(
            lines,
            vec![
                (8.272, "I been running".into()),
                (8.272, "大脑回旋着".into()),
                (11.991, "Trippin over".into()),
            ]
        );
    }

    /// lrc 缺失时返回空（不 panic）
    #[test]
    fn timeline_from_lyric_missing_lrc() {
        let v: serde_json::Value = serde_json::json!({"nolyric": true});
        assert!(timeline_from_lyric(&v).is_empty());
    }

    /// 嵌入优先级：lrc > tlyric > romalrc > yrc（HashMap 顺序无关）
    #[test]
    fn pick_embed_priority() {
        let mut got = HashMap::new();
        got.insert("yrc", "逐词".to_string());
        got.insert("tlyric", "翻译".to_string());
        got.insert("romalrc", "罗马音".to_string());
        got.insert("lrc", "源语言".to_string());
        assert_eq!(pick_embed(&got).as_deref(), Some("源语言"));
        got.remove("lrc");
        assert_eq!(pick_embed(&got).as_deref(), Some("翻译"));
        got.remove("tlyric");
        assert_eq!(pick_embed(&got).as_deref(), Some("罗马音"));
        got.remove("romalrc");
        assert_eq!(pick_embed(&got).as_deref(), Some("逐词"));
        got.remove("yrc");
        assert!(pick_embed(&got).is_none());
    }
}
