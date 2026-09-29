# ncm — 网易云音乐统一命令行工具

Rust 重写版，替代远端 `pyncm`（8.3 MB aarch64 二进制）与 `musicdl`（30+ 依赖包）。
行为规格源自 Python 原型（`ncmt-local`）的实测验证，CLI 选项同名兼容。

## 功能

- **搜索**：`ncm search 关键词 -n 10`，对齐表格输出，`--download 1|1,3|all` 直接下载
- **下载**：单曲 / 歌单 / 专辑 / 歌手链接 / `list.123`，**默认 FLAC + 歌词 + 保留音频**
- **浏览**：`ncm list <歌单/专辑/歌手>` 表格列出；统一选择器 `-d 1,3,5-8/all/1-` 对
  search/list/daily 都生效；TTY 下没给 `-d` 会提示交互输入编号（回车仅列出）
- **登录（可选）**：`--session <pyncm dump>`（兼容 PYNCM/legacy 双格式）或
  `--cookie "MUSIC_U=...; __csrf=..."`（浏览器 F12 直接粘），环境变量 `NCM_SESSION`/`NCM_COOKIE` 同效；
  登录后可用 `ncm playlists`（我的歌单）与 `ncm daily`（每日推荐）
- **播放**：`ncm play <ID/链接/关键词|daily>`，探测 `--player` → `$NCM_PLAYER` → `mpv` → `ffplay`，
  都没有则打印可播放 URL；`--json` 结构化输出
- **歌词**：`--lrcs lrc,tlyric,romalrc,yrc,ass` 多类型（每类型独立文件，yrc 转 ASS），
  `--lrc-dir A --lrc-dir B` 多路径，`--only-lrc` 只下歌词（无版权歌照样可用）
- **动画**：下载进度条（`████░░ 45.7% · 14.6/30.5 MB · 4.1 MB/s`）原地刷新、
  取址/歌词阶段 spinner、管道输出自动降级不刷屏、`--json` 零污染
- **完整性防线**：Range 探测 → `.part` + 长度比对 → 魔数/时长/体积校验 →
  FLAC 按 STREAMINFO 未压缩量反推下界 → 全过才落盘；试听默认跳过；高音质失败自动降级重试

## 用法

```bash
ncm search 青花瓷 -n 5                 # 搜索列表
ncm search 晴天 --download 1           # 搜索并下载第 1 条
ncm 259138                             # 快捷下载（== ncm dl 259138）
ncm dl <歌单链接> -o ~/music           # 歌单批量
ncm list 24381616 -n 30                # 浏览歌单前 30 首（TTY 会提示选编号）
ncm list 24381616 -d 1,3               # 只下载第 1、3 首
ncm playlists -n 20                    # 我的歌单（需登录）
ncm daily -d all                       # 每日推荐全部下载（需登录）
ncm play daily --shuffle               # 随机播一首每日推荐
ncm play 401722227 --quality lossless  # 播放指定歌曲
ncm dl <关键词> -S --quality exhigh    # 搜索下载、指定音质
```

常用选项（`ncm dl --help` 全量）：

| 选项 | 默认 | 说明 |
|------|------|------|
| `-o, --output` | `.` | 音频输出目录 |
| `--lrc-dir` | 随音频 | 歌词目录，可重复 |
| `--lrcs` | `lrc,tlyric,romalrc` | 歌词类型 |
| `--only-lrc` | 关 | 只下歌词 |
| `-q, --quality` | `lossless` | standard/exhigh/lossless/hires |
| `-t, --template` | `{artists} - {track}` | 文件名模板 |
| `--skip-existing` | 关 | 跳过已存在文件 |
| `--allow-trial` | 关 | 放行 30 秒试听 |
| `--json` | 关 | 机器可读输出 |

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

`clap` + `tokio` + `reqwest(rustls)` + `lofty` + `serde_json` + `aes/base64/flate2`（session 解析），
无 jq、无 Python；登录可选（不登录也能搜索/下载/VIP 试听外全功能）。
