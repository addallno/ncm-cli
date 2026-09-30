# ncm — 网易云音乐统一命令行工具

Rust 重写版，替代远端 `pyncm`（8.3 MB aarch64 二进制）与 `musicdl`（30+ 依赖包）。
行为规格源自 Python 原型（`ncmt-local`）的实测验证，CLI 选项同名兼容。

## 功能

- **浏览 / 下载分工**：`search` `list` `daily` `playlists` 只负责看（表格 / `--json`），
  `dl` 是唯一下载入口；`-d 1,3,5-8/all/1-` 选择器只在 `dl` 出现，TTY 下没给 `-d`
  会交互选编号（回车仅列出）
- **下载**：单曲 / 歌单 / 专辑 / 歌手链接 / `list.123` / `daily` / 关键词，
  **默认 FLAC + 歌词 + 保留音频**；进度条 / spinner / 完整性防线（Range → `.part`
  → 魔数/时长校验 → 全过才落盘）
- **播放**：`ncm play <ID/链接/关键词|daily>`，队列整批喂给 mpv（`n/p` 切曲、
  `←/→` 快退进、空格暂停、`/` 搜索、`q` 退出），歌词随播放滚动；`--save -o 目录`
  先下载到本地再播本地文件；`--shuffle` 洗牌、`-n` 队列长度、`--json` 输出队列
- **登录持久化**：`ncm login --session <pyncm dump>`（兼容 PYNCM/legacy）或
  `--cookie "MUSIC_U=...; __csrf=..."`（浏览器 F12 直接粘），凭据保存到
  `~/.config/ncm/cookie`（0600），之后所有命令自动加载；`ncm whoami` 看状态、
  `ncm login --logout` 登出；环境变量 `NCM_SESSION`/`NCM_COOKIE` 仍可临时覆盖
- **歌词下载**：`--lrcs lrc,tlyric,romalrc,yrc,ass` 多类型（yrc 转 ASS），
  `--lrc-dir A --lrc-dir B` 多路径，`--only-lrc` 只下歌词（无版权歌照样可用）
- **信息 / 补全**：`ncm info <ID/链接/关键词>` 看详情（歌名/歌手/专辑/时长/费用/版权），
  `ncm completions bash|zsh|fish|powershell` 生成自动补全脚本
- **登录后专属**：`ncm playlists`（我的歌单）、`ncm daily`（每日推荐）

## 用法

```bash
ncm login --session dump.json       # 保存登录态（一次性，之后自动加载）
ncm search 晴天 -n 10               # 搜索浏览（表格）
ncm search 晴天 -q -n 3 --json      # 静默 JSON（管道友好）
ncm dl 晴天 -d 1                    # 按编号下载搜索结果
ncm dl 晴天 -n 20 -d 3              # 扩大候选到 20 首再选第 3 首
ncm 259138                          # 快捷下载（== ncm dl 259138）
ncm dl <歌单链接> -o ~/music        # 歌单批量（TTY 交互选编号，管道下全量）
ncm list 24381616 -n 30             # 浏览歌单前 30 首（看，不下载）
ncm daily                           # 浏览每日推荐
ncm dl daily -d all                 # 每日推荐全部下载
ncm play daily --shuffle            # 随机队列播每日推荐
ncm play 晴天 -n 5                  # 搜索最相关 5 首连播
ncm play 401722227 --quality lossless --save -o ~/music
ncm play <歌单链接> --json          # 只输出播放队列 JSON
ncm info 晴天                       # 歌曲详情
ncm whoami                          # 登录状态
ncm completions bash > ~/.local/share/bash-completion/completions/ncm
```

常用选项（`ncm dl --help` 全量）：

| 选项 | 默认 | 说明 |
|------|------|------|
| `-o, --output` | `.` | 音频输出目录 |
| `-n, --limit` | `10` | 关键词候选数量 |
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
| `q` | 退出 |

歌词同步：`ncm play` 自动拉 LRC 并通过 mpv IPC socket 跟随进度滚动显示
（`--no-lyric` 关闭；ffplay 无歌词显示，建议装 mpv）。

## FLAC 取址

官方接口匿名 `br=999000` 会被服务端降级成 320k mp3，因此走第三方源（按优先级，
429/5xx 自动熔断）：`xuanluoge → kangqiovo → vincentzyu`，全失败回退官方（降级 mp3）。
无版权歌（官方 404 + 第三方全挂）会明确报因，不误判为链接问题。

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
