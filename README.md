# ncm — 网易云音乐统一命令行工具

Rust 重写版，替代远端 `pyncm`（8.3 MB aarch64 二进制）与 `musicdl`（30+ 依赖包）。
行为规格源自 Python 原型（`ncmt-local`）的实测验证，CLI 选项同名兼容。

## 功能

- **浏览 / 下载分工**：`search` `list` `daily` `playlists` `recommend` 只负责看
  （表格 / `--json`），`download`（别名 `dl`）是唯一下载入口；
  `-d 1,3,5-8/all/1-` 选择器只在 `download` 出现，TTY 下没给 `-d` 会交互选编号
  （回车仅列出）
- **多类型搜索 / 首页推荐**：`ncm search -t song,playlist,...` 逗号分隔多类型
  （数字与英文别名可混用），一张统一表 + 「类型」列；`ncm recommend` 默认只列
  每日推荐歌曲，`-t daily,toplist,fm,radar` 选推荐位；直接 `ncm list <ID>` 浏览、
  `ncm download list.<ID>` 下载
- **严格子命令**：`ncm <未知命令>` 直接报 `unrecognized subcommand`，
  不再把未知参数悄悄当成下载
- **播放漫游**：`ncm play roam` 单开私人FM漫游歌单（首曲开播，队列将尽自动
  接FM无限续播）；普通队列加 `--roam` 同样开启续播（mpv 下无感接力）
- **下载**：单曲 / 歌单 / 专辑 / 歌手链接 / `list.123` / `daily` / 关键词，
  **默认 FLAC + 歌词 + 保留音频**；标签写入标题/歌手/专辑/歌词/封面外，**嵌入
  `ncm-id`**（FLAC 为 Vorbis 键，MP3 为 TXXX 帧，凭此可反查歌 ID）；进度条 /
  spinner / 完整性防线（Range → `.part` → 魔数/时长校验 → 全过才落盘）；完成后出结果一览表（`#`/状态/曲目/来源/大小/备注）
  与耗时汇总，管道/脚本下输出干净无 ANSI 裸序列
- **播放**：`ncm play <ID/链接/关键词|daily|roam>`，队列喂给 mpv（`<`/`>` 切曲、
  `←/→` 快退进、空格暂停、`/` 搜索、`q` 退出；开 `--prefetch-playlist`
  预开下一首，跳过时新曲已就绪不用等缓冲），歌词随播放滚动并按终端宽度
  自适应截行（窄屏不换行叠影）；原文与译文分两行显示（line1 时间+原文，
  line2 译文 dim）；**分批取址**：先播前 6 首，后续随播随取新链接
  （网易直链约 10 分钟过期，全量预取会导致长队列后段 403 雪崩）；`--save -o 目录`
  先下载到本地再播本地文件；`--shuffle` 洗牌、`-n` 队列长度、`--json` 输出队列；
  播放器参数经 `--` 透传（mpv 需 `=` 形式）：`ncm play 晴天 -- --volume=60 --ao=alsa`；
  mpv 非 fatal 日志全静默（杂散 stderr 不再打乱歌词底行），`播放完毕 · N 首` 含续喂数
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
  `profile.<板块>` 会话绑定；`ncm config get/list/unset` 查改删
- **歌词下载**：`--lrcs lrc,tlyric,romalrc,yrc,ass` 多类型（yrc 转 ASS），
  `--lrc-dir A --lrc-dir B` 多路径，`--only-lrc` 只下歌词（无版权歌照样可用）
- **信息 / 补全**：`ncm info <ID/链接/关键词>` 看详情（歌名/歌手/专辑/时长/费用/版权），
  `ncm completions bash|zsh|fish|powershell` 生成自动补全脚本
- **登录后专属**：`ncm playlists`（我的歌单）、`ncm daily`（每日推荐）
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
ncm list 24381616 -n 30             # 浏览歌单前 30 首（看，不下载）
ncm daily                           # 浏览每日推荐
ncm download daily -d all           # 每日推荐全部下载
ncm recommend                       # 首页推荐（默认只列每日推荐歌曲）
ncm recommend -t toplist,fm -n 20   # 追加榜单与私人FM（见推荐位表）
ncm play daily --shuffle            # 随机队列播每日推荐
ncm play roam                       # 私人漫游：FM 歌单 + 无限续播
ncm play 晴天 -n 5                  # 搜索最相关 5 首连播
ncm play 401722227 --quality lossless --save -o ~/music
ncm play <歌单链接> --json          # 只输出播放队列 JSON
ncm info 晴天                       # 歌曲详情
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
  下载: ncm download "周杰伦" -d 编号  ·  浏览歌单: ncm list <ID>
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

### `-n` 总条数语义

`-n/--limit` 统一表示**该命令输出的总条数**（多类型 / 多推荐位按类平分，余数给
前几类）；`0` = 全部（适用处：`search`/`recommend`/`list`/`daily`/`playlists`/
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
| `-q, --quality` | `lossless` | standard/exhigh/lossless/hires |
| `-t, --template` | `{artists} - {track}` | 文件名模板 |
| `--skip-existing` | 关 | 跳过已存在文件 |
| `--allow-trial` | 关 | 放行 30 秒试听 |
| `--json` | 关 | 机器可读输出 |

### mpv 播放键位（`ncm play`）

| 键 | 动作 |
|----|------|
| `空格` | 暂停 / 继续 |
| `←` / `→` | 快退 / 快进 5s（`Shift` 1s） |
| `n` / `p` | 下一首 / 上一首 |
| `/` | 跳转搜索 |
| `d` | 下载当前曲（键可改：`ncm config set play.download-key <键>`） |
| `q` | 退出 |

歌词同步：`ncm play` 自动拉 LRC 并通过 mpv IPC socket 跟随进度滚动显示
（`--no-lyric` 关闭；ffplay 无歌词显示，建议装 mpv）。底行实时显示
`♪ [i/N] mm:ss/mm:ss [进度条] 当前歌词`（每秒刷新，非 TTY 场景自动静默）。

## FLAC 取址

官方接口匿名 `br=999000` 会被服务端降级成 320k mp3，因此走第三方源（按优先级，
429/5xx 自动熔断）：`xuanluoge → kangqiovo → vincentzyu`，全失败回退官方（降级 mp3）。
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
NCM_MIRROR=http://127.0.0.1:3000,http://127.0.0.1:3100 ncm play daily  # 多候选
NCM_MIRROR=none ncm download 186016            # 禁用本地 mirror（第三方直连仍生效）
```

- 本地 mirror 优先级：`--mirror` > 环境变量 `NCM_MIRROR` > 默认 `http://127.0.0.1:3000,http://127.0.0.1:3100`；`none` 禁用
- 单次匹配超时 30s；全部兜底失败时来源标 `官方·试听（无完整源）`，报错并入失败原因
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
搜索/下载/播放，`daily`/`playlists` 需要登录）。
