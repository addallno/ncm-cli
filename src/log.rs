//! 运行日志 `ncm.run.log`：事件驱动，为大规模播放测试（100-500 首）定位问题而设计。
//!
//! - 位置：`~/.config/ncm/ncm.run.log`（与凭据/配置同目录），启动时 >5MB 归档 `.1`
//! - 格式：`2026-11-24T12:34:56.789Z [INFO ] [tag] msg`（UTC 毫秒，便于跨会话对时）
//! - 只记事件（toast/对账/IPC/下载/nav/启动参数），不记逐轮心跳，防爆量
//! - std `Mutex<File>` 直写、无新 crate；任何失败静默降级——日志永不拖垮主流程

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static SINK: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// 终端镜像开关（-v/--verbose）：默认关——日志只落盘、正常输出静默；
/// 开启后每行 stderr 镜像，且 init 打印日志产出位置（except -v，m0116）
static CONSOLE: AtomicBool = AtomicBool::new(false);

pub fn set_console(v: bool) {
    CONSOLE.store(v, Ordering::Relaxed);
}

fn console() -> bool {
    CONSOLE.load(Ordering::Relaxed)
}

/// 行格式（纯函数，单测覆盖）：`1970-01-01T00:00:00.000Z [INFO ] [tag] msg`
fn fmt_line(epoch_ms: i64, level: &str, tag: &str, msg: &str) -> String {
    let secs = epoch_ms.div_euclid(1000);
    let ms = epoch_ms.rem_euclid(1000);
    let date = crate::fmt_unix_date(secs);
    let tod = secs.rem_euclid(86_400);
    format!(
        "{date}T{:02}:{:02}:{:02}.{ms:03}Z [{level:<5}] [{tag}] {msg}",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    )
}

/// 启动初始化：打开日志（超限归档上一份）并写启动头；任何一步失败即放弃（静默降级）
pub fn init() {
    let dir = crate::session::config_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("ncm.run.log");
    if let Ok(md) = std::fs::metadata(&path) {
        if md.len() > MAX_BYTES {
            let bak = dir.join("ncm.run.log.1");
            let _ = std::fs::remove_file(&bak);
            let _ = std::fs::rename(&path, &bak);
        }
    }
    let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = SINK.set(Mutex::new(f));
    // 日志产出位置只在 -v 时打印到终端（m0116：正常输出保持静默）
    if console() {
        eprintln!("  日志 → {}", path.display());
    }
    info(
        "log",
        &format!(
            "=== ncm 启动 · pid {} · {} ===",
            std::process::id(),
            path.display()
        ),
    );
}

/// INFO：事件节点（toast、对账动作、nav 切曲、启动参数、下载收据）
pub fn info(tag: &str, msg: &str) {
    write_line("INFO", tag, msg);
}

/// WARN：失败可自愈/需留意（修复失败、IPC 抖动、mpv stderr 提示行、下载跳过）
pub fn warn(tag: &str, msg: &str) {
    write_line("WARN", tag, msg);
}

/// ERROR：硬失败（下载失败、退出异常）
pub fn error(tag: &str, msg: &str) {
    write_line("ERROR", tag, msg);
}

fn write_line(level: &str, tag: &str, msg: &str) {
    let Some(m) = SINK.get() else {
        return; // 未初始化（或初始化失败）：静默丢弃
    };
    let Ok(mut f) = m.lock() else {
        return; // 锁中毒：不重用，避免 panic 传染
    };
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let line = fmt_line(ms, level, tag, msg);
    let _ = writeln!(f, "{line}");
    if console() {
        eprintln!("{line}"); // -v：stderr 镜像（fqdt core_log::set_console 先例）
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 行格式：UTC 毫秒时间戳 + 级别定宽补位 + tag/msg
    #[test]
    fn fmt_line_shape() {
        let l = fmt_line(0, "INFO", "toast", "队列已修复");
        assert_eq!(l, "1970-01-01T00:00:00.000Z [INFO ] [toast] 队列已修复");
        let l = fmt_line(1_763_965_496_789, "ERROR", "mpv", "insert-at rejected");
        assert_eq!(
            l,
            "2025-11-24T06:24:56.789Z [ERROR] [mpv] insert-at rejected"
        );
        let l = fmt_line(-1, "WARN", "ipc", "timeout"); // 负 epoch：div_euclid 落 1969-12-31
        assert_eq!(l, "1969-12-31T23:59:59.999Z [WARN ] [ipc] timeout");
    }
}
