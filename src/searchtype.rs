//! 搜索/推荐 type 解析、配额与统一表行提取
// 多任务分期交付：本任务尚无调用方，后续任务接入后移除本行
#![allow(dead_code)]
use serde_json::Value;

/// type 英文别名表（大小写不敏感；数字直接透传，不做白名单）
const TYPE_ALIASES: &[(&str, u32)] = &[
    ("song", 1),
    ("album", 10),
    ("artist", 100),
    ("playlist", 1000),
    ("user", 1002),
    ("mv", 1004),
    ("lyric", 1006),
    ("radio", 1009),
    ("video", 1014),
    ("cloud", 1018),
    ("sound", 2000),
];

/// `search -t` 的 long_help（clap 静态文案）
pub const TYPE_HELP: &str = "\
搜索类型，逗号分隔，可混用数字与别名（如 1,1000 或 song,playlist）：
  1   song     单曲（默认）
  10  album    专辑
  100 artist   歌手
  1000 playlist 歌单
  1002 user    用户（仅列 ID）
  1004 mv      MV（仅列 ID）
  1006 lyric   歌词（仅列）
  1009 radio   主播电台（仅列 ID）
  1014 video   视频（仅列 ID）
  1018 cloud   综合（服务端不分页，limit 可能无效）
  2000 sound   声音（仅列）
未收录的数字也接受，按通用降级提取显示。";

/// 解析单个 type 标记（数字全放开；英文别名查表；其余报错）
pub fn parse_type_token(tok: &str) -> Result<u32, String> {
    let t = tok.trim();
    if t.is_empty() {
        return Err("type 不能为空".into());
    }
    if let Ok(v) = t.parse::<u32>() {
        return Ok(v);
    }
    for (a, v) in TYPE_ALIASES {
        if t.eq_ignore_ascii_case(a) {
            return Ok(*v);
        }
    }
    let all: Vec<&str> = TYPE_ALIASES.iter().map(|(a, _)| *a).collect();
    Err(format!("未知 type “{t}”（数字或别名：{}）", all.join("/")))
}

/// 保序去重
pub fn dedup_types(types: Vec<u32>) -> Vec<u32> {
    let mut out = Vec::new();
    for t in types {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// 总数 n 平分 k 类；余数依次给前几类；n=0 表示不限（配额 0）
pub fn quotas(n: u32, k: usize) -> Vec<u32> {
    if k == 0 {
        return vec![];
    }
    let base = n / k as u32;
    let rem = (n % k as u32) as usize;
    (0..k).map(|i| base + u32::from(i < rem)).collect()
}

/// 类型列中文文案；未收录 → `类型N`
pub fn type_label(ty: u32) -> String {
    match ty {
        1 => "单曲".into(),
        10 => "专辑".into(),
        100 => "歌手".into(),
        1000 => "歌单".into(),
        1002 => "用户".into(),
        1004 => "MV".into(),
        1006 => "歌词".into(),
        1009 => "电台".into(),
        1014 => "视频".into(),
        1018 => "综合".into(),
        2000 => "声音".into(),
        _ => format!("类型{ty}"),
    }
}

pub struct RenderRow {
    pub name: String,
    pub artist: String,
    pub meta: String,
    pub id: String,
    pub label: &'static str,
}

fn str_of(v: Option<&str>) -> String {
    v.unwrap_or("-").to_string()
}

/// 从结果条目提取统一表行；字段缺失显示 `-`，不 panic
pub fn extract_row(ty: u32, it: &Value) -> RenderRow {
    let id = it["id"]
        .as_u64()
        .or_else(|| it["userId"].as_u64())
        .map(|x| x.to_string())
        .unwrap_or_else(|| "-".into());
    let name = str_of(
        it["name"]
            .as_str()
            .or(it["nickname"].as_str())
            .or(it["title"].as_str()),
    );
    match ty {
        1 | 10 => {
            let artist = str_of(
                it["ar"][0]["name"]
                    .as_str()
                    .or(it["artists"][0]["name"].as_str()),
            );
            let meta = if ty == 1 {
                it["dt"]
                    .as_u64()
                    .or_else(|| it["duration"].as_u64())
                    .map(crate::model::hms)
                    .unwrap_or_else(|| "-".into())
            } else {
                it["size"]
                    .as_u64()
                    .map(|n| format!("{n} 首"))
                    .unwrap_or_else(|| "-".into())
            };
            RenderRow {
                name,
                artist,
                meta,
                id,
                label: if ty == 1 { "单曲" } else { "专辑" },
            }
        }
        100 => RenderRow {
            meta: it["musicSize"]
                .as_u64()
                .map(|n| format!("{n} 首"))
                .unwrap_or_else(|| "-".into()),
            name,
            artist: "-".into(),
            id,
            label: "歌手",
        },
        1000 => RenderRow {
            artist: str_of(it["creator"]["nickname"].as_str()),
            meta: it["trackCount"]
                .as_u64()
                .map(|n| format!("{n} 首"))
                .unwrap_or_else(|| "-".into()),
            name,
            id,
            label: "歌单",
        },
        1002 => RenderRow {
            name,
            artist: "-".into(),
            meta: "-".into(),
            id,
            label: "用户",
        },
        1004 => RenderRow {
            artist: str_of(it["artists"][0]["name"].as_str()),
            meta: it["duration"]
                .as_u64()
                .map(crate::model::hms)
                .unwrap_or_else(|| "-".into()),
            name,
            id,
            label: "MV",
        },
        1006 => RenderRow {
            name,
            artist: str_of(it["ar"][0]["name"].as_str()),
            meta: "-".into(),
            id,
            label: "歌词",
        },
        1009 => RenderRow {
            artist: str_of(it["dj"]["nickname"].as_str()),
            name,
            meta: "-".into(),
            id,
            label: "电台",
        },
        1014 => RenderRow {
            artist: str_of(it["creator"][0]["userName"].as_str()),
            meta: it["duration"]
                .as_u64()
                .map(crate::model::hms)
                .unwrap_or_else(|| "-".into()),
            name,
            id,
            label: "视频",
        },
        2000 => RenderRow {
            artist: str_of(it["creator"]["nickname"].as_str()),
            name,
            meta: "-".into(),
            id,
            label: "声音",
        },
        // 未收录类型：通用降级（仅提取 name/id，artist/meta 固定 "-"；label 每行一个小字符串，量级 ≤ limit）
        _ => RenderRow {
            name,
            artist: "-".into(),
            meta: "-".into(),
            id,
            label: Box::leak(format!("类型{ty}").into_boxed_str()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_numbers_and_aliases() {
        assert_eq!(parse_type_token("1").unwrap(), 1);
        assert_eq!(parse_type_token("song").unwrap(), 1);
        assert_eq!(parse_type_token("PLAYLIST").unwrap(), 1000);
        assert!(parse_type_token("2000").is_ok()); // 全放开
        assert!(parse_type_token("nonsense").is_err());
        assert!(parse_type_token("").is_err());
    }

    #[test]
    fn quotas_split_total() {
        assert_eq!(quotas(10, 2), vec![5, 5]);
        assert_eq!(quotas(10, 3), vec![4, 3, 3]);
        assert_eq!(quotas(1, 2), vec![1, 0]);
        assert_eq!(quotas(0, 2), vec![0, 0]); // 0 = 不限
    }

    #[test]
    fn dedup_keeps_order() {
        assert_eq!(dedup_types(vec![1, 1000, 1, 1000, 10]), vec![1, 1000, 10]);
    }

    #[test]
    fn labels() {
        assert_eq!(type_label(1), "单曲");
        assert_eq!(type_label(1000), "歌单");
        assert_eq!(type_label(9999), "类型9999");
    }

    #[test]
    fn extract_song_and_playlist_rows() {
        let song = json!({"id": 5, "name": "晴天", "ar": [{"name": "周杰伦"}], "dt": 269000});
        let r = extract_row(1, &song);
        assert_eq!(
            (r.name.as_str(), r.artist.as_str(), r.meta.as_str(), r.label),
            ("晴天", "周杰伦", "04:29", "单曲")
        );
        let pl = json!({"id": 9, "name": "雷达", "creator": {"nickname": "甲"}, "trackCount": 30});
        let r = extract_row(1000, &pl);
        assert_eq!(
            (r.artist.as_str(), r.meta.as_str(), r.label),
            ("甲", "30 首", "歌单")
        );
        // 字段缺失降级不 panic
        let r = extract_row(1004, &json!({"name": "mv"}));
        assert_eq!((r.artist.as_str(), r.meta.as_str()), ("-", "-"));
        // duration 存在 → hms 格式；缺失 → "-"
        let r = extract_row(1004, &json!({"id": 1, "name": "v", "duration": 91000}));
        assert_eq!((r.meta.as_str(), r.label), ("01:31", "MV"));
        // 计数字段缺失 → "-"（未知不等于 0）
        let r = extract_row(
            1000,
            &json!({"id": 1, "name": "歌单x", "creator": {"nickname": "乙"}}),
        );
        assert_eq!(r.meta.as_str(), "-");
        let r = extract_row(10, &json!({"id": 2, "name": "专辑x"}));
        assert_eq!(r.meta.as_str(), "-");
        let r = extract_row(100, &json!({"id": 3, "name": "歌手x"}));
        assert_eq!(r.meta.as_str(), "-");
        let r = extract_row(1014, &json!({"name": "v2"}));
        assert_eq!(r.meta.as_str(), "-");
    }
}
