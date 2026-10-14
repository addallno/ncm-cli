//! 网易云接口封装：读走明文 /api，写走 weapi/eapi 加密通道
use anyhow::Context;
use anyhow::Result;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

pub const BASE: &str = "https://music.163.com";
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
/// eapi 用桌面客户端 UA（pyncm 实测可写）
const UA_EAPI: &str = "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36 Chrome/91.0.4472.164 NeteaseMusicDesktop/2.10.2.200154";
/// pyncm eapi_config 设备指纹（weapi/eapi 写通道共用）
const EAPI_FINGERPRINT: &str =
    "os=iPhone OS; appver=10.0.0; osver=16.2; channel=distribution; deviceId=pyncm!";
const BATCH: usize = 200;

/// 写通道响应统一解析：明文 JSON / 尾部 \\x10 填充 / AES 密文依次尝试；
/// HTTP 非 2xx 或解析失败时带状态码与 body 前缀报错（不打印完整 body 防泄漏）
fn parse_write_body(
    ns: &str,
    path: &str,
    status: reqwest::StatusCode,
    bytes: &[u8],
) -> Result<Value> {
    // pyncm EapiDecrypt 有 strip("\\x10") 兜底，weapi 偶发同款填充
    let trimmed: &[u8] = bytes.strip_suffix(b"\x10").unwrap_or(bytes);
    let v: Option<Value> = serde_json::from_slice(trimmed).ok().or_else(|| {
        crate::crypto::eapi_decrypt(bytes).and_then(|s| serde_json::from_str::<Value>(&s).ok())
    });
    let Some(v) = v else {
        let head: String = bytes
            .iter()
            .take(64)
            .map(|b| {
                if b.is_ascii_graphic() || *b == b' ' {
                    *b as char
                } else {
                    '.'
                }
            })
            .collect();
        anyhow::bail!(
            "{ns}{path} 响应无法解析（HTTP {status}，{} 字节，前缀「{head}」），可能加密参数不被服务端接受",
            bytes.len()
        );
    };
    let code = v["code"].as_i64().unwrap_or(-1);
    if code != 200 {
        let msg = v["message"]
            .as_str()
            .or_else(|| v["msg"].as_str())
            .unwrap_or("");
        let hint = match code {
            301 => "（未登录或 csrf 失效，请重新 ncm login）",
            _ => "",
        };
        anyhow::bail!("{ns}{path} 失败 code={code} {msg}{hint}");
    }
    Ok(v)
}

#[derive(Clone)]
pub struct Ncm {
    pub client: Client,
    /// 第三方源多为自签证书，单独一个跳过校验的 client
    pub insecure: Client,
    /// weapi csrf_token（cookie 里的 __csrf，无则空串）
    csrf: String,
    /// 登录 cookie 原文（eapi 请求需拼设备指纹 cookie）
    cookie: String,
}

impl Ncm {
    /// 登录版：附加 MUSIC_U 等登录 cookie
    pub fn with_cookie(cookie: Option<&str>) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static(BASE),
        );
        let raw = cookie.unwrap_or("").to_string();
        let cookie_val = if raw.is_empty() {
            "os=pc; appver=2.9.7;".to_string()
        } else {
            format!("os=pc; appver=2.9.7; {raw}")
        };
        // weapi 需要 cookie 里的 __csrf 作 csrf_token
        let csrf = raw
            .split(';')
            .filter_map(|kv| kv.trim().strip_prefix("__csrf="))
            .next()
            .unwrap_or("")
            .to_string();
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
        Ok(Self {
            client,
            insecure,
            csrf,
            cookie: raw,
        })
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

    /// weapi 写操作：payload 为业务参数（自动附 csrf_token），POST /weapi/<path>
    async fn post_weapi(
        &self,
        path: &str,
        mut payload: serde_json::Map<String, Value>,
    ) -> Result<Value> {
        // 无 __csrf 不再前置拦截：读类 weapi（sublist/cloud 等）空 csrf 可用，
        // 写类失败由服务端 code 报告（下方 context 补登录提示）
        payload.insert("csrf_token".into(), json!(self.csrf));
        let (params, enc_sec_key) =
            crate::crypto::weapi_encrypt(&Value::Object(payload).to_string());
        let url = format!("{BASE}/weapi{path}");
        // pyncm weapi 也带设备指纹 cookie（eapi_config）
        let weapi_cookie = format!("{}; {}", EAPI_FINGERPRINT, self.cookie);
        // 写操作不重试：超时后服务端可能已执行，重试会重复建歌单等
        let resp = self
            .client
            .post(&url)
            .query(&[("csrf_token", self.csrf.as_str())])
            .header(reqwest::header::COOKIE, &weapi_cookie)
            .form(&[
                ("params", params.as_str()),
                ("encSecKey", enc_sec_key.as_str()),
            ])
            .send()
            .await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        let v = parse_write_body("/weapi", path, status, &bytes).with_context(|| {
            if self.csrf.is_empty() {
                "cookie 无 __csrf（部分写操作需完整 Cookie：浏览器复制时带上 __csrf 重新 ncm login --cookie）"
            } else {
                "weapi 请求失败"
            }
        })?;
        Ok(v)
    }

    /// eapi 写操作：payload 为业务参数（自动附 header 指纹），POST /eapi/<path>
    /// 响应可能是明文 JSON 或 AES 密文，均尝试解析
    async fn post_eapi(
        &self,
        path: &str,
        business: serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let mut payload = business;
        payload.insert("header".into(), json!(crate::crypto::eapi_header_json()));
        let api_path = format!("/api{path}");
        let params = crate::crypto::eapi_encrypt(&api_path, &Value::Object(payload).to_string());
        let url = format!("{BASE}/eapi{path}");
        // eapi 要求桌面 UA + 设备指纹 cookie（pyncm eapi_config）
        let eapi_cookie = format!("{}; {}", EAPI_FINGERPRINT, self.cookie);
        // 写操作不重试（同 post_weapi）
        let resp = self
            .client
            .post(&url)
            .header(reqwest::header::USER_AGENT, UA_EAPI)
            .header(reqwest::header::REFERER, "")
            .header(reqwest::header::COOKIE, &eapi_cookie)
            .form(&[("params", params.as_str())])
            .send()
            .await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        let v = parse_write_body("/eapi", path, status, &bytes)?;
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
            anyhow::bail!("私人FM失败 code={code}（需登录：ncm login）");
        }
        Ok(data["data"].as_array().cloned().unwrap_or_default())
    }

    /// 歌曲评论（明文读接口，可匿名）；返回 (热门评论列表, 总数)
    pub async fn song_comments(&self, song_id: u64, limit: u32) -> Result<(Vec<Value>, u64)> {
        let v = self
            .get_json(
                &format!("/api/v1/resource/comments/R_SO_4_{song_id}"),
                &[
                    ("rid", song_id.to_string()),
                    ("offset", "0".into()),
                    ("total", "true".into()),
                    ("limit", limit.to_string()),
                    ("beforeTime", "0".into()),
                ],
            )
            .await?;
        let total = v["total"].as_u64().unwrap_or(0);
        let hot = v["hotComments"].as_array().cloned().unwrap_or_default();
        Ok((hot, total))
    }

    /// 私人FM 跳过（eapi，pyncm SetSkipRadioContent）
    pub async fn fm_skip(&self, song_id: u64) -> Result<()> {
        self.post_eapi(
            "/v1/radio/get",
            serde_json::Map::from_iter([
                ("e_r".into(), json!("true")),
                ("songId".into(), json!(song_id.to_string())),
                ("alg".into(), json!("itembased")),
                ("time".into(), json!("0")),
            ]),
        )
        .await?;
        Ok(())
    }

    /// 私人FM 红心/取消红心（eapi，pyncm SetLikeRadioContent）
    pub async fn fm_like(&self, track_id: u64, like: bool) -> Result<()> {
        self.post_eapi(
            "/v1/radio/like",
            serde_json::Map::from_iter([
                ("e_r".into(), json!("true")),
                ("like".into(), json!(like.to_string().to_lowercase())),
                ("trackId".into(), json!(track_id.to_string())),
                ("alg".into(), json!("itembased")),
                ("time".into(), json!("0")),
            ]),
        )
        .await?;
        Ok(())
    }

    /// 私人FM 踩/移除（eapi，pyncm SetTrashRadioContent）
    pub async fn fm_trash(&self, song_id: u64) -> Result<()> {
        self.post_eapi(
            "/v1/radio/trash/add",
            serde_json::Map::from_iter([
                ("e_r".into(), json!("true")),
                ("songId".into(), json!(song_id.to_string())),
                ("alg".into(), json!("itembased")),
                ("time".into(), json!("0")),
            ]),
        )
        .await?;
        Ok(())
    }

    /// 每日签到（weapi，pyncm SetSignin）；type 0=移动端+4经验 1=网页端+1经验；返回获得经验数
    pub async fn daily_signin(&self, dtype: u8) -> Result<i64> {
        let v = self
            .post_weapi(
                "/point/dailyTask",
                serde_json::Map::from_iter([("type".into(), json!(dtype.to_string()))]),
            )
            .await?;
        Ok(v["point"].as_i64().unwrap_or(0))
    }

    /// 收藏的专辑列表（weapi，pyncm GetUserAlbumSubs）
    pub async fn album_subs(&self, limit: u32) -> Result<Vec<Value>> {
        let v = self
            .post_weapi(
                "/album/sublist",
                serde_json::Map::from_iter([("limit".into(), json!(limit.to_string()))]),
            )
            .await?;
        Ok(v["data"].as_array().cloned().unwrap_or_default())
    }

    /// 收藏的歌手列表（weapi，pyncm GetUserArtistSubs）
    pub async fn artist_subs(&self, limit: u32) -> Result<Vec<Value>> {
        let v = self
            .post_weapi(
                "/artist/sublist",
                serde_json::Map::from_iter([("limit".into(), json!(limit.to_string()))]),
            )
            .await?;
        Ok(v["data"].as_array().cloned().unwrap_or_default())
    }

    /// MV 详情（weapi，pyncm GetMVDetail）
    pub async fn mv_detail(&self, mv_id: u64) -> Result<Value> {
        let v = self
            .post_weapi(
                "/v1/mv/detail",
                serde_json::Map::from_iter([("id".into(), json!(mv_id.to_string()))]),
            )
            .await?;
        Ok(v["data"].clone())
    }

    /// MV 播放地址（weapi，pyncm GetMVResource）；res = 240/480/720/1080
    pub async fn mv_url(&self, mv_id: u64, res: u32) -> Result<Value> {
        let v = self
            .post_weapi(
                "/song/enhance/play/mv/url",
                serde_json::Map::from_iter([
                    ("id".into(), json!(mv_id.to_string())),
                    ("r".into(), json!(res.to_string())),
                ]),
            )
            .await?;
        Ok(v["data"].clone())
    }

    /// 云盘歌曲列表（weapi，pyncm GetCloudDriveInfo）；返回 (歌曲列表, 总数)
    /// data 条目顶层是云盘元数据（fileName/artist 为字符串），标准 song 结构在
    /// simpleSong 里——归一化成 song 结构，表格/下载（ncm download <id>）才能直接用
    pub async fn cloud_songs(&self, limit: u32, offset: u32) -> Result<(Vec<Value>, u64)> {
        let v = self
            .post_weapi(
                "/v1/cloud/get",
                serde_json::Map::from_iter([
                    ("limit".into(), json!(limit.to_string())),
                    ("offset".into(), json!(offset.to_string())),
                ]),
            )
            .await?;
        let count = v["count"].as_u64().unwrap_or(0);
        let songs: Vec<Value> = v["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|it| {
                        let mut ss = it
                            .get("simpleSong")
                            .filter(|s| s.is_object() && !s["name"].is_null())
                            .cloned()
                            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                        if ss["id"].as_u64().is_none_or(|v| v == 0) {
                            // simpleSong 缺 id 时用云盘条目的 songId 兜底
                            if let Some(sid) = it["songId"].as_u64() {
                                ss["id"] = json!(sid);
                            }
                        }
                        if let (Some(fn_), None) = (it["fileName"].as_str(), ss["name"].as_str()) {
                            // 极少数无名条目：用原文件名（去扩展名）顶上
                            let base = fn_.rsplit_once('.').map(|(b, _)| b).unwrap_or(fn_);
                            ss["name"] = json!(base);
                        }
                        ss
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok((songs, count))
    }

    /// 新建歌单（eapi，pyncm SetCreatePlaylist）；返回新歌单 id
    pub async fn playlist_create(&self, name: &str) -> Result<u64> {
        let v = self
            .post_eapi(
                "/playlist/create",
                serde_json::Map::from_iter([
                    ("name".into(), json!(name)),
                    ("privacy".into(), json!("0")),
                ]),
            )
            .await?;
        v["id"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("创建歌单成功但未返回 id"))
    }

    /// 删除歌单（eapi，pyncm SetRemovePlaylist；含其中全部歌曲，不可恢复）
    pub async fn playlist_delete(&self, id: u64) -> Result<()> {
        self.post_eapi(
            "/playlist/remove",
            serde_json::Map::from_iter([
                ("ids".into(), json!(format!("[{id}]"))),
                ("self".into(), json!("True")),
            ]),
        )
        .await?;
        Ok(())
    }

    /// 歌单元信息（改名前回填简介/标签用；/v1/playlist/update 是全量更新）
    pub async fn playlist_meta(&self, id: u64) -> Result<(String, String)> {
        let data = self
            .get_json("/api/v6/playlist/detail", &[("id", id.to_string())])
            .await?;
        let p = &data["playlist"];
        Ok((
            p["description"].as_str().unwrap_or("").to_string(),
            p["tags"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default(),
        ))
    }

    /// 歌单改名（weapi update 全量提交；先查原简介/标签防清空）
    pub async fn playlist_rename(&self, id: u64, name: &str) -> Result<()> {
        let (desc, tags) = self.playlist_meta(id).await.unwrap_or_default();
        self.post_weapi(
            "/v1/playlist/update",
            serde_json::Map::from_iter([
                ("id".into(), json!(id.to_string())),
                ("name".into(), json!(name)),
                ("desc".into(), json!(desc)),
                ("tags".into(), json!(tags)),
            ]),
        )
        .await?;
        Ok(())
    }

    /// 歌单增删曲目（weapi，pyncm SetManipulatePlaylistTrack）；op = "add" | "del"
    pub async fn playlist_tracks_op(
        &self,
        op: &str,
        playlist_id: u64,
        track_ids: &[u64],
    ) -> Result<()> {
        // pyncm 传 JSON 数组字符串（元素为字符串 id）
        let ids_json =
            serde_json::to_string(&track_ids.iter().map(|x| x.to_string()).collect::<Vec<_>>())?;
        self.post_weapi(
            "/v1/playlist/manipulate/tracks",
            serde_json::Map::from_iter([
                ("trackIds".into(), json!(ids_json)),
                ("pid".into(), json!(playlist_id.to_string())),
                ("op".into(), json!(op)),
                ("imme".into(), json!("true")),
            ]),
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
