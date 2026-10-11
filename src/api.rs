//! 网易云明文接口封装（无需 weapi/eapi 加密）
use anyhow::Result;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

pub const BASE: &str = "https://music.163.com";
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
const BATCH: usize = 200;

#[derive(Clone)]
pub struct Ncm {
    pub client: Client,
    /// 第三方源多为自签证书，单独一个跳过校验的 client
    pub insecure: Client,
}

impl Ncm {
    /// 登录版：附加 MUSIC_U 等登录 cookie
    pub fn with_cookie(cookie: Option<&str>) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static(BASE),
        );
        let cookie_val = match cookie {
            Some(c) => format!("os=pc; appver=2.9.7; {c}"),
            None => "os=pc; appver=2.9.7;".to_string(),
        };
        headers.insert(
            reqwest::header::COOKIE,
            reqwest::header::HeaderValue::from_str(&cookie_val)?,
        );
        // musl 静态二进制在 Termux 下缺 /etc/resolv.conf，挂自定义 resolver 提供 UDP 回退
        let client = Client::builder()
            .user_agent(UA)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .dns_resolver(std::sync::Arc::new(crate::dns::FallbackDns::default()))
            .build()?;
        let insecure = Client::builder()
            .user_agent(UA)
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(15))
            .dns_resolver(std::sync::Arc::new(crate::dns::FallbackDns::default()))
            .build()?;
        Ok(Self { client, insecure })
    }

    async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{BASE}{path}")
        };
        let resp = crate::retry::retry_reqwest(|| self.client.get(&url).query(query).send())
            .await?
            .error_for_status()?;
        Ok(resp.json::<Value>().await.unwrap_or(Value::Null))
    }

    /// form-urlencoded POST（歌单增删改等写操作）；校验 code==200
    async fn post_form(&self, path: &str, form: &[(&str, String)]) -> Result<Value> {
        let url = format!("{BASE}{path}");
        let resp = crate::retry::retry_reqwest(|| self.client.post(&url).form(form).send())
            .await?
            .error_for_status()?;
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        let code = v["code"].as_i64().unwrap_or(-1);
        if code != 200 {
            let msg = v["message"]
                .as_str()
                .or_else(|| v["msg"].as_str())
                .unwrap_or("");
            anyhow::bail!("{path} 失败 code={code} {msg}");
        }
        Ok(v)
    }

    /// 按 type 搜索；返回 (最终type, 条目)。type=1018 综合：按 result.order 展开各子类
    pub async fn search_typed(
        &self,
        keyword: &str,
        ty: u32,
        limit: u32,
    ) -> Result<Vec<(u32, Value)>> {
        let data = self
            .get_json(
                "/api/search/get/web",
                &[
                    ("type", ty.to_string()),
                    ("s", keyword.into()),
                    ("limit", limit.to_string()),
                    ("offset", "0".into()),
                ],
            )
            .await?;
        let result = &data["result"];
        let mut out: Vec<(u32, Value)> = Vec::new();
        if ty == 1018 {
            // 综合：order 声明子类顺序；key → 子 type 映射
            const CLOUD_KEYS: &[(&str, u32)] = &[
                ("songs", 1),
                ("albums", 10),
                ("artists", 100),
                ("playlists", 1000),
                ("mvs", 1004),
                ("djRadios", 1009),
                ("videos", 1014),
            ];
            if let Some(order) = result["order"].as_array() {
                for k in order.iter().filter_map(|x| x.as_str()) {
                    if let Some((_, sub)) = CLOUD_KEYS.iter().find(|(key, _)| *key == k) {
                        if let Some(arr) = result[k].as_array() {
                            out.extend(arr.iter().map(|x| (*sub, x.clone())));
                        }
                    }
                }
            }
            return Ok(out);
        }
        // 单类型：取 result 内该 type 的数组键；未收录键 → 找第一个数组
        let key = match ty {
            1 | 1006 => "songs",
            10 => "albums",
            100 => "artists",
            1000 => "playlists",
            1002 => "userprofiles",
            1004 => "mvs",
            1009 => "djRadios",
            1014 => "videos",
            2000 => "sounds",
            _ => "",
        };
        let arr = if !key.is_empty() {
            result[key].as_array()
        } else {
            None
        };
        let arr = arr.or_else(|| {
            result
                .as_object()
                .and_then(|m| m.values().find_map(|v| v.as_array()))
        });
        if let Some(arr) = arr {
            out.extend(arr.iter().map(|x| (ty, x.clone())));
        }
        if ty == 1 {
            // 单曲保持既有相关度排序
            let mut songs: Vec<Value> = out.iter().map(|(_, v)| v.clone()).collect();
            rank_songs(&mut songs, keyword);
            out = songs.into_iter().map(|v| (1, v)).collect();
        }
        Ok(out)
    }

    /// 兼容旧签名：仅搜单曲
    pub async fn search(&self, keyword: &str, limit: u32) -> Result<Vec<Value>> {
        Ok(self
            .search_typed(keyword, 1, limit)
            .await?
            .into_iter()
            .map(|(_, v)| v)
            .collect())
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
        let resp = crate::retry::retry_reqwest(|| self.client.get(url).send())
            .await?
            .error_for_status()?;
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

    /// 当前登录用户（uid + 昵称 + VIP），未登录返回 None
    pub async fn whoami(&self) -> Result<Option<(u64, String)>> {
        let data = self.get_json("/api/nuser/account/get", &[]).await?;
        let uid = match data["profile"]["userId"]
            .as_u64()
            .or_else(|| data["account"]["id"].as_u64())
        {
            Some(u) => u,
            None => return Ok(None),
        };
        let name = data["profile"]["nickname"]
            .as_str()
            .or_else(|| data["account"]["userName"].as_str())
            .unwrap_or("(未知)")
            .to_string();
        Ok(Some((uid, name)))
    }

    /// 当前登录用户是否 VIP（vipType：0=无 11=黑胶 100=年费）；未登录按非 VIP
    pub async fn vip_status(&self) -> Result<bool> {
        let data = self.get_json("/api/nuser/account/get", &[]).await?;
        Ok(data["profile"]["vipType"].as_i64().unwrap_or(0) != 0)
    }

    /// 我的歌单列表（含"我喜欢的音乐"红心歌单，通常为首项）
    pub async fn user_playlists(&self, uid: u64) -> Result<Vec<Value>> {
        let data = self
            .get_json(
                // 注意：老路径 /api/v1/user/playlist 已被官方下线（JSON code=404）
                "/api/user/playlist",
                &[
                    ("uid", uid.to_string()),
                    ("limit", "1000".into()),
                    ("offset", "0".into()),
                ],
            )
            .await?;
        let code = data["code"].as_i64().unwrap_or(-1);
        if code != 200 {
            let msg = data["message"].as_str().unwrap_or("");
            anyhow::bail!("获取歌单失败 code={code} {msg}");
        }
        Ok(data["playlist"].as_array().cloned().unwrap_or_default())
    }

    /// 每日推荐歌曲（需登录）
    pub async fn daily_songs(&self) -> Result<Vec<Value>> {
        let data = self
            .get_json("/api/v1/discovery/recommend/songs", &[])
            .await?;
        Ok(data["data"]["dailySongs"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// 每日推荐歌单列表（需登录）；私人雷达在其中按名称筛出
    pub async fn daily_playlists(&self) -> Result<Vec<Value>> {
        let data = self
            .get_json("/api/v1/discovery/recommend/resource", &[])
            .await?;
        Ok(data["recommend"].as_array().cloned().unwrap_or_default())
    }

    /// 官方榜单列表（首页排行榜推荐位，元素为歌单结构，可直接 list/download）
    pub async fn toplist(&self) -> Result<Vec<Value>> {
        let data = self.get_json("/api/toplist", &[]).await?;
        let code = data["code"].as_i64().unwrap_or(-1);
        if code != 200 {
            anyhow::bail!("获取榜单失败 code={code}");
        }
        Ok(data["list"].as_array().cloned().unwrap_or_default())
    }

    /// 私人FM 一批歌曲（播放漫游数据源，元素为标准 song 结构）
    pub async fn radio_songs(&self) -> Result<Vec<Value>> {
        let data = self.get_json("/api/v1/radio/get", &[]).await?;
        let code = data["code"].as_i64().unwrap_or(-1);
        if code != 200 {
            anyhow::bail!("私人FM失败 code={code}");
        }
        Ok(data["data"].as_array().cloned().unwrap_or_default())
    }

    /// 新建歌单；返回新歌单 id
    pub async fn playlist_create(&self, name: &str) -> Result<u64> {
        let v = self
            .post_form("/api/playlist/create", &[("name", name.into())])
            .await?;
        v["id"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("创建歌单成功但未返回 id"))
    }

    /// 删除歌单（含其中全部歌曲，不可恢复）
    pub async fn playlist_delete(&self, id: u64) -> Result<()> {
        self.post_form("/api/playlist/delete", &[("id", id.to_string())])
            .await?;
        Ok(())
    }

    /// 歌单改名（只传 name，不动描述/标签）
    pub async fn playlist_rename(&self, id: u64, name: &str) -> Result<()> {
        self.post_form(
            "/api/playlist/update",
            &[
                ("id", id.to_string()),
                ("name", name.into()),
                ("desc", String::new()),
                ("tags", String::new()),
            ],
        )
        .await?;
        Ok(())
    }

    /// 歌单增删曲目；op = "add" | "del"，trackIds 逗号分隔
    pub async fn playlist_tracks_op(
        &self,
        op: &str,
        playlist_id: u64,
        track_ids: &[u64],
    ) -> Result<()> {
        let ids = track_ids
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",");
        self.post_form(
            "/api/playlist/manipulate/tracks",
            &[
                ("op", op.into()),
                ("pid", playlist_id.to_string()),
                ("trackIds", ids),
                ("imme", "true".into()),
            ],
        )
        .await?;
        Ok(())
    }
}

/// 按与关键词的相关度稳定排序（sort_by_key 稳定，同档保持接口原序）：
/// 0 = 歌名完全相等；1 = 歌名前缀；2 = 歌名/歌手包含全部关键词；3 = 有关键词未命中
fn rank_songs(songs: &mut [Value], keyword: &str) {
    let kw = keyword.trim().to_lowercase();
    if kw.is_empty() || songs.len() < 2 {
        return;
    }
    let terms: Vec<&str> = kw.split_whitespace().collect();
    songs.sort_by_key(|s| {
        let name = s["name"].as_str().unwrap_or("").to_lowercase();
        let artist = crate::model::first_artist(s).to_lowercase();
        let hay = format!("{name} {artist}");
        if !terms.iter().all(|t| hay.contains(t)) {
            return 3u8;
        }
        if name == kw {
            0
        } else if name.starts_with(&kw) {
            1
        } else {
            2
        }
    });
}
