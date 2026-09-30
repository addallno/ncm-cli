//! 播放器交互：mpv IPC 歌词同步（unix socket JSON-RPC）
//!
//! mpv 以 `--input-ipc-server=<sock>` 启动后，本模块连接 socket、
//! observe `time-pos` / `playlist-pos`，在 stderr 底行滚动显示当前歌词。
//! 右侧/seek 时 mpv 推送 time-pos 变化，歌词自然跟随。
#[cfg(unix)]
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
#[cfg(unix)]
use tokio::net::UnixStream;

/// 一首曲目的歌词集（按时间升序）
pub struct LyricTrack {
    pub label: String,
    pub lines: Vec<(f64, String)>,
}

/// 生成 mpv IPC socket 路径（tmp 下，带 pid 避免冲突）
pub fn socket_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ncm-mpv-{}.sock", std::process::id()))
}

/// 删除可能残留的旧 socket 文件
pub fn cleanup_socket(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// 启动歌词同步任务；mpv 退出（socket EOF）后自动结束并换行收尾
///
/// `tracks[i]` 对应 mpv 播放列表第 i 项；超出范围的 playlist-pos 显示歌名占位。
/// Windows 无 unix domain socket，为空实现（调用方已用 `cfg!(unix)` 门控）。
#[cfg(unix)]
pub fn spawn_overlay(
    sock: std::path::PathBuf,
    tracks: Arc<Vec<LyricTrack>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = overlay_loop(&sock, tracks.as_ref()).await {
            // mpv 不在或提前退出属正常收尾，静默；其余打一行提示
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("\r\x1b[2K(歌词同步: {e})");
            }
        } else {
            eprintln!("\r\x1b[2K");
        }
    })
}

#[cfg(not(unix))]
pub fn spawn_overlay(
    _sock: std::path::PathBuf,
    _tracks: Arc<Vec<LyricTrack>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {})
}

#[cfg(unix)]
async fn overlay_loop(sock: &Path, tracks: &[LyricTrack]) -> std::io::Result<()> {
    // mpv 启动后才创建 socket，重试等待（最多 ~5s）
    let stream = {
        let mut last = None;
        let mut ok = None;
        for _ in 0..50 {
            match UnixStream::connect(sock).await {
                Ok(s) => {
                    ok = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        match ok {
            Some(s) => s,
            None => {
                return Err(last.unwrap_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "连接 mpv IPC 失败")
                }))
            }
        }
    };
    let (read, mut write) = stream.into_split();
    for (id, prop) in [(1, "time-pos"), (2, "playlist-pos")] {
        let cmd = format!("{{\"command\":[\"observe_property\",{id},\"{prop}\"]}}\n");
        write.write_all(cmd.as_bytes()).await?;
    }
    write.flush().await?;

    let mut cur_idx: usize = 0;
    let mut last_line: Option<String> = None;
    let mut lines = TokioBufReader::new(read).lines();
    while let Ok(Some(raw)) = lines.next_line().await {
        let Ok(ev) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if ev["event"] == "property-change" {
            match ev["name"].as_str() {
                Some("playlist-pos") => {
                    if let Some(i) = ev["data"].as_u64() {
                        cur_idx = i as usize;
                        last_line = None; // 切曲后重置，等新 time-pos
                        print_row(tracks, cur_idx, None);
                    }
                }
                Some("time-pos") => {
                    if let Some(t) = ev["data"].as_f64() {
                        let line = current_line(tracks, cur_idx, t);
                        if line != last_line {
                            last_line = line.clone();
                            print_row(tracks, cur_idx, line.as_deref());
                        }
                    }
                }
                _ => {}
            }
        } else if ev["event"] == "shutdown" {
            // end-file 在播放列表每首结束时也触发，不退出——切曲由 playlist-pos 驱动
            break;
        }
    }
    Ok(())
}

/// 二分查找当前时间对应的歌词行；无命中返回 None
#[cfg(unix)]
fn current_line(tracks: &[LyricTrack], idx: usize, t: f64) -> Option<String> {
    let track = tracks.get(idx)?;
    if track.lines.is_empty() {
        return Some(track.label.clone()); // 无歌词 → 显示歌名占位
    }
    // 找到最后一个 t_line <= t 的行（容忍 seek 回退）
    let mut lo = 0usize;
    let mut hi = track.lines.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if track.lines[mid].0 <= t {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 && track.lines[0].0 > t {
        return Some(track.label.clone()); // 前奏段
    }
    let (ts, text) = &track.lines[lo.saturating_sub(1)];
    // 停留超过 30s 视为该行已过期（长间隔），回退歌名占位
    if t - ts > 30.0 {
        return Some(track.label.clone());
    }
    Some(text.clone())
}

/// 在 stderr 单行刷新歌词（覆盖行内旧内容）
#[cfg(unix)]
fn print_row(tracks: &[LyricTrack], idx: usize, text: Option<&str>) {
    let total = tracks.len();
    let name = tracks.get(idx).map(|t| t.label.as_str()).unwrap_or("");
    let body = text.unwrap_or(name);
    // ANSI 宽度裁剪：中日韩宽字符按 2 列估
    let mut width = 0usize;
    let mut clipped = String::new();
    for ch in body.chars() {
        let w = if ch as u32 > 0x1100 { 2 } else { 1 };
        if width + w > 60 {
            break;
        }
        width += w;
        clipped.push(ch);
    }
    eprint!(
        "\r\x1b[2K\x1b[36m♪ [{}/{}]\x1b[0m {clipped}",
        idx + 1,
        total
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());
}
