//! ncm — 网易云音乐统一命令行工具（搜索 / 下载 / 歌词）
mod api;
mod dns;
mod download;
mod lrc;
mod model;
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
    #[command(subcommand)]
    cmd: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 搜索歌曲并列出
    Search {
        /// 搜索关键词（可多个词，自动合并）
        #[arg(required = true)]
        keyword: Vec<String>,
        /// 结果数量
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: DownloadArgs,
    },
    /// 下载歌曲 / 歌单 / 专辑 / 歌手 / 关键词
    Dl {
        /// 歌曲ID / 歌单·专辑·歌手链接 / list.123 / 关键词（自动搜索取最相关）
        targets: Vec<String>,
        #[command(flatten)]
        opts: DownloadArgs,
    },
    /// 浏览歌单/专辑/歌手的作品列表
    List {
        /// 歌单/专辑/歌手 ID 或链接
        target: String,
        /// 只列出前 N 首（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: u32,
        #[command(flatten)]
        opts: DownloadArgs,
    },
    /// 列出我创建/收藏的歌单（需登录）
    Playlists {
        /// 最多显示数量（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: u32,
        /// 机器可读的 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 每日推荐歌曲（需登录），表格 + -d 选择下载
    Daily {
        /// 只列出前 N 首（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: u32,
        #[command(flatten)]
        opts: DownloadArgs,
    },
    /// 播放歌曲（喂 URL 给 $NCM_PLAYER/mpv/ffplay；都没有则打印 URL）
    Play {
        /// 歌曲 ID / 链接 / 关键词；daily = 每日推荐
        target: String,
        /// 列表场景随机选一首（daily / 歌单）
        #[arg(long)]
        shuffle: bool,
        /// 指定播放器程序（默认探测 $NCM_PLAYER → mpv → ffplay）
        #[arg(long, value_name = "CMD")]
        player: Option<String>,
        /// 音质
        #[arg(long, default_value = "exhigh", value_parser = ["lossless", "standard", "exhigh", "hires"])]
        quality: String,
        /// 只输出 JSON（不调用播放器）
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args, Clone)]
struct DownloadArgs {
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
    // 注：不加 -n 短选项——flatten 与 search -n (limit) 冲突（clap debug_assert 会 panic）
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
    /// 下载指定项：1,3 / 5-8 / 1- / all（对 search/dl/list 的结果统一生效）
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
async fn fetch_row(
    ncm: &Ncm,
    id: u64,
    quality: &str,
    allow_trial: bool,
    wanted_flac: bool,
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
    }

    if row["url"].as_str().unwrap_or("").is_empty() {
        let code = row["code"].as_i64();
        let detail = code.map(|c| format!("（{c}）")).unwrap_or_default();
        let hint = if last_err.trim().is_empty() {
            format!("官方无版权或需 VIP{detail}")
        } else {
            format!("官方无版权或需 VIP{detail}，第三方源失败: {last_err}")
        };
        return (None, source, hint, "unavailable");
    }
    if !row["freeTrialInfo"].is_null() && !allow_trial {
        let start = row["freeTrialInfo"]["start"].as_i64().unwrap_or(0);
        let end = row["freeTrialInfo"]["end"].as_i64().unwrap_or(0);
        return (
            None,
            source,
            format!("试听片段 {start}-{end} 秒（--allow-trial 可下载）"),
            "trial",
        );
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

/// 批量下载编排 + 汇总
/// 列表格 →（TTY 且未给 -d：交互询问编号）→ 有选择则下载，否则仅展示
async fn browse(ncm: &Ncm, songs: Vec<Value>, desc: &str, opts: DownloadArgs) -> Result<i32> {
    let mut printed = false;
    let mut sel = opts.download.clone();
    if sel.is_none() && !opts.json && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        print_search_table(&songs);
        printed = true;
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
    }
    match sel {
        Some(s) => {
            let picked = pick_items(&songs, &s)?;
            download_songs(ncm, picked, vec![format!("{desc} 选择 {s}")], opts).await
        }
        None => {
            if opts.json {
                print_search_json(&songs)?;
            } else if !printed {
                print_search_table(&songs);
            }
            Ok(0)
        }
    }
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
        ("mpv", &["--no-video", "--really-quiet"]),
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

async fn run_batch(
    ncm: &Ncm,
    targets: Vec<String>,
    search_kw: Option<&str>,
    search_limit: u32,
    opts: DownloadArgs,
) -> Result<i32> {
    // 收集曲目
    let mut songs: Vec<Value> = Vec::new();
    let mut scope: Vec<String> = Vec::new();

    if let Some(kw) = search_kw {
        if !opts.json {
            ui::step(&format!("搜索 “{kw}”"));
        }
        let found = ncm.search(kw, search_limit).await?;
        if found.is_empty() {
            bail!("未搜索到结果");
        }
        if opts.download.is_none() {
            print_search_table(&found);
            if opts.json {
                print_search_json(&found)?;
            }
            return Ok(0);
        }
        scope.push(format!("搜索 “{kw}”({})", found.len()));
        songs = found;
    }

    for target in &targets {
        if is_target_like(target) {
            let (found, desc) = resolve_target(ncm, target, opts.artist).await?;
            scope.push(format!("{desc}({})", found.len()));
            songs.extend(found);
        } else {
            // 非 ID/链接 → 视为关键词，取最相关 1 首（交互多选见 list 命令）
            ui::step(&format!("搜索 “{target}”"));
            let found = ncm.search(target, 1).await?;
            if found.is_empty() {
                bail!("未搜索到结果: {target}");
            }
            scope.push(format!("搜索 “{target}”(1)"));
            songs.extend(found);
        }
    }

    if let Some(sel) = &opts.download {
        let picked = pick_items(&songs, sel)?;
        scope.push(format!("选择 {sel}({}/{})", picked.len(), songs.len()));
        songs = picked;
    }

    download_songs(ncm, songs, scope, opts).await
}

/// 下载已收集的曲目：校验选项 → 逐首处理 → 汇总报告
async fn download_songs(
    ncm: &Ncm,
    songs: Vec<Value>,
    scope: Vec<String>,
    opts: DownloadArgs,
) -> Result<i32> {
    if opts.only_lrc && opts.no_lrc {
        bail!("--only-lrc 与 --no-lrc 互斥，只能指定其一");
    }
    if opts.dry_run && opts.json {
        bail!("--dry-run 与 --json 互斥");
    }
    if songs.is_empty() {
        ui::fail("没有可处理的曲目");
        return Ok(1);
    }
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
            ui::fail(&format!("{}/{} {} · {e}", i + 1, total, truncate(&r.title)));
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

    report(&items, &opts, &output)
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
            vec![
                (i + 1).to_string(),
                s["name"].as_str().unwrap_or("").to_string(),
                first_artist(s),
                album.to_string(),
                hms(dur),
                s["id"].as_u64().unwrap_or(0).to_string(),
            ]
        })
        .collect();
    ui::table(&["#", "歌名", "歌手", "专辑", "时长", "ID"], &rows);
    ui::info("下载指定项: -d 1,3 / 5-8 / all  （ncm list <歌单ID> 可浏览）");
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

fn report(items: &[ItemResult], opts: &DownloadArgs, output: &std::path::Path) -> Result<i32> {
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
            "items": json_items,
            "summary": {"audio": audio, "lrc": lrc, "bytes": bytes,
                        "skipped": skipped, "failed": failed},
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(if failed.is_empty() { 0 } else { 1 });
    }

    println!();
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
    let done = if opts.only_lrc { lrc } else { audio };
    ui::step(&format!("完成 {done} · {}", parts.join(" · ")));
    if !skipped.is_empty() {
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
    }
    if !failed.is_empty() {
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
    }
    Ok(if failed.is_empty() { 0 } else { 1 })
}

const SUBCOMMANDS: [&str; 7] = ["search", "dl", "list", "playlists", "daily", "play", "help"];

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
            if opts.json && opts.download.is_none() {
                let found = ncm.search(&kw, limit).await?;
                print_search_json(&found)?;
                return Ok(0);
            }
            run_batch(&ncm, Vec::new(), Some(&kw), limit, opts).await
        }
        Some(Commands::Dl { targets, opts }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            if targets.is_empty() {
                bail!("请提供歌曲 ID / 歌单·专辑链接 / 关键词（ncm help dl 查看用法）");
            }
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            run_batch(&ncm, targets, None, 10, opts).await
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
            browse(&ncm, songs, &desc, opts).await
        }
        Some(Commands::Daily { limit, opts }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let mut songs = ncm.daily_songs().await?;
            if songs.is_empty() {
                bail!("每日推荐为空（该接口需登录：--session/--cookie 或 NCM_SESSION/NCM_COOKIE）");
            }
            if limit > 0 {
                songs.truncate(limit as usize);
            }
            let desc = format!("每日推荐（{} 首）", songs.len());
            browse(&ncm, songs, &desc, opts).await
        }
        Some(Commands::Play {
            target,
            shuffle,
            player,
            quality,
            json,
        }) => {
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let songs = if target.eq_ignore_ascii_case("daily") {
                let s = ncm.daily_songs().await?;
                if s.is_empty() {
                    bail!("每日推荐为空（该接口需登录）");
                }
                s
            } else if is_target_like(&target) {
                let (found, _) = resolve_target(&ncm, &target, false).await?;
                if found.is_empty() {
                    bail!("目标没有可播放的曲目");
                }
                found
            } else {
                let found = ncm.search(&target, 1).await?;
                if found.is_empty() {
                    bail!("未搜索到结果: {target}");
                }
                found
            };
            let song = if shuffle && songs.len() > 1 {
                let idx = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .subsec_nanos() as usize)
                    % songs.len();
                &songs[idx]
            } else {
                &songs[0]
            };
            let track = Track::from_json(song);
            let wanted_flac = matches!(quality.as_str(), "lossless" | "hires");
            let spin = ui::Spinner::start(format!("取址 · {}", track.title));
            let (row, source, reason, _) =
                fetch_row(&ncm, track.id, &quality, true, wanted_flac).await;
            spin.stop();
            let row = row.context(reason)?;
            let url = row["url"]
                .as_str()
                .filter(|u| !u.is_empty())
                .context("没有可播放的地址")?;
            let br = row["br"].as_u64().unwrap_or(0);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "id": track.id,
                        "name": track.title,
                        "artists": track.artists,
                        "url": url,
                        "br": br,
                        "source": source,
                    }))?
                );
                return Ok(0);
            }
            match find_player(player.as_deref()) {
                Some((prog, args)) => {
                    ui::step(&format!(
                        "▶ {} - {} · {source} · {} kbps",
                        track.title,
                        track.artists,
                        br / 1000
                    ));
                    let status = std::process::Command::new(&prog)
                        .args(&args)
                        .arg(url)
                        .status()
                        .with_context(|| format!("启动播放器失败: {prog}"))?;
                    if !status.success() {
                        bail!("播放器退出码 {status}");
                    }
                    Ok(0)
                }
                None => {
                    ui::info("未找到播放器（可安装 mpv/ffplay 或用 --player/$NCM_PLAYER 指定），地址如下:");
                    println!("{url}");
                    Ok(0)
                }
            }
        }
        Some(Commands::Playlists { limit, json }) => {
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let uid = ncm.whoami().await?.context(
                "登录态无效：请用 --session/--cookie 或 NCM_SESSION/NCM_COOKIE 提供凭据",
            )?;
            let mut lists = ncm.user_playlists(uid).await?;
            if limit > 0 {
                lists.truncate(limit as usize);
            }
            if json {
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
            ui::info("下载: ncm dl list.<ID> -d 1,3  ·  浏览: ncm list <ID>");
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
