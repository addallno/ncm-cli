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
    tracks: Arc<std::sync::RwLock<Vec<LyricTrack>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use std::io::IsTerminal;
        let tty = std::io::stderr().is_terminal();
        if let Err(e) = overlay_loop(&sock, &tracks).await {
            // mpv 不在或提前退出属正常收尾，静默；其余打一行提示
            if e.kind() != std::io::ErrorKind::NotFound {
                if tty {
                    eprintln!("\r\x1b[2K(歌词同步: {e})");
                } else {
                    eprintln!("(歌词同步: {e})");
                }
            }
        } else if tty {
            eprintln!("\r\x1b[2K");
        }
    })
}

#[cfg(not(unix))]
pub fn spawn_overlay(
    _sock: std::path::PathBuf,
    _tracks: Arc<std::sync::RwLock<Vec<LyricTrack>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {})
}

/// 漫游用：连接 mpv IPC 发送命令（不读回应，适合 loadfile append）
#[cfg(unix)]
pub async fn ipc_send(sock: &Path, cmd: &str) -> std::io::Result<()> {
    let mut s = UnixStream::connect(sock).await?;
    s.write_all(cmd.as_bytes()).await?;
    s.write_all(b"\n").await?;
    s.flush().await?;
    Ok(())
}

/// 漫游用：查询单个属性（按 request_id 匹配回应）；连接失败或属性错误返回 None
#[cfg(unix)]
pub async fn ipc_get(sock: &Path, prop: &str, req_id: u64) -> Option<Value> {
    let mut s = UnixStream::connect(sock).await.ok()?;
    let cmd = format!("{{\"command\":[\"get_property\",\"{prop}\"],\"request_id\":{req_id}}}\n");
    s.write_all(cmd.as_bytes()).await.ok()?;
    s.flush().await.ok()?;
    let mut lines = TokioBufReader::new(s).lines();
    // mpv 可能先回无关行，最多读 8 条找匹配的 request_id
    for _ in 0..8 {
        let raw = lines.next_line().await.ok()??;
        let v: Value = serde_json::from_str(&raw).ok()?;
        if v["request_id"].as_u64() == Some(req_id) {
            return (v["error"].as_str() == Some("success")).then(|| v["data"].clone());
        }
    }
    None
}

#[cfg(unix)]
async fn overlay_loop(
    sock: &Path,
    tracks: &Arc<std::sync::RwLock<Vec<LyricTrack>>>,
) -> std::io::Result<()> {
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
    for (id, prop) in [(1, "time-pos"), (2, "playlist-pos"), (3, "duration")] {
        let cmd = format!("{{\"command\":[\"observe_property\",{id},\"{prop}\"]}}\n");
        write.write_all(cmd.as_bytes()).await?;
    }
    write.flush().await?;

    let mut cur_idx: usize = 0;
    let mut last_line: Option<(String, Option<String>)> = None;
    let mut last_sec: Option<u64> = None;
    let mut cur_t: f64 = 0.0;
    let mut dur: Option<f64> = None;
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
                        last_sec = None;
                        cur_t = 0.0;
                        dur = None;
                        let ts = tracks.read().unwrap_or_else(|e| e.into_inner());
                        // 切曲往滚动历史打一行歌名（先清底行，行独立可回翻）
                        if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                            let label = ts
                                .get(cur_idx)
                                .map(|t| t.label.as_str())
                                .unwrap_or_default();
                            clear_overlay_rows(); // 清两行底行，歌名行进滚动历史
                            eprintln!(
                                "{}",
                                crate::ui::dim(&format!(
                                    "▶ [{}/{}] {}",
                                    cur_idx + 1,
                                    ts.len(),
                                    label
                                ))
                            );
                        }
                        print_row(ts.as_slice(), cur_idx, None, None, None);
                    }
                }
                Some("time-pos") => {
                    if let Some(t) = ev["data"].as_f64() {
                        cur_t = t;
                        let ts = tracks.read().unwrap_or_else(|e| e.into_inner());
                        let line = current_line(ts.as_slice(), cur_idx, t);
                        let sec = t as u64;
                        if line != last_line || last_sec != Some(sec) {
                            last_line = line.clone();
                            last_sec = Some(sec);
                            let disp = line
                                .as_ref()
                                .map(|(main, sub)| (main.as_str(), sub.as_deref()));
                            print_row(ts.as_slice(), cur_idx, disp, Some(t), dur);
                        }
                    }
                }
                Some("duration") => {
                    dur = ev["data"].as_f64().filter(|d| *d > 0.0);
                    let ts = tracks.read().unwrap_or_else(|e| e.into_inner());
                    print_row(
                        ts.as_slice(),
                        cur_idx,
                        last_line
                            .as_ref()
                            .map(|(main, sub)| (main.as_str(), sub.as_deref())),
                        (cur_t > 0.0).then_some(cur_t),
                        dur,
                    );
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
fn current_line(tracks: &[LyricTrack], idx: usize, t: f64) -> Option<(String, Option<String>)> {
    let track = tracks.get(idx)?;
    if track.lines.is_empty() {
        return Some((track.label.clone(), None)); // 无歌词 → 歌名占位
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
        return Some((track.label.clone(), None)); // 前奏段
    }
    let i = lo - 1;
    let (ts, text) = &track.lines[i];
    // 停留超过 30s 视为该行已过期（长间隔），回退歌名占位
    if t - ts > 30.0 {
        return Some((track.label.clone(), None));
    }
    // 网易 lrc 原文/译文同刻成对（稳定排序后原文在前）：前条=原文，当前=译文
    if i >= 1 && track.lines[i - 1].0 == track.lines[i].0 {
        Some((track.lines[i - 1].1.clone(), Some(text.clone())))
    } else {
        Some((text.clone(), None))
    }
}

/// 底行是否已写到第二行（译文行）——刷新回移与清理用
static ON_SECOND_LINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 清掉双行底行（line1 主词 + line2 译文），光标回到首行行首
///（切曲打歌名行、播放结束清理时调用）
pub fn clear_overlay_rows() {
    use std::sync::atomic::Ordering::Relaxed;
    if ON_SECOND_LINE.swap(false, Relaxed) {
        eprint!("\r\x1b[2K\x1b[1A\r\x1b[2K");
    } else {
        eprint!("\r\x1b[2K");
    }
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

/// 在 stderr 双行刷新歌词底行：line1 = 前缀+时间进度+原文，line2 = 译文（dim）
#[cfg(unix)]
fn print_row(
    tracks: &[LyricTrack],
    idx: usize,
    text: Option<(&str, Option<&str>)>,
    t: Option<f64>,
    dur: Option<f64>,
) {
    use std::io::IsTerminal;
    // 非 TTY（管道/重定向）静默，避免裸 ANSI 序列污染输出
    if !std::io::stderr().is_terminal() {
        return;
    }
    let total = tracks.len();
    let name = tracks.get(idx).map(|t| t.label.as_str()).unwrap_or("");
    let (main_body, sub_body) = text.unwrap_or((name, None));
    let prefix = format!("♪ [{}/{}]", idx + 1, total);
    // 时间与进度条：mm:ss/mm:ss [bar]
    let meta = match (t, dur) {
        (Some(tt), Some(d)) if d > 0.0 => {
            if tt <= d {
                crate::ui::dim(&format!(
                    " {} / {} {} ",
                    fmt_t(tt),
                    fmt_t(d),
                    crate::ui::render_bar(tt / d, 8)
                ))
            } else {
                crate::ui::dim(&format!(" {} / {} ", fmt_t(tt), fmt_t(d)))
            }
        }
        (Some(tt), _) => crate::ui::dim(&format!(" {} ", fmt_t(tt))),
        _ => String::new(),
    };
    // 按 stderr 实际终端宽度算预算（写死 60 会在宽屏截太短、窄屏换行叠影）
    let term_w = terminal_size::terminal_size_of(std::io::stderr())
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);
    let budget = term_w
        .saturating_sub(crate::ui::display_width(&prefix))
        .saturating_sub(crate::ui::display_width(&meta))
        .saturating_sub(1);
    let clipped = crate::ui::truncate_width(main_body, budget);
    let sub_clipped = sub_body.map(|s| crate::ui::truncate_width(s, budget));
    use std::sync::atomic::Ordering::Relaxed;
    let mut out = String::new();
    if ON_SECOND_LINE.load(Relaxed) {
        out.push_str("\x1b[1A\r\x1b[2K"); // 上移回 line1
    }
    out.push_str(&format!(
        "\r\x1b[2K\x1b[36m{prefix}\x1b[0m{meta}{clipped}\n\r\x1b[2K"
    ));
    if let Some(s) = &sub_clipped {
        out.push_str(&crate::ui::dim(s));
    }
    ON_SECOND_LINE.store(true, Relaxed);
    eprint!("{out}");
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

/// 秒 → mm:ss
#[cfg(unix)]
fn fmt_t(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{current_line, LyricTrack};

    fn track(lines: Vec<(f64, String)>) -> LyricTrack {
        LyricTrack {
            label: "占位歌名".into(),
            lines,
        }
    }

    // 网易 lrc 原文/译文同刻成对：主行=原文、副行=译文（分行显示的依据）
    #[test]
    fn paired_lines_split_main_and_translation() {
        let t = track(vec![
            (10.0, "原文行".into()),
            (10.0, "译文行".into()),
            (20.0, "下一句".into()),
        ]);
        let (main, sub) = current_line(std::slice::from_ref(&t), 0, 10.5).unwrap();
        assert_eq!(main, "原文行");
        assert_eq!(sub.as_deref(), Some("译文行"));
        let (main2, sub2) = current_line(std::slice::from_ref(&t), 0, 21.0).unwrap();
        assert_eq!(main2, "下一句");
        assert!(sub2.is_none());
    }

    // 无歌词/前奏段回退歌名占位，不带副行
    #[test]
    fn label_fallback_has_no_sub() {
        let t = track(vec![(30.0, "第一句".into())]);
        let (main, sub) = current_line(std::slice::from_ref(&t), 0, 5.0).unwrap();
        assert_eq!(main, "占位歌名");
        assert!(sub.is_none());
    }
}
