//! 网络瞬时失败静默退避重试（DNS/连接/超时），HTTP 状态码等非网络错误不重试
use std::future::Future;
use std::time::Duration;

/// 重试间隔（秒），指数退避：1,2,4,8,16，最多 5 次
const BACKOFF: [u64; 5] = [1, 2, 4, 8, 16];

/// 是否可重试的网络瞬时错误：连接/超时/请求/传输错误，状态码错误（4xx/5xx）直接返回
pub fn transient(e: &reqwest::Error) -> bool {
    !e.is_status()
        && (e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() || e.is_decode())
}

/// 静默重试封装：失败且可重试则按 BACKOFF 退避后重试，全部耗尽或不可重试时返回最后一次错误
pub async fn retry_reqwest<T, F, Fut>(mut op: F) -> std::result::Result<T, reqwest::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, reqwest::Error>>,
{
    let mut i = 0;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if i >= BACKOFF.len() || !transient(&e) {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_secs(BACKOFF[i])).await;
                i += 1;
            }
        }
    }
}
