//! 网易云明文接口封装（无需 weapi/eapi 加密）
use anyhow::Result;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

pub const BASE: &str = "https://music.163.com";
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
const BATCH: usize = 200;

pub struct Ncm {
    pub client: Client,
    /// 第三方源多为自签证书，单独一个跳过校验的 client
    pub insecure: Client,
}

impl Ncm {
    pub fn new() -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static(BASE),
        );
        headers.insert(
            reqwest::header::COOKIE,
            reqwest::header::HeaderValue::from_static("os=pc; appver=2.9.7;"),
        );
        let client = Client::builder()
            .user_agent(UA)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()?;
        let insecure = Client::builder()
            .user_agent(UA)
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self { client, insecure })
    }

    async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{BASE}{path}")
        };
        let resp = self
            .client
            .get(&url)
            .query(query)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json::<Value>().await.unwrap_or(Value::Null))
    }

    /// 搜索：type=1 单曲
    pub async fn search(&self, keyword: &str, limit: u32) -> Result<Vec<Value>> {
        let data = self
            .get_json(
                "/api/search/get/web",
                &[
                    ("type", "1".into()),
                    ("s", keyword.into()),
                    ("limit", limit.to_string()),
                    ("offset", "0".into()),
                ],
            )
            .await?;
        Ok(data["result"]["songs"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// 单曲详情（按 BATCH 分批）
    pub async fn song_detail(&self, ids: &[u64]) -> Result<Vec<Value>> {
        let mut songs = Vec::new();
        for chunk in ids.chunks(BATCH) {
            let ids_json = serde_json::to_string(chunk)?;
            let data = self
                .get_json("/api/song/detail", &[("ids", ids_json)])
                .await?;
            if let Some(arr) = data["songs"].as_array() {
                songs.extend_from_slice(arr);
            }
        }
        Ok(songs)
    }

    /// 歌单：详情取 trackIds 后分批拉曲目
    pub async fn playlist(&self, id: u64) -> Result<(Vec<Value>, String)> {
        let data = self
            .get_json("/api/v6/playlist/detail", &[("id", id.to_string())])
            .await?;
        let name = data["playlist"]["name"].as_str().unwrap_or("").to_string();
        let ids: Vec<u64> = data["playlist"]["trackIds"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|t| t["id"].as_u64()).collect())
            .unwrap_or_default();
        Ok((self.song_detail(&ids).await?, name))
    }

    /// 专辑
    pub async fn album(&self, id: u64) -> Result<(Vec<Value>, String)> {
        let data = self.get_json(&format!("/api/album/{id}"), &[]).await?;
        let name = data["album"]["name"].as_str().unwrap_or("").to_string();
        let songs = data["album"]["songs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok((songs, name))
    }

    /// 歌手热门曲
    pub async fn artist(&self, id: u64) -> Result<(Vec<Value>, String)> {
        let data = self.get_json(&format!("/api/v1/artist/{id}"), &[]).await?;
        let name = data["artist"]["name"].as_str().unwrap_or("").to_string();
        let songs = data["hotSongs"].as_array().cloned().unwrap_or_default();
        Ok((songs, name))
    }

    /// 播放地址（匿名 br=999000 会被服务端降级成 320k mp3）
    pub async fn player_url(&self, id: u64, br: u64) -> Result<Value> {
        let ids = serde_json::to_string(&[id])?;
        let data = self
            .get_json(
                "/api/song/enhance/player/url",
                &[("ids", ids), ("br", br.to_string())],
            )
            .await?;
        Ok(data["data"]
            .as_array()
            .and_then(|a| a.first().cloned())
            .unwrap_or(Value::Null))
    }

    /// 下载图片等小资源字节（封面用），超限报错
    pub async fn fetch_bytes(&self, url: &str, max: usize) -> Result<Vec<u8>> {
        let resp = self.client.get(url).send().await?.error_for_status()?;
        let mut out = Vec::new();
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if out.len() + chunk.len() > max {
                anyhow::bail!("资源超过 {max} 字节上限");
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    /// 歌词（lv/kv/tv/yv/yrv 打开全部类型）
    pub async fn lyric(&self, id: u64) -> Result<Value> {
        self.get_json(
            "/api/song/lyric",
            &[
                ("id", id.to_string()),
                ("lv", "1".into()),
                ("kv", "1".into()),
                ("tv", "-1".into()),
                ("yv", "1".into()),
                ("yrv", "1".into()),
            ],
        )
        .await
    }
}
