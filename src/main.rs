//! ncm — 网易云音乐统一命令行工具（搜索 / 下载 / 歌词）
mod api;
mod config;
mod crypto;
mod dns;
mod download;
mod lrc;
mod mirror;
mod model;
mod player;
mod retry;
mod searchtype;
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
    /// 指定本次命令使用的 profile 名（优先级高于板块绑定；需已 ncm login --profile 保存）
    #[arg(short = 'u', long = "user", global = true, value_name = "NAME")]
    user: Option<String>,
    #[command(subcommand)]
    cmd: Option<Commands>,
}

/// recommend 的推荐位枚举（clap ValueEnum 自动列出可能值）
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum RecKind {
    /// 每日推荐歌曲（需登录）
    Daily,
    /// 官方榜单
    Toplist,
    /// 私人FM 首批曲目
    Fm,
    /// 私人雷达歌单
    Radar,
}

#[derive(Subcommand)]
enum Commands {
    /// 搜索并列出表格（-t 选类型如 1,1000；下载用 ncm download <关键词>）
    Search {
        /// 搜索关键词（可多个词，自动合并）
        #[arg(required = true)]
        keyword: Vec<String>,
        /// 搜索类型，逗号分隔（数字或别名，多类型合并为一张表）
        #[arg(
            short = 't',
            long = "type",
            value_delimiter = ',',
            value_parser = searchtype::parse_type_token,
            default_value = "1",
            long_help = searchtype::TYPE_HELP
        )]
        types: Vec<u32>,
        /// 输出总条数（多类型时平分；0 = 全部）
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 下载（唯一下载入口，别名 dl）：歌曲 ID / 歌单·专辑链接 / 关键词；--daily 下每日推荐
    #[command(name = "download", visible_alias = "dl")]
    Download {
        /// 歌曲ID / 歌单·专辑·歌手链接 / list.123 / 关键词
        targets: Vec<String>,
        /// 下载每日推荐（需登录；可与其它目标叠加）
        #[arg(long)]
        daily: bool,
        #[command(flatten)]
        opts: DownloadArgs,
    },
    /// 浏览歌单/专辑/歌手的作品列表（下载用 ncm download <链接>）
    List {
        /// 歌单/专辑/歌手 ID 或链接
        target: String,
        /// 只列出前 N 首（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 歌单管理（需登录；list=查看我的歌单，默认操作"我喜欢的音乐"）
    #[command(visible_alias = "pl")]
    Playlist {
        #[command(subcommand)]
        action: Option<PlaylistAction>,
    },
    /// 收藏/取消收藏歌曲到"我喜欢的音乐"（需登录）
    Like {
        /// 歌曲 ID / 链接，可多个
        ids: Vec<String>,
        /// 取消收藏（从红心歌单移除）
        #[arg(long)]
        unlike: bool,
    },
    /// 每日推荐歌曲（需登录；下载: ncm download --daily · 播放: ncm play --daily）
    Daily {
        /// 只列出前 N 首（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 首页推荐（-t daily,toplist,fm,radar，默认 daily 只列每日推荐歌曲）
    Recommend {
        /// 推荐位，逗号分隔：daily 每日推荐 / toplist 榜单 / fm 私人FM / radar 私人雷达
        #[arg(
            short = 't',
            long = "type",
            value_delimiter = ',',
            default_value = "daily"
        )]
        types: Vec<crate::RecKind>,
        /// 输出总条数（多推荐位平分；0 = 全部）
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
        /// 会话 profile 名（多账号：保存到 ~/.config/ncm/cookie.<名>；配 profile.<板块> 绑定到板块）
        #[arg(long, value_name = "名称")]
        profile: Option<String>,
    },
    /// 查看当前登录状态
    Whoami,
    /// 持久化配置（download-dir / play.download-key / profile.<板块>，写入 ~/.config/ncm/config.json）
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
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
    /// 歌曲热门评论（需登录；ID / 链接）
    Comment {
        /// 歌曲 ID / 链接
        target: String,
        /// 显示前 N 条热门评论
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 每日签到（需登录；移动端 +4 经验）
    Sign {
        /// 网页端签到（+1 经验；默认移动端 +4）
        #[arg(long)]
        web: bool,
    },
    /// 收藏的专辑 / 歌手列表（需登录）
    Subs {
        /// 类型，逗号分隔：album 专辑 / artist 歌手（默认两者）
        #[arg(
            short = 't',
            long = "type",
            value_delimiter = ',',
            default_value = "album,artist"
        )]
        types: Vec<String>,
        /// 每类最多条数
        #[arg(short = 'n', long, default_value_t = 30)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// MV 详情与播放地址（--play 直接交给 mpv 播放）
    Mv {
        /// MV ID
        id: u64,
        /// 清晰度：240/480/720/1080
        #[arg(long, default_value_t = 1080, value_parser = clap::builder::TypedValueParser::map(clap::builder::PossibleValuesParser::new(["240", "480", "720", "1080"]), |s: String| s.parse::<u32>().unwrap()))]
        res: u32,
        /// 用 mpv 播放该 MV
        #[arg(long)]
        play: bool,
        /// 机器可读的 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 云盘歌曲列表（需登录；下载用 ncm download <ID>）
    Cloud {
        /// 最多条数
        #[arg(short = 'n', long, default_value_t = 30)]
        limit: u32,
        /// 偏移（翻页）
        #[arg(long, default_value_t = 0)]
        offset: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 私人FM 操作：红心 / 踩 / 跳过（需登录）
    Fm {
        #[command(subcommand)]
        action: FmAction,
    },
    /// 播放歌曲（队列喂给 mpv/ffplay，mpv 下 </> 切曲、←/→ 快退进、歌词同步滚动）
    Play {
        /// 歌曲 ID / 链接 / 关键词（可多个，按序拼接）
        target: Vec<String>,
        /// 播放每日推荐队列（需登录；可与其它目标叠加，置于最前）
        #[arg(help_heading = "队列", long)]
        daily: bool,
        /// 从文件读取目标列表（每行一个，与命令行目标拼接）
        #[arg(short = 'f', long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// 最多播放 N 首（0 = 全部；关键词默认取最相关 1 首，-n 指定则取前 N 首连续播）
        #[arg(help_heading = "队列", short = 'n', long, default_value_t = 0)]
        limit: u32,
        /// 播放队列随机洗牌
        #[arg(help_heading = "队列", long)]
        shuffle: bool,
        /// 指定播放器程序（默认探测 $NCM_PLAYER → mpv → ffplay）
        #[arg(help_heading = "播放器", long, value_name = "CMD")]
        player: Option<String>,
        /// 音质
        #[arg(help_heading = "播放器", long, default_value = "lossless", value_parser = ["lossless", "standard", "exhigh", "hires"])]
        quality: String,
        /// 只输出播放队列 JSON（不调用播放器）
        #[arg(help_heading = "输出与保存", long)]
        json: bool,
        /// 播放前先下载到本地并播放本地文件
        #[arg(help_heading = "输出与保存", long)]
        save: bool,
        /// 下载目录（--save / 快捷下载时生效；未指定取 config download-dir）
        #[arg(help_heading = "输出与保存", short = 'o', long, value_name = "目录")]
        output: Option<PathBuf>,
        /// 不显示滚动歌词
        #[arg(help_heading = "播放器", long = "no-lyric")]
        no_lyric: bool,
        /// 播放漫游：队列将尽时自动接私人FM无限续播（需 mpv；单独 --roam 即纯 FM 开播）
        #[arg(help_heading = "队列", long)]
        roam: bool,
        /// 透传给播放器的参数（经 `--` 分隔，置于最后同名以用户为准；mpv 需 `=` 形式，如 -- --volume=60 --ao=alsa）
        #[arg(help_heading = "播放器", last = true, value_name = "ARGS")]
        player_args: Vec<std::ffi::OsString>,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// 写入配置项（如: ncm config set download-dir /sdcard/Music/NCMusic）
    Set {
        /// 配置键（download-dir / play.download-key / profile.<板块>）
        key: String,
        /// 配置值
        value: String,
    },
    /// 读取单个配置项
    Get {
        /// 配置键
        key: String,
    },
    /// 列出全部已配置项
    List,
    /// 删除配置项
    Unset {
        /// 配置键
        key: String,
    },
}

/// 歌单管理子命令
#[derive(Subcommand)]
enum PlaylistAction {
    /// 列出我创建/收藏的歌单（下载用 ncm download list.<ID>）
    List {
        /// 最多显示数量（0 = 全部）
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 新建歌单
    Create {
        /// 歌单名
        name: String,
    },
    /// 删除歌单（含全部歌曲，不可恢复）
    Delete {
        /// 歌单名或 ID（默认"我喜欢的音乐"）
        target: Option<String>,
        /// 跳过确认直接删除
        #[arg(long)]
        force: bool,
    },
    /// 歌单改名
    Rename {
        /// 歌单名或 ID（默认"我喜欢的音乐"）
        target: Option<String>,
        /// 新名称
        name: String,
    },
    /// 添加歌曲到歌单（ID 或链接，可多个）
    Add {
        /// 歌曲 ID / 链接，可多个
        ids: Vec<String>,
        /// 目标歌单名或 ID（默认"我喜欢的音乐"）
        #[arg(short = 'p', long = "playlist", value_name = "名或ID")]
        playlist: Option<String>,
    },
    /// 从歌单移除歌曲（ID，可多个）
    Remove {
        /// 歌曲 ID，可多个
        ids: Vec<u64>,
        /// 目标歌单名或 ID（默认"我喜欢的音乐"）
        #[arg(short = 'p', long = "playlist", value_name = "名或ID")]
        playlist: Option<String>,
    },
}

/// 私人FM 操作子命令
#[derive(Subcommand)]
enum FmAction {
    /// 红心当前 FM 歌曲（推荐算法会多推同类）
    Like {
        /// 歌曲 ID
        id: u64,
        /// 取消红心
        #[arg(long)]
        unlike: bool,
    },
    /// 踩：移除并少推同类
    Trash {
        /// 歌曲 ID
        id: u64,
    },
    /// 跳过：拉下一批 FM 歌曲并列出
    Skip {
        /// 当前歌曲 ID（0 = 不上报）
        #[arg(default_value_t = 0)]
        id: u64,
        #[command(flatten)]
        opts: ViewArgs,
    },
    /// 拉取一批 FM 歌曲
    Get {
        /// 拉取条数
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
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
    /// 关键词候选取前 N 首（仅 download 关键词时生效；TTY 下可交互选择，非 TTY 无 -d 取 1 首）
    #[arg(help_heading = "选择", short = 'n', long, default_value_t = 10)]
    limit: u32,
    /// 音频输出目录（未指定时取 config download-dir，再未配置为当前目录）
    #[arg(help_heading = "输出", short = 'o', long, value_name = "目录")]
    output: Option<PathBuf>,
    /// 歌词输出目录（可重复指定多个；默认与音频同目录）
    #[arg(help_heading = "歌词", long = "lrc-dir", value_name = "目录")]
    lrc_dir: Vec<PathBuf>,
    /// 要下载的歌词类型，逗号分隔
    #[arg(help_heading = "歌词", long, default_value = LRC_DEFAULT)]
    lrcs: String,
    /// 不下载任何歌词
    #[arg(help_heading = "歌词", long = "no-lrc")]
    no_lrc: bool,
    /// 只下载歌词，不保存音频
    #[arg(help_heading = "歌词", long = "only-lrc", alias = "lyric-only")]
    only_lrc: bool,
    /// 音质：standard/exhigh/lossless/hires
    #[arg(help_heading = "输出", long, default_value = "lossless")]
    quality: String,
    /// 静默：隐藏进度动画与过程输出，仅保留结果
    #[arg(help_heading = "执行", short = 'q', long)]
    quiet: bool,
    /// 预演：取址并报告将写入的文件，不实际下载
    #[arg(help_heading = "执行", long)]
    dry_run: bool,
    /// 文件名模板
    #[arg(
        help_heading = "输出",
        short = 't',
        long,
        default_value = "{artists} - {track}"
    )]
    template: String,
    /// 跳过已存在的音频文件
    #[arg(
        help_heading = "输出",
        long = "skip-existing",
        visible_alias = "no-overwrites"
    )]
    skip_existing: bool,
    /// 允许下载 30 秒试听片段（默认跳过）
    #[arg(help_heading = "输出", long = "allow-trial")]
    allow_trial: bool,
    /// 按歌手维度解析目标
    #[arg(help_heading = "选择", short = 'a', long = "artist", alias = "ar")]
    artist: bool,
    /// 机器可读的 JSON 输出
    #[arg(long)]
    json: bool,
    /// 按编号选择要下载的项：1,3 / 5-8 / 1- / all（download 结果统一生效）
    #[arg(help_heading = "选择", short = 'd', long)]
    download: Option<String>,
    /// 并发下载数（>1 时关闭逐曲转圈/行内进度，仅保留结果行）
    #[arg(help_heading = "执行", short = 'j', long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=16))]
    jobs: u32,
    /// 下载完成后导出 M3U 播放列表（含成功曲目，路径相对播放列表所在目录）
    #[arg(help_heading = "输出", long, value_name = "文件")]
    m3u: Option<PathBuf>,
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
    /// 歌词接口失败原因（音频成功时也报告）
    lrc_err: Option<String>,
    source: String,
    /// 曲长（毫秒，M3U EXTINF 用）
    duration_ms: u64,
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
/// 从歌曲 json 三处兼容取 picUrl（al/album/picUrl），空串视为缺失
fn pick_pic_url(song: &Value) -> Option<&str> {
    song["al"]["picUrl"]
        .as_str()
        .or_else(|| song["album"]["picUrl"].as_str())
        .or_else(|| song["picUrl"].as_str())
        .filter(|u| !u.is_empty())
}

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
        lrc_err: None,
        source: "—".into(),
        duration_ms: track.duration_ms,
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
            && ["mp3", "flac", "m4a", "wav", "ogg"]
                .iter()
                .any(|e| crate::download::append_ext(&audio_base, e).exists())
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
        let got = match lrc::fetch_lrcs(ncm, track.id, lrc_kinds).await {
            Ok(g) => {
                spin.stop();
                g
            }
            Err(e) => {
                spin.stop();
                ui::info(&format!("歌词获取失败（不影响音频）: {e:#}"));
                result.lrc_err = Some(format!("{e:#}"));
                std::collections::HashMap::new()
            }
        };
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
        embed_lyrics = lrc::pick_embed(&got);
        if got.is_empty() && result.audio.is_none() && result.error.is_none() {
            result.error = Some("无可用歌词".into());
        }
        if result.source == "—" {
            result.source = source;
        }
    }

    // 封面（仅音频存在时；下载失败不阻塞）
    if result.audio.is_some() {
        // 搜索接口返回的 json 不带 picUrl，先取本地三处，缺失则二跳 song/detail 补全
        let mut pic = pick_pic_url(song).map(str::to_owned);
        if pic.is_none() {
            let detail = ncm.song_detail(&[track.id]).await.ok();
            pic = detail
                .as_ref()
                .and_then(|v| v.first())
                .and_then(pick_pic_url)
                .map(str::to_owned);
        }
        match pic {
            Some(url) => match ncm.fetch_bytes(&url, 8 * 1024 * 1024).await {
                Ok(bytes) => embed_cover = Some(bytes),
                Err(e) => ui::info(&format!("封面下载失败（不影响文件）: {e}")),
            },
            None => ui::info("该曲无封面信息，跳过嵌入"),
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
            Some(track.id),
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

/// 旧目标别名迁移提示：daily/today/roam 现为 --daily/--roam 标志，
/// 字面命中时本次按关键词搜索，提示避免静默换义后用户困惑（json/quiet 下自动静默）
fn hint_legacy_alias(targets: &[String], is_play: bool) {
    if targets
        .iter()
        .any(|t| matches!(t.as_str(), "daily" | "today"))
    {
        ui::info("daily/today 已移除目标别名，每日推荐请用 --daily（本次按关键词搜索）");
    }
    if is_play && targets.iter().any(|t| t == "roam") {
        ui::info("roam 已移除目标别名，私人漫游请用 --roam（本次按关键词搜索）");
    }
}

/// download 的批量编排：收集曲目（--daily / ID / 链接 / 关键词）→ TTY 交互选号 → 下载
///
/// 关键词取 `-n` 候选；非 TTY 且未给 `-d` 时取最相关 1 首（兼容旧用法），
/// TTY 下列出候选表并交互选择（回车仅展示不下载）。
async fn run_batch(
    ncm: &Ncm,
    targets: Vec<String>,
    opts: DownloadArgs,
    daily: bool,
) -> Result<i32> {
    let mut songs: Vec<Value> = Vec::new();
    let mut scope: Vec<String> = Vec::new();
    let tty = !opts.json && std::io::IsTerminal::is_terminal(&std::io::stdin());
    let interactive = tty && opts.download.is_none();

    // --daily：每日推荐置于最前（与其它目标叠加）
    if daily {
        let s = ncm.daily_songs().await?;
        if s.is_empty() {
            bail!("每日推荐为空（该接口需登录：ncm login --session/--cookie）");
        }
        scope.push(format!("每日推荐({})", s.len()));
        songs.extend(s);
    }

    // 多目标逐个解析：单个失败记录并继续（全部失败才整体报错），不再一错全盘放弃
    let mut target_errs: Vec<String> = Vec::new();
    for target in &targets {
        let step: Result<()> = if is_target_like(target) {
            let (found, desc) = resolve_target(ncm, target, opts.artist).await?;
            scope.push(format!("{desc}({})", found.len()));
            songs.extend(found);
            Ok(())
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
            Ok(())
        };
        if let Err(e) = step {
            target_errs.push(format!("{target}: {e:#}"));
        }
    }
    for e in &target_errs {
        ui::fail(&format!("目标解析失败 · {e}"));
    }

    if songs.is_empty() {
        if scope.is_empty() && !target_errs.is_empty() {
            bail!("全部目标解析失败: {}", target_errs.join("；"));
        }
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

    let out_arg = opts
        .output
        .clone()
        .or_else(config::download_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let output = out_arg.canonicalize().unwrap_or_else(|_| {
        std::fs::create_dir_all(&out_arg).ok();
        std::fs::canonicalize(&out_arg).unwrap_or_else(|_| out_arg.clone())
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
    // 结果行打印（两分支共用）：按完成顺序输出，编号取自原下标
    let show = |idx: usize, r: &ItemResult| {
        if json_flag {
            // JSON 模式逐条静默，最后统一输出
        } else if opts.dry_run {
            // dry-run 行已在 download_one 打印
        } else if let Some(s) = &r.skipped {
            ui::skip(&format!(
                "{}/{} {} · {s}",
                idx + 1,
                total,
                truncate(&r.title)
            ));
        } else if let Some(e) = &r.error {
            ui::fail(&format!(
                "{}/{} {} · {}",
                idx + 1,
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
                idx + 1,
                total,
                truncate(&r.title),
                parts.join(" · ")
            ));
        }
    };

    if opts.jobs > 1 {
        // 并发分支：--jobs N 限流，结果行按完成顺序即时输出，最后按原下标排序出报告
        // 注：future 在 for 循环里直接构造（闭包写法会触发 spawn 的 HRTB 泛型提升失败）
        use futures_util::StreamExt;
        ui::set_concurrent(true);
        // 预绑定引用：async move 块内直接用会逐次 move 外层拥有的值
        let opts_r = &opts;
        let lrc_kinds_r = &lrc_kinds;
        let quality_r = &quality;
        let output_r = &output;
        let lrc_dirs_r = &lrc_dirs;
        let mut futs = Vec::with_capacity(songs.len());
        for (i, song) in songs.iter().enumerate() {
            let fut = async move {
                let r = download_one(
                    ncm,
                    song,
                    opts_r,
                    lrc_kinds_r,
                    quality_r,
                    output_r,
                    lrc_dirs_r,
                    i + 1,
                    total,
                )
                .await;
                (i, r)
            };
            futs.push(fut);
        }
        let mut paired: Vec<(usize, ItemResult)> = Vec::new();
        let mut stream = futures_util::stream::iter(futs).buffer_unordered(opts.jobs as usize);
        while let Some((i, r)) = stream.next().await {
            show(i, &r);
            paired.push((i, r));
        }
        ui::set_concurrent(false);
        paired.sort_by_key(|(i, _)| *i);
        items = paired.into_iter().map(|(_, r)| r).collect();
    } else {
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
            show(i, &r);
            items.push(r);
        }
    }

    if let Some(m3u) = &opts.m3u {
        write_m3u(m3u, &items)?;
    }

    let code = report(&items, &opts, &output, started.elapsed())?;
    Ok((code, items))
}

/// 导出 M3U 播放列表：仅成功下载的曲目，路径相对播放列表所在目录（无法相对时用绝对路径）
fn write_m3u(path: &std::path::Path, items: &[ItemResult]) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut out = String::from("#EXTM3U\n");
    let mut n = 0usize;
    for it in items.iter().filter(|i| i.audio.is_some()) {
        let audio = std::path::Path::new(it.audio.as_deref().unwrap_or(""));
        let rel = audio.strip_prefix(&base).unwrap_or(audio);
        out.push_str(&format!(
            "#EXTINF:{},{title} - {artists}\n{file}\n",
            (it.duration_ms / 1000).min(i64::MAX as u64),
            title = it.title,
            artists = it.artists,
            file = rel.display(),
        ));
        n += 1;
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(out.as_bytes())?;
    ui::ok(&format!("M3U 导出 {n} 首 → {}", path.display()));
    Ok(())
}

fn truncate(s: &str) -> String {
    ui::truncate_width(s, 30)
}

/// Unix 秒 → "YYYY-MM-DD"（Howard Hinnant 民用历算法，无 chrono 依赖）
fn fmt_unix_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
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

/// 多类型统一表：通用列 + 类型列（单组 type=1 仍走 print_search_table 保持原状）
fn print_render_table(rows: &[searchtype::RenderRow]) {
    let body: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            vec![
                (i + 1).to_string(),
                r.name.clone(),
                r.artist.clone(),
                r.meta.clone(),
                r.label.to_string(),
                r.id.clone(),
            ]
        })
        .collect();
    ui::table(
        &["#", "名称", "艺术家·创建者", "时长·曲数", "类型", "ID"],
        &body,
    );
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
                    "lrc_error": i.lrc_err,
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
                } else if let Some(e) = &r.lrc_err {
                    ui::warn(&format!("歌词失败: {}", ui::truncate_width(e, 20)))
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

/// 并发检测已存各 profile 的 VIP，返回第一个 VIP 的 profile 名；全非 VIP/仅 default → None
async fn detect_vip_profile() -> Option<String> {
    let profiles = session::list_profiles();
    if profiles.len() <= 1 {
        return None;
    }
    let checks: Vec<_> = profiles
        .iter()
        .map(|p| {
            let p = p.clone();
            async move {
                let cookie = session::load_profile(Some(&p))?;
                let ncm = api::Ncm::with_cookie(Some(&cookie)).ok()?;
                let vip = ncm.vip_status().await.unwrap_or(false);
                vip.then_some(p)
            }
        })
        .collect();
    let found = futures_util::future::join_all(checks).await;
    let hit = found.into_iter().flatten().next()?;
    ui::info(&format!("检测到 profile `{hit}` 为 VIP，自动选用"));
    Some(hit)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match run(cli).await {
        Ok(c) => c,
        Err(e) => {
            ui::fail(&format!("{e:#}"));
            1
        }
    };
    std::process::exit(code);
}

/// 命令 → 板块名（决定用哪个 profile 凭据：`ncm config set profile.<板块> <名>`）
fn section_of(cmd: Option<&Commands>) -> &'static str {
    match cmd {
        Some(Commands::Search { .. }) => "search",
        Some(Commands::Download { .. }) => "download",
        Some(Commands::List { .. }) | Some(Commands::Playlist { .. }) => "list",
        Some(Commands::Daily { .. }) | Some(Commands::Recommend { .. }) => "recommend",
        Some(Commands::Play { daily, roam, .. }) => {
            // 板块优先级与目标一致：漫游 > 每日推荐 > 普通播放
            if *roam {
                "roam"
            } else if *daily {
                "recommend"
            } else {
                "play"
            }
        }
        _ => "default",
    }
}

/// 歌单目标解析：名称或 ID → (id, name)；target 为 None 时取"我喜欢的音乐"
fn resolve_playlist(lists: &[Value], target: Option<&str>) -> Result<(u64, String)> {
    let Some(t) = target else {
        // 默认红心歌单（subscribed=false 且名称匹配）
        for p in lists {
            if p["name"].as_str() == Some("我喜欢的音乐") {
                return Ok((p["id"].as_u64().unwrap_or(0), "我喜欢的音乐".to_string()));
            }
        }
        // 兜底：第一个自己创建的歌单
        for p in lists {
            if p["subscribed"].as_bool() == Some(false) {
                let name = p["name"].as_str().unwrap_or("").to_string();
                return Ok((p["id"].as_u64().unwrap_or(0), name));
            }
        }
        anyhow::bail!("未找到歌单，请用 -p 指定");
    };
    // 纯数字 → 按 ID 匹配
    if t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty() {
        let id: u64 = t.parse()?;
        for p in lists {
            if p["id"].as_u64() == Some(id) {
                let name = p["name"].as_str().unwrap_or("").to_string();
                return Ok((id, name));
            }
        }
        return Ok((id, t.to_string())); // 不在列表也允许（收藏的歌单等）
    }
    // 名称匹配（精确优先，再前缀）
    for p in lists {
        if p["name"].as_str() == Some(t) {
            let name = p["name"].as_str().unwrap_or("").to_string();
            return Ok((p["id"].as_u64().unwrap_or(0), name));
        }
    }
    for p in lists {
        if let Some(n) = p["name"].as_str() {
            if n.starts_with(t) {
                return Ok((p["id"].as_u64().unwrap_or(0), n.to_string()));
            }
        }
    }
    anyhow::bail!("未找到歌单「{t}」，可用 ncm pl list 查看")
}

/// 歌曲 ID / 链接 → u64（纯数字或 URL 尾段）
fn parse_id_or_url(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
        return s.parse().map_err(|_| anyhow::anyhow!("无效 ID: {s}"));
    }
    if s.starts_with("http") {
        if let Some(pos) = s.find("?id=") {
            let id = &s[pos + 4..];
            let id = id.split(['&', '#']).next().unwrap_or("");
            if !id.is_empty() {
                return id
                    .parse()
                    .map_err(|_| anyhow::anyhow!("无法从链接解析 ID: {s}"));
            }
        }
        let path = s.split('?').next().unwrap_or("");
        let tail = path.trim_end_matches('/');
        if let Some((_, last)) = tail.rsplit_once('/') {
            if !last.is_empty() && last.chars().all(|c| c.is_ascii_digit()) {
                return last
                    .parse()
                    .map_err(|_| anyhow::anyhow!("无法从链接解析 ID: {s}"));
            }
        }
    }
    anyhow::bail!("无法识别的歌曲目标: {s}")
}

async fn run(cli: Cli) -> Result<i32> {
    ui::set_color(!cli.no_color && std::env::var_os("NO_COLOR").is_none());
    mirror::init(cli.mirror.as_deref());
    // 登录凭据：--session/--cookie 优先，回退 NCM_SESSION/NCM_COOKIE 环境变量，
    // 再 --user 显式指定，再按板块绑的 profile
    let session = cli
        .session
        .clone()
        .or_else(|| std::env::var_os("NCM_SESSION").map(std::path::PathBuf::from));
    let cookie = cli
        .cookie
        .clone()
        .or_else(|| std::env::var("NCM_COOKIE").ok());
    // download 且未显式指定凭据时，自动检测已存 profile 的 VIP 并优先
    let user = if cli.user.is_some()
        || session.is_some()
        || cookie.is_some()
        || !matches!(cli.cmd, Some(Commands::Download { .. }))
    {
        cli.user.clone()
    } else {
        detect_vip_profile().await
    };
    let auth = session::resolve_auth_for(
        section_of(cli.cmd.as_ref()),
        session.as_deref(),
        cookie.as_deref(),
        user.as_deref(),
    )?;
    match cli.cmd {
        Some(Commands::Search {
            keyword,
            types,
            limit,
            opts,
        }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let kw = keyword.join(" ");
            let _sp = ui::Spinner::start("搜索中…");
            let types = searchtype::dedup_types(types);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let qs = searchtype::quotas(limit, types.len());
            let mut groups: Vec<(u32, Vec<(u32, Value)>)> = Vec::new();
            for (&ty, &q) in types.iter().zip(&qs) {
                // 配额 0 = 不限，请求上限 100
                let mut items = ncm
                    .search_typed(&kw, ty, if q == 0 { 100 } else { q })
                    .await?;
                // 服务端不遵守 limit 时按配额截断（JSON 与表格分支共用此处）
                searchtype::truncate_to_quota(&mut items, q);
                groups.push((ty, items));
            }
            let total: usize = groups.iter().map(|(_, v)| v.len()).sum();
            if opts.json {
                if groups.len() == 1 {
                    let items: Vec<Value> = groups[0].1.iter().map(|(_, v)| v.clone()).collect();
                    print_search_json(&items)?;
                } else {
                    // 多类型：{ "1": [...], "1000": [...] }，键按 type 字符串字典序
                    // （serde_json::Map 默认 BTreeMap，非输入顺序）
                    let mut obj = serde_json::Map::new();
                    for (ty, items) in &groups {
                        obj.insert(
                            ty.to_string(),
                            Value::Array(items.iter().map(|(_, v)| v.clone()).collect()),
                        );
                    }
                    println!("{}", serde_json::to_string_pretty(&Value::Object(obj))?);
                }
            } else if total == 0 {
                ui::info(&format!("未搜索到结果: {kw}"));
            } else if groups.len() == 1 && groups[0].0 == 1 {
                // 单组单曲：保持原输出不变
                let items: Vec<Value> = groups[0].1.iter().map(|(_, v)| v.clone()).collect();
                print_search_table(&items);
                ui::info(&format!("下载: ncm download \"{kw}\" -d 编号"));
            } else {
                let mut rows: Vec<searchtype::RenderRow> = Vec::new();
                for (_ty, items) in &groups {
                    rows.extend(items.iter().map(|(t, v)| searchtype::extract_row(*t, v)));
                }
                print_render_table(&rows);
                ui::info(&format!(
                    "下载: ncm download \"{kw}\" -d 编号  ·  浏览歌单: ncm list <ID>"
                ));
            }
            Ok(0)
        }
        Some(Commands::Download {
            targets,
            daily,
            opts,
        }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            if targets.is_empty() && !daily {
                bail!(
                    "请提供歌曲 ID / 歌单·专辑链接 / 关键词，或加 --daily 下每日推荐（ncm help download）"
                );
            }
            hint_legacy_alias(&targets, false);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            run_batch(&ncm, targets, opts, daily).await
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
                ui::info(&format!("下载: ncm download \"{target}\" -d 编号"));
            }
            Ok(0)
        }
        Some(Commands::Daily { limit, opts }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let _sp = ui::Spinner::start("读取每日推荐…");
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
                ui::info("下载: ncm download --daily -d 编号  ·  播放: ncm play --daily");
            }
            Ok(0)
        }
        Some(Commands::Recommend { types, limit, opts }) => {
            let _sp = ui::Spinner::start("读取首页推荐…");
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            // 去重保序
            let mut kinds: Vec<RecKind> = Vec::new();
            for &k in &types {
                if !kinds.contains(&k) {
                    kinds.push(k);
                }
            }
            let qs = searchtype::quotas(limit, kinds.len());
            // 各源取曲目：单曲源 Vec<Value>；榜单是歌单列表单独处理
            let mut song_rows: Vec<searchtype::RenderRow> = Vec::new();
            let mut topo_rows: Vec<searchtype::RenderRow> = Vec::new();
            let mut json_out = serde_json::Map::new();
            let mut errors: Vec<String> = Vec::new();
            for (&k, &q) in kinds.iter().zip(&qs) {
                let cap = if q == 0 { usize::MAX } else { q as usize };
                let key = match k {
                    RecKind::Daily => "daily",
                    RecKind::Toplist => "toplist",
                    RecKind::Fm => "fm",
                    RecKind::Radar => "radar",
                };
                match k {
                    RecKind::Daily => match ncm.daily_songs().await {
                        Ok(mut s) => {
                            if s.len() > cap {
                                s.truncate(cap);
                            }
                            json_out.insert(key.into(), Value::Array(s.clone()));
                            song_rows.extend(s.iter().map(|v| searchtype::extract_row(1, v)));
                        }
                        Err(e) => errors.push(format!("每日推荐失败: {e:#}")),
                    },
                    RecKind::Fm => match ncm.radio_songs().await {
                        Ok(mut s) => {
                            if s.len() > cap {
                                s.truncate(cap);
                            }
                            json_out.insert(key.into(), Value::Array(s.clone()));
                            song_rows.extend(s.iter().map(|v| searchtype::extract_row(1, v)));
                        }
                        Err(e) => errors.push(format!("私人FM失败: {e:#}")),
                    },
                    RecKind::Toplist => match ncm.toplist().await {
                        Ok(mut t) => {
                            if t.len() > cap {
                                t.truncate(cap);
                            }
                            json_out.insert(key.into(), Value::Array(t.clone()));
                            topo_rows.extend(t.iter().map(|it| {
                                searchtype::RenderRow {
                                    name: it["name"].as_str().unwrap_or("-").into(),
                                    artist: it["updateFrequency"].as_str().unwrap_or("-").into(),
                                    meta: format!("{} 首", it["trackCount"].as_u64().unwrap_or(0)),
                                    id: it["id"]
                                        .as_u64()
                                        .map(|x| x.to_string())
                                        .unwrap_or_else(|| "-".into()),
                                    label: "榜单",
                                }
                            }));
                        }
                        Err(e) => errors.push(format!("榜单获取失败: {e:#}")),
                    },
                    RecKind::Radar => {
                        // 每日推荐歌单中筛「私人雷达」；接口失败或未命中给统一兜底
                        let found = ncm.daily_playlists().await.ok().and_then(|pls| {
                            pls.iter()
                                .find(|p| {
                                    p["name"].as_str().is_some_and(|n| n.contains("私人雷达"))
                                })
                                .and_then(|p| p["id"].as_u64())
                        });
                        match found {
                            Some(id) => match resolve_list(&ncm, &id.to_string()).await {
                                Ok((mut songs, desc)) => {
                                    if songs.len() > cap {
                                        songs.truncate(cap);
                                    }
                                    json_out.insert(
                                        key.into(),
                                        json!({ "playlist_id": id, "desc": desc, "songs": songs }),
                                    );
                                    song_rows.extend(
                                        songs.iter().map(|v| searchtype::extract_row(1, v)),
                                    );
                                }
                                Err(e) => errors.push(format!("私人雷达歌单解析失败: {e:#}")),
                            },
                            None => errors.push(
                                "未找到私人雷达歌单（可 ncm list <私人雷达链接> 手动浏览）".into(),
                            ),
                        }
                    }
                }
            }
            if opts.json {
                if kinds.len() == 1 && kinds[0] != RecKind::Toplist {
                    // 单非榜单源：直接输出该源的值（daily/fm 为数组，radar 为
                    // {playlist_id,desc,songs} 对象），保持旧 daily JSON 形状
                    let v = json_out
                        .values()
                        .next()
                        .cloned()
                        .unwrap_or(Value::Array(vec![]));
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&Value::Object(json_out))?
                    );
                }
                if !errors.is_empty() {
                    bail!("{}", errors.join("；"));
                }
                return Ok(0);
            }
            let rows: Vec<searchtype::RenderRow> = song_rows.into_iter().chain(topo_rows).collect();
            if rows.is_empty() {
                if errors.is_empty() {
                    bail!("推荐内容为空（每日推荐/雷达需登录: ncm login）");
                }
                // 错误文案自身常带括号，用分号衔接登录提示，避免出现双括号
                bail!("{}；每日推荐/雷达需登录: ncm login", errors.join("；"));
            }
            print_render_table(&rows);
            for e in &errors {
                ui::info(e);
            }
            ui::info("下载: ncm download -d 编号  ·  浏览: ncm list <ID>");
            Ok(0)
        }
        Some(Commands::Play {
            target,
            daily,
            file,
            limit,
            shuffle,
            player,
            quality,
            json,
            save,
            output,
            no_lyric,
            roam,
            player_args,
        }) => {
            if json && save {
                bail!("--json 与 --save 互斥");
            }
            let _ = quality_br(&quality)?;
            // 下载目录：-o > config download-dir > 当前目录（--save 与快捷下载共用）
            let out_dir = output
                .clone()
                .or_else(config::download_dir)
                .unwrap_or_else(|| PathBuf::from("."));
            // 透传参数要求 UTF-8（与内部 args: Vec<String> 统一，避免静默失真）
            let player_args: Vec<String> = player_args
                .iter()
                .map(|a| {
                    a.to_str().map(str::to_string).with_context(|| {
                        format!("透传参数含非 UTF-8 字节: {}", a.to_string_lossy())
                    })
                })
                .collect::<Result<_>>()?;
            // guard 续喂成功计数（播放完毕报告用；仅 mpv+unix 分支会累加）
            let fed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            // 快捷下载用 download 板块凭据（可与播放板块不同，如 VIP 下载号）
            #[cfg(unix)]
            let dl_ncm = Ncm::with_cookie(
                session::resolve_auth_for(
                    "download",
                    session.as_deref(),
                    cookie.as_deref(),
                    user.as_deref(),
                )?
                .as_deref(),
            )?;

            // 1) 收集播放队列：命令行目标 + 文件目标拼接
            let mut targets: Vec<String> = target;
            if let Some(fp) = &file {
                let content = std::fs::read_to_string(fp)
                    .with_context(|| format!("读取目标文件失败: {}", fp.display()))?;
                for line in content.lines() {
                    let t = line.trim();
                    if !t.is_empty() && !t.starts_with('#') {
                        targets.push(t.to_string());
                    }
                }
            }
            if targets.is_empty() && !daily && !roam {
                bail!("未指定播放目标（歌曲 ID/链接/关键词、-f 文件，或 --daily 每日推荐 / --roam 私人漫游）");
            }
            hint_legacy_alias(&targets, true);
            let mut sp_collect = ui::Spinner::start("读取播放队列…");

            let mut songs: Vec<Value> = Vec::new();
            let mut descs: Vec<String> = Vec::new();
            // --daily：每日推荐置于队首（与其它目标叠加）
            if daily {
                let s = ncm.daily_songs().await?;
                if s.is_empty() {
                    bail!("每日推荐为空（该接口需登录：ncm login --session/--cookie）");
                }
                descs.push("每日推荐".to_string());
                songs.extend(s);
            }
            for t in &targets {
                let batch: Vec<Value> = if is_target_like(t) {
                    let (found, d) = resolve_target(&ncm, t, false).await?;
                    if found.is_empty() {
                        bail!("“{d}” 没有可播放的曲目");
                    }
                    descs.push(d);
                    found
                } else {
                    let n = if limit > 0 { limit } else { 1 };
                    let found = ncm.search(t, n).await?;
                    if found.is_empty() {
                        bail!("未搜索到结果: {t}");
                    }
                    descs.push(format!("搜索 “{t}”"));
                    found
                };
                songs.extend(batch);
            }
            // 纯 --roam（无其它目标）：私人FM 首批即队列
            if songs.is_empty() && roam {
                let s = ncm.radio_songs().await?;
                if s.is_empty() {
                    bail!("私人FM为空（该接口需登录：ncm login --session/--cookie）");
                }
                descs.push("私人漫游".to_string());
                songs.extend(s);
            }
            let desc = descs.join(" + ");
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
                    output: Some(out_dir.clone()),
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
                    jobs: 1,
                    m3u: None,
                };
                let (_code, items) = download_songs(
                    &ncm,
                    songs.clone(),
                    vec![format!("{desc} 下载到 {}", out_dir.display())],
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
            // 快捷下载队列：(曲 id, 标签)，与 mpv 播放列表索引对齐（guard 续入时同步 push）
            let queue: std::sync::Arc<std::sync::RwLock<Vec<(u64, String)>>> =
                std::sync::Arc::new(std::sync::RwLock::new(
                    play_tracks
                        .iter()
                        .map(|t| (t.id, format!("{} - {}", t.title, t.artists)))
                        .collect(),
                ));

            // 7) 启动播放
            let locations: Vec<String> = entries.iter().map(|(_, s, _, _)| s.loc()).collect();
            let n_play = entries.len();
            let n_queue = n_play + pending.len();
            sp_collect.shutdown();
            ui::step(&format!("▶ {desc} · 播放 {n_queue} 首 · {prog}"));
            // 开播异步展示当前曲热度：红心状态 + 评论总数（不阻塞开播，
            // 查到后经 toast 在第二行短暂显示；匿名时仅评论数）
            #[cfg(unix)]
            {
                let info_ncm = ncm.clone();
                let first_id = play_tracks.first().map(|t| t.id);
                tokio::spawn(async move {
                    let Some(id) = first_id else { return };
                    let mut parts: Vec<String> = Vec::new();
                    if let Ok(Some((uid, _))) = info_ncm.whoami().await {
                        if let Ok(lists) = info_ncm.user_playlists(uid).await {
                            let pid = lists
                                .iter()
                                .find(|p| p["name"].as_str() == Some("我喜欢的音乐"))
                                .and_then(|p| p["id"].as_u64());
                            if let Some(pid) = pid {
                                match info_ncm.song_in_playlist(pid, id).await {
                                    Ok(true) => parts.push("已喜欢 ♥".into()),
                                    Ok(false) => parts.push("未喜欢".into()),
                                    Err(_) => {}
                                }
                            }
                        }
                    }
                    if let Ok((_, total)) = info_ncm.song_comments(id, 1).await {
                        let s = if total >= 10_000 {
                            format!("{:.1}万", total as f64 / 10_000.0)
                        } else {
                            total.to_string()
                        };
                        parts.push(format!("评论 {s}"));
                    }
                    if !parts.is_empty() {
                        crate::player::toast(&format!("♪ {}", parts.join(" · ")));
                    }
                });
            }
            // 播放队列卡片：歌名/歌手 · 来源/音质（试听曲标警告；热度标 ★）
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
                // 热度：like_count > 0 时显示 ★N（N 为喜欢人数，>1万缩写为 x.x万）
                let lc = play_tracks[i].like_count;
                let heat = if lc > 0 {
                    let s = if lc >= 10_000 {
                        format!("{:.1}万", lc as f64 / 10_000.0)
                    } else {
                        lc.to_string()
                    };
                    format!(" {}", ui::soft_blue(&format!("★{s}")))
                } else {
                    String::new()
                };
                ui::info(&format!(
                    "  {}. {label}{heat} · {source}{quality}{trial}",
                    i + 1
                ));
            }
            if n_queue > show_n {
                ui::info(&format!("  … 共 {n_queue} 首"));
            }
            if !pending.is_empty() {
                ui::info("  分批加载 · 链接过期前自动续取后段曲目");
            }
            if is_mpv {
                #[cfg(unix)]
                ui::info(&format!(
                    "mpv 键位: 空格 暂停 · 0/9 音量 · ←/→ 快退/快进 · </> 上/下曲 · {} 下载当前曲 · {} 收藏当前曲 · q 退出",
                    config::play_download_key(),
                    config::play_like_key()
                ));
                #[cfg(not(unix))]
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
                // 抑制 mpv 状态/info 行防止刷屏（歌词底行或每首提示接管输出）；
                // 只放行 error + 音频模块 warn，stderr 经 spawn_stderr_filter 过滤
                // 后以 toast 打出（如无法识别音频设备），不再直写终端打乱 overlay
                args.push("--term-status-msg=".to_string());
                args.push("--msg-level=all=error,ao=warn,ad=warn".to_string());
                // 禁 mpv 自带终端着色（toast 统一重上灰，防红色串色）
                args.push("--msg-color=no".to_string());
                // mpv 的 terminal input 在本环境不初始化（不读 tty），键入只被内核
                // 回显后遭 overlay 擦除；改由 ncm 键盘接管，mpv 侧禁自读防双端抢键
                #[cfg(unix)]
                args.push("--input-terminal=no".to_string());
                // 直链交给 ffmpeg：禁用 ytdl 解析（坏掉的 yt-dlp 只会刷 traceback）
                args.push("--ytdl=no".to_string());
                // 预开播放列表下一条：按 > 跳过时新曲已就绪，无需等冷加载缓冲
                args.push("--prefetch-playlist=yes".to_string());
            }
            // 守护任务：续取/漫游 + 快捷下载消费（都需要 IPC socket）
            #[cfg(unix)]
            let guard_needed = is_mpv;
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
            // 透传用户参数：置于内置参数之后（mpv 同名选项后者生效）；ffplay 选项不兼容，忽略并提示
            if !player_args.is_empty() {
                if is_ffplay {
                    ui::info("透传参数仅支持 mpv（ffplay 选项体系不同），已忽略");
                } else {
                    args.extend(player_args);
                }
            }
            // 播放守护：pending 随播随取（在线地址现取防过期）+ 可选漫游接FM
            // + 键盘接管（ipc_send/ipc_get 为 unix 实现，Windows 仅提示不支持）
            #[cfg(unix)]
            let raw_guard = if guard_needed && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                player::RawGuard::enable()
            } else {
                None
            };
            #[cfg(unix)]
            let (guard_handle, key_reader, quick_tasks) = if guard_needed {
                if roam {
                    ui::info("漫游已开启 · 队列将尽时自动接私人FM续播");
                }
                // 快捷下载/收藏任务句柄：播放器退出后 join，防 .part 截断
                let quick_tasks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<
                    tokio::task::JoinHandle<()>,
                >::new(
                )));
                // 键盘接管：raw 模式就绪才接管（非 TTY 场景按键本就到不了）
                let key_reader = raw_guard.as_ref().map(|_| {
                    player::spawn_key_reader(
                        sock.clone(),
                        config::play_download_key(),
                        config::play_like_key(),
                    )
                });
                let handle = Some(tokio::spawn(guard_loop(
                    sock.clone(),
                    ncm.clone(),
                    lyric_tracks.clone(),
                    quality.clone(),
                    pending,
                    roam,
                    fed.clone(),
                    queue.clone(),
                    dl_ncm,
                    out_dir.clone(),
                    quick_tasks.clone(),
                )));
                (handle, key_reader, Some(quick_tasks))
            } else {
                if roam {
                    ui::info("漫游需要 mpv（当前播放器无 IPC 接口），已忽略");
                }
                (None, None, None)
            };
            #[cfg(not(unix))]
            let quick_tasks: Option<
                std::sync::Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
            > = None;
            #[cfg(not(unix))]
            let guard_handle: Option<tokio::task::JoinHandle<()>> = {
                if roam {
                    ui::info("播放漫游暂不支持当前平台（需要 unix + mpv IPC），已忽略");
                }
                let _ = (lyric_tracks, pending, queue);
                None
            };

            let status = if is_ffplay {
                // ffplay 不支持播放列表：逐首串行（首尾相接）
                let mut last = None;
                for loc in &locations {
                    let mut cmd = tokio::process::Command::new(&prog);
                    cmd.args(&args)
                        .arg(loc)
                        .stderr(std::process::Stdio::piped());
                    let mut child = cmd
                        .spawn()
                        .with_context(|| format!("启动播放器失败: {prog}"))?;
                    // stderr 过滤：错误/设备信息走 toast，不直写终端打乱底行
                    let log = child.stderr.take().map(player::spawn_output_filter);
                    let r = child.wait().await.context("等待播放器失败")?;
                    if let Some(h) = log {
                        h.abort();
                    }
                    last = Some(r);
                    if !r.success() {
                        break;
                    }
                }
                last.context("ffplay 未启动")?
            } else {
                // mpv / 自定义播放器：一次性喂入全部地址（mpv 自动建播放列表）
                let mut cmd = tokio::process::Command::new(&prog);
                cmd.args(&args)
                    .args(&locations)
                    .stderr(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped());
                let mut child = cmd
                    .spawn()
                    .with_context(|| format!("启动播放器失败: {prog}"))?;
                // stderr/stdout 都收进过滤器：错误/音频设备等有用行经 toast 打出，
                // 其余噪声丢弃，防任何一路直写终端打乱歌词底行
                let log = child.stderr.take().map(player::spawn_output_filter);
                let log2 = child.stdout.take().map(player::spawn_output_filter);
                let status = child.wait().await.context("等待播放器失败")?;
                if let Some(h) = log {
                    // EOF 随 mpv 退出自然结束；等一拍让末尾错误行打完再回收
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    h.abort();
                }
                if let Some(h) = log2 {
                    h.abort();
                }
                status
            };
            if let Some(h) = guard_handle {
                h.abort();
            }
            #[cfg(unix)]
            if let Some(l) = key_reader {
                l.abort();
            }
            // 等快捷下载/收藏任务收尾：guard 已停不会再 spawn 新任务，
            // 限时 join 防下载未完 .part 被进程退出截断
            if let Some(tasks) = quick_tasks {
                let tasks = std::mem::take(&mut *tasks.lock().unwrap_or_else(|e| e.into_inner()));
                if !tasks.is_empty() {
                    ui::info("等待快捷下载/收藏完成…");
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        futures_util::future::join_all(tasks),
                    )
                    .await;
                }
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
            ui::ok(&format!(
                "播放完毕 · {} 首",
                n_play + fed.load(std::sync::atomic::Ordering::Relaxed)
            ));
            Ok(0)
        }
        Some(Commands::Login {
            session,
            cookie,
            logout,
            profile,
        }) => {
            let prof = profile.as_deref();
            if logout {
                let had = session::clear_profile(prof)?;
                let name = prof.unwrap_or("默认");
                if had {
                    ui::ok(&format!("已删除 {name} profile 的登录凭据"));
                } else {
                    ui::info(&format!("{name} profile 没有已保存的凭据"));
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
            } else if let Some(saved) = session::load_profile(prof) {
                saved
            } else {
                ui::info(&format!(
                    "未保存凭据。用法:\n  ncm login --session <pyncm文件>\n  ncm login --cookie \"MUSIC_U=...; __csrf=...\"\n  ncm login --profile <名称>  保存/查看指定 profile（多账号）\n  ncm login --logout [--profile <名称>]  登出\n凭据将保存到 {}",
                    session::cookie_path(prof).display()
                ));
                return Ok(0);
            };
            let ncm = Ncm::with_cookie(Some(&auth_str))?;
            let _sp = ui::Spinner::start("检查登录状态…");
            match ncm.whoami().await? {
                Some((uid, name)) => {
                    if given {
                        let path = session::save_profile(prof, &auth_str)?;
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
        Some(Commands::Config { action }) => {
            match action {
                ConfigAction::Set { key, value } => {
                    config::set(&key, &value)?;
                    ui::ok(&format!("{key} = {value}"));
                }
                ConfigAction::Get { key } => match config::get(&key)? {
                    Some(v) => println!("{key} = {v}"),
                    None => ui::info(&format!("{key} 未配置（ncm config set {key} <值>）")),
                },
                ConfigAction::List => {
                    let all = config::list();
                    if all.is_empty() {
                        ui::info(&format!(
                            "无已配置项（配置文件: {}）",
                            config::config_path().display()
                        ));
                    } else {
                        for (k, v) in &all {
                            println!("{k} = {v}");
                        }
                        ui::info(&format!("配置文件: {}", config::config_path().display()));
                    }
                }
                ConfigAction::Unset { key } => {
                    if config::unset(&key)? {
                        ui::ok(&format!("已删除 {key}"));
                    } else {
                        ui::info(&format!("{key} 未配置"));
                    }
                }
            }
            Ok(0)
        }
        Some(Commands::Whoami) => {
            let _sp = ui::Spinner::start("查询账号信息…");
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
            let _sp = ui::Spinner::start("读取歌曲详情…");
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
        Some(Commands::Comment {
            target,
            limit,
            opts,
        }) => {
            let _sp = ui::Spinner::start("读取评论…");
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let id = parse_id_or_url(&target)?;
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let (hot, total) = ncm.song_comments(id, limit).await?;
            if opts.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "id": id, "total": total, "hot_comments": hot
                    }))?
                );
                return Ok(0);
            }
            if hot.is_empty() {
                ui::info(&format!("歌曲 {id} 暂无热门评论（总评论 {total} 条）"));
                return Ok(0);
            }
            for (i, c) in hot.iter().take(limit as usize).enumerate() {
                let user = c["user"]["nickname"].as_str().unwrap_or("匿名");
                let liked = c["likedCount"].as_u64().unwrap_or(0);
                let time = c["time"].as_i64().unwrap_or(0);
                let date = if time > 0 {
                    fmt_unix_date(time / 1000)
                } else {
                    String::new()
                };
                let content = c["content"].as_str().unwrap_or("");
                println!(
                    "{}. {} {} 赞{}",
                    ui::accent(&(i + 1).to_string()),
                    ui::accent(user),
                    ui::dim(&date),
                    ui::dim(&liked.to_string())
                );
                println!("   {content}");
            }
            if !opts.quiet {
                ui::info(&format!(
                    "热门 {}/{total} 条 · 查看全部: ncm comment {id} -n 20",
                    hot.len()
                ));
            }
            Ok(0)
        }
        Some(Commands::Sign { web }) => {
            let _sp = ui::Spinner::start("签到中…");
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let (uid, _) = ncm
                .whoami()
                .await?
                .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
            let dtype = if web { 1 } else { 0 };
            let point = ncm.daily_signin(dtype).await?;
            let kind = if web { "网页端" } else { "移动端" };
            ui::ok(&format!("{kind}签到成功 uid={uid} +{point} 经验"));
            Ok(0)
        }
        Some(Commands::Subs { types, limit, opts }) => {
            let _sp = ui::Spinner::start("读取收藏的专辑 / 歌手…");
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            ncm.whoami()
                .await?
                .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
            let want_album = types.iter().any(|t| t.eq_ignore_ascii_case("album"));
            let want_artist = types.iter().any(|t| t.eq_ignore_ascii_case("artist"));
            let (albums, artists) = tokio::try_join!(
                async {
                    if want_album {
                        ncm.album_subs(limit).await
                    } else {
                        Ok(vec![])
                    }
                },
                async {
                    if want_artist {
                        ncm.artist_subs(limit).await
                    } else {
                        Ok(vec![])
                    }
                },
            )?;
            if opts.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "albums": albums, "artists": artists
                    }))?
                );
                return Ok(0);
            }
            if !albums.is_empty() {
                println!("{}", ui::accent("收藏的专辑"));
                let rows: Vec<Vec<String>> = albums
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        vec![
                            (i + 1).to_string(),
                            a["name"].as_str().unwrap_or("").to_string(),
                            a["artist"]["name"].as_str().unwrap_or("").to_string(),
                            a["size"].as_u64().unwrap_or(0).to_string(),
                            a["id"].as_u64().unwrap_or(0).to_string(),
                        ]
                    })
                    .collect();
                ui::table(&["#", "专辑名", "歌手", "曲数", "ID"], &rows);
                println!();
            }
            if !artists.is_empty() {
                println!("{}", ui::accent("收藏的歌手"));
                let rows: Vec<Vec<String>> = artists
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        vec![
                            (i + 1).to_string(),
                            a["name"].as_str().unwrap_or("").to_string(),
                            a["musicSize"].as_u64().unwrap_or(0).to_string(),
                            a["albumSize"].as_u64().unwrap_or(0).to_string(),
                            a["id"].as_u64().unwrap_or(0).to_string(),
                        ]
                    })
                    .collect();
                ui::table(&["#", "歌手名", "歌曲数", "专辑数", "ID"], &rows);
            }
            if albums.is_empty() && artists.is_empty() {
                ui::info("没有收藏的专辑/歌手");
            }
            Ok(0)
        }
        Some(Commands::Mv {
            id,
            res,
            play,
            json,
        }) => {
            let _sp = ui::Spinner::start("读取 MV 信息…");
            ui::set_json(json);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let detail = ncm.mv_detail(id).await?;
            let url_v = ncm.mv_url(id, res).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"detail": detail, "url": url_v}))?
                );
                return Ok(0);
            }
            let name = detail["name"].as_str().unwrap_or("");
            let artist = detail["artistName"]
                .as_str()
                .or_else(|| detail["artists"][0]["name"].as_str())
                .unwrap_or("");
            let dur = detail["duration"].as_u64().unwrap_or(0);
            println!("{} - {} {}", ui::accent(name), artist, ui::dim(&hms(dur)));
            let url = url_v["url"].as_str().unwrap_or("");
            if url.is_empty() {
                ui::fail("该 MV 无可用播放地址（版权/地区限制）");
                return Ok(1);
            }
            if play {
                let (prog, extra) =
                    find_player(None).context("未找到 mpv/ffplay，可用 --json 拿地址手动播放")?;
                let status = tokio::process::Command::new(&prog)
                    .args(&extra)
                    .arg("--no-video=never")
                    .arg(url)
                    .status()
                    .await
                    .with_context(|| format!("启动播放器 {prog} 失败"))?;
                if !status.success() {
                    ui::fail("MV 播放器异常退出");
                    return Ok(1);
                }
            } else {
                println!("{}", ui::accent(url));
                if !ui::quiet() {
                    ui::info("播放: ncm mv <ID> --play");
                }
            }
            Ok(0)
        }
        Some(Commands::Cloud {
            limit,
            offset,
            opts,
        }) => {
            let _sp = ui::Spinner::start("读取云盘歌曲…");
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            ncm.whoami()
                .await?
                .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
            let (songs, count) = ncm.cloud_songs(limit, offset).await?;
            if opts.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"count": count, "songs": songs}))?
                );
                return Ok(0);
            }
            if songs.is_empty() {
                ui::info("云盘为空");
                return Ok(0);
            }
            print_search_table(&songs);
            if !opts.quiet {
                ui::info(&format!(
                    "云盘 {}/{} 条 · 翻页: --offset {} · 下载: ncm download <ID>",
                    songs.len(),
                    count,
                    offset + limit
                ));
            }
            Ok(0)
        }
        Some(Commands::Fm { action }) => match action {
            FmAction::Like { id, unlike } => {
                let _sp = ui::Spinner::start("FM 处理中…");
                let ncm = Ncm::with_cookie(auth.as_deref())?;
                ncm.whoami()
                    .await?
                    .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
                ncm.fm_like(id, !unlike).await?;
                let verb = if unlike { "取消红心" } else { "红心" };
                ui::ok(&format!("FM {verb} 曲目 {id}"));
                Ok(0)
            }
            FmAction::Trash { id } => {
                let _sp = ui::Spinner::start("FM 处理中…");
                let ncm = Ncm::with_cookie(auth.as_deref())?;
                ncm.whoami()
                    .await?
                    .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
                ncm.fm_trash(id).await?;
                ui::ok(&format!("已踩曲目 {id}，将少推同类"));
                Ok(0)
            }
            FmAction::Skip { id, opts } => {
                let _sp = ui::Spinner::start("FM 处理中…");
                ui::set_json(opts.json);
                ui::set_quiet(opts.quiet);
                let ncm = Ncm::with_cookie(auth.as_deref())?;
                ncm.whoami()
                    .await?
                    .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
                if id > 0 {
                    ncm.fm_skip(id).await?;
                }
                let songs = ncm.radio_songs().await?;
                if opts.json {
                    println!("{}", serde_json::to_string_pretty(&Value::Array(songs))?);
                    return Ok(0);
                }
                print_search_table(&songs);
                Ok(0)
            }
            FmAction::Get { limit, opts } => {
                let _sp = ui::Spinner::start("读取私人FM…");
                ui::set_json(opts.json);
                ui::set_quiet(opts.quiet);
                let ncm = Ncm::with_cookie(auth.as_deref())?;
                ncm.whoami()
                    .await?
                    .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
                let mut songs = ncm.radio_songs().await?;
                if limit > 0 {
                    songs.truncate(limit as usize);
                }
                if opts.json {
                    println!("{}", serde_json::to_string_pretty(&Value::Array(songs))?);
                    return Ok(0);
                }
                print_search_table(&songs);
                Ok(0)
            }
        },
        Some(Commands::Playlist { action }) => {
            let _sp = ui::Spinner::start("歌单操作…");
            let action = action.unwrap_or(PlaylistAction::List {
                limit: 50,
                opts: ViewArgs {
                    json: false,
                    quiet: false,
                },
            });
            match action {
                PlaylistAction::List { limit, opts } => {
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
                        ui::info("浏览: ncm list <ID>  ·  下载: ncm download list.<ID> -d 编号");
                    }
                    Ok(0)
                }
                _ => {
                    let ncm = Ncm::with_cookie(auth.as_deref())?;
                    let (uid, _) = ncm
                        .whoami()
                        .await?
                        .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
                    match action {
                        PlaylistAction::Create { name } => {
                            if name.trim().is_empty() {
                                anyhow::bail!("歌单名不能为空");
                            }
                            let id = ncm.playlist_create(&name).await?;
                            ui::ok(&format!("已创建歌单「{name}」 id={id}"));
                            Ok(0)
                        }
                        PlaylistAction::Delete { target, force } => {
                            let lists = ncm.user_playlists(uid).await?;
                            let (id, name) = resolve_playlist(&lists, target.as_deref())?;
                            if name == "我喜欢的音乐" {
                                anyhow::bail!("不能删除「我喜欢的音乐」");
                            }
                            if !force
                                && !ui::confirm(&format!("确认删除歌单「{name}」（含全部歌曲）？"))
                            {
                                ui::info("已取消");
                                return Ok(1);
                            }
                            ncm.playlist_delete(id).await?;
                            ui::ok(&format!("已删除歌单「{name}」"));
                            Ok(0)
                        }
                        PlaylistAction::Rename { target, name } => {
                            let lists = ncm.user_playlists(uid).await?;
                            let (id, old) = resolve_playlist(&lists, target.as_deref())?;
                            ncm.playlist_rename(id, &name).await?;
                            ui::ok(&format!("「{old}」已改名为「{name}」"));
                            Ok(0)
                        }
                        PlaylistAction::Add { ids, playlist } => {
                            if ids.is_empty() {
                                anyhow::bail!("请至少指定一首歌曲 ID 或链接");
                            }
                            let lists = ncm.user_playlists(uid).await?;
                            let (pid, pname) = resolve_playlist(&lists, playlist.as_deref())?;
                            let mut track_ids = Vec::new();
                            for s in &ids {
                                track_ids.push(parse_id_or_url(s)?);
                            }
                            ncm.playlist_tracks_op("add", pid, &track_ids).await?;
                            ui::ok(&format!("已添加 {} 首到「{pname}」", track_ids.len()));
                            Ok(0)
                        }
                        PlaylistAction::Remove { ids, playlist } => {
                            if ids.is_empty() {
                                anyhow::bail!("请至少指定一首歌曲 ID");
                            }
                            let lists = ncm.user_playlists(uid).await?;
                            let (pid, pname) = resolve_playlist(&lists, playlist.as_deref())?;
                            ncm.playlist_tracks_op("del", pid, &ids).await?;
                            ui::ok(&format!("已从「{pname}」移除 {} 首", ids.len()));
                            Ok(0)
                        }
                        PlaylistAction::List { .. } => unreachable!("已在上方处理"),
                    }
                }
            }
        }
        Some(Commands::Like { ids, unlike }) => {
            if ids.is_empty() {
                anyhow::bail!("请至少指定一首歌曲 ID、链接或（--unlike 时）歌名关键词");
            }
            // 分拣：ID/链接直接收；名字仅 --unlike 支持（在红心歌单内模糊匹配）
            let mut track_ids: Vec<u64> = Vec::new();
            let mut names: Vec<String> = Vec::new();
            for s in &ids {
                match parse_id_or_url(s) {
                    Ok(id) => track_ids.push(id),
                    Err(_) => {
                        if !unlike {
                            anyhow::bail!("「{s}」不是歌曲 ID/链接；按歌名取消喜欢请加 --unlike");
                        }
                        names.push(s.clone());
                    }
                }
            }
            let _sp = ui::Spinner::start("读取「我喜欢的音乐」…");
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let (uid, _) = ncm
                .whoami()
                .await?
                .context("登录态无效：ncm login --session/--cookie 保存登录后重试")?;
            let lists = ncm.user_playlists(uid).await?;
            let (pid, _) = resolve_playlist(&lists, None)
                .context("账号下未找到「我喜欢的音乐」歌单（红心歌单），请先在网页端创建")?;
            if !names.is_empty() {
                let (tracks, _) = ncm.playlist(pid).await?;
                drop(_sp); // 匹配完成先停，进入交互/删除前不再转
                for name in &names {
                    let q = name.to_lowercase();
                    let hits: Vec<(u64, String)> = tracks
                        .iter()
                        .filter_map(|t| {
                            let title = t["name"].as_str()?;
                            if !title.to_lowercase().contains(&q) {
                                return None;
                            }
                            let id = t["id"].as_u64()?;
                            let artists: Vec<&str> = t["ar"]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x["name"].as_str())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            Some((id, format!("{title} - {}", artists.join("/"))))
                        })
                        .collect();
                    match hits.len() {
                        0 => ui::skip(&format!("红心歌单无名称含「{name}」的歌曲")),
                        1 => {
                            ui::info(&format!("匹配 1 首：{}", hits[0].1));
                            track_ids.push(hits[0].0);
                        }
                        _ => {
                            println!();
                            ui::table(
                                &["#", "曲目"],
                                &hits
                                    .iter()
                                    .enumerate()
                                    .map(|(i, (_, label))| vec![(i + 1).to_string(), label.clone()])
                                    .collect::<Vec<_>>(),
                            );
                            if !ui::is_tty() {
                                anyhow::bail!(
                                    "「{name}」匹配到 {} 首，非交互环境无法选择，请改用歌曲 ID",
                                    hits.len()
                                );
                            }
                            print!("  输入要取消喜欢的编号（如 1,3 / 1-2 / all，回车跳过）: ");
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                            let mut line = String::new();
                            if std::io::stdin().read_line(&mut line).is_err()
                                || line.trim().is_empty()
                            {
                                ui::info(&format!("已跳过「{name}」"));
                                continue;
                            }
                            for tok in line.trim().split([',', ' ']) {
                                let t = tok.trim();
                                if t.is_empty() {
                                    continue;
                                }
                                if t.eq_ignore_ascii_case("all") {
                                    track_ids.extend(hits.iter().map(|(id, _)| *id));
                                    continue;
                                }
                                let (lo, hi) = match t.split_once('-') {
                                    Some((a, b)) => (
                                        a.parse::<usize>().unwrap_or(0),
                                        b.parse::<usize>().unwrap_or(hits.len()),
                                    ),
                                    None => {
                                        let v = t.parse::<usize>().unwrap_or(0);
                                        (v, v)
                                    }
                                };
                                if lo < 1 || hi > hits.len() || lo > hi {
                                    ui::fail(&format!("无效编号：{t}（共 {} 条）", hits.len()));
                                    continue;
                                }
                                track_ids.extend(hits[lo - 1..=hi - 1].iter().map(|(id, _)| *id));
                            }
                        }
                    }
                }
            }
            track_ids.sort_unstable();
            track_ids.dedup();
            if track_ids.is_empty() {
                anyhow::bail!("没有可操作的歌曲（检查 ID / 名称匹配结果）");
            }
            let op = if unlike { "del" } else { "add" };
            let res = ncm.playlist_tracks_op(op, pid, &track_ids).await;
            if let Err(ref e) = res {
                // 歌已在红心歌单：语义已达成，不当失败
                if op == "add" && Ncm::is_duplicate_track_err(e) {
                    ui::skip(&format!(
                        "{} 首已在「我喜欢的音乐」，无需重复收藏",
                        track_ids.len()
                    ));
                    return Ok(0);
                }
            }
            res?;
            let msg = if unlike {
                format!("从「我喜欢的音乐」取消收藏 {} 首", track_ids.len())
            } else {
                format!("收藏 {} 首到「我喜欢的音乐」", track_ids.len())
            };
            ui::ok(&msg);
            Ok(0)
        }
        // 无子命令（纯 flags 或空参数）：打印帮助
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
/// 信号量限 8 并发：无上限 join_all 会瞬间打满接口触发风控
async fn fetch_urls(
    ncm: &Ncm,
    tracks: &[Track],
    quality: &str,
) -> Vec<Result<(String, u64, String), String>> {
    use futures_util::future::join_all;
    let wanted_flac = matches!(quality, "lossless" | "hires");
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let sem = &sem;
    join_all(tracks.iter().map(|t| async move {
        let _permit = sem.acquire().await.expect("信号量未关闭");
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
        let lines = lrc::timeline_from_lyric(&v);
        (!lines.is_empty()).then_some(lines)
    }))
    .await
}

/// 快捷下载：播放中按 <key> 触发，独立任务下载当前曲（song_detail 补全元数据
/// 后复用 download 下载管线；质量同 play 参数，目录取 -o / config download-dir）
#[cfg(unix)]
async fn quick_download(ncm: Ncm, id: u64, label: String, out_dir: PathBuf, quality: String) {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        player::clear_overlay_rows(); // 下载输出进滚动历史，先清底行避免插行打乱光标
    }
    ui::step(&format!("快捷下载 · {label}"));
    let songs = match ncm.song_detail(&[id]).await {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            ui::skip(&format!("快捷下载 · {label} · 歌曲详情为空"));
            return;
        }
        Err(e) => {
            ui::skip(&format!("快捷下载 · {label} · 获取详情失败: {e}"));
            return;
        }
    };
    let opts = DownloadArgs {
        limit: 0,
        output: Some(out_dir),
        lrc_dir: Vec::new(),
        lrcs: LRC_DEFAULT.to_string(),
        no_lrc: false,
        only_lrc: false,
        quality,
        quiet: false,
        dry_run: false,
        template: "{artists} - {track}".to_string(),
        skip_existing: true,
        allow_trial: false,
        artist: false,
        json: false,
        download: None,
        jobs: 1,
        m3u: None,
    };
    if let Err(e) = download_songs(&ncm, songs, vec![format!("快捷下载 · {label}")], opts).await
    {
        ui::skip(&format!("快捷下载 · {label} · 失败: {e:#}"));
    }
}

/// 时间种子洗牌（xorshift64*，满足播放随机即可，无外部 rand 依赖）
/// 播放守护：队尾将尽时把 pending 续入 mpv（在线地址现取防过期），pending
/// 耗尽且开启漫游时拉私人FM补充；歌词容器与播放列表索引对齐；
/// 消费快捷下载按键请求（<key> 下载当前曲，用 download 板块凭据）；
/// mpv 退出（IPC 连不上连续 3 次）静默结束
#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
async fn guard_loop(
    sock: std::path::PathBuf,
    ncm: Ncm,
    tracks: Option<std::sync::Arc<std::sync::RwLock<Vec<player::LyricTrack>>>>,
    quality: String,
    mut pending: std::collections::VecDeque<PendingItem>,
    roam: bool,
    fed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    queue: std::sync::Arc<std::sync::RwLock<Vec<(u64, String)>>>,
    dl_ncm: Ncm,
    dl_dir: PathBuf,
    quick: std::sync::Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) {
    use std::sync::atomic::Ordering;
    let wanted_flac = matches!(quality.as_str(), "lossless" | "hires");
    let mut req_id: u64 = 9000;
    let mut fails: u32 = 0; // 启动期 socket 未就绪的容忍计数
                            // 会话内已入队曲 id：FM 批次跨次拉取会重复回推已播曲，入队前去重
    let mut seen: std::collections::HashSet<u64> = queue
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(id, _)| *id)
        .collect();
    seen.extend(pending.iter().map(|p| p.track.id));
    let mut dup_streak: u32 = 0; // 连续整批重复计数（防服务端只回同一批时卡死）
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        // 查询失败 = mpv 未就绪或已退出（连续 3 次才认定退出）
        // 注意：仅 ipc 调用 Err 计失败；pos=-1 是 mpv 空闲/空队列的合法值，不当断连
        req_id += 1;
        let pos_v = player::ipc_get(&sock, "playlist-pos", req_id).await;
        req_id += 1;
        let count_v = player::ipc_get(&sock, "playlist-count", req_id).await;
        let (Some(pos_v), Some(count_v)) = (pos_v, count_v) else {
            fails += 1;
            if fails >= 3 {
                return;
            }
            continue;
        };
        fails = 0;
        let Some(pos) = pos_v.as_i64().filter(|&p| p >= 0).map(|p| p as u64) else {
            continue; // 队列空/空闲：mpv 还活着，等下轮
        };
        let mut count = count_v.as_u64().unwrap_or(0);
        // 快捷下载请求：取当前曲 spawn 独立下载，不阻塞续曲轮询（多次按下合并为一次）
        if player::DOWNLOAD_REQ.swap(0, Ordering::Relaxed) > 0 {
            let hit = queue
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(pos as usize)
                .cloned();
            if let Some((id, label)) = hit {
                let h = tokio::spawn(quick_download(
                    dl_ncm.clone(),
                    id,
                    label,
                    dl_dir.clone(),
                    quality.clone(),
                ));
                quick.lock().unwrap_or_else(|e| e.into_inner()).push(h);
            }
        }
        // 快捷收藏请求：取当前曲加入"我喜欢的音乐"（多次按下合并为一次）
        if player::LIKE_REQ.swap(0, Ordering::Relaxed) > 0 {
            let hit = queue
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(pos as usize)
                .cloned();
            if let Some((id, label)) = hit {
                let like_ncm = dl_ncm.clone();
                let h = tokio::spawn(async move {
                    // 逐级收集错误，失败不谎报成功
                    let outcome: anyhow::Result<()> = async {
                        let (uid, _) = like_ncm
                            .whoami()
                            .await?
                            .context("未登录：ncm login 保存登录后重试")?;
                        let lists = like_ncm.user_playlists(uid).await?;
                        let (pid, _) =
                            resolve_playlist(&lists, None).context("未找到「我喜欢的音乐」歌单")?;
                        like_ncm
                            .playlist_tracks_op("add", pid, std::slice::from_ref(&id))
                            .await
                    }
                    .await;
                    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                        player::clear_overlay_rows();
                    }
                    match outcome {
                        Ok(()) => ui::ok(&format!("已收藏「{label}」到我喜欢的音乐")),
                        Err(e) if Ncm::is_duplicate_track_err(&e) => {
                            // 歌已在红心歌单：语义已达成，提示而非报错
                            ui::skip(&format!("「{label}」已在「我喜欢的音乐」"));
                        }
                        Err(e) => ui::fail(&format!("收藏失败：{e:#}")),
                    }
                });
                quick.lock().unwrap_or_else(|e| e.into_inner()).push(h);
            }
        }
        if pos + 3 < count {
            continue; // 距队尾还早
        }
        // pending 耗尽且漫游开启：拉私人FM补充；未开漫游则继续轮询（快捷下载监听至 mpv 退出）
        if pending.is_empty() {
            if !roam {
                continue;
            }
            let fm = ncm.radio_songs().await.unwrap_or_default();
            if fm.is_empty() {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            }
            let mut added = 0usize;
            for song in &fm {
                let t = Track::from_json(song);
                if seen.insert(t.id) {
                    pending.push_back(PendingItem {
                        label: format!("{} - {}", t.title, t.artists),
                        src: PlaySrc::Url(String::new()),
                        track: t,
                    });
                    added += 1;
                }
            }
            if added == 0 {
                dup_streak += 1;
                if dup_streak < 3 {
                    // 整批与已入队重复：隔 10s 换一批，连 3 批全重复才降级接受防卡死
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    continue;
                }
                dup_streak = 0;
                for song in fm {
                    let t = Track::from_json(&song);
                    seen.insert(t.id);
                    pending.push_back(PendingItem {
                        label: format!("{} - {}", t.title, t.artists),
                        src: PlaySrc::Url(String::new()),
                        track: t,
                    });
                }
            } else {
                dup_streak = 0;
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
                        // 续取失败提示入滚动历史：先清 overlay 双行，避免插行打乱光标状态
                        if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                            player::clear_overlay_rows();
                        }
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
                .map(|v| lrc::timeline_from_lyric(&v))
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
            queue
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .push((t.id, format!("{} - {}", t.title, t.artists)));
            count += 1;
            fed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::Parser;

    /// Unix 秒 → 日期：已知时间点与闰年边界
    #[test]
    fn fmt_unix_date_known_points() {
        assert_eq!(fmt_unix_date(0), "1970-01-01");
        assert_eq!(fmt_unix_date(1_760_000_000), "2025-10-09");
        assert_eq!(fmt_unix_date(1_709_164_800), "2024-02-29"); // 闰日
        assert_eq!(fmt_unix_date(-86_400), "1969-12-31"); // 负数（1970 前）
    }

    /// 未知首参不再注入 dl，由 clap 报 unrecognized subcommand
    #[test]
    fn unknown_first_arg_is_not_search_or_dl() {
        assert!(Cli::try_parse_from(["ncm", "259138"]).is_err());
        assert!(Cli::try_parse_from(["ncm", "moon"]).is_err());
        // 真正未知词同样报错
        assert!(Cli::try_parse_from(["ncm", "bogus", "259138"]).is_err());
        // 显式子命令不受影响
        assert!(Cli::try_parse_from(["ncm", "download", "259138"]).is_ok());
        assert!(Cli::try_parse_from(["ncm", "dl", "259138"]).is_ok());
    }

    /// download 为正式名，dl 为别名，同一 variant
    #[test]
    fn download_alias_parses() {
        let a = Cli::try_parse_from(["ncm", "download", "1"]).unwrap();
        let b = Cli::try_parse_from(["ncm", "dl", "1"]).unwrap();
        assert!(matches!(a.cmd, Some(Commands::Download { .. })));
        assert!(matches!(b.cmd, Some(Commands::Download { .. })));
    }

    /// 命令 → 板块映射（决定用哪个 profile 凭据）
    #[test]
    fn section_of_commands() {
        let cases: Vec<(Vec<&str>, &str)> = vec![
            (vec!["ncm", "dl", "1"], "download"),
            (vec!["ncm", "download", "--daily"], "download"),
            (vec!["ncm", "play", "--roam"], "roam"),
            (vec!["ncm", "play", "--daily"], "recommend"),
            (vec!["ncm", "play", "--roam", "--daily"], "roam"),
            (vec!["ncm", "play", "259138"], "play"),
            (vec!["ncm", "play", "daily"], "play"), // 旧别名已移除：daily 现按关键词搜索
            (vec!["ncm", "daily"], "recommend"),
            (vec!["ncm", "recommend"], "recommend"),
            (vec!["ncm", "search", "moon"], "search"),
            (vec!["ncm", "list", "1"], "list"),
            (vec!["ncm", "whoami"], "default"),
        ];
        for (args, want) in cases {
            let cli = Cli::try_parse_from(args.clone()).expect("应解析成功");
            assert_eq!(section_of(cli.cmd.as_ref()), want, "{args:?}");
        }
    }

    /// `--` 之后的参数全部透传给播放器，不被 ncm 自身选项解析拦截
    #[test]
    fn trailing_player_args_parsed() {
        let cli = Cli::try_parse_from([
            "ncm",
            "play",
            "123",
            "--no-lyric",
            "--",
            "--volume",
            "60",
            "--ao",
            "alsa",
        ])
        .expect("应解析成功");
        match cli.cmd {
            Some(Commands::Play {
                player_args,
                no_lyric,
                ..
            }) => {
                assert!(no_lyric);
                assert_eq!(
                    player_args,
                    ["--volume", "60", "--ao", "alsa"].map(std::ffi::OsString::from)
                );
            }
            _ => panic!("应为 Play 命令"),
        }
    }

    /// picUrl 三处兼容取值：al（网易详情）、album（另一形状）、顶层；空串/缺失为 None
    #[test]
    fn pick_pic_url_fallback_shapes() {
        use serde_json::json;
        assert_eq!(
            super::pick_pic_url(&json!({"al": {"picUrl": "http://a/p.jpg"}})),
            Some("http://a/p.jpg")
        );
        assert_eq!(
            super::pick_pic_url(&json!({"album": {"picUrl": "http://b/p.jpg"}})),
            Some("http://b/p.jpg")
        );
        assert_eq!(
            super::pick_pic_url(&json!({"picUrl": "http://c/p.jpg"})),
            Some("http://c/p.jpg")
        );
        assert_eq!(super::pick_pic_url(&json!({"al": {"picUrl": ""}})), None);
        assert_eq!(super::pick_pic_url(&json!({"name": "无图"})), None);
    }

    /// -t 支持数字、别名、逗号分隔，解析为去重 Vec<u32>
    #[test]
    fn search_type_flag_parses() {
        let cli = Cli::try_parse_from(["ncm", "search", "-t", "1,1000", "moon"]).unwrap();
        match cli.cmd {
            Some(Commands::Search { types, .. }) => assert_eq!(types, vec![1, 1000]),
            _ => panic!("应为 Search"),
        }
        let cli = Cli::try_parse_from(["ncm", "search", "-t", "song,playlist,SONG", "x"]).unwrap();
        match cli.cmd {
            // value_parser 逐 token 解析；去重在 handler 做，此处断言解析值
            Some(Commands::Search { types, .. }) => assert_eq!(types, vec![1, 1000, 1]),
            _ => panic!("应为 Search"),
        }
        // 未知别名 → clap 解析错误
        assert!(Cli::try_parse_from(["ncm", "search", "-t", "bogus", "x"]).is_err());
        // 不给 -t 默认 [1]
        let cli = Cli::try_parse_from(["ncm", "search", "moon"]).unwrap();
        match cli.cmd {
            Some(Commands::Search { types, .. }) => assert_eq!(types, vec![1]),
            _ => panic!("应为 Search"),
        }
    }

    /// recommend -t 值域与默认
    #[test]
    fn recommend_type_flag_parses() {
        use crate::RecKind;
        // 默认 daily
        let cli = Cli::try_parse_from(["ncm", "recommend"]).unwrap();
        match cli.cmd {
            Some(Commands::Recommend { types, .. }) => {
                assert_eq!(types, vec![RecKind::Daily])
            }
            _ => panic!("应为 Recommend"),
        }
        // 逗号多值
        let cli = Cli::try_parse_from(["ncm", "recommend", "-t", "fm,radar"]).unwrap();
        match cli.cmd {
            Some(Commands::Recommend { types, .. }) => {
                assert_eq!(types, vec![RecKind::Fm, RecKind::Radar])
            }
            _ => panic!("应为 Recommend"),
        }
        // 未知值报错
        assert!(Cli::try_parse_from(["ncm", "recommend", "-t", "nope"]).is_err());
    }
}
