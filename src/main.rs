//! ncm — 网易云音乐统一命令行工具（搜索 / 下载 / 歌词）
mod api;
mod download;
mod lrc;
mod model;
mod source;
mod tags;
mod ui;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Instant;

use api::Ncm;
use lrc::LRC_DEFAULT;
use model::{first_artist, hms, resolve_target, sanitize, Track};

#[derive(Parser)]
#[command(
    name = "ncm",
    version,
    about = "ncm — 网易云音乐搜索/下载/歌词工具",
    long_about = None,
    disable_help_subcommand = true
)]
struct Cli {
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
        /// 下载指定项：1 / 1,3 / all（缺省仅列出）
        #[arg(long)]
        download: Option<String>,
        #[command(flatten)]
        opts: DownloadArgs,
    },
    /// 下载歌曲 / 歌单 / 专辑 / 歌手
    Dl {
        /// 歌曲ID / 单曲·歌单·专辑·歌手链接 / list.123
        targets: Vec<String>,
        /// 按关键词搜索后下载
        #[arg(short = 'S', long)]
        search: Option<String>,
        /// 配合 --search 的结果数
        #[arg(long, default_value_t = 5)]
        limit: u32,
        #[command(flatten)]
        opts: DownloadArgs,
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
    #[arg(short = 'q', long, default_value = "lossless")]
    quality: String,
    /// 文件名模板
    #[arg(short = 't', long, default_value = "{artists} - {track}")]
    template: String,
    /// 跳过已存在的音频文件
    #[arg(long = "skip-existing", alias = "no-overwrite")]
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
}

fn quality_br(q: &str) -> Result<u64> {
    match q {
        "standard" => Ok(128_000),
        "exhigh" | "high" => Ok(320_000),
        "lossless" | "super" | "auto" => Ok(999_000),
        "hires" | "master" => Ok(1_920_000),
        _ => bail!("未知音质 \"{q}\"，可用: standard/exhigh/lossless/hires"),
    }
}

fn normalize_quality(q: &str) -> &'static str {
    match q {
        "high" => "exhigh",
        "super" | "auto" => "lossless",
        "master" => "hires",
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

        if let Err(e) = tags::write_tags(&path, &track.title, &track.artists, &track.album) {
            ui::info(&format!("标签写入失败（不影响文件）: {e}"));
        }
        result.bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        result.audio = Some(path.display().to_string());
        result.source = source.clone();
    }

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
        if got.is_empty() && result.audio.is_none() && result.error.is_none() {
            result.error = Some("无可用歌词".into());
        }
        if result.source == "—" {
            result.source = source;
        }
    }

    result
}

/// 批量下载编排 + 汇总
async fn run_batch(
    ncm: &Ncm,
    targets: Vec<String>,
    search_kw: Option<&str>,
    search_limit: u32,
    download_sel: Option<&str>,
    opts: DownloadArgs,
) -> Result<i32> {
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

    if !opts.only_lrc {
        std::fs::create_dir_all(&output)?;
    }

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
        if download_sel.is_none() {
            print_search_table(&found);
            if opts.json {
                print_search_json(&found)?;
            }
            return Ok(0);
        }
        songs = pick_search(&found, download_sel.unwrap())?;
        scope.push(format!("搜索 “{kw}”({})", songs.len()));
    }

    for target in &targets {
        let (found, desc) = resolve_target(ncm, target, opts.artist).await?;
        scope.push(format!("{desc}({})", found.len()));
        songs.extend(found);
    }

    if songs.is_empty() {
        ui::fail("没有可处理的曲目");
        return Ok(1);
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
            vec![
                (i + 1).to_string(),
                s["name"].as_str().unwrap_or("").to_string(),
                first_artist(s),
                s["album"]["name"].as_str().unwrap_or("").to_string(),
                hms(s["duration"].as_u64().unwrap_or(0)),
                s["id"].as_u64().unwrap_or(0).to_string(),
            ]
        })
        .collect();
    ui::table(&["#", "歌名", "歌手", "专辑", "时长", "ID"], &rows);
    ui::info("下载指定项: ncm search 关键词 --download 1  （或 1,3 / all）");
}

fn print_search_json(songs: &[Value]) -> Result<()> {
    let arr: Vec<Value> = songs
        .iter()
        .map(|s| {
            json!({
                "id": s["id"],
                "name": s["name"],
                "artists": s["artists"].as_array().map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>()),
                "album": s["album"]["name"],
                "duration_ms": s["duration"],
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&Value::Array(arr))?);
    Ok(())
}

fn pick_search(songs: &[Value], selector: &str) -> Result<Vec<Value>> {
    if selector.eq_ignore_ascii_case("all") {
        return Ok(songs.to_vec());
    }
    let mut picked = Vec::new();
    for token in selector.split([',', ' ']) {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        match t.parse::<usize>() {
            Ok(n) if n >= 1 && n <= songs.len() => picked.push(songs[n - 1].clone()),
            _ => bail!("选择超出范围: {t}（共 {} 条）", songs.len()),
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
            "items": json_items,
            "summary": {"audio": audio, "lrc": lrc, "bytes": bytes,
                        "skipped": skipped, "failed": failed},
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(if failed.is_empty() { 0 } else { 1 });
    }

    println!();
    let mut parts: Vec<String> = Vec::new();
    if opts.only_lrc {
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

const SUBCOMMANDS: [&str; 5] = ["search", "dl", "convert", "lyrics", "watch"];

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
    match cli.cmd {
        Some(Commands::Search {
            keyword,
            limit,
            download,
            opts,
        }) => {
            ui::set_json(opts.json);
            let kw = keyword.join(" ");
            let ncm = Ncm::new()?;
            if opts.json && download.is_none() {
                let found = ncm.search(&kw, limit).await?;
                print_search_json(&found)?;
                return Ok(0);
            }
            run_batch(
                &ncm,
                Vec::new(),
                Some(&kw),
                limit,
                download.as_deref(),
                opts,
            )
            .await
        }
        Some(Commands::Dl {
            targets,
            search,
            limit,
            opts,
        }) => {
            ui::set_json(opts.json);
            if targets.is_empty() && search.is_none() {
                bail!("请提供歌曲 ID / 链接，或用 --search 关键词");
            }
            let ncm = Ncm::new()?;
            run_batch(&ncm, targets, search.as_deref(), limit, None, opts).await
        }
        // 快捷方式：`ncm 259138` == `ncm dl 259138`
        None => {
            let raw: Vec<String> = std::env::args().skip(1).collect();
            let positional = raw.iter().find(|a| !a.starts_with('-'));
            if positional.is_none() {
                Cli::command().print_help()?;
                return Ok(0);
            }
            let mut args = vec!["dl".to_string()];
            args.extend(raw);
            let cli2 = Cli::parse_from(args);
            match cli2.cmd {
                Some(Commands::Dl {
                    targets,
                    search,
                    limit,
                    opts,
                }) => {
                    ui::set_json(opts.json);
                    let ncm = Ncm::new()?;
                    run_batch(&ncm, targets, search.as_deref(), limit, None, opts).await
                }
                _ => unreachable!(),
            }
        }
    }
}

use clap::CommandFactory;
