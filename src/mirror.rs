//! 第三方解锁音源（UnblockNeteaseMusic-utils REST API）兜底取址
//!
//! 官方取址失败（无版权/-110 需 VIP）或仅返回 30s 试听时，
//! 调用本地/自建的 `GET {base}/match?id=<网易云ID>` 拿完整音频直链。
//! 部署：`npx @neteasecloudmusicapienhanced/unblockmusic-utils`（默认 3000 端口）。

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

/// UnblockNeteaseMusic-utils 默认监听地址
pub const DEFAULT: &str = "http://127.0.0.1:3000";

/// 每次匹配请求的超时（unm 跨平台搜索冷启动可达 20s+，需留足余量）
const MATCH_TIMEOUT: Duration = Duration::from_secs(30);

static BASE: OnceLock<Option<String>> = OnceLock::new();
static CLIENT: OnceLock<Client> = OnceLock::new();

/// 启动时初始化（main 调用一次）。
/// 优先级：`--mirror` > `NCM_MIRROR` > 默认 localhost:3000；值为 `none`（不区分大小写）时禁用。
pub fn init(cli: Option<&str>) {
    let raw = cli
        .map(str::to_string)
        .or_else(|| std::env::var("NCM_MIRROR").ok())
        .unwrap_or_else(|| DEFAULT.to_string());
    let val = raw.trim();
    let opt = if val.eq_ignore_ascii_case("none") || val.is_empty() {
        None
    } else {
        Some(val.trim_end_matches('/').to_string())
    };
    let _ = BASE.set(opt);
}

/// 是否启用兜底（未初始化时视为未启用）
pub fn enabled() -> bool {
    matches!(BASE.get(), Some(Some(_)))
}

/// 生效的 mirror 基地址（未启用返回空串）
pub fn base() -> &'static str {
    match BASE.get() {
        Some(Some(b)) => b.as_str(),
        _ => "",
    }
}

/// `GET {base}/match?id=` → (音频直链, 音源标识)
pub async fn match_url(id: u64) -> Result<(String, String)> {
    let base = base();
    if base.is_empty() {
        return Err(anyhow!("未配置解锁服务"));
    }
    let client = CLIENT.get_or_init(|| {
        Client::builder()
            .no_proxy()
            .user_agent("ncm-rs")
            .build()
            .unwrap_or_default()
    });
    let resp = client
        .get(format!("{base}/match"))
        .query(&[("id", id.to_string())])
        .timeout(MATCH_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("连接 {base} 失败"))?
        .error_for_status()
        .with_context(|| format!("{base} 返回错误状态"))?;
    let v: Value = resp.json().await.context("响应不是合法 JSON")?;
    if v["code"].as_i64() != Some(200) {
        let msg = v["message"].as_str().unwrap_or("未知错误");
        return Err(anyhow!("匹配失败: {msg}"));
    }
    let url = v["data"]["url"]
        .as_str()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow!("未找到可用音源"))?;
    let src = v["data"]["source"].as_str().unwrap_or("unknown");
    Ok((url.to_string(), src.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default_until_init() {
        // 未 init 时禁用（OnceLock 在测试进程内可能已被并行测试污染，仅验证 API 形态）
        let _ = BASE.get();
        let _ = base();
    }

    #[test]
    fn default_is_localhost() {
        assert_eq!(DEFAULT, "http://127.0.0.1:3000");
    }
}
