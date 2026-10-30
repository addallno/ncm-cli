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

/// 默认候选地址（逗号分隔，unm 常见部署端口：npx 默认 3000，本项目 Termux 脚本 3100）
pub const DEFAULT: &str = "http://127.0.0.1:3000,http://127.0.0.1:3100";

/// 每次匹配请求的超时（unm 冷启动慢时跳过 mirror 走第三方兜底；
/// 30s 会让开播取址整体拖到数十秒，10s 足够热启动的本地服务）
const MATCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 候选地址列表（空 = 禁用）；`--mirror`/`NCM_MIRROR` 支持逗号分隔多个
static BASE: OnceLock<Vec<String>> = OnceLock::new();
static CLIENT: OnceLock<Client> = OnceLock::new();

/// 启动时初始化（main 调用一次）。
/// 优先级：`--mirror` > `NCM_MIRROR` > `config mirror` 键 > 默认 localhost:3000,3100；
/// 值为 `none`（不区分大小写）时禁用；逗号分隔多个候选地址依次尝试。
pub fn init(cli: Option<&str>) {
    let raw = cli
        .map(str::to_string)
        .or_else(|| std::env::var("NCM_MIRROR").ok())
        .or_else(crate::config::mirror)
        .unwrap_or_else(|| DEFAULT.to_string());
    let val = raw.trim();
    let bases: Vec<String> = if val.eq_ignore_ascii_case("none") || val.is_empty() {
        Vec::new()
    } else {
        val.split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let _ = BASE.set(bases);
}

/// 是否启用兜底（未初始化时视为未启用）
pub fn enabled() -> bool {
    matches!(BASE.get(), Some(v) if !v.is_empty())
}

/// `GET {base}/match?id=` → (音频直链, 音源标识)
pub async fn match_url(id: u64) -> Result<(String, String)> {
    let bases = match BASE.get() {
        Some(v) if !v.is_empty() => v,
        _ => return Err(anyhow!("未配置解锁服务")),
    };
    let client = CLIENT.get_or_init(|| {
        Client::builder()
            .no_proxy()
            .user_agent("ncm-rs")
            .build()
            .unwrap_or_default()
    });
    // 依次尝试各候选地址，全部失败时聚合错误便于诊断
    let mut errs: Vec<String> = Vec::new();
    for base in bases {
        match try_match(client, base, id).await {
            Ok(v) => return Ok(v),
            Err(e) => errs.push(format!("{base}: {e}")),
        }
    }
    Err(anyhow!("{}", errs.join("；")))
}

/// 对单个候选地址发起 /match
async fn try_match(client: &Client, base: &str, id: u64) -> Result<(String, String)> {
    let resp = crate::retry::retry_reqwest(|| {
        client
            .get(format!("{base}/match"))
            .query(&[("id", id.to_string())])
            .timeout(MATCH_TIMEOUT)
            .send()
    })
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
        let _ = enabled();
    }

    #[test]
    fn default_is_multi_candidate() {
        assert_eq!(DEFAULT, "http://127.0.0.1:3000,http://127.0.0.1:3100");
    }
}
