//! 曲目字段提取、文件名模板、目标解析
use crate::api::Ncm;
use anyhow::{bail, Result};
use serde_json::Value;

/// 统一的曲目信息（兼容 artists/ar、album/al 两种接口命名）
pub struct Track {
    pub id: u64,
    pub title: String,
    pub artists: String,
    pub album: String,
    pub duration_ms: u64,
    pub no: u64,
    pub publish_ms: i64,
}

impl Track {
    pub fn from_json(v: &Value) -> Self {
        let artists = v
            .get("artists")
            .or_else(|| v.get("ar"))
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x["name"].as_str())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        let album = v
            .get("album")
            .or_else(|| v.get("al"))
            .map(|a| a["name"].as_str().unwrap_or(""))
            .unwrap_or("")
            .to_string();
        let publish = v
            .get("album")
            .or_else(|| v.get("al"))
            .and_then(|a| a["publishTime"].as_i64())
            .unwrap_or(0);
        Self {
            id: v["id"].as_u64().unwrap_or(0),
            title: v["name"].as_str().unwrap_or("").to_string(),
            artists,
            album,
            duration_ms: v["duration"].as_u64().unwrap_or(0),
            no: v["no"]
                .as_u64()
                .or_else(|| v["position"].as_u64())
                .unwrap_or(0),
            publish_ms: publish,
        }
    }

    /// 渲染文件名模板：{track} {artists} {album} {title} {id} {year} {no} {duration}
    pub fn render_name(&self, template: &str) -> String {
        let year = if self.publish_ms > 0 {
            let secs = self.publish_ms / 1000;
            let dt = time_from_unix(secs);
            format!("{dt:04}")
        } else {
            String::new()
        };
        let values: [(&str, String); 8] = [
            ("track", self.title.clone()),
            ("artists", self.artists.clone()),
            ("album", self.album.clone()),
            ("title", format!("{} - {}", self.title, self.artists)),
            ("id", self.id.to_string()),
            ("year", year),
            ("no", self.no.to_string()),
            ("duration", self.duration_ms.to_string()),
        ];
        let mut out = template.to_string();
        for (key, val) in values {
            out = out.replace(&format!("{{{key}}}"), &val);
        }
        sanitize(&out)
    }
}

/// 极简年份换算（避免引入 chrono 依赖）
fn time_from_unix(secs: i64) -> i64 {
    let days = secs.div_euclid(86_400);
    // 1970-01-01 起，按 400 年闰年循环估算年份（对 publishTime 精度足够）
    let mut year = 1970;
    let mut left = days;
    loop {
        let len = if is_leap(year) { 366 } else { 365 };
        if left < len {
            break;
        }
        left -= len;
        year += 1;
    }
    year
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// 去掉文件系统非法字符，限制长度
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\r' | '\n' | '\t' => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    let mut out: String = trimmed.chars().take(150).collect();
    if out.is_empty() {
        out = "unknown".into();
    }
    out
}

pub fn hms(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// 解析目标：ID / 链接 / list.123 → 曲目列表 + 描述
pub async fn resolve_target(
    ncm: &Ncm,
    target: &str,
    prefer_artist: bool,
) -> Result<(Vec<Value>, String)> {
    let target = target.trim();
    let (mut kind, mut id) = ("song", String::new());

    if target.chars().all(|c| c.is_ascii_digit()) && !target.is_empty() {
        id = target.to_string();
    } else if let Some((k, v)) = target.split_once('.') {
        if matches!(k, "list" | "song" | "album" | "artist")
            && v.chars().all(|c| c.is_ascii_digit())
        {
            kind = if k == "list" { "playlist" } else { k };
            id = v.to_string();
        }
    } else if target.starts_with("http") {
        kind = if target.contains("playlist") {
            "playlist"
        } else if target.contains("album") {
            "album"
        } else if target.contains("artist") {
            "artist"
        } else {
            "song"
        };
        if let Some(pos) = target.find("?id=") {
            id = target[pos + 4..]
                .split(['&', '#'])
                .next()
                .unwrap_or("")
                .to_string();
        }
        if !id.chars().all(|c| c.is_ascii_digit()) {
            let path = target.split('?').next().unwrap_or("");
            let tail = path.trim_end_matches('/');
            if let Some((_, last)) = tail.rsplit_once('/') {
                if last.chars().all(|c| c.is_ascii_digit()) && !last.is_empty() {
                    id = last.to_string();
                }
            }
        }
    }

    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        bail!("无法识别的目标: {target}");
    }
    let id: u64 = id.parse()?;

    if prefer_artist || kind == "artist" {
        let (songs, name) = ncm.artist(id).await?;
        return Ok((songs, format!("歌手 {name}（{id}）")));
    }
    match kind {
        "playlist" => {
            let (songs, name) = ncm.playlist(id).await?;
            Ok((songs, format!("歌单 {name}（{id}）")))
        }
        "album" => {
            let (songs, name) = ncm.album(id).await?;
            Ok((songs, format!("专辑 {name}（{id}）")))
        }
        _ => {
            let songs = ncm.song_detail(&[id]).await?;
            Ok((songs, format!("单曲 {id}")))
        }
    }
}

/// 搜索结果首行歌手名（显示用）
pub fn first_artist(v: &Value) -> String {
    v.get("artists")
        .or_else(|| v.get("ar"))
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.first())
        .and_then(|x| x["name"].as_str())
        .unwrap_or("")
        .to_string()
}
