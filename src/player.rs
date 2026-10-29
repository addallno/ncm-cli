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
    /// 曲 id（切曲行查红心集合用）
    pub id: u64,
    /// 喜欢人数（每日推荐/私人FM 自带；歌单来源切曲时经 song_red_count 补取）
    /// 喜欢人数；None=未知（FM/日推来源数据自带 Some，歌单等切曲补取后置 Some 含 0）
    pub like_count: Option<u64>,
    /// like_count 是否已尝试补取且成功（失败不置位：下次切回该曲自动重试）
    pub red_fetched: bool,
    /// 评论总数；None=未知（play.show-comments 开启时切曲补取）
    pub comment_count: Option<u64>,
    /// comment_count 是否已尝试补取且成功（失败不置位，同 red_fetched）
    pub cmt_fetched: bool,
}

/// 播放会话共享红心状态：已收藏曲 id 集合 + 红心歌单 id。
/// play 启动后异步加载（whoami→红心歌单→trackIds 一次拉全量），
/// 切曲行查表显示 ♥（零额外请求）；l 键预判已收藏、收藏成功后就地维护
pub struct LikedCtx {
    pub ids: std::collections::HashSet<u64>,
    /// 红心歌单 id（收藏/取消操作复用，免每次 whoami+user_playlists）
    pub red_pid: Option<u64>,
}
pub type SharedLiked = std::sync::Arc<std::sync::RwLock<LikedCtx>>;

/// 切曲行显示配置：符号与 ♥ 门控（main 读 config 后传入，player 不依赖 config 模块）
pub struct DisplayCfg {
    /// 「我已收藏」标记符号（默认 ♥）
    pub heart_sym: String,
    /// 喜欢人数前缀符号（默认 *）
    pub heat_sym: String,
    /// 是否显示「我已收藏」（play.show-liked 且非红心歌单）
    pub show_heart: bool,
    /// 状态栏是否显示评论总数（play.show-comments，默认关）
    pub show_comments: bool,
}

/// 快捷请求类型（键盘接管键触发）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quick {
    Download,
    Like,
    Unlike,
}

/// 快捷请求队列：(类型, 曲 id, 标签)。
/// 按键瞬间经 IPC 锁定当时正在播放的曲目——若只记"按键计数"等 guard 轮询
/// 再取 playlist-pos，2 秒窗口内切曲会把别的歌（如下一首）下载/收藏错
pub static QUICK_REQS: std::sync::Mutex<Vec<(Quick, u64, String)>> =
    std::sync::Mutex::new(Vec::new());

/// 用户经 q/Ctrl+C 主动退出播放：置位后 mpv 退出不再自动重试（异常退出才重试）
pub static USER_QUIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    unlike_key: String,
    queue: std::sync::Arc<std::sync::RwLock<Vec<(u64, String, u64)>>>,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::AsyncReadExt;
    tokio::spawn(async move {
        let mut req_id: u64 = 50_000;
        let dl = dl_key.as_bytes().first().copied();
        let lk = like_key.as_bytes().first().copied();
        let ul = unlike_key.as_bytes().first().copied();
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
                    USER_QUIT.store(true, std::sync::atomic::Ordering::Relaxed);
                    for _ in 0..20 {
                        if ipc_send(&sock, r#"{"command":["quit"]}"#).await.is_ok() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    return;
                }
                _ if dl.is_some_and(|k| k == b)
                    || lk.is_some_and(|k| k == b)
                    || ul.is_some_and(|k| k == b) =>
                {
                    // 按下瞬间取 playlist-pos 锁定曲目（防后续切曲错位）
                    // 同键配置多动作时优先级 d > l > u（与 download/like 既有行为一致）
                    let kind = if dl.is_some_and(|k| k == b) {
                        Quick::Download
                    } else if lk.is_some_and(|k| k == b) {
                        Quick::Like
                    } else {
                        Quick::Unlike
                    };
                    req_id += 1;
                    let locked = async {
                        let pos = ipc_get(&sock, "playlist-pos", req_id)
                            .await
                            .and_then(|v| v.as_i64())?;
                        let (id, label, _) = queue
                            .read()
                            .unwrap_or_else(|e| e.into_inner())
                            .get(pos as usize)
                            .cloned()?;
                        Some((id, label))
                    }
                    .await;
                    match locked {
                        Some((id, label)) => {
                            QUICK_REQS
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push((kind, id, label));
                        }
                        // 空闲(pos=-1)/IPC 失败/越界：明确提示，不再静默吞键
                        None => feedback("× 无法锁定当前曲目"),
                    }
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
    liked: SharedLiked,
    ncm: crate::api::Ncm,
    disp: DisplayCfg,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use std::io::IsTerminal;
        let tty = std::io::stderr().is_terminal();
        if let Err(e) = overlay_loop(&sock, &tracks, &liked, &ncm, &disp).await {
            // mpv 不在或提前退出属正常收尾，静默；其余清完底行再打一行提示
            if e.kind() != std::io::ErrorKind::NotFound {
                if tty {
                    clear_overlay_rows();
                    eprintln!("(歌词同步: {e})");
                } else {
                    eprintln!("(歌词同步: {e})");
                }
            }
        } else if tty {
            clear_overlay_rows();
        }
    })
}

#[cfg(not(unix))]
pub fn spawn_overlay(
    _sock: std::path::PathBuf,
    _tracks: Arc<std::sync::RwLock<Vec<LyricTrack>>>,
    _liked: SharedLiked,
    _ncm: crate::api::Ncm,
    _disp: DisplayCfg,
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
    liked: &SharedLiked,
    ncm: &crate::api::Ncm,
    disp: &DisplayCfg,
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
            let ltxt = line.as_ref().map(|(m, s)| (m.as_str(), s.as_deref()));
            print_row(
                ts.as_slice(),
                cur_idx,
                ltxt,
                (cur_t > 0.0).then_some(cur_t),
                dur,
                liked,
                disp,
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
                        // 歌单/搜索来源 Track 无 likeCount/评论数：切曲时并行补取
                        //（各自 1s 超时；成功才置 fetched，失败下次切回自动重试；
                        // FM/日推自带值或已取到则跳过，避免重复请求）
                        {
                            let (tid, need_red, need_cmt) = {
                                let ts = tracks.read().unwrap_or_else(|e| e.into_inner());
                                match ts.get(cur_idx) {
                                    Some(t) => (
                                        t.id,
                                        t.like_count.is_none() && !t.red_fetched,
                                        disp.show_comments
                                            && t.comment_count.is_none()
                                            && !t.cmt_fetched,
                                    ),
                                    None => (0, false, false),
                                }
                            };
                            if need_red || need_cmt {
                                // 并行发两个请求，任一失败只影响自己的字段
                                let red_fut = async {
                                    if need_red {
                                        tokio::time::timeout(
                                            Duration::from_secs(1),
                                            ncm.song_red_count(tid),
                                        )
                                        .await
                                        .ok()
                                        .and_then(Result::ok)
                                    } else {
                                        None
                                    }
                                };
                                let cmt_fut = async {
                                    if need_cmt {
                                        tokio::time::timeout(
                                            Duration::from_secs(1),
                                            ncm.song_comments(tid, 1),
                                        )
                                        .await
                                        .ok()
                                        .and_then(Result::ok)
                                        .map(|(_, total)| total)
                                    } else {
                                        None
                                    }
                                };
                                let (red, cmt) = tokio::join!(red_fut, cmt_fut);
                                let mut ts = tracks.write().unwrap_or_else(|e| e.into_inner());
                                if let Some(t) = ts.get_mut(cur_idx) {
                                    if t.id != tid {
                                        // 读写锁间隙已切走，丢弃过期结果
                                    } else {
                                        // Some(0) 也写入：取到过就显示 *0，便于识别接口失败
                                        if let Some(n) = red {
                                            t.like_count = Some(n);
                                            t.red_fetched = true;
                                        }
                                        if let Some(n) = cmt {
                                            t.comment_count = Some(n);
                                            t.cmt_fetched = true;
                                        }
                                    }
                                }
                            }
                        }
                        let ts = tracks.read().unwrap_or_else(|e| e.into_inner());
                        // 切曲往滚动历史打一行歌名（先清底行，行独立可回翻）；
                        // 收藏/热度已上移状态栏（行1），不再拼在歌名后
                        if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                            let label = ts.get(cur_idx).map(|t| t.label.as_str()).unwrap_or("");
                            clear_overlay_rows(); // 清底行，歌名行进滚动历史
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
                        print_row(ts.as_slice(), cur_idx, None, None, None, liked, disp);
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
                            let ltxt = line
                                .as_ref()
                                .map(|(main, sub)| (main.as_str(), sub.as_deref()));
                            print_row(ts.as_slice(), cur_idx, ltxt, Some(t), dur, liked, disp);
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
                        liked,
                        disp,
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

/// 底行已占行数（0=未画，2=状态栏+歌词，3=含译文）——刷新回移与清理按深度处理
static OVERLAY_ROWS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

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

/// 清掉全部底行（状态栏+歌词[+译文]），光标回到首行行首
///（切曲打歌名行、播放结束清理时调用）
pub fn clear_overlay_rows() {
    let _g = canvas_lock();
    clear_overlay_rows_locked();
}

/// 无锁版清理：调用方已持 CANVAS 时用（防 std Mutex 不可重入死锁）
fn clear_overlay_rows_locked() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = OVERLAY_ROWS.swap(0, Relaxed);
    if n == 0 {
        eprint!("\r\x1b[2K");
    } else {
        // 自最底行逐行上移清到首行（光标当前在最底行行尾）
        for k in 0..n {
            eprint!("\r\x1b[2K");
            if k + 1 < n {
                eprint!("\x1b[1A");
            }
        }
    }
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

/// 外部状态变化（红心集合加载完成等）后唤醒 overlay 重画；
/// overlay 400ms 轮询消费，暂停无事件时也能刷新
pub fn mark_dirty() {
    CANVAS_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// 在 stderr 刷新播放底行（2~3 行，按 OVERLAY_ROWS 深度回移重画）：
/// 行1 状态栏 = 前缀+时间进度+♥/*N 收藏热度 + 右侧空一格接 toast（2s 反馈）
/// 行2 歌词原文（缩进 2 空格），行3 译文（淡蓝，与原文同列缩进，仅在有译文时占行）
#[cfg(unix)]
fn print_row(
    tracks: &[LyricTrack],
    idx: usize,
    text: Option<(&str, Option<&str>)>,
    t: Option<f64>,
    dur: Option<f64>,
    liked: &SharedLiked,
    disp: &DisplayCfg,
) {
    use std::io::IsTerminal;
    // 非 TTY（管道/重定向）静默，避免裸 ANSI 序列污染输出
    if !std::io::stderr().is_terminal() {
        return;
    }
    // 持画布锁：与 toast/清行互斥，防交错写打乱多行状态
    let _g = canvas_lock();
    let total = tracks.len();
    let track = tracks.get(idx);
    let name = track.map(|t| t.label.as_str()).unwrap_or("");
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
    // 状态栏收藏/热度段：♥ 已收藏、\ 未收藏等宽占位（show_heart 门控）；
    // *N 喜欢人数（>0 才显示，soft_blue，≥1万 显示 x.x万）
    let mut status = String::new();
    if let Some(tr) = track {
        if disp.show_heart {
            let lk = liked.read().unwrap_or_else(|e| e.into_inner());
            if lk.ids.contains(&tr.id) {
                status.push_str(&disp.heart_sym);
            } else {
                status.push('\\'); // 与 ♥ 同宽占位，状态栏列位不跳
            }
        }
        if let Some(lc) = tr.like_count {
            let n = if lc >= 10_000 {
                format!("{:.1}万", lc as f64 / 10_000.0)
            } else {
                lc.to_string()
            };
            if !status.is_empty() {
                status.push(' ');
            }
            status.push_str(&crate::ui::soft_blue(&format!("{}{n}", disp.heat_sym)));
        }
        // 评论总数（play.show-comments 门控；未开启或未取到则不显示）
        if disp.show_comments {
            if let Some(cc) = tr.comment_count {
                let n = if cc >= 10_000 {
                    format!("{:.1}万", cc as f64 / 10_000.0)
                } else {
                    cc.to_string()
                };
                if !status.is_empty() {
                    status.push(' ');
                }
                status.push_str(&crate::ui::dim(&format!("评{n}")));
            }
        }
    }
    // 按键就地反馈（音量/暂停）：状态栏右侧空一格拼接，2s 后自动消失
    let fb = feedback_now();
    // 按 stderr 实际终端宽度算预算（写死 60 会在宽屏截太短、窄屏换行叠影）
    let term_w = terminal_size::terminal_size_of(std::io::stderr())
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);
    // 行1 状态栏：prefix + gap + 状态段 + toast，超宽右截（toast 先被截，状态段保左）
    let gap = if meta.is_empty() {
        " ".to_string()
    } else {
        meta
    };
    let mut row1 = format!("\x1b[36m{prefix}\x1b[0m{gap}{status}");
    if let Some(f) = &fb {
        if !status.is_empty() {
            row1.push(' ');
        }
        row1.push_str(&format!("\x1b[33m{f}\x1b[0m"));
    }
    row1 = crate::ui::truncate_width(&row1, term_w);
    // 行2/行3 歌词：缩进 2 空格同列对齐，预算 = 终宽 - 缩进 - 留边
    let budget = term_w.saturating_sub(2 + 1);
    let row2 = format!("  {}", crate::ui::truncate_width(main_body, budget));
    let row3 = sub_body.filter(|s| !s.is_empty()).map(|s| {
        format!(
            "  {}",
            crate::ui::soft_blue(&crate::ui::truncate_width(s, budget))
        )
    });
    use std::sync::atomic::Ordering::Relaxed;
    let mut out = String::new();
    // 回移到首行（旧行深 >1 时上移 行深-1），逐行清后重画
    let prev = OVERLAY_ROWS.load(Relaxed);
    if prev > 1 {
        out.push_str(&format!("\x1b[{}A", prev - 1));
    }
    out.push_str(&format!("\r\x1b[2K{row1}\n\r\x1b[2K{row2}"));
    let mut rows = 2u8;
    if let Some(r3) = &row3 {
        out.push_str(&format!("\n\r\x1b[2K{r3}"));
        rows = 3;
    }
    // 旧行更深（3→2 译文消失）：下移清残留行再回到当前底行
    if prev > rows {
        for _ in 0..(prev - rows) {
            out.push_str("\n\r\x1b[2K");
        }
        out.push_str(&format!("\x1b[{}A", prev - rows));
    }
    OVERLAY_ROWS.store(rows, Relaxed);
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
            id: 0,
            like_count: None,
            red_fetched: false,
            comment_count: None,
            cmt_fetched: false,
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
