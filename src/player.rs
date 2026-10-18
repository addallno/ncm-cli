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

/// 快捷下载请求计数（键盘接管键触发，guard_loop 消费）
pub static DOWNLOAD_REQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 快捷收藏请求计数（键盘接管键触发，guard_loop 消费）
pub static LIKE_REQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 终端 raw 模式守卫：播放期关掉内核回显与规范模式（字符不再被回显、无需等回车），
/// ISIG 一并关（Ctrl+C 作为字节交给键盘接管走优雅退出，Drop 恢复 termios）；
/// Drop（含 panic 路径）恢复原设置
#[cfg(unix)]
pub struct RawGuard {
    fd: i32,
    orig: libc::termios,
}

#[cfg(unix)]
impl RawGuard {
    /// stdin 非 TTY（管道/重定向）返回 None，此时不启用键盘接管
    pub fn enable() -> Option<Self> {
        unsafe {
            let fd = libc::STDIN_FILENO;
            if libc::isatty(fd) == 0 {
                return None;
            }
            let mut orig = std::mem::MaybeUninit::<libc::termios>::uninit();
            if libc::tcgetattr(fd, orig.as_mut_ptr()) != 0 {
                return None;
            }
            let orig = orig.assume_init();
            let mut raw = orig;
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
                return None;
            }
            Some(Self { fd, orig })
        }
    }
}

#[cfg(unix)]
impl Drop for RawGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.orig);
        }
    }
}

/// 播放期键盘接管：mpv 的 terminal input 在当前环境不初始化（不读 tty、不设 raw，
/// 键入字符仅被内核回显后遭歌词 overlay 擦除），故由 ncm 独占 stdin（raw+noecho），
/// 按键映射为 mpv IPC 命令；mpv 侧配 --input-terminal=no 禁自读，防恢复后双端抢键。
/// 调用前须先 RawGuard::enable（读到的字节是 raw 模式才不等回车、不回显）
#[cfg(unix)]
pub fn spawn_key_reader(
    sock: std::path::PathBuf,
    dl_key: String,
    like_key: String,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::AsyncReadExt;
    tokio::spawn(async move {
        let mut req_id: u64 = 50_000;
        let dl = dl_key.as_bytes().first().copied();
        let lk = like_key.as_bytes().first().copied();
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => return, // EOF/读错：stdin 断开，结束接管
                Ok(_) => {}
            }
            let b = buf[0];
            // ESC 序列（方向键 CSI/SS3、含修饰的变体）：100ms 窗口累积读到终结符
            // A/B/C/D 即映射 mpv 方向键名；超时/不匹配（单按 ESC 等）丢弃
            if b == 0x1b {
                let mut seq: Vec<u8> = Vec::new();
                let mut slot = 0u8;
                while seq.len() < 8 {
                    match tokio::time::timeout(
                        Duration::from_millis(100),
                        stdin.read(std::slice::from_mut(&mut slot)),
                    )
                    .await
                    {
                        Ok(Ok(1)) => {
                            seq.push(slot);
                            if matches!(slot, b'A' | b'B' | b'C' | b'D') {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                let key = match seq.last() {
                    Some(b'A') => "UP",
                    Some(b'B') => "DOWN",
                    Some(b'C') => "RIGHT",
                    Some(b'D') => "LEFT",
                    _ => continue,
                };
                let cmd = serde_json::json!({ "command": ["keypress", key] }).to_string();
                let _ = ipc_send(&sock, &cmd).await;
                continue;
            }
            match b {
                b' ' => {
                    req_id += 1;
                    let paused = ipc_get(&sock, "pause", req_id)
                        .await
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let nv = !paused;
                    let _ = ipc_send(
                        &sock,
                        &format!(r#"{{"command":["set_property","pause",{nv}]}}"#),
                    )
                    .await;
                    feedback(if nv { "▷ 已暂停" } else { "▶ 播放中" });
                }
                b'9' | b'0' => {
                    // 与 mpv 默认一致：9 减、0 加
                    let step = if b == b'9' { -2 } else { 2 };
                    if let Some(v) = ipc_add_volume(&sock, step).await {
                        feedback(format!("♪ 音量 {v:.0}%"));
                    }
                }
                0x03 | b'q' => {
                    // Ctrl+C（raw 下 ISIG 已关，字节直达）与 q：经 IPC 优雅退出，
                    // 主流程 wait 返回后 Drop 恢复 termios 并收尾；
                    // socket 未就绪（mpv 启动中）时重试 ~5s，防键被吞后退不出去
                    for _ in 0..20 {
                        if ipc_send(&sock, r#"{"command":["quit"]}"#).await.is_ok() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    return;
                }
                _ if dl.is_some_and(|k| k == b) => {
                    DOWNLOAD_REQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                _ if lk.is_some_and(|k| k == b) => {
                    LIKE_REQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                // 其余可打印 ASCII 键位语义交回 mpv 默认绑定（[ ] 变速、< > 切曲、
                // , . seek、m 静音、f 全屏、数字跳转等全可用）
                0x21..=0x7e => {
                    let ch = (b as char).to_string();
                    let cmd = serde_json::json!({ "command": ["keypress", ch] }).to_string();
                    let _ = ipc_send(&sock, &cmd).await;
                }
                0x7f => {
                    let _ = ipc_send(&sock, r#"{"command":["keypress","BS"]}"#).await;
                }
                _ => {} // \r\n 与高位 UTF-8 字节丢弃
            }
        }
    })
}

/// 生成 mpv IPC socket 路径（tmp 下，带 pid 避免冲突）
pub fn socket_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ncm-mpv-{}.sock", std::process::id()))
}

/// 删除可能残留的旧 socket 文件
pub fn cleanup_socket(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// 播放期提示：overlay 活着时走第二行 FEEDBACK（2s 自动消失不留痕，
/// 与按键反馈同位同风格）；overlay 停了才退回「清底行+滚动区打印」老路径
pub fn toast(msg: &str) {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return;
    }
    #[cfg(unix)]
    {
        if OVERLAY_ALIVE.load(std::sync::atomic::Ordering::Relaxed) {
            feedback(msg.to_string());
            return;
        }
    }
    let _g = canvas_lock();
    clear_overlay_rows_locked();
    eprintln!("{}", crate::ui::dim(msg));
    // 底行被清：标记重画，overlay 即刻恢复歌词（暂停时无 time-pos 事件也要恢复）
    CANVAS_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// 捕获 mpv/ffplay stderr：终端不再直接受其输出（底行歌词不会被插行打乱），
/// 过滤后把有用的行（错误/音频设备/解码等）以 toast 打出
pub fn spawn_output_filter<R>(output: R) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(output).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(msg) = useful_log(&line) {
                toast(&format!("× {msg}"));
            }
        }
    })
}

/// 剥离 ANSI 转义序列（mpv 可能带终端色，toast 统一重上灰色防串色）
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.clone().next() == Some('[') {
            it.next();
            for c2 in it.by_ref() {
                if c2.is_ascii_alphabetic() || c2 == '@' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 从播放器日志行提取值得展示的内容；噪声/正常信息返回 None
fn useful_log(line: &str) -> Option<String> {
    let cleaned = strip_ansi(line);
    let t = cleaned.trim();
    if t.is_empty() {
        return None;
    }
    // 我们自己的 IPC/memfd 杂散日志（高频且无意义）
    if t.contains("memfd") || t.contains("[ipc") || t.contains("ipc_") {
        return None;
    }
    // 正常播放信息（已有 ▶ 切曲行/队列表接管）；比较用 trim 后文本
    if t.starts_with("Playing:") || t.starts_with("(+)") || t.starts_with("AV:") {
        return None;
    }
    Some(t.to_string())
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

/// 漫游用：查询单个属性（按 request_id 匹配回应）；连接失败、超时或属性错误返回 None
/// 全程 2s 超时兜底：mpv 卡死/不回包时 guard 不能无限挂在 next_line 上
#[cfg(unix)]
pub async fn ipc_get(sock: &Path, prop: &str, req_id: u64) -> Option<Value> {
    let fut = async move {
        let mut s = UnixStream::connect(sock).await.ok()?;
        let cmd =
            format!("{{\"command\":[\"get_property\",\"{prop}\"],\"request_id\":{req_id}}}\n");
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
    };
    tokio::time::timeout(Duration::from_secs(2), fut)
        .await
        .ok()
        .flatten()
}

/// 同连接顺序 add volume 后读回显值：防两条独立连接交错时 get 读到加前的旧音量
#[cfg(unix)]
async fn ipc_add_volume(sock: &Path, step: i64) -> Option<f64> {
    use tokio::io::AsyncWriteExt as _;
    let fut = async move {
        let mut s = UnixStream::connect(sock).await.ok()?;
        s.write_all(
            format!("{{\"command\":[\"add\",\"volume\",{step}],\"request_id\":49998}}\n")
                .as_bytes(),
        )
        .await
        .ok()?;
        let mut lines = TokioBufReader::new(s).lines();
        for _ in 0..8 {
            let raw = lines.next_line().await.ok()??;
            let v: Value = serde_json::from_str(&raw).ok()?;
            if v["request_id"].as_u64() == Some(49998) {
                break;
            }
        }
        let mut s2 = lines.into_inner();
        s2.write_all(b"{\"command\":[\"get_property\",\"volume\"],\"request_id\":49999}\n")
            .await
            .ok()?;
        s2.flush().await.ok()?;
        let mut lines = TokioBufReader::new(s2).lines();
        for _ in 0..8 {
            let raw = lines.next_line().await.ok()??;
            let v: Value = serde_json::from_str(&raw).ok()?;
            if v["request_id"].as_u64() == Some(49999) {
                return (v["error"].as_str() == Some("success"))
                    .then(|| v["data"].as_f64())
                    .flatten();
            }
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(2), fut)
        .await
        .ok()
        .flatten()
}

#[cfg(unix)]
async fn overlay_loop(
    sock: &Path,
    tracks: &Arc<std::sync::RwLock<Vec<LyricTrack>>>,
) -> std::io::Result<()> {
    // overlay 生命周期标记：活着时 toast 走第二行 FEEDBACK，退出（含提前 Err）
    // 自动回落为滚动区打印，防提示发给已死的渲染循环而丢失
    struct AliveGuard;
    impl Drop for AliveGuard {
        fn drop(&mut self) {
            OVERLAY_ALIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    OVERLAY_ALIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    let _alive = AliveGuard;
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
    loop {
        // 400ms 轮询：toast 清底行后无事件（暂停/停秒）也要立即重画，防歌词滞留
        let next = tokio::time::timeout(Duration::from_millis(400), lines.next_line()).await;
        if CANVAS_DIRTY.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let ts = tracks.read().unwrap_or_else(|e| e.into_inner());
            let line = current_line(ts.as_slice(), cur_idx, cur_t);
            let disp = line.as_ref().map(|(m, s)| (m.as_str(), s.as_deref()));
            print_row(
                ts.as_slice(),
                cur_idx,
                disp,
                (cur_t > 0.0).then_some(cur_t),
                dur,
            );
            last_line = line;
            last_sec = (cur_t >= 0.0).then_some(cur_t as u64);
        }
        let raw = match next {
            Ok(Ok(Some(raw))) => raw,
            Ok(Ok(None)) | Ok(Err(_)) => break, // EOF / 读错误 → 结束
            Err(_) => continue,                 // 超时：只处理 dirty
        };
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

/// stderr 底行画布锁：overlay 刷新 / toast / 清行互斥，防交错写打乱双行状态
static CANVAS: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// 画布被 toast 清过：overlay 需立即重画，否则暂停/无事件时歌词滞留
static CANVAS_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// overlay_loop 是否在跑：跑着才走 FEEDBACK 第二行渲染，停了 fallback 滚动区打印
static OVERLAY_ALIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 持有画布锁（overlay print_row 与 toast 互斥写 stderr）
pub(crate) fn canvas_lock() -> std::sync::MutexGuard<'static, ()> {
    CANVAS.lock().unwrap_or_else(|e| e.into_inner())
}

/// 按键就地反馈槽（非 toast）：overlay 渲染时拼在歌词行末尾，2s TTL
#[cfg(unix)]
static FEEDBACK: std::sync::Mutex<Option<(String, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// 写入按键就地反馈并唤醒 overlay 重画；2s 后自动过期（置 dirty 触发重画清除）
#[cfg(unix)]
fn feedback(msg: impl Into<String>) {
    *FEEDBACK.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((msg.into(), std::time::Instant::now()));
    CANVAS_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        CANVAS_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
    });
}

/// 读取未过期的就地反馈（print_row 渲染拼接用）
#[cfg(unix)]
fn feedback_now() -> Option<String> {
    let g = FEEDBACK.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref()
        .filter(|(_, at)| at.elapsed() < Duration::from_secs(2))
        .map(|(m, _)| m.clone())
}

/// 清掉双行底行（line1 主词 + line2 译文），光标回到首行行首
///（切曲打歌名行、播放结束清理时调用）
pub fn clear_overlay_rows() {
    let _g = canvas_lock();
    clear_overlay_rows_locked();
}

/// 无锁版清理：调用方已持 CANVAS 时用（防 std Mutex 不可重入死锁）
fn clear_overlay_rows_locked() {
    use std::sync::atomic::Ordering::Relaxed;
    if ON_SECOND_LINE.swap(false, Relaxed) {
        eprint!("\r\x1b[2K\x1b[1A\r\x1b[2K");
    } else {
        eprint!("\r\x1b[2K");
    }
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

/// 在 stderr 双行刷新歌词底行：line1 = 前缀+时间进度+原文，line2 = 译文（淡蓝，与原文对齐）
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
    // 持画布锁：与 toast/清行互斥，防交错写打乱双行状态
    let _g = canvas_lock();
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
    // 按键就地反馈（音量/暂停）：第二行行首显示作 toast，2s 后自动消失；
    // 与译文同行共存（toast 左、译文仍在原文对齐列）
    let fb = feedback_now();
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
    // meta 为空（切曲首帧无时间）时补一个空格，防 prefix 与歌名黏连
    let gap = if meta.is_empty() {
        " ".to_string()
    } else {
        meta
    };
    out.push_str(&format!(
        "\r\x1b[2K\x1b[36m{prefix}\x1b[0m{gap}{clipped}\n\r\x1b[2K"
    ));
    // 第二行 = toast（黄字，行首）+ 对齐列起译文（淡蓝）；两者可共存
    let align = crate::ui::display_width(&prefix) + crate::ui::display_width(&gap);
    let line2 = match (fb, &sub_clipped) {
        (Some(f), Some(s)) => {
            // toast 太宽挤到译文时，译文紧跟其后空两格，防叠印
            let pad = align.saturating_sub(crate::ui::display_width(&f)).max(2);
            format!(
                "\x1b[33m{f}\x1b[0m{}{}",
                " ".repeat(pad),
                crate::ui::soft_blue(s)
            )
        }
        (Some(f), None) => format!("\x1b[33m{f}\x1b[0m"),
        (None, Some(s)) => {
            let pad = " ".repeat(align);
            crate::ui::soft_blue(&format!("{pad}{s}"))
        }
        (None, None) => String::new(),
    };
    out.push_str(&line2);
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
    use super::{current_line, useful_log, LyricTrack};

    // mpv stderr 过滤：噪声丢弃、错误/设备行保留
    #[test]
    fn useful_log_filters_noise_keeps_errors() {
        assert!(useful_log("[ao/alsa] device unavailable").is_some());
        assert!(useful_log("failed to open audio device").is_some());
        assert!(useful_log("").is_none());
        assert!(useful_log("  ").is_none());
        assert!(useful_log("memfd_create failed").is_none());
        assert!(useful_log("[ipc_1] Write error").is_none());
        assert!(useful_log("Playing: https://x/1.flac").is_none());
        assert!(useful_log(" (+) Audio --aid=1").is_none());
        assert_eq!(
            useful_log("  [ao/pulse] cannot open  ").as_deref(),
            Some("[ao/pulse] cannot open")
        );
    }

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
