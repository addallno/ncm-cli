# ncm — 网易云音乐统一命令行工具

单二进制网易云客户端：搜索、下载、播放、歌词、歌单管理一站搞定，登录一次全命令复用。

## 功能

- **浏览 / 下载分工**：`search` `list` `daily` `pl list` `recommend` 只负责看
  （表格 / `--json`），`download`（别名 `dl`）是唯一下载入口；
  `-d 1,3,5-8/all/1-` 选择器只在 `download` 出现，TTY 下没给 `-d` 会交互选编号
  （回车仅列出）
- **多类型搜索 / 首页推荐**：`ncm search -t song,playlist,...` 逗号分隔多类型
  （数字与英文别名可混用），一张统一表 + 「类型」列；`ncm recommend` 默认只列
  每日推荐歌曲，`-t daily,toplist,fm,radar` 选推荐位；直接 `ncm info playlist <ID>` 浏览、
  `ncm download list.<ID>` 下载
- **严格子命令**：`ncm <未知命令>` 直接报 `unrecognized subcommand`，
  不再把未知参数悄悄当成下载
- **播放漫游**：`ncm play --roam` 单开私人FM漫游歌单（首曲开播，队列将尽自动
  接FM无限续播）；普通队列加 `--roam` 同样开启续播（mpv 下无感接力）
- **下载**：单曲 / 歌单 / 专辑 / 歌手链接 / `list.123` / `--daily` / 关键词，
  **默认 FLAC + 歌词 + 保留音频**；标签写入标题/歌手/专辑/歌词/封面外，**嵌入
  `ncm-id`**（FLAC 为 Vorbis 键，MP3 为 TXXX 帧，凭此可反查歌 ID）；进度条 /
  spinner / 完整性防线（Range → `.part` → 魔数/时长校验 → 全过才落盘）；完成后出结果一览表（`#`/状态/曲目/来源/大小/备注）
  与耗时汇总，管道/脚本下输出干净无 ANSI 裸序列
- **播放**：`ncm play <ID/链接/关键词>` + `--daily`（每日推荐）/ `--roam`（FM
  续播），支持多目标按序连播
  （`ncm play "晴天" "七里香"`、`ncm play --daily`）与 `-f playlist.txt` 文件列表（每行一个目标，
  `#` 开头跳过，可与命令行目标混用）；队列喂给 mpv（`<`/`>` 切曲、
  `←/→` 快退进、空格暂停、`/` 搜索、`q` 退出；开 `--prefetch-playlist`
  预开下一首，跳过时新曲已就绪不用等缓冲），歌词随播放滚动并按终端宽度
  自适应截行（窄屏不换行叠影）；原文与译文分两行显示（line1 时间+原文，
  line2 译文 dim）；**分批取址**：先播前 6 首，后续随播随取新链接
  （网易直链约 10 分钟过期，全量预取会导致长队列后段 403 雪崩）；`--save -o 目录`
  先下载到本地再播本地文件；`--shuffle` 洗牌、`-n` 队列长度、`--json` 输出队列；
  播放器参数经 `--` 透传（mpv 需 `=` 形式）：`ncm play 晴天 -- --volume=60 --ao=alsa`；
  mpv 日志经管道过滤：error/音频设备等有用行打成 dim toast（`♪ 音量 60%`/`♪ 速度 1.50x`
  经 IPC 轮询），memfd/ipc 杂散与 info 行不打乱歌词底行，`播放完毕 · N 首` 含续喂数
- **登录持久化**：`ncm login --session <pyncm dump>`（兼容 PYNCM/legacy）或
  `--cookie "MUSIC_U=...; __csrf=..."`（浏览器 F12 直接粘），凭据保存到
  `~/.config/ncm/cookie`（0600），之后所有命令自动加载；`ncm whoami` 看状态、
  `ncm login --logout` 登出；环境变量 `NCM_SESSION`/`NCM_COOKIE` 仍可临时覆盖
- **多会话与板块绑定**：`ncm login --profile <名>` 另存一份凭据（`cookie.<名>`），
  `ncm config set profile.<板块> <名>` 把板块绑到该会话（板块：`download`/`play`/
  `recommend`/`roam`/`search`/`list`/`default`）——如下载用 VIP 号、听歌用小号：
  `ncm login --profile vip --cookie "..."` + `ncm config set profile.download vip`
- **持久化配置**：`ncm config set <键> <值>`（不改文件，0600 白名单键）：
  `download-dir` 默认下载目录、`play.download-key` 播放快捷下载键（默认 `d`）、
  `play.like-key` 播放收藏键（默认 `l`）、
  `play.unlike-key` 播放取消收藏键（默认 `u`）、
  `play.show-comments` 播放状态栏是否显示评论数（默认 `false`，开启后切曲并行补取），
  `download.jobs` 默认并发下载数（默认 `1`，1-16，`-j` 未指定时回退）、
  `mirror` 解锁音源地址（默认 `127.0.0.1:3000,3100`，`none` 禁用，逗号分隔多候选）、
  `play.prefetch` 分批取址预取首数（默认 `6`，1-64，防网易直链过期 403 雪崩）、
  `quality` 默认音质（standard/exhigh/lossless/hires，play/download 共用回退）、
  `download.confirm` 多条目下载是否 y/N 确认（默认 `true`）、
  `profile.<板块>` 会话绑定；`ncm config get/list/unset` 查改删
- **歌词下载**：`--lrcs lrc,tlyric,romalrc,yrc,ass` 多类型（yrc 转 ASS），
  `--lrc-dir A --lrc-dir B` 多路径，`--only-lrc` 只下歌词（无版权歌照样可用）
- **信息 / 补全**：`ncm info song <ID/链接/关键词>` 看详情（歌名/歌手/专辑/时长/费用/版权），
  `ncm info lyric <ID/链接/关键词>` 打印 LRC 歌词到标准输出（可重定向），
  `ncm info playlist/album/artist <ID/链接>` 浏览曲目（`-n` 前 N 首），
  `ncm completions bash|zsh|fish|powershell` 生成自动补全脚本
- **登录后专属**：`ncm pl list`（我的歌单）、`ncm daily`（每日推荐）、
  `ncm pl create/delete/rename/add/remove`（歌单管理）、
  `ncm like <ID...> [--unlike]`（收藏到红心歌单，播放中 mpv 按 `l` 同效）、
  `ncm sign`（每日签到，移动端 +4 经验，`--web` 网页端 +1）、
  `ncm info subs`（收藏的专辑/歌手列表，`-t album,artist` 选类型）
- **评论 / 私人FM / 云盘 / MV**：`ncm info comment <歌曲ID>` 热门评论（可匿名，
  `-n` 条数）；`ncm fm like/trash/skip/get` 私人FM 红心/踩/跳过/拉取；
  `ncm info cloud` 云盘歌曲列表（`--offset` 翻页，下载用 `ncm download <ID>`）；
  `ncm info mv <ID>` MV 详情 + 各清晰度地址（`--play` 交给 mpv，`--json` 取链接）
- **VIP / 无版权解锁**：官方取址失败（`-110` 需 VIP / 404 无版权）或仅给 30s 试听时，
  自动回退本机解锁服务（UnblockNeteaseMusic-utils）拿完整直链，来源标 `mirror·xxx`；
  `--mirror <URL|none>` / `NCM_MIRROR` 配置地址（默认 `http://127.0.0.1:3000`）

## 用法

```bash
ncm login --session dump.json       # 保存登录态（一次性，之后自动加载）
ncm config set download-dir ~/music # 默认下载目录（download/play -o 未指定时生效）
ncm config set profile.download vip # 下载板块绑定 VIP 会话（login --profile vip 存的）
ncm config list                     # 查看全部配置
ncm search 晴天 -n 10               # 搜索浏览（表格，默认单曲）
ncm search -t song,playlist 周杰伦  # 多类型统一表（-t 逗号分隔，见下节）
ncm search 晴天 -q -n 3 --json      # 静默 JSON（管道友好）
ncm download 晴天 -d 1              # 按编号下载搜索结果（别名 ncm dl）
ncm download 晴天 -n 20 -d 3        # 扩大候选到 20 首再选第 3 首
ncm download <歌单链接> -o ~/music  # 歌单批量（TTY 交互选编号，管道下全量）
ncm download <歌单ID> -d 1-10 -y  # 只下前 10 首并跳过确认（多条目默认给预估+确认）
ncm download --daily -j 4 --m3u list.m3u  # 4 路并发下载并导出 M3U 播放列表
ncm info playlist 24381616 -n 30             # 浏览歌单前 30 首（看，不下载）
ncm daily                           # 浏览每日推荐
ncm pl list                         # 我的歌单（ncm pl 省略 list 同效）
ncm pl add 123456 -p 我喜欢的音乐    # 歌单加歌（pl create/delete/rename/remove 同族）
ncm like 123456                     # 收藏到红心歌单（--unlike 取消）
ncm sign                            # 每日签到（+4 经验，--web 网页端 +1）
ncm info comment 27536479                # 歌曲热门评论（可匿名）
ncm fm get -n 10                    # 私人FM 拉一批（fm like/trash/skip 操作）
ncm info subs                            # 收藏的专辑 + 歌手（-t album 只看一类）
ncm info cloud -n 30                     # 云盘歌曲列表（--offset 翻页）
ncm info mv 5432915 --play               # MV 详情并交给 mpv 播放
ncm download --daily -d all           # 每日推荐全部下载
ncm recommend                       # 首页推荐（默认只列每日推荐歌曲）
ncm recommend -t toplist,fm -n 20   # 追加榜单与私人FM（见推荐位表）
ncm play --daily --shuffle            # 随机队列播每日推荐
ncm play --roam                       # 私人漫游：FM 歌单 + 无限续播
ncm play 晴天 -n 5                  # 搜索最相关 5 首连播
ncm play "晴天" "七里香"              # 多目标按序拼接（daily/roam 现为 --daily/--roam 标志）
ncm play -f queue.txt --shuffle     # 文件列表（每行一个目标，# 注释）
ncm play 401722227 --quality lossless --save -o ~/music
ncm play <歌单链接> --json          # 只输出播放队列 JSON
ncm info song 晴天                       # 歌曲详情
ncm whoami                          # 登录状态
ncm completions bash > ~/.local/share/bash-completion/completions/ncm
```

### `search -t` 搜索类型

`-t/--type` 逗号分隔，数字与英文别名可混用；未收录的数字 type 也接受（通用降级
提取，类型列显示 `类型N`）。完整表同样见 `ncm search --help`。

| 数字 | 别名 | 含义 | 备注 |
|------|------|------|------|
| 1 | `song` | 单曲 | 默认 |
| 10 | `album` | 专辑 | |
| 100 | `artist` | 歌手 | |
| 1000 | `playlist` | 歌单 | |
| 1002 | `user` | 用户 | 仅列出 ID |
| 1004 | `mv` | MV | 仅列出 ID |
| 1006 | `lyric` | 歌词 | 仅列出 |
| 1009 | `radio` | 主播电台 | 仅列出 ID |
| 1014 | `video` | 视频 | 仅列出 ID |
| 1018 | `cloud` | 综合 | limit 无效（服务端不分页） |
| 2000 | `sound` | 声音 | 仅列出 |

```bash
ncm search -t song,playlist -n 4 周杰伦
```

```text
  #  名称                       艺术家·创建者   时长·曲数  类型  ID
  ────────────────────────────────────────────────────────────────────────────
  1  屋顶                       周杰伦          05:19      单曲  5257138
  2  想你就写信 (Live)          周杰伦          03:58      单曲  509781655
  3  周杰伦-Jay 『网易云精选』  Buradarrr       144 首     歌单  6792103822
  4  周杰伦全免费听             念着倒才傻n186  34 首      歌单  10057662169
  ────────────────────────────────────────────────────────────────────────────
  下载: ncm download "周杰伦" -d 编号  ·  浏览歌单: ncm info playlist <ID>
```

多类型按输入顺序分组、组内保持服务端返回顺序；`--json` 单类型输出数组，
多类型输出对象 `{"1": [...], "1000": [...]}`（键为 type 数字字符串，按字典序）。

### `recommend -t` 推荐位

| 值 | 数据源 | 说明 |
|----|--------|------|
| `daily` | 每日推荐歌曲 | 需登录，**默认**（不带 `-t` 只列它，不再默认铺榜单） |
| `toplist` | 官方榜单 | 榜单歌单表 |
| `fm` | 私人FM | 首批曲目 |
| `radar` | 私人雷达 | 私人雷达歌单（需登录，未命中给明确提示） |

```bash
ncm recommend                        # 等效 -t daily
ncm recommend -t daily,toplist -n 10 # 多推荐位，统一表输出
ncm recommend -t fm,radar
```

`--json` 形状（实测）：单 `daily`/`fm` → **数组**；单 `toplist` → 对象
`{"toplist": [...]}`；单 `radar` → 对象 `{"playlist_id","desc","songs"}`（未命中/未登录输出 `[]` 并报错）；多源 → 对象 `{"daily": [...], "toplist": [...]}`。

### `-n` 总条数语义

`-n/--limit` 统一表示**该命令输出的总条数**（多类型 / 多推荐位按类平分，余数给
前几类）；`0` = 全部（适用处：`search`/`recommend`/`list`/`daily`/`pl list`/
`play`；其中 `search` 单类单次请求上限 100，`recommend` 各源无 limit 参数，
`0`/大值受服务端单次返回量限制）。`download` 的 `-n`
是关键词候选取前 N 首（默认 10）。

常用选项（`ncm download --help` 全量，别名 `ncm dl`）：

| 选项 | 默认 | 说明 |
|------|------|------|
| `-o, --output` | config `download-dir`，无则 `.` | 音频输出目录 |
| `-n, --limit` | `10` | 关键词候选取前 N 首 |
| `-d, --download` | — | 按编号选择：`1,3` / `5-8` / `all` |
| `--lrc-dir` | 随音频 | 歌词目录，可重复 |
| `--lrcs` | `lrc,tlyric,romalrc` | 歌词类型 |
| `--only-lrc` | 关 | 只下歌词 |
| `-q, --quiet` | 关 | 静默输出 |
| `--quality` | `lossless` | standard/exhigh/lossless/hires |
| `-t, --template` | `{artists} - {track}` | 文件名模板 |
| `--skip-existing` | 关 | 跳过已存在文件 |
| `--allow-trial` | 关 | 放行 30 秒试听 |
| `-j, --jobs` | `download.jobs` | 并发下载数（1-16；未指定回退 config，>1 关闭逐曲动画） |
| `--m3u` | — | 完成后导出 M3U（成功曲目，路径相对列表文件） |
| `--json` | 关 | 机器可读输出 |

### mpv 播放键位（`ncm play`）

| 键 | 动作 |
|----|------|
| `空格` | 暂停 / 继续 |
| `←` / `→` | 快退 / 快进 5s（`Shift` 1s） |
| `n` / `p` | 下一首 / 上一首 |
| `/` | 跳转搜索 |
| `d` | 下载当前曲（键可改：`ncm config set play.download-key <键>`） |
| `l` | 收藏当前曲到红心歌单（键可改：`ncm config set play.like-key <键>`） |
| `u` | 取消收藏当前曲（键可改：`ncm config set play.unlike-key <键>`） |
| `q` | 退出 |

歌词同步：`ncm play` 自动拉 LRC 并通过 mpv IPC socket 跟随进度滚动显示
（`--no-lyric` 关闭；ffplay 无歌词显示，建议装 mpv）。底行实时显示
`♪ [i/N] mm:ss/mm:ss [进度条] 当前歌词`（每秒刷新，非 TTY 场景自动静默）。
状态栏尾部显示收藏状态（`play.show-liked` 门控，符号 `play.heart-symbol` 可改）与
喜欢人数（`*N`，FM/日推自带，歌单等来源切曲时后台补取，失败 6s 后自动重试直到取到）；
评论数默认不显示，`ncm config set play.show-comments true` 开启后同样后台补取并在
状态栏显示 `评N`（`play.show-comments=false` 关闭）。播放红心歌单（我喜欢的音乐）
时按已收藏显示 `♥`（加载中默认 ♥，不再显示易困惑的斜杠占位）。

## FLAC 取址

官方接口匿名 `br=999000` 会被服务端降级成 320k mp3，因此走第三方源（三源并发、
先到先得，429/5xx 自动熔断）：`xuanluoge / kangqiovo / vincentzyu`，全失败回退官方（降级 mp3）。
无版权歌（官方 404 + 第三方全挂）会明确报因，不误判为链接问题。

## 解锁音源（mirror + 第三方直连）

官方渠道对 VIP 曲返回 `-110`（url 为空）或仅 30s 试听，非会员无法播完/下载。
`fetch_row` 自动按序兜底：

1. **第三方直连源**（`xuanluoge` / `kangqiovo` / `vincentzyu`）：无需任何本地服务，
   直接解析出完整直链，音质可能较低但可完整播放/下载（来源标源名）
2. **本地 mirror 解锁服务**（UnblockNeteaseMusic-utils）：第三方未命中时兜底，
   支持逗号分隔配置多个候选地址依次尝试

```bash
# 可选：部署本地解锁服务（Node ≥18，默认 3000 端口）
npx @neteasecloudmusicapienhanced/unblockmusic-utils

ncm play 2088507144                      # 试听曲自动走第三方直连（无需本地服务）
ncm --mirror http://127.0.0.1:3100 download 186016   # 显式指定本地服务
NCM_MIRROR=http://127.0.0.1:3000,http://127.0.0.1:3100 ncm play --daily  # 多候选
NCM_MIRROR=none ncm download 186016            # 禁用本地 mirror（第三方直连仍生效）
```

- 本地 mirror 优先级：`--mirror` > 环境变量 `NCM_MIRROR` > `config mirror` 键 > 默认 `http://127.0.0.1:3000,http://127.0.0.1:3100`；`none` 禁用
- 单次匹配超时 10s；全部兜底失败时来源标 `官方·试听（无完整源）`，报错并入失败原因
- 成功时来源显示为 `mirror·<音源>`，下载校验按未知码率处理（不误判体积）

## 构建

```bash
cargo build --release        # 本地
cargo test && cargo clippy -- -D warnings && cargo fmt --check
```

CI（GitHub Actions）：push 即 fmt/clippy/test，随后产出
`linux-x86_64(musl 静态) / linux-aarch64 / windows / macos` 四平台二进制；
打 `v*` tag 自动发 Release。

## 依赖

`clap` + `clap_complete` + `tokio` + `reqwest(rustls)` + `lofty` + `serde_json` +
`aes/base64/flate2`（session 解析），无 jq、无 Python；登录可选（不登录也能
搜索/下载/播放，`daily`/`pl list` 需要登录）。
