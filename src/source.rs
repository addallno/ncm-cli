//! 第三方无损取址源（官方匿名 br=999000 会被降级成 mp3，FLAC 必须走这里）
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Mutex;

use crate::api::Ncm;

static SOURCE_DOWN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// 熔断：429/5xx/403 失败的源本会话不再尝试
fn mark_down(name: &'static str, err: &str) {
    if ["429", "500", "502", "503", "403"]
        .iter()
        .any(|code| err.contains(code))
    {
        let mut guard = SOURCE_DOWN.lock().unwrap();
        guard.get_or_insert_with(HashSet::new).insert(name);
    }
}

fn is_down(name: &str) -> bool {
    SOURCE_DOWN
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.contains(name))
        .unwrap_or(false)
}

/// 从第三方接口返回里递归挖出 http 直链
pub fn dig_url(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["url", "data", "song", "downUrl", "musicUrl", "link"] {
                if let Some(v) = map.get(key) {
                    match v {
                        Value::String(s)
                            if s.starts_with("http")
                                && !s
                                    .get(..20)
                                    .is_some_and(|x| x.to_lowercase().contains("html")) =>
                        {
                            return Some(s.clone())
                        }
                        Value::Object(_) | Value::Array(_) => {
                            if let Some(found) = dig_url(v) {
                                return Some(found);
                            }
                        }
                        _ => {}
                    }
                }
            }
            None
        }
        Value::Array(arr) => arr.iter().find_map(dig_url),
        _ => None,
    }
}

async fn tp_xuanluoge(ncm: &Ncm, id: u64) -> anyhow::Result<String> {
    let resp = ncm
        .insecure
        .get("https://music.qinglvai.top/api/index.php")
        .query(&[
            ("route", "/music/url".to_string()),
            ("id", id.to_string()),
            ("level", "lossless".to_string()),
            ("use", "play".to_string()),
            ("source", "netease".to_string()),
        ])
        .send()
        .await?
        .error_for_status()?;
    let body: Value = resp.json().await?;
    dig_url(&body).ok_or_else(|| anyhow::anyhow!("响应无可用 url"))
}

async fn tp_kangqiovo(ncm: &Ncm, id: u64) -> anyhow::Result<String> {
    let resp = ncm
        .insecure
        .post("https://ncm.kangqiovo.com/Song_V1")
        .form(&[
            ("url", id.to_string()),
            ("level", "lossless".to_string()),
            ("type", "json".to_string()),
        ])
        .send()
        .await?
        .error_for_status()?;
    let body: Value = resp.json().await?;
    dig_url(&body).ok_or_else(|| anyhow::anyhow!("响应无可用 url"))
}

async fn tp_vincentzyu(ncm: &Ncm, id: u64) -> anyhow::Result<String> {
    let resp = ncm
        .insecure
        .get(format!(
            "http://xwl.vincentzyu233.cn:51217/v2/music/netease?id={id}&quality=9"
        ))
        .send()
        .await?
        .error_for_status()?;
    let body: Value = resp.json().await?;
    dig_url(&body).ok_or_else(|| anyhow::anyhow!("响应无可用 url"))
}

/// 第三方源解析 future 类型（源名 + 结果）；生命周期随 ncm 借用
type TpFut<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = (&'static str, anyhow::Result<String>)> + Send + 'a,
    >,
>;

/// 并发尝试第三方源（先到先得），返回 (直链, 源名, 最近一次错误)
/// 串行时每个失败源都要等满超时（3 源最坏 ~3× 超时），并发后最坏 1× 超时
pub async fn resolve_lossless(ncm: &Ncm, id: u64) -> (Option<String>, &'static str, String) {
    let mut futs: Vec<TpFut<'_>> = Vec::new();
    // 已熔断的源不启动，省一次注定失败的请求
    if !is_down("xuanluoge") {
        futs.push(Box::pin(async move {
            ("xuanluoge", tp_xuanluoge(ncm, id).await)
        }));
    }
    if !is_down("kangqiovo") {
        futs.push(Box::pin(async move {
            ("kangqiovo", tp_kangqiovo(ncm, id).await)
        }));
    }
    if !is_down("vincentzyu") {
        futs.push(Box::pin(async move {
            ("vincentzyu", tp_vincentzyu(ncm, id).await)
        }));
    }
    let mut last_err = String::new();
    let mut pending = futs;
    while !pending.is_empty() {
        let ((name, res), _idx, rest) = futures_util::future::select_all(pending).await;
        pending = rest;
        match res {
            Ok(url) => return (Some(url), name, last_err),
            Err(err) => {
                let text = err.to_string();
                mark_down(name, &text);
                let msg = format!("{name}: {}", truncate(&text, 80));
                if last_err.trim().is_empty() {
                    last_err = msg;
                } else {
                    last_err = format!("{last_err}；{msg}");
                }
            }
        }
    }
    (None, "", last_err)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}
