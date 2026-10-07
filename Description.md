# ncm-cli — 网易云音乐统一命令行工具（Rust）

## 用途

Rust 重写版网易云 CLI，替代远端 `pyncm`（8.3 MB aarch64）与 `musicdl`（30+ 依赖包），
单二进制覆盖：多类型搜索（`search -t` 逗号分隔）、下载（音频+歌词+封面）、浏览歌单、
每日推荐与首页推荐（`recommend -t daily/toplist/fm,radar`）、队列播放（mpv
交互+歌词同步）、登录持久化、歌曲详情、shell 补全。严格子命令：`ncm <未知命令>`
报 `unrecognized subcommand`（不再自动当下载）。
CLI 选项与 Python 原型 `ncmt-local` 同名兼容。

## 架构

```
src/
├── main.rs      CLI 定义（clap derive）+ 命令分支 + run_batch（download 统一入口）+ 播放队列编排
├── searchtype.rs search/recommend 的 type 解析（英文别名表、配额 quotas、统一表行提取 extract_row）
├── api.rs       明文接口封装（search/detail/playlist/lyric/player_url/daily/user_playlists/whoami），
│                Ncm::with_cookie 注入登录 Cookie
├── session.rs   pyncm session 双格式解析 + 多 profile 凭据持久化（~/.config/ncm/cookie[.<名>]，0600；
│                resolve_auth_for 优先级 --session --cookie > 板块绑定 profile > 默认文件）
├── config.rs    持久化配置（~/.config/ncm/config.json 白名单：download-dir / play.download-key /
│                profile.<板块>，set/get/list/unset，0700/0600）
├── source.rs    第三方 FLAC 源（xuanluoge/kangqiovo/vincentzyu，429/5xx 熔断），失败回退官方
├── mirror.rs    解锁音源（UnblockNeteaseMusic-utils REST /match）：官方 -110/404/试听时兜底，
│                --mirror / NCM_MIRROR 配置（默认 127.0.0.1:3000，none 禁用），30s 超时
├── download.rs  下载核心：Range 探测、.part+长度比对、魔数/时长/STREAMINFO 校验、降级重试
├── model.rs     Track 模型、文件名模板、目标解析（ID/链接/list.123 → 曲目列表）
├── player.rs    mpv IPC 客户端：Unix socket observe time-pos/playlist-pos，
│                歌词二分定位 + 终端单行滚动（♪ [i/N]）
├── lrc.rs       LRC 时间轴解析（parse_lrc_timeline）、yrc 转 ASS、多类型歌词
├── tags.rs      lofty 写 ID3：封面嵌入、音轨编号、多类型歌词帧、ncm-id 反查键
├── lrc.txt      内嵌歌词资源
└── ui.rs        进度条/spinner/表格/颜色（NO_COLOR、TTY 检测、管道降级）
```

## 命令

| 命令 | 说明 |
|------|------|
| `ncm search <kw>` | 纯浏览：表格 / `--json`（下载走 download）；`-t/--type` 逗号分隔多类型（数字与别名混用，如 `1,1000`/`song,playlist`），统一表 + 「类型」列，`-n` 总条数按类平分（0 = 全部，单次上限 100/类）；单 type JSON 为数组、多 type 为 `{"1":[…],"1000":[…]}` 对象 |
| `ncm download <目标>`（别名 `ncm dl`） | 唯一下载入口：单曲/歌单/专辑/歌手/关键词/`daily`；`-d` 选择器、TTY 交互、`-n` 候选数；完成出结果一览表（状态/来源/大小/失败原因）+ 耗时汇总 |
| `ncm list <目标>` | 纯浏览歌单/专辑/歌手作品列表 |
| `ncm playlists` | 我创建/收藏的歌单（需登录） |
| `ncm daily` | 每日推荐浏览（需登录） |
| `ncm play <目标\|daily\|roam>` | 队列播放：`-n` 队列数、`--shuffle`、`--save -o` 先下载、`--json`、`--no-lyric`、`--roam` 播放漫游（队列将尽自动接私人FM续播）、`roam` 目标单开私人FM漫游歌单；mpv 下 <>/←→ 交互+歌词滚动（`--prefetch-playlist` 预开下首，跳过无缓冲等待），双行底行 `mm:ss/mm:ss [进度条] 原文` + `译文(dim)` 按终端宽度截行（非 TTY 静默）；切曲历史打一行 `▶ [i/N] 歌名`；**参数透传** `-- --volume=60 --ao=alsa`（经 `--` 分隔置于最后，同名以用户为准，仅 mpv，ffplay 忽略）；mpv `--msg-level=all=fatal`（memfd/ipc 杂散日志不打乱底行），播放计数含 guard 续喂；**分批取址**：先播前 6 首，后续随播随取（防网易直链 ~10min 过期 403 雪崩）；取址先本地 mirror 再第三方（快起播） |
| `ncm recommend` | 首页推荐：`-t/--type` 推荐位逗号分隔（`daily` 每日推荐·默认 / `toplist` 官方榜单 / `fm` 私人FM / `radar` 私人雷达），默认只列每日推荐歌曲；`-n` 总条数按位平分（0 = 全部），统一表 / `--json` |
| `ncm login/--logout` | 保存/查看/清除登录凭据（持久化）；`--profile <名>` 多账号另存 `cookie.<名>` |
| `ncm config set/get/list/unset` | 持久化配置：`download-dir` 默认下载目录、`play.download-key` 播放快捷下载键（默认 `d`）、`profile.<板块>` 板块绑定会话（板块：default/download/play/recommend/roam/search/list） |
| `ncm whoami` | 当前登录状态（昵称/uid/凭据来源） |
| `ncm info <目标>` | 歌曲详情（标题行/歌名/歌手/专辑/时长/费用着色/版权），VIP 曲提示 mirror 解锁，`--json` |
| `ncm completions <shell>` | 生成 bash/zsh/fish/powershell 补全 |

严格子命令：未知首参由 clap 报 `unrecognized subcommand 'xxx'`（`ncm 123` 不再
自动当下载），无参数打印 help。
`-n/--limit` 统一语义 = 输出总条数，多类型/多推荐位平分，`0` = 全部
（`search`/`recommend`/`list`/`daily`/`playlists`/`play`；`download` 为候选数）。

登录凭据优先级：`--session <file>` / `--cookie <str>` > 环境变量 `NCM_SESSION`/`NCM_COOKIE` >
板块绑定的 profile（`ncm config set profile.<板块> <名>`）> `ncm login` 默认持久化文件。
板块判定（`section_of`）：download（别名 dl）=download、play roam=roam、play daily/daily/recommend=recommend、
search=search、list/playlists=list、其余=default；播放中按下载键用 download 板块凭据下载。

取址兜底：官方失败/试听 → 第三方直连源（xuanluoge/kangqiovo/vincentzyu，无需本地服务）→ 本地 mirror（`--mirror` > `NCM_MIRROR` > 默认 3000,3100 多候选，`none` 禁用）。

## 构建与验证

```bash
cargo build --release
cargo test && cargo clippy --all-targets && cargo fmt --check
```

CI（GitHub Actions，push 触发）：fmt/clippy/test → 四平台产物
（linux-x86_64 musl 静态 / linux-aarch64 / windows / macos），`v*` tag 发 Release。

## 依赖

clap、clap_complete、tokio、reqwest(rustls)、lofty、serde_json、aes/base64/flate2；无 jq、无 Python。
