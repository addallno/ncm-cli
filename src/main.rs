//! ncm — 网易云音乐统一命令行工具（搜索 / 下载 / 歌词）
mod api;
mod dns;
mod download;
mod lrc;
mod mirror;
mod model;
mod player;
mod session;
mod source;
mod tags;
mod ui;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Instant;

use api::Ncm;
use lrc::LRC_DEFAULT;
use model::{first_artist, hms, resolve_list, resolve_target, sanitize, Track};

#[derive(Parser)]
#[command(
    name = "ncm",
    version,
    about = "ncm — 网易云音乐搜索/下载/歌词工具",
    long_about = None
)]
struct Cli {
    /// 禁用彩色输出（NO_COLOR 环境变量同样生效）
    #[arg(long, global = true)]
    no_color: bool,
    /// pyncm session dump 文件（兼容 PYNCM/legacy 两种格式；NCM_SESSION 同效）
    #[arg(long, global = true, value_name = "FILE")]
    session: Option<PathBuf>,
    /// 登录 Cookie，如 "MUSIC_U=xxx; __csrf=yyy"（NCM_COOKIE 同效）
    #[arg(long, global = true, value_name = "STR")]
    cookie: Option<String>,
    /// 解锁音源服务地址（UnblockNeteaseMusic-utils；NCM_MIRROR 同效，none=禁用）
    #[arg(long, global = true, value_name = "URL")]
    mirror: Option<String>,
    #[command(subcommand)]
    cmd: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 搜索歌曲并列出表格（下载用 ncm dl <关键词>）
    Search {
        /// 搜索关键词（可多个词，自动合并）
        #[arg(required = true)]
        keyword: Vec<String>,
        /// 结果数量
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 下载（唯一下载入口）：歌曲ID / 歌单·专辑链接 / 关键词 / daily（每日推荐）
    Dl {
        /// 歌曲ID / 歌单·专辑·歌手链接 / list.123 / daily（每日推荐）/ 关键词
        targets: Vec<String>,
        #[command(flatten)]
        opts: DownloadArgs,
    },
    /// 浏览歌单/专辑/歌手的作品列表（下载用 ncm dl <链接>）
    List {
        /// 歌单/专辑/歌手 ID 或链接
        target: String,
        /// 只列出前 N 首（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 列出我创建/收藏的歌单（需登录，下载用 ncm dl list.<ID>）
    Playlists {
        /// 最多显示数量（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 每日推荐歌曲（需登录；下载: ncm dl daily · 播放: ncm play daily）
    Daily {
        /// 只列出前 N 首（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 首页推荐：每日推荐歌曲 + 官方榜单歌单（歌曲需登录）
    Recommend {
        /// 每类最多显示数量（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 保存/查看/清除登录凭据（持久化到 ~/.config/ncm/，之后自动加载）
    Login {
        /// pyncm session dump 文件（与 --cookie 二选一）
        #[arg(long, value_name = "FILE")]
        session: Option<PathBuf>,
        /// Cookie 字符串，如 "MUSIC_U=xxx; __csrf=yyy"
        #[arg(long, value_name = "STR")]
        cookie: Option<String>,
        /// 登出：删除已保存的凭据
        #[arg(long)]
        logout: bool,
    },
    /// 查看当前登录状态
    Whoami,
    /// 查看歌曲详情（ID / 专辑 / 发行日期 / 音质 / 版权等）
    Info {
        /// 歌曲 ID / 链接 / 关键词
        target: String,
        /// 机器可读的 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 生成 shell 自动补全脚本
    Completions {
        /// 目标 shell
        shell: clap_complete::Shell,
    },
    /// 播放歌曲（队列喂给 mpv/ffplay，mpv 下 </> 切曲、←/→ 快退进、歌词同步滚动）
    Play {
        /// 歌曲 ID / 链接 / 关键词；daily = 每日推荐，roam = 仅私人漫游（FM 无限续播）
        target: String,
        /// 最多播放 N 首（0 = 全部；关键词默认取最相关 1 首，-n 指定则取前 N 首连续播）
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: u32,
        /// 播放队列随机洗牌
        #[arg(long)]
        shuffle: bool,
        /// 指定播放器程序（默认探测 $NCM_PLAYER → mpv → ffplay）
        #[arg(long, value_name = "CMD")]
        player: Option<String>,
        /// 音质
        #[arg(long, default_value = "exhigh", value_parser = ["lossless", "standard", "exhigh", "hires"])]
        quality: String,
        /// 只输出播放队列 JSON（不调用播放器）
        #[arg(long)]
        json: bool,
        /// 播放前先下载到本地并播放本地文件
        #[arg(long)]
        save: bool,
        /// 下载目录（--save 时生效）
        #[arg(short = 'o', long, default_value = ".")]
        output: PathBuf,
        /// 不显示滚动歌词
        #[arg(long = "no-lyric")]
        no_lyric: bool,
        /// 播放漫游：队列将尽时自动接私人FM无限续播（需 mpv；目标为 roam 时已默认开启）
        #[arg(long)]
        roam: bool,
    },
}

/// 浏览类命令（search/list/daily/playlists）的通用输出选项
#[derive(Args, Clone)]
struct ViewArgs {
    /// 机器可读的 JSON 输出
    #[arg(long)]
    json: bool,
    /// 静默：隐藏过程输出，仅保留表格/JSON
    #[arg(short = 'q', long)]
    quiet: bool,
}

#[derive(Args, Clone)]
struct DownloadArgs {
    /// 关键词候选取前 N 首（仅 dl 关键词时生效；TTY 下可交互选择，非 TTY 无 -d 取 1 首）
    #[arg(short = 'n', long, default_value_t = 10)]
    limit: u32,
    /// 音频输出目录
    #[arg(short = 'o', long, default_value = ".")]
    output: PathBuf,
    /// 歌词输出目录（可重复指定多个；默认与音频同目录）
    #[arg(long = "lrc-dir", value_name = "目录")]
    lrc_dir: Vec<PathBuf>,
    /// 要下载的歌词类型，逗号分隔
    #[arg(long, default_value = LRC_DEFAULT)]
    lrcs: String,
    /// 不下载任何歌词
    #[arg(long = "no-lrc")]
    no_lrc: bool,
    /// 只下载歌词，不保存音频
    #[arg(long = "only-lrc", alias = "lyric-only")]
    only_lrc: bool,
    /// 音质：standard/exhigh/lossless/hires
    #[arg(long, default_value = "lossless")]
    quality: String,
    /// 静默：隐藏进度动画与过程输出，仅保留结果
    #[arg(short = 'q', long)]
    quiet: bool,
    /// 预演：取址并报告将写入的文件，不实际下载
    #[arg(long)]
    dry_run: bool,
    /// 文件名模板
    #[arg(short = 't', long, default_value = "{artists} - {track}")]
    template: String,
    /// 跳过已存在的音频文件
    #[arg(long = "skip-existing", visible_alias = "no-overwrites")]
    skip_existing: bool,
    /// 允许下载 30 秒试听片段（默认跳过）
    #[arg(long = "allow-trial")]
    allow_trial: bool,
    /// 按歌手维度解析目标
    #[arg(short = 'a', long = "artist", alias = "ar")]
    artist: bool,
    /// 机器可读的 JSON 输出
    #[arg(long)]
    json: bool,
    /// 按编号选择要下载的项：1,3 / 5-8 / 1- / all（dl 结果统一生效）
    #[arg(short = 'd', long)]
    download: Option<String>,
}

fn quality_br(q: &str) -> Result<u64> {
    match q {
        "standard" => Ok(128_000),
        "exhigh" => Ok(320_000),
        "lossless" => Ok(999_000),
        "hires" => Ok(1_920_000),
        _ => bail!("未知音质 \"{q}\"，可用: standard/exhigh/lossless/hires"),
    }
}

fn normalize_quality(q: &str) -> &'static str {
    match q {
        "lossless" => "lossless",
        "standard" => "standard",
        "exhigh" => "exhigh",
        "hires" => "hires",
        _ => "lossless",
    }
}

/// 单曲处理结果
struct ItemResult {
    title: String,
    artists: String,
    audio: Option<String>,
    lrc: usize,
    bytes: u64,
    skipped: Option<String>,
    error: Option<String>,
    source: String,
}

/// 取播放地址：lossless 先第三方，再回退官方；返回 (row, source, reason, kind)
/// 官方返回是否不完整：无地址或仅 30s 试听
fn row_incomplete(row: &Value) -> bool {
    row["url"].as_str().unwrap_or("").is_empty() || !row["freeTrialInfo"].is_null()
}

/// 第三方直连源兜底：成功写 row/source 并清 last_err，失败把错误并入 last_err
async fn tp_fallback(
    ncm: &Ncm,
    id: u64,
    row: &mut Value,
    source: &mut String,
    last_err: &mut String,
) {
    let (url, name, err) = source::resolve_lossless(ncm, id).await;
    if let Some(url) = url {
        // br=0 表示未知：下载侧跳过码率体积下界校验，播放侧不显示音质
        *row = json!({"url": url, "br": 0u64});
        *source = name.to_string();
        last_err.clear();
    } else if !err.trim().is_empty() {
        *last_err = if last_err.trim().is_empty() {
            format!("第三方源失败: {err}")
        } else {
            format!("{last_err}；第三方源失败: {err}")
        };
    }
}

/// 本地 mirror 解锁服务兜底（语义同 tp_fallback）
async fn mirror_fallback(id: u64, row: &mut Value, source: &mut String, last_err: &mut String) {
    match mirror::match_url(id).await {
        Ok((url, src)) => {
            *row = json!({"url": url, "br": 0u64});
            *source = format!("mirror·{src}");
            last_err.clear();
        }
        Err(e) => {
            let msg = format!("mirror: {e}");
            *last_err = if last_err.trim().is_empty() {
                msg
            } else {
                format!("{last_err}；{msg}")
            };
        }
    }
}

async fn fetch_row(
    ncm: &Ncm,
    id: u64,
    quality: &str,
    allow_trial: bool,
    wanted_flac: bool,
    // 播放场景 true：先本地 mirror（快起播，低质可接受）再第三方；下载 false：先第三方保音质
    mirror_first: bool,
) -> (Option<Value>, String, String, &'static str) {
    let mut row = Value::Null;
    let mut source = String::new();
    let mut last_err = String::new();

    if wanted_flac {
        let (url, name, err) = source::resolve_lossless(ncm, id).await;
        if let Some(url) = url {
            row = json!({"url": url, "br": 999_000});
            source = name.to_string();
        } else {
            last_err = err;
        }
    }

    if row.is_null() {
        match ncm
            .player_url(id, quality_br(quality).unwrap_or(999_000))
            .await
        {
            Ok(r) => row = r,
            Err(e) => last_err = format!("{last_err} 官方: {e}"),
        }
        source = "官方".into();

        // 官方无地址（无版权/-110 需 VIP）或仅 30s 试听 → 按场景顺序兜底
        let official_bad = row_incomplete(&row);
        if official_bad && mirror_first && mirror::enabled() {
            mirror_fallback(id, &mut row, &mut source, &mut last_err).await;
        }
        // 第三方源（lossless 请求已在上方试过，不重复）
        if official_bad && row_incomplete(&row) && !wanted_flac {
            tp_fallback(ncm, id, &mut row, &mut source, &mut last_err).await;
        }
        // 第三方未提供完整音源 → 本地 mirror 兜底（下载场景顺序在后）
        if official_bad && row_incomplete(&row) && mirror::enabled() && !mirror_first {
            mirror_fallback(id, &mut row, &mut source, &mut last_err).await;
        }
    }

    if row["url"].as_str().unwrap_or("").is_empty() {
        let code = row["code"].as_i64();
        let detail = code.map(|c| format!("（{c}）")).unwrap_or_default();
        let why = match code {
            Some(-110) => "需要 VIP",
            Some(-111) | Some(404) => "无版权或已下架",
            _ => "官方无版权或需 VIP",
        };
        let mut hint = format!("{why}{detail}");
        if !last_err.trim().is_empty() {
            hint.push_str(&format!("，第三方源失败: {last_err}"));
        }
        if !mirror::enabled() {
            hint.push_str(
                "；--mirror/NCM_MIRROR 接解锁服务可解锁（npx @neteasecloudmusicapienhanced/unblockmusic-utils）",
            );
        }
        return (None, source, hint, "unavailable");
    }
    if !row["freeTrialInfo"].is_null() {
        if !allow_trial {
            let start = row["freeTrialInfo"]["start"].as_i64().unwrap_or(0);
            let end = row["freeTrialInfo"]["end"].as_i64().unwrap_or(0);
            return (
                None,
                source,
                format!("试听片段 {start}-{end} 秒（--allow-trial 可下载）"),
                "trial",
            );
        }
        if !source.ends_with("试听") {
            source.push_str("·试听");
        }
        // 第三方源与本地 mirror 都没拿到完整音源时标注，避免看起来像"没走兜底"
        if !last_err.trim().is_empty() {
            source.push_str("（无完整源）");
        }
    }
    (Some(row), source, String::new(), "ok")
}

/// 处理单曲：取址 → 下载音频（可选）→ 写标签 → 写歌词
#[allow(clippy::too_many_arguments)]
async fn download_one(
    ncm: &Ncm,
    song: &Value,
    opts: &DownloadArgs,
    lrc_kinds: &[&'static str],
    quality: &str,
    output: &std::path::Path,
    lrc_dirs: &[PathBuf],
    index: usize,
    total: usize,
) -> ItemResult {
    let track = Track::from_json(song);
    let name = track.render_name(&opts.template);
    let label = ui::truncate_width(
        &format!("[{index}/{total}] {} - {}", track.title, track.artists),
        72,
    );
    ui::step(&label);

    let mut result = ItemResult {
        title: track.title.clone(),
        artists: track.artists.clone(),
        audio: None,
        lrc: 0,
        bytes: 0,
        skipped: None,
        error: None,
        source: "—".into(),
    };

    let mut row = Value::Null;
    let mut source = "—".to_string();
    if !opts.only_lrc {
        let spin = ui::Spinner::start(format!("取址 · {}", track.title));
        let (r, src, reason, kind) = fetch_row(
            ncm,
            track.id,
            quality,
            opts.allow_trial,
            matches!(quality, "lossless" | "hires"),
            false, // 下载：第三方保音质优先，mirror 兜底
        )
        .await;
        spin.stop();
        match r {
            Some(v) => {
                row = v;
                source = src;
            }
            None => {
                if kind == "trial" {
                    result.skipped = Some(reason);
                } else {
                    result.error = Some(reason);
                }
                return result;
            }
        }
    }

    let expect_ms = if opts.allow_trial {
        0
    } else {
        row["time"].as_u64().unwrap_or(track.duration_ms)
    };
    let expect_br = row["br"]
        .as_u64()
        .unwrap_or_else(|| quality_br(quality).unwrap_or(999_000));

    if !opts.only_lrc {
        let audio_base = output.join(sanitize(&name));
        if opts.skip_existing
            && ["mp3", "flac", "m4a", "wav"]
                .iter()
                .any(|e| audio_base.with_extension(e).exists())
        {
            result.skipped = Some("文件已存在".into());
            return result;
        }

        if opts.dry_run {
            println!(
                "[dry-run] {} - {} | 目标: {} | 源: {} | 音质: {}",
                track.title,
                track.artists,
                audio_base.display(),
                source,
                quality
            );
            return result;
        }

        let url = row["url"].as_str().unwrap_or("").to_string();
        let mut spin = ui::Spinner::start("校验链接 · 准备下载");
        let spin_flag = spin.stop_flag();
        let mut live = ui::Live::new();
        let started = Instant::now();
        let mut first_progress = true;
        let mut last_bucket: i32 = -1;

        let mut cb = |done: u64, total_bytes: u64| {
            if first_progress {
                use std::sync::atomic::Ordering;
                spin_flag.store(true, Ordering::Relaxed);
                first_progress = false;
            }
            let bucket = if total_bytes > 0 {
                (done as f64 / total_bytes as f64 * 4.0) as i32
            } else {
                0
            };
            let force = !ui::is_tty() && (bucket != last_bucket || done >= total_bytes);
            if ui::is_tty() || force {
                last_bucket = bucket;
                let text = ui::progress_line(done, total_bytes, started);
                live.update(&text, force);
            }
        };

        let download_result = download::download_audio(
            ncm,
            &url,
            &audio_base,
            expect_ms,
            if expect_ms > 0 { Some(expect_br) } else { None },
            &mut cb,
        )
        .await;
        spin.shutdown();
        live.finish();

        let path = match download_result {
            Ok((p, _)) => p,
            Err(why) => {
                // 校验失败时降级 standard 重试一次（对应 musicdl 的逐音质重试）
                if quality != "standard" && why.contains("校验失败") {
                    match ncm.player_url(track.id, 128_000).await {
                        Ok(row2)
                            if !row2["url"].as_str().unwrap_or("").is_empty()
                                && row2["freeTrialInfo"].is_null() =>
                        {
                            let retry_ms = row2["time"].as_u64().unwrap_or(expect_ms);
                            let retry_br = row2["br"].as_u64().unwrap_or(128_000);
                            let retry_url = row2["url"].as_str().unwrap_or("").to_string();
                            let mut live2 = ui::Live::new();
                            let started2 = Instant::now();
                            let mut cb2 = |d: u64, t: u64| {
                                if ui::is_tty() {
                                    live2.update(&ui::progress_line(d, t, started2), false);
                                }
                            };
                            let retry = download::download_audio(
                                ncm,
                                &retry_url,
                                &audio_base,
                                retry_ms,
                                Some(retry_br),
                                &mut cb2,
                            )
                            .await;
                            live2.finish();
                            source = format!("{source}→官方");
                            match retry {
                                Ok((p, _)) => p,
                                Err(e2) => {
                                    result.error = Some(format!("{why}; 降级重试失败: {e2}"));
                                    return result;
                                }
                            }
                        }
                        _ => {
                            result.error = Some(why);
                            return result;
                        }
                    }
                } else {
                    result.error = Some(why);
                    return result;
                }
            }
        };

        result.bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        result.audio = Some(path.display().to_string());
        result.source = source.clone();
    }

    let mut embed_lyrics: Option<String> = None;
    let mut embed_cover: Option<Vec<u8>> = None;

    // 歌词
    if !lrc_kinds.is_empty() {
        let spin = ui::Spinner::start(format!("歌词 · {} 类型", lrc_kinds.len()));
        let got = lrc::fetch_lrcs(ncm, track.id, lrc_kinds).await;
        spin.stop();
        let base = match &result.audio {
            Some(p) => PathBuf::from(p).with_file_name(sanitize(&name)),
            None => output.join(sanitize(&name)),
        };
        let dirs: Vec<PathBuf> = if lrc_dirs.is_empty() {
            vec![base
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| output.to_path_buf())]
        } else {
            lrc_dirs.to_vec()
        };
        let written = lrc::write_lrcs(&got, &base, &dirs);
        result.lrc = written.len();
        embed_lyrics = got
            .get("lrc")
            .cloned()
            .or_else(|| got.values().next().cloned());
        if got.is_empty() && result.audio.is_none() && result.error.is_none() {
            result.error = Some("无可用歌词".into());
        }
        if result.source == "—" {
            result.source = source;
        }
    }

    // 封面（仅音频存在时；下载失败不阻塞）
    if result.audio.is_some() {
        let pic = song["al"]["picUrl"]
            .as_str()
            .or_else(|| song["album"]["picUrl"].as_str())
            .or_else(|| song["picUrl"].as_str())
            .filter(|u| !u.is_empty());
        if let Some(url) = pic {
            match ncm.fetch_bytes(url, 8 * 1024 * 1024).await {
                Ok(bytes) => embed_cover = Some(bytes),
                Err(e) => ui::info(&format!("封面下载失败（不影响文件）: {e}")),
            }
        }
    }

    // 标签统一在歌词/封面就绪后写入
    if let Some(p) = &result.audio {
        if let Err(e) = tags::write_tags(
            std::path::Path::new(p),
            &track.title,
            &track.artists,
            &track.album,
            embed_lyrics.as_deref(),
            embed_cover.as_deref(),
        ) {
            ui::info(&format!("标签写入失败（不影响文件）: {e}"));
        }
    }

    result
}

/// 探测播放器：--player → $NCM_PLAYER → mpv → ffplay；返回 (程序, 额外参数)
fn find_player(spec: Option<&str>) -> Option<(String, Vec<String>)> {
    // 显式指定（--player / $NCM_PLAYER）：直接按空白切分使用
    if let Some(s) = spec {
        let mut it = s.split_whitespace().map(String::from);
        let prog = it.next()?;
        return Some((prog, it.collect()));
    }
    if let Ok(v) = std::env::var("NCM_PLAYER") {
        let v = v.trim();
        if !v.is_empty() {
            let mut it = v.split_whitespace().map(String::from);
            let prog = it.next()?;
            return Some((prog, it.collect()));
        }
    }
    // 候选程序需真实存在（带 --version 探测）
    let candidates: [(&str, &[&str]); 2] = [
        ("mpv", &["--no-video"]),
        ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "error"]),
    ];
    for (prog, args) in candidates {
        let ok = std::process::Command::new(prog)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();
        if ok {
            return Some((
                prog.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            ));
        }
    }
    None
}

/// dl 的批量编排：收集曲目（ID/链接/关键词/daily）→ TTY 交互选号 → 下载
///
/// 关键词取 `-n` 候选；非 TTY 且未给 `-d` 时取最相关 1 首（兼容旧用法），
/// TTY 下列出候选表并交互选择（回车仅展示不下载）。
async fn run_batch(ncm: &Ncm, targets: Vec<String>, opts: DownloadArgs) -> Result<i32> {
    let mut songs: Vec<Value> = Vec::new();
    let mut scope: Vec<String> = Vec::new();
    let tty = !opts.json && std::io::IsTerminal::is_terminal(&std::io::stdin());
    let interactive = tty && opts.download.is_none();

    for target in &targets {
        if target.eq_ignore_ascii_case("daily") || target.eq_ignore_ascii_case("today") {
            let s = ncm.daily_songs().await?;
            if s.is_empty() {
                bail!("每日推荐为空（该接口需登录：ncm login --session/--cookie）");
            }
            scope.push(format!("每日推荐({})", s.len()));
            songs.extend(s);
        } else if is_target_like(target) {
            let (found, desc) = resolve_target(ncm, target, opts.artist).await?;
            scope.push(format!("{desc}({})", found.len()));
            songs.extend(found);
        } else {
            // 关键词：交互或显式 -d 时保留全部候选，否则只取最相关 1 首
            let n = if interactive || opts.download.is_some() {
                opts.limit
            } else {
                1
            };
            if !opts.json {
                ui::step(&format!("搜索 “{target}”"));
            }
            let found = ncm.search(target, n).await?;
            if found.is_empty() {
                bail!("未搜索到结果: {target}");
            }
            scope.push(format!("搜索 “{target}”({})", found.len()));
            songs.extend(found);
        }
    }

    if songs.is_empty() {
        if scope.is_empty() {
            bail!("没有可下载的曲目");
        }
        bail!("没有可下载的曲目: {}", scope.join(" / "));
    }

    // 已给 -d 直接选择；否则 TTY 交互询问（回车仅展示）
    let mut sel = opts.download.clone();
    if sel.is_none() && tty {
        print_search_table(&songs);
        ui::info("下载指定项: -d 1,3 / 5-8 / all");
        use std::io::Write;
        eprint!("输入要下载的编号 (如 1,3,5-8/all，回车仅列出): ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? > 0 {
            let t = line.trim().to_string();
            if !t.is_empty() {
                sel = Some(t);
            }
        }
        match sel {
            Some(s) => {
                let picked = pick_items(&songs, &s)?;
                scope.push(format!("选择 {s}({}/{})", picked.len(), songs.len()));
                songs = picked;
            }
            None => return Ok(0), // 仅展示，不下载
        }
    } else if let Some(s) = &sel {
        let picked = pick_items(&songs, s)?;
        scope.push(format!("选择 {s}({}/{})", picked.len(), songs.len()));
        songs = picked;
    }

    download_songs(ncm, songs, scope, opts)
        .await
        .map(|(code, _)| code)
}

/// 下载已收集的曲目：校验选项 → 逐首处理 → 汇总报告
/// 返回 (退出码, 每首结果——供 --save 播放取本地路径)
async fn download_songs(
    ncm: &Ncm,
    songs: Vec<Value>,
    scope: Vec<String>,
    opts: DownloadArgs,
) -> Result<(i32, Vec<ItemResult>)> {
    if opts.only_lrc && opts.no_lrc {
        bail!("--only-lrc 与 --no-lrc 互斥，只能指定其一");
    }
    if opts.dry_run && opts.json {
        bail!("--dry-run 与 --json 互斥");
    }
    if songs.is_empty() {
        ui::fail("没有可处理的曲目");
        return Ok((1, Vec::new()));
    }
    let started = Instant::now();
    let lrc_kinds: Vec<&'static str> = if opts.no_lrc {
        Vec::new()
    } else {
        lrc::parse_lrc_selection(&opts.lrcs)?
    };
    let quality = normalize_quality(&opts.quality).to_string();
    let _ = quality_br(&opts.quality)?;

    let output = opts.output.canonicalize().unwrap_or_else(|_| {
        std::fs::create_dir_all(&opts.output).ok();
        std::fs::canonicalize(&opts.output).unwrap_or_else(|_| opts.output.clone())
    });
    let lrc_dirs: Vec<PathBuf> = opts
        .lrc_dir
        .iter()
        .map(|p| {
            std::fs::create_dir_all(p).ok();
            std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())
        })
        .collect();

    if !opts.only_lrc && !opts.dry_run {
        std::fs::create_dir_all(&output)?;
    }

    if !opts.json {
        ui::step(&format!("共 {} 首 · {}", songs.len(), scope.join(" + ")));
    }

    let mut items: Vec<ItemResult> = Vec::new();
    let total = songs.len();
    let json_flag = opts.json;
    for (i, song) in songs.iter().enumerate() {
        let r = download_one(
            ncm,
            song,
            &opts,
            &lrc_kinds,
            &quality,
            &output,
            &lrc_dirs,
            i + 1,
            total,
        )
        .await;
        if json_flag {
            // JSON 模式逐条静默，最后统一输出
        } else if opts.dry_run {
            // dry-run 行已在 download_one 打印
        } else if let Some(s) = &r.skipped {
            ui::skip(&format!("{}/{} {} · {s}", i + 1, total, truncate(&r.title)));
        } else if let Some(e) = &r.error {
            ui::fail(&format!(
                "{}/{} {} · {}",
                i + 1,
                total,
                truncate(&r.title),
                ui::truncate_width(e, 64)
            ));
        } else {
            let mut parts: Vec<String> = Vec::new();
            if let Some(p) = &r.audio {
                let file = std::path::Path::new(p)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                parts.push(format!(
                    "{file} · {} · {}",
                    ui::human_size(r.bytes),
                    r.source
                ));
            }
            if r.lrc > 0 {
                parts.push(format!("歌词×{}", r.lrc));
            }
            ui::ok(&format!(
                "{}/{} {} · {}",
                i + 1,
                total,
                truncate(&r.title),
                parts.join(" · ")
            ));
        }
        items.push(r);
    }

    let code = report(&items, &opts, &output, started.elapsed())?;
    Ok((code, items))
}

fn truncate(s: &str) -> String {
    ui::truncate_width(s, 30)
}

fn print_search_table(songs: &[Value]) {
    let rows: Vec<Vec<String>> = songs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let album = s["album"]["name"]
                .as_str()
                .or_else(|| s["al"]["name"].as_str())
                .unwrap_or("");
            let dur = s["duration"]
                .as_u64()
                .or_else(|| s["dt"].as_u64())
                .unwrap_or(0);
            let name = s["name"].as_str().unwrap_or("");
            let id = s["id"].as_u64().unwrap_or(0);
            vec![
                (i + 1).to_string(),
                ui::accent(name),
                first_artist(s),
                album.to_string(),
                ui::dim(&hms(dur)),
                ui::dim(&id.to_string()),
            ]
        })
        .collect();
    ui::table(&["#", "歌名", "歌手", "专辑", "时长", "ID"], &rows);
}

fn print_search_json(songs: &[Value]) -> Result<()> {
    let arr: Vec<Value> = songs
        .iter()
        .map(|s| {
            json!({
                "id": s["id"],
                "name": s["name"],
                "artists": s["artists"].as_array().or_else(|| s["ar"].as_array())
                    .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>()),
                "album": s["album"]["name"].as_str().or_else(|| s["al"]["name"].as_str()),
                "duration_ms": s["duration"].as_u64().or_else(|| s["dt"].as_u64()),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&Value::Array(arr))?);
    Ok(())
}

/// 是否像 ID/链接目标（纯数字 / list.123 形式 / URL），否则按关键词处理
fn is_target_like(t: &str) -> bool {
    let t = t.trim();
    t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty()
        || t.starts_with("http")
        || t.split_once('.').is_some_and(|(k, v)| {
            matches!(k, "list" | "song" | "album" | "artist")
                && v.chars().all(|c| c.is_ascii_digit())
        })
}

/// 选择器解析：`all` / 单号 `3` / 列表 `1,3` / 范围 `5-8`、`1-`（到末尾）、`-3`（开头到 3）
fn pick_items(items: &[Value], selector: &str) -> Result<Vec<Value>> {
    if selector.eq_ignore_ascii_case("all") {
        return Ok(items.to_vec());
    }
    let n = items.len();
    let mut picked = Vec::new();
    for token in selector.split([',', ' ']) {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        let (lo, hi) = match t.split_once('-') {
            // 范围：`5-8` / `1-`（到末尾）/ `-3`（开头到 3）
            Some((a, b)) => {
                let lo = if a.is_empty() {
                    1
                } else {
                    a.parse::<usize>()
                        .map_err(|_| anyhow::anyhow!("无效编号: {a}"))?
                };
                let hi = if b.is_empty() {
                    n
                } else {
                    b.parse::<usize>()
                        .map_err(|_| anyhow::anyhow!("无效编号: {b}"))?
                };
                (lo, hi)
            }
            None => {
                let v = t
                    .parse::<usize>()
                    .map_err(|_| anyhow::anyhow!("无效编号: {t}"))?;
                (v, v)
            }
        };
        if lo < 1 || hi > n || lo > hi {
            bail!("选择超出范围: {t}（共 {n} 条）");
        }
        for idx in lo..=hi {
            let item = &items[idx - 1];
            if !picked.iter().any(|x: &Value| x["id"] == item["id"]) {
                picked.push(item.clone());
            }
        }
    }
    if picked.is_empty() {
        bail!("未选择任何结果");
    }
    Ok(picked)
}

fn report(
    items: &[ItemResult],
    opts: &DownloadArgs,
    output: &std::path::Path,
    elapsed: std::time::Duration,
) -> Result<i32> {
    let audio = items.iter().filter(|i| i.audio.is_some()).count();
    let lrc: usize = items.iter().map(|i| i.lrc).sum();
    let bytes: u64 = items.iter().map(|i| i.bytes).sum();
    let skipped: Vec<String> = items
        .iter()
        .filter_map(|i| i.skipped.as_ref().map(|s| format!("{}（{s}）", i.title)))
        .collect();
    let failed: Vec<String> = items
        .iter()
        .filter(|i| i.error.is_some() && i.audio.is_none())
        .map(|i| format!("{}（{}）", i.title, i.error.clone().unwrap_or_default()))
        .collect();

    if opts.json {
        let json_items: Vec<Value> = items
            .iter()
            .map(|i| {
                json!({
                    "title": i.title,
                    "artists": i.artists,
                    "audio": i.audio,
                    "lrc": i.lrc,
                    "bytes": i.bytes,
                    "source": i.source,
                    "skipped": i.skipped,
                    "error": i.error,
                })
            })
            .collect();
        let out = json!({
            "version": env!("CARGO_PKG_VERSION"),
            "output": output.display().to_string(),
            "quality": opts.quality,
            "dry_run": opts.dry_run,
            "elapsed_secs": elapsed.as_secs(),
            "items": json_items,
            "summary": {"audio": audio, "lrc": lrc, "bytes": bytes,
                        "skipped": skipped, "failed": failed},
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(if failed.is_empty() { 0 } else { 1 });
    }

    println!();
    // 结果一览表（dry-run 已有逐首 [dry-run] 行，不重复）
    if !opts.dry_run && !items.is_empty() {
        let rows: Vec<Vec<String>> = items
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let icon = if r.error.is_some() && r.audio.is_none() {
                    ui::bad("✘")
                } else if r.skipped.is_some() {
                    ui::warn("◌")
                } else {
                    ui::good("✔")
                };
                let note = if let Some(e) = &r.error {
                    ui::truncate_width(e, 30)
                } else if let Some(s) = &r.skipped {
                    s.clone()
                } else if r.lrc > 0 {
                    format!("歌词×{}", r.lrc)
                } else {
                    String::new()
                };
                vec![
                    (i + 1).to_string(),
                    icon,
                    ui::truncate_width(&format!("{} - {}", r.title, r.artists), 40),
                    r.source.clone(),
                    if r.bytes > 0 {
                        ui::human_size(r.bytes)
                    } else {
                        "—".into()
                    },
                    note,
                ]
            })
            .collect();
        ui::table(&["#", "状态", "曲目", "来源", "大小", "备注"], &rows);
    }

    let mut parts: Vec<String> = Vec::new();
    if opts.dry_run {
        parts.push(format!("dry-run 预演 {} 项 · 未写入文件", items.len()));
    } else if opts.only_lrc {
        parts.push(format!("歌词文件 {lrc}"));
    } else {
        parts.push(format!("音频 {audio}"));
        if lrc > 0 {
            parts.push(format!("歌词 {lrc} 份"));
        }
        if bytes > 0 {
            parts.push(ui::human_size(bytes).to_string());
        }
    }
    let secs = elapsed.as_secs();
    let dur_str = if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    };
    parts.push(format!("耗时 {dur_str}"));
    let done = if opts.only_lrc { lrc } else { audio };
    ui::step(&format!("完成 {done} · {}", parts.join(" · ")));
    if !skipped.is_empty() {
        if opts.dry_run {
            ui::info(&format!(
                "跳过 {}: {}",
                skipped.len(),
                skipped
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("；")
            ));
        } else {
            ui::skip(&format!("跳过 {} 首", skipped.len()));
        }
    }
    if !failed.is_empty() {
        if opts.dry_run {
            ui::fail(&format!(
                "失败 {}: {}",
                failed.len(),
                failed
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("；")
            ));
        } else {
            ui::fail(&format!("失败 {} 首（详情见上表）", failed.len()));
        }
    }
    Ok(if failed.is_empty() { 0 } else { 1 })
}

// 播放在线地址预取窗口：网易直链约 10 分钟过期，只先喂前 N 首，其余随播随取
const PLAY_PREFETCH: usize = 6;

const SUBCOMMANDS: [&str; 12] = [
    "search",
    "dl",
    "list",
    "playlists",
    "daily",
    "recommend",
    "play",
    "login",
    "whoami",
    "info",
    "completions",
    "help",
];

/// 快捷方式：首个参数不是子命令时自动补 `dl`（`ncm 259138` == `ncm dl 259138`）；
/// 纯 flags（-h/-V）不改写，交给顶层解析
fn inject_dl_subcommand() -> Vec<String> {
    let mut args: Vec<String> = std::env::args().collect();
    let positional = args[1..].iter().find(|a| !a.starts_with('-'));
    if let Some(p) = positional {
        if !SUBCOMMANDS.contains(&p.as_str()) {
            args.insert(1, "dl".to_string());
        }
    }
    args
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse_from(inject_dl_subcommand());
    let code = match run(cli).await {
        Ok(c) => c,
        Err(e) => {
            ui::fail(&format!("{e:#}"));
            1
        }
    };
    std::process::exit(code);
}

async fn run(cli: Cli) -> Result<i32> {
    ui::set_color(!cli.no_color && std::env::var_os("NO_COLOR").is_none());
    mirror::init(cli.mirror.as_deref());
    // 登录凭据：--session/--cookie 优先，回退 NCM_SESSION/NCM_COOKIE 环境变量
    let session = cli
        .session
        .clone()
        .or_else(|| std::env::var_os("NCM_SESSION").map(std::path::PathBuf::from));
    let cookie = cli
        .cookie
        .clone()
        .or_else(|| std::env::var("NCM_COOKIE").ok());
    let auth = session::resolve_auth(session.as_deref(), cookie.as_deref())?;
    match cli.cmd {
        Some(Commands::Search {
            keyword,
            limit,
            opts,
        }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let kw = keyword.join(" ");
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let found = ncm.search(&kw, limit).await?;
            if opts.json {
                print_search_json(&found)?;
            } else if found.is_empty() {
                ui::info(&format!("未搜索到结果: {kw}"));
            } else {
                print_search_table(&found);
                ui::info(&format!("下载: ncm dl \"{kw}\" -d 编号"));
            }
            Ok(0)
        }
        Some(Commands::Dl { targets, opts }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            if targets.is_empty() {
                bail!("请提供歌曲 ID / 歌单·专辑链接 / 关键词 / daily（ncm help dl 查看用法）");
            }
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            run_batch(&ncm, targets, opts).await
        }
        Some(Commands::List {
            target,
            limit,
            opts,
        }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let (mut songs, desc) = resolve_list(&ncm, &target).await?;
            if limit > 0 {
                songs.truncate(limit as usize);
            }
            if opts.json {
                print_search_json(&songs)?;
            } else if songs.is_empty() {
                ui::info(&format!("{desc} 没有可显示的曲目"));
            } else {
                print_search_table(&songs);
                ui::info(&format!("下载: ncm dl \"{target}\" -d 编号"));
            }
            Ok(0)
        }
        Some(Commands::Daily { limit, opts }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let mut songs = ncm.daily_songs().await?;
            if songs.is_empty() {
                bail!("每日推荐为空（该接口需登录：ncm login --session/--cookie 保存登录态）");
            }
            if limit > 0 {
                songs.truncate(limit as usize);
            }
            if opts.json {
                print_search_json(&songs)?;
            } else {
                print_search_table(&songs);
                ui::info("下载: ncm dl daily -d 编号  ·  播放: ncm play daily");
            }
            Ok(0)
        }
        Some(Commands::Recommend { limit, opts }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            // 每日推荐（需登录，失败降级为仅榜单）+ 官方榜单
            let mut daily = ncm.daily_songs().await.unwrap_or_default();
            let toplist = ncm.toplist().await?;
            if daily.is_empty() && toplist.is_empty() {
                bail!("推荐与榜单均为空（每日推荐需登录：ncm login --session/--cookie）");
            }
            if limit > 0 {
                daily.truncate(limit as usize);
            }
            if opts.json {
                let out = json!({
                    "daily": Value::Array(daily),
                    "toplists": Value::Array(
                        if limit > 0 { toplist.into_iter().take(limit as usize).collect() } else { toplist },
                    ),
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(0);
            }
            if daily.is_empty() {
                ui::info("每日推荐为空（需登录 ncm login）");
            } else {
                ui::step("每日推荐");
                print_search_table(&daily);
                ui::info("下载: ncm dl daily -d 编号  ·  播放: ncm play daily");
            }
            if !toplist.is_empty() {
                ui::step("官方榜单");
                let rows: Vec<Vec<String>> = toplist
                    .iter()
                    .take(if limit > 0 {
                        limit as usize
                    } else {
                        usize::MAX
                    })
                    .enumerate()
                    .map(|(i, t)| {
                        vec![
                            (i + 1).to_string(),
                            t["name"].as_str().unwrap_or("").to_string(),
                            t["updateFrequency"].as_str().unwrap_or("").to_string(),
                            t["trackCount"].as_u64().unwrap_or(0).to_string(),
                            t["id"].as_u64().unwrap_or(0).to_string(),
                        ]
                    })
                    .collect();
                ui::table(&["#", "榜单名", "更新", "曲数", "ID"], &rows);
                ui::info("浏览: ncm list <ID>  ·  下载: ncm dl list.<ID> -d 编号");
            }
            Ok(0)
        }
        Some(Commands::Play {
            target,
            limit,
            shuffle,
            player,
            quality,
            json,
            save,
            output,
            no_lyric,
            roam,
        }) => {
            if json && save {
                bail!("--json 与 --save 互斥");
            }
            let _ = quality_br(&quality)?;
            let ncm = Ncm::with_cookie(auth.as_deref())?;

            // 1) 收集播放队列
            let desc;
            let mut songs: Vec<Value> = if target.eq_ignore_ascii_case("roam") {
                // 私人漫游：FM 首曲开播，后续由漫游续播无限接
                desc = "私人漫游".to_string();
                let s = ncm.radio_songs().await?;
                if s.is_empty() {
                    bail!("私人FM为空（该接口需登录：ncm login --session/--cookie）");
                }
                s
            } else if target.eq_ignore_ascii_case("daily") || target.eq_ignore_ascii_case("today") {
                desc = "每日推荐".to_string();
                let s = ncm.daily_songs().await?;
                if s.is_empty() {
                    bail!("每日推荐为空（该接口需登录：ncm login --session/--cookie）");
                }
                s
            } else if is_target_like(&target) {
                let (found, d) = resolve_target(&ncm, &target, false).await?;
                if found.is_empty() {
                    bail!("“{d}” 没有可播放的曲目");
                }
                desc = d;
                found
            } else {
                desc = format!("搜索 “{target}”");
                let n = if limit > 0 { limit } else { 1 };
                let found = ncm.search(&target, n).await?;
                if found.is_empty() {
                    bail!("未搜索到结果: {target}");
                }
                found
            };
            // 私人漫游模式默认开启无限续播（等效 --roam）
            let roam = roam || target.eq_ignore_ascii_case("roam");
            if limit > 0 && songs.len() > limit as usize {
                songs.truncate(limit as usize);
            }
            if songs.is_empty() {
                bail!("没有可播放的曲目");
            }
            if shuffle && songs.len() > 1 {
                shuffle_by_time(&mut songs);
            }
            let tracks: Vec<Track> = songs.iter().map(Track::from_json).collect();
            let total = tracks.len();

            // 2) JSON 模式：全量取址输出队列
            if json {
                let results = fetch_urls(&ncm, &tracks, &quality).await;
                let mut arr: Vec<Value> = Vec::with_capacity(total);
                for (t, r) in tracks.iter().zip(results) {
                    match r {
                        Ok((url, br, source)) => arr.push(json!({
                            "id": t.id,
                            "name": t.title,
                            "artists": t.artists,
                            "album": t.album,
                            "url": url,
                            "br": br,
                            "source": source,
                        })),
                        Err(reason) => arr.push(json!({
                            "id": t.id,
                            "name": t.title,
                            "error": reason,
                        })),
                    }
                }
                println!("{}", serde_json::to_string_pretty(&Value::Array(arr))?);
                return Ok(0);
            }

            // 3) 播放器探测（无播放器直接打印地址，省去下载/取歌词）
            let Some((prog, base_args)) = find_player(player.as_deref()) else {
                ui::info(
                    "未找到播放器（可安装 mpv/ffplay 或用 --player/$NCM_PLAYER 指定），队列地址:",
                );
                let urls = fetch_urls(&ncm, &tracks, &quality).await;
                for (t, r) in tracks.iter().zip(urls) {
                    match r {
                        Ok((url, _, _)) => println!("{} - {}\t{url}", t.title, t.artists),
                        Err(reason) => println!("{} - {}\t[失败: {reason}]", t.title, t.artists),
                    }
                }
                return Ok(0);
            };
            let is_mpv = std::path::Path::new(&prog)
                .file_name()
                .is_some_and(|f| f == "mpv");
            let is_ffplay = std::path::Path::new(&prog)
                .file_name()
                .is_some_and(|f| f == "ffplay");

            // 4) 播放来源：--save 先下载（失败回落在线地址）；否则在线取址
            let mut slots: Vec<Slot> = tracks
                .iter()
                .map(|t| (format!("{} - {}", t.title, t.artists), Err(String::new())))
                .collect();
            if save {
                let dl_opts = DownloadArgs {
                    limit: 0,
                    output: output.clone(),
                    lrc_dir: Vec::new(),
                    lrcs: LRC_DEFAULT.to_string(),
                    no_lrc: false,
                    only_lrc: false,
                    quality: quality.clone(),
                    quiet: false,
                    dry_run: false,
                    template: "{artists} - {track}".to_string(),
                    skip_existing: true,
                    allow_trial: false,
                    artist: false,
                    json: false,
                    download: None,
                };
                let (_code, items) = download_songs(
                    &ncm,
                    songs.clone(),
                    vec![format!("{desc} 下载到 {}", output.display())],
                    dl_opts,
                )
                .await?;
                for (slot, item) in slots.iter_mut().zip(items.iter()) {
                    if let Some(p) = &item.audio {
                        slot.1 = Ok((PlaySrc::File(PathBuf::from(p)), "本地文件".into(), 0));
                    }
                }
            }
            // 分批取址：mpv+unix 下只预取前 PLAY_PREFETCH 首在线地址（网易直链
            // 约 10 分钟过期，全量预取会让长队列后段 403 雪崩），其余随播随取
            let batching = cfg!(unix) && is_mpv;
            let prefetch = if batching { PLAY_PREFETCH } else { usize::MAX };
            let missing: Vec<usize> = slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.1.is_err())
                .map(|(i, _)| i)
                .collect();
            if !missing.is_empty() {
                let sub: Vec<Track> = missing
                    .iter()
                    .take(prefetch)
                    .map(|&i| tracks[i].clone())
                    .collect();
                if !save {
                    ui::step(&format!("取址 · {desc}（{} / {total} 首）", sub.len()));
                }
                let got = fetch_urls(&ncm, &sub, &quality).await;
                for (idx, r) in missing.iter().take(prefetch).zip(got) {
                    slots[*idx].1 = r.map(|(url, br, src)| (PlaySrc::Url(url), src, br));
                }
            }

            // 5) 汇总成功/失败；分批模式下超出预取窗口的曲进 pending 随播随取
            let mut entries: Vec<(String, PlaySrc, String, u64)> = Vec::new();
            let mut play_tracks: Vec<Track> = Vec::new();
            let mut failed: Vec<String> = Vec::new();
            let mut pending: std::collections::VecDeque<PendingItem> =
                std::collections::VecDeque::new();
            let mut fed_urls = 0usize;
            let mut cutoff = false;
            for ((label, src), t) in slots.into_iter().zip(tracks) {
                match src {
                    Ok((s, source, br)) => {
                        if cutoff {
                            // 预取窗口后的曲（含本地文件）延后 append，保持播放顺序
                            pending.push_back(PendingItem {
                                label,
                                src: s,
                                track: t,
                            });
                            continue;
                        }
                        let is_url = matches!(s, PlaySrc::Url(_));
                        entries.push((label, s, source, br));
                        play_tracks.push(t);
                        if is_url {
                            fed_urls += 1;
                            if fed_urls >= prefetch {
                                cutoff = true;
                            }
                        }
                    }
                    Err(reason) if !reason.is_empty() => failed.push(format!("{label} · {reason}")),
                    Err(_) if batching => pending.push_back(PendingItem {
                        label,
                        src: PlaySrc::Url(String::new()),
                        track: t,
                    }),
                    Err(_) => failed.push(format!("{label} · 无播放地址")),
                }
            }
            for f in &failed {
                ui::skip(f);
            }
            if entries.is_empty() {
                bail!("全部曲目取址失败，无法播放");
            }

            // 6) 歌词预取（mpv + unix 下 IPC 底行：有词滚歌词，无词显示歌名占位）
            let want_ipc = !no_lyric && is_mpv && cfg!(unix);
            let lyric_tracks: Option<std::sync::Arc<std::sync::RwLock<Vec<player::LyricTrack>>>> =
                if want_ipc {
                    let ids: Vec<u64> = play_tracks.iter().map(|t| t.id).collect();
                    let got = fetch_lyrics(&ncm, &ids).await;
                    let lts: Vec<player::LyricTrack> = play_tracks
                        .iter()
                        .zip(got)
                        .map(|(t, lines)| player::LyricTrack {
                            label: format!("{} - {}", t.title, t.artists),
                            lines: lines.unwrap_or_default(),
                        })
                        .collect();
                    Some(std::sync::Arc::new(std::sync::RwLock::new(lts)))
                } else {
                    None
                };
            let want_lyric = lyric_tracks.is_some();

            // 7) 启动播放
            let locations: Vec<String> = entries.iter().map(|(_, s, _, _)| s.loc()).collect();
            let n_play = entries.len();
            let n_queue = n_play + pending.len();
            ui::step(&format!("▶ {desc} · 播放 {n_queue} 首 · {prog}"));
            // 播放队列卡片：歌名/歌手 · 来源/音质（试听曲标警告）
            let show_n = n_play.min(10);
            for (i, (label, _, source, br)) in entries.iter().take(show_n).enumerate() {
                let br = *br;
                let quality = match br {
                    0 => String::new(),
                    b if b >= 999_000 => " · 无损".to_string(),
                    b => format!(" · {}kbps", b / 1000),
                };
                let trial = if source.contains("试听") {
                    format!(" {}", ui::warn("⚠ 30s试听"))
                } else {
                    String::new()
                };
                ui::info(&format!("  {}. {label} · {source}{quality}{trial}", i + 1));
            }
            if n_queue > show_n {
                ui::info(&format!("  … 共 {n_queue} 首"));
            }
            if !pending.is_empty() {
                ui::info("  分批加载 · 链接过期前自动续取后段曲目");
            }
            if is_mpv {
                ui::info("mpv 键位: 空格 暂停 · ←/→ 快退/快进 · </> 上/下曲 · / 搜索 · q 退出");
            }
            if !no_lyric && !cfg!(unix) && is_mpv {
                ui::info("当前平台不支持 mpv IPC 歌词同步，已跳过");
            } else if lyric_tracks.is_some() && !is_mpv {
                ui::info("歌词同步需要 mpv（当前播放器无 IPC 接口），已跳过");
            }

            let mut args: Vec<String> = base_args.clone();
            let sock = player::socket_path();
            if is_mpv {
                // 抑制 mpv 状态/info 行防止刷屏（歌词底行或每首提示接管输出）
                args.push("--term-status-msg=".to_string());
                args.push("--msg-level=cplayer=warn,ffmpeg=warn,ipc=no".to_string());
                // 直链交给 ffmpeg：禁用 ytdl 解析（坏掉的 yt-dlp 只会刷 traceback）
                args.push("--ytdl=no".to_string());
                // 预开播放列表下一条：按 > 跳过时新曲已就绪，无需等冷加载缓冲
                args.push("--prefetch-playlist=yes".to_string());
            }
            // 守护任务需要 IPC socket：歌词底行 或 分批续取/漫游
            #[cfg(unix)]
            let guard_needed = is_mpv && (!pending.is_empty() || roam);
            #[cfg(not(unix))]
            let guard_needed = false;
            if is_mpv && (want_lyric || guard_needed) {
                player::cleanup_socket(&sock);
                args.push(format!("--input-ipc-server={}", sock.display()));
            }
            let overlay = if want_lyric {
                args.push("--term-playing-msg=".to_string());
                Some(player::spawn_overlay(
                    sock.clone(),
                    lyric_tracks.clone().expect("want_lyric 保证 Some"),
                ))
            } else if is_mpv {
                // 无歌词 IPC 时用 mpv 模板每首打一行，兼作切曲提示
                args.push(
                    "--term-playing-msg=♪ [${playlist-pos}/${playlist-count}] ${media-title}"
                        .to_string(),
                );
                None
            } else {
                None
            };
            // 播放守护：pending 随播随取（在线地址现取防过期）+ 可选漫游接FM
            //（ipc_send/ipc_get 为 unix 实现，Windows 仅提示不支持）
            #[cfg(unix)]
            let guard_handle = if guard_needed {
                if roam {
                    ui::info("漫游已开启 · 队列将尽时自动接私人FM续播");
                }
                Some(tokio::spawn(guard_loop(
                    sock.clone(),
                    ncm.clone(),
                    lyric_tracks.clone(),
                    quality.clone(),
                    pending,
                    roam,
                )))
            } else {
                if roam {
                    ui::info("漫游需要 mpv（当前播放器无 IPC 接口），已忽略");
                }
                None
            };
            #[cfg(not(unix))]
            let guard_handle: Option<tokio::task::JoinHandle<()>> = {
                if roam {
                    ui::info("播放漫游暂不支持当前平台（需要 unix + mpv IPC），已忽略");
                }
                let _ = (lyric_tracks, pending);
                None
            };

            let status = if is_ffplay {
                // ffplay 不支持播放列表：逐首串行（首尾相接）
                let mut last = None;
                for loc in &locations {
                    let r = std::process::Command::new(&prog)
                        .args(&args)
                        .arg(loc)
                        .status()
                        .with_context(|| format!("启动播放器失败: {prog}"))?;
                    last = Some(r);
                    if !r.success() {
                        break;
                    }
                }
                last.context("ffplay 未启动")?
            } else {
                // mpv / 自定义播放器：一次性喂入全部地址（mpv 自动建播放列表）
                let mut cmd = std::process::Command::new(&prog);
                cmd.args(&args).args(&locations);
                tokio::task::spawn_blocking(move || cmd.status())
                    .await
                    .context("等待播放器失败")?
                    .with_context(|| format!("启动播放器失败: {prog}"))?
            };
            if let Some(h) = guard_handle {
                h.abort();
            }
            if overlay.is_some() && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                player::clear_overlay_rows();
            }
            player::cleanup_socket(&sock);
            if !status.success() && status.code().is_some_and(|c| c != 0) {
                ui::info(&format!("播放器退出码 {}", status.code().unwrap_or(0)));
            }
            if !failed.is_empty() {
                ui::info(&format!("（{} 首未播放，见上方跳过提示）", failed.len()));
            }
            ui::ok(&format!("播放完毕 · {n_play} 首"));
            Ok(0)
        }
        Some(Commands::Login {
            session,
            cookie,
            logout,
        }) => {
            if logout {
                let had = session::clear_saved()?;
                if had {
                    ui::ok("已删除保存的登录凭据");
                } else {
                    ui::info("没有已保存的凭据");
                }
                return Ok(0);
            }
            // 显式输入 → 解析并保存；无输入 → 用已保存凭据检查状态
            let given = session.is_some() || cookie.is_some();
            let auth_str = if let Some(p) = &session {
                let raw = std::fs::read(p)
                    .with_context(|| format!("读取 session 文件失败: {}", p.display()))?;
                session::parse_session(&raw)
                    .with_context(|| format!("解析 session 文件失败: {}", p.display()))?
            } else if let Some(c) = &cookie {
                let c = c.trim();
                if !c.contains('=') {
                    bail!("cookie 格式应为 \"MUSIC_U=xxx; __csrf=yyy\"");
                }
                c.to_string()
            } else if let Some(saved) = session::load_saved() {
                saved
            } else {
                ui::info(&format!(
                    "未保存凭据。用法:\n  ncm login --session <pyncm文件>\n  ncm login --cookie \"MUSIC_U=...; __csrf=...\"\n  ncm login --logout  登出\n凭据将保存到 {}",
                    session::saved_cookie_path().display()
                ));
                return Ok(0);
            };
            let ncm = Ncm::with_cookie(Some(&auth_str))?;
            match ncm.whoami().await? {
                Some((uid, name)) => {
                    if given {
                        let path = session::save_cookie(&auth_str)?;
                        ui::ok(&format!(
                            "登录成功并已保存：{name} (uid {uid})\n  凭据: {}",
                            path.display()
                        ));
                    } else {
                        ui::ok(&format!(
                            "已保存凭据有效：{name} (uid {uid})（ncm login --logout 可登出）"
                        ));
                    }
                    Ok(0)
                }
                None => {
                    if given {
                        bail!("凭据无效或已过期，未保存");
                    }
                    bail!("已保存凭据已失效，请重新 ncm login --session/--cookie");
                }
            }
        }
        Some(Commands::Whoami) => {
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            match ncm.whoami().await? {
                Some((uid, name)) => {
                    let via = if session::load_saved().is_some() {
                        "（持久化凭据）"
                    } else {
                        "（本次会话凭据）"
                    };
                    ui::ok(&format!("{name} · uid {uid} {via}"));
                    Ok(0)
                }
                None => {
                    ui::info("未登录（ncm login --session/--cookie 保存登录态）");
                    Ok(1)
                }
            }
        }
        Some(Commands::Info { target, json }) => {
            ui::set_json(json);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let id = if let Ok(id) = target.parse::<u64>() {
                id
            } else {
                let found = if is_target_like(&target) {
                    resolve_target(&ncm, &target, false).await?.0
                } else {
                    ncm.search(&target, 1).await?
                };
                match found.first() {
                    Some(s) => s["id"].as_u64().context("搜索结果没有歌曲 ID")?,
                    None => bail!("未找到: {target}"),
                }
            };
            let details = ncm.song_detail(&[id]).await?;
            let song = details.first().context("详情为空")?;
            if json {
                println!("{}", serde_json::to_string_pretty(song)?);
                return Ok(0);
            }
            let t = Track::from_json(song);
            let fee_raw = song["fee"].as_u64().unwrap_or(0);
            let fee = match fee_raw {
                0 => ui::good("免费"),
                1 => ui::warn("VIP"),
                4 => ui::warn("专辑购买"),
                8 => "低品免费".into(),
                n => format!("fee={n}"),
            };
            let mv = song["mv"]
                .as_u64()
                .filter(|m| *m > 0)
                .map(|m| m.to_string())
                .unwrap_or_else(|| "无".into());
            let maxbr = song["privilege"]["maxbr"]
                .as_u64()
                .or_else(|| song["maxbr"].as_u64())
                .map(|b| format!("{} kbps", b / 1000));
            let copyright = match song["copyright"].as_u64() {
                Some(0) => Some("翻唱/改编".to_string()),
                Some(2) => Some("网易云独家".to_string()),
                _ => None,
            };
            ui::step(&format!("♪ {} · {}", t.title, t.artists));
            let mut rows: Vec<Vec<String>> = vec![
                vec!["ID".into(), id.to_string()],
                vec!["歌名".into(), t.title],
                vec!["歌手".into(), t.artists],
                vec!["专辑".into(), t.album],
                vec!["时长".into(), hms(t.duration_ms)],
                vec!["费用".into(), fee],
                vec!["MV".into(), mv],
            ];
            if let Some(b) = maxbr {
                rows.push(vec!["最高音质".into(), b]);
            }
            if let Some(c) = copyright {
                rows.push(vec!["版权".into(), c]);
            }
            ui::table(&["字段", "值"], &rows);
            if matches!(fee_raw, 1 | 4) {
                ui::info("VIP 曲 · 配置 --mirror / NCM_MIRROR 可解锁完整音质");
            }
            Ok(0)
        }
        Some(Commands::Completions { shell }) => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "ncm", &mut std::io::stdout());
            Ok(0)
        }
        Some(Commands::Playlists { limit, opts }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let uid = ncm
                .whoami()
                .await?
                .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?
                .0;
            let mut lists = ncm.user_playlists(uid).await?;
            if limit > 0 {
                lists.truncate(limit as usize);
            }
            if opts.json {
                let arr: Vec<Value> = lists
                    .iter()
                    .map(|p| {
                        json!({
                            "id": p["id"],
                            "name": p["name"],
                            "track_count": p["trackCount"],
                            "subscribed": p["subscribed"],
                            "creator": p["creator"]["nickname"],
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&Value::Array(arr))?);
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = lists
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let mine = p["subscribed"].as_bool() == Some(false);
                    vec![
                        (i + 1).to_string(),
                        p["name"].as_str().unwrap_or("").to_string(),
                        p["trackCount"].as_u64().unwrap_or(0).to_string(),
                        if mine { "创建" } else { "收藏" }.to_string(),
                        p["id"].as_u64().unwrap_or(0).to_string(),
                    ]
                })
                .collect();
            ui::table(&["#", "歌单名", "曲数", "来源", "ID"], &rows);
            if !opts.quiet {
                ui::info("浏览: ncm list <ID>  ·  下载: ncm dl list.<ID> -d 编号");
            }
            Ok(0)
        }
        // 无子命令（纯 flags 或空参数）：inject 已把裸参数注入 dl，走到这里只可能是空输入
        None => {
            Cli::command().print_help()?;
            Ok(0)
        }
    }
}

use clap::CommandFactory;

/// 分批续播的待入队项：本地文件直接 append，在线地址在 append 时重新取址
///（网易直链有时效，预取过期会 403）
struct PendingItem {
    label: String,
    src: PlaySrc,
    track: Track,
}

/// 播放来源：本地文件（--save 下载所得）或在线地址
/// 播放取址槽位：(曲目标签, (播放源, 来源名, 码率) 或失败原因)
type Slot = (String, Result<(PlaySrc, String, u64), String>);

enum PlaySrc {
    File(PathBuf),
    Url(String),
}

impl PlaySrc {
    fn loc(&self) -> String {
        match self {
            PlaySrc::File(p) => p.display().to_string(),
            PlaySrc::Url(u) => u.clone(),
        }
    }
}

/// 并发批量取播放地址；返回与输入等长的 Ok(url, br, source) / Err(原因)
async fn fetch_urls(
    ncm: &Ncm,
    tracks: &[Track],
    quality: &str,
) -> Vec<Result<(String, u64, String), String>> {
    use futures_util::future::join_all;
    let wanted_flac = matches!(quality, "lossless" | "hires");
    join_all(tracks.iter().map(|t| async move {
        let (row, source, reason, _) = fetch_row(ncm, t.id, quality, true, wanted_flac, true).await;
        let row = row.ok_or(reason)?;
        let url = row["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(String::from)
            .ok_or_else(|| "没有可播放的地址".to_string())?;
        Ok((url, row["br"].as_u64().unwrap_or(0), source))
    }))
    .await
}

/// 并发预取 LRC 歌词（data.lrc.lyric → 时间轴）；失败/空返回 None
async fn fetch_lyrics(ncm: &Ncm, ids: &[u64]) -> Vec<Option<Vec<(f64, String)>>> {
    use futures_util::future::join_all;
    join_all(ids.iter().map(|&id| async move {
        let v = ncm.lyric(id).await.ok()?;
        let text = v["lrc"]["lyric"].as_str()?;
        let lines = lrc::parse_lrc_timeline(text);
        (!lines.is_empty()).then_some(lines)
    }))
    .await
}

/// 时间种子洗牌（xorshift64*，满足播放随机即可，无外部 rand 依赖）
/// 播放守护：队尾将尽时把 pending 续入 mpv（在线地址现取防过期），pending
/// 耗尽且开启漫游时拉私人FM补充；歌词容器与播放列表索引对齐；
/// mpv 退出（IPC 连不上连续 3 次）静默结束
#[cfg(unix)]
async fn guard_loop(
    sock: std::path::PathBuf,
    ncm: Ncm,
    tracks: Option<std::sync::Arc<std::sync::RwLock<Vec<player::LyricTrack>>>>,
    quality: String,
    mut pending: std::collections::VecDeque<PendingItem>,
    roam: bool,
) {
    let wanted_flac = matches!(quality.as_str(), "lossless" | "hires");
    let mut req_id: u64 = 9000;
    let mut fails: u32 = 0; // 启动期 socket 未就绪的容忍计数
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        // 查询失败 = mpv 未就绪或已退出（连续 3 次才认定退出）
        req_id += 1;
        let pos = player::ipc_get(&sock, "playlist-pos", req_id)
            .await
            .and_then(|v| v.as_u64());
        req_id += 1;
        let count = player::ipc_get(&sock, "playlist-count", req_id)
            .await
            .and_then(|v| v.as_u64());
        let (Some(pos), Some(mut count)) = (pos, count) else {
            fails += 1;
            if fails >= 3 {
                return;
            }
            continue;
        };
        fails = 0;
        if pos + 3 < count {
            continue; // 距队尾还早
        }
        // pending 耗尽且漫游开启：拉私人FM补充
        if pending.is_empty() {
            if !roam {
                return;
            }
            let fm = ncm.radio_songs().await.unwrap_or_default();
            if fm.is_empty() {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            }
            for song in fm {
                let t = Track::from_json(&song);
                pending.push_back(PendingItem {
                    label: format!("{} - {}", t.title, t.artists),
                    src: PlaySrc::Url(String::new()),
                    track: t,
                });
            }
        }
        // 预追到前方余量 ≥8 首；取址失败的曲跳过换下一首
        while pos + 8 > count {
            let Some(item) = pending.pop_front() else {
                break;
            };
            let t = &item.track;
            // 在线地址现取（预取链接会过期，防 403），本地文件直接用
            let loc = match &item.src {
                PlaySrc::File(_) => item.src.loc(),
                PlaySrc::Url(_) => {
                    let (row, _source, _reason, _kind) =
                        fetch_row(&ncm, t.id, &quality, true, wanted_flac, true).await;
                    let url = row
                        .as_ref()
                        .and_then(|r| r["url"].as_str())
                        .filter(|u| !u.is_empty())
                        .map(String::from);
                    let Some(url) = url else {
                        ui::skip(&format!("{} · 续取失败", item.label));
                        continue;
                    };
                    url
                }
            };
            // 歌词（失败留空，底行 label 占位照常）
            let lines = ncm
                .lyric(t.id)
                .await
                .ok()
                .and_then(|v| v["lrc"]["lyric"].as_str().map(lrc::parse_lrc_timeline))
                .unwrap_or_default();
            // 先 append 到 mpv，确认命令送达再写 tracks，保持索引对齐
            let cmd = serde_json::json!({"command": ["loadfile", loc, "append"]}).to_string();
            if player::ipc_send(&sock, &cmd).await.is_err() {
                return;
            }
            if let Some(ts) = &tracks {
                ts.write()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(player::LyricTrack {
                        label: format!("{} - {}", t.title, t.artists),
                        lines,
                    });
            }
            count += 1;
        }
    }
}

fn shuffle_by_time(v: &mut [Value]) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    for i in (1..v.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = (seed as usize) % (i + 1);
        v.swap(i, j);
    }
}
