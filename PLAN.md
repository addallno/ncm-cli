# ncm-rs 开发计划（PLAN.md）

> 按步骤执行，每步完成后 `cargo clippy -- -D warnings && cargo fmt --check` 全绿 + 手测 + commit。
> 最后一步统一 push 触发 CI。

## A. 现状

- `search`（表格/JSON、`--download` 选择）、`dl`（快捷方式、多目标、进度动画、
  第三方源 FLAC、歌词多类型、`--json`）已实现并手测通过；CI 已推送（run 36523525590 **未确认结果**）。
- **已确认缺陷**：音频内 **无嵌入封面、无嵌入歌词**（mutagen 实测 pictures=0、无 LYRICS 键）。

## B. Bug 与质量修复

| # | 问题 | 方案 |
|---|------|------|
| B1 | 嵌入封面/歌词缺失 | `tags::write_tags` 增加 `lyrics`/`cover` 参数：`insert_text(ItemKey::Lyrics,..)`（ID3v2→USLT、FLAC→LYRICS）+ `push_picture(CoverFront)`（mime 按魔数 jpeg/png）；封面取 `song.al.picUrl`，下载失败仅警告；标签写入移到歌词获取之后统一执行 |
| B2 | CI 结果未确认 | `gh run list` 看 36523525590，失败则修 |
| B3 | 死代码/冗余 | ① `SUBCOMMANDS` 含不存在的 convert/lyrics/watch；② `run()` None 分支重复解析（inject 已覆盖）；③ `quality` 别名 high/super/auto/master 冗余，只留 standard/exhigh/lossless/hires |
| B4 | 选项互斥未校验 | `--only-lrc` 与 `--no-lrc` 同时给 → 明确报错 |
| B5 | 测试产物 | 清理 /tmp/ncmrs*；README 与新选项同步 |

## C. CLI 惯例化（对照 curl / yt-dlp / git / ripgrep）

| 现状 | 改为 | 依据 |
|------|------|------|
| `-q, --quality` | `--quality`（无短），新增 `-q, --quiet` | `-q`=quiet 是 curl/git/yt-dlp 通例 |
| `--skip-existing` | 仅别名 `--no-overwrites`（**不加 `-n`**：与 `search -n/--limit` 的全局短选项冲突，clap debug_assert 会 panic） | yt-dlp `-n, --no-overwrites` |
| search `--download` | 加 `-d` 短选项 | 直觉惯例 |
| `dl -S/--search` + `dl --limit` | **删除**，targets 传非数字非 URL 时自动当关键词搜索 | 消除与 `search` 子命令的重复入口，且 `ncm 关键词` 直接可用 |
| `disable_help_subcommand = true` | 启用 `ncm help <cmd>` | git/curl 通例 |
| 无 | 新增 `--no-color` + 识别 `NO_COLOR` 环境变量 | no-color.org 事实标准 |
| 无 | 新增 `--dry-run`（取址+报告将写文件，不落盘） | ansible/terraform 通例 |
| 保留不动 | `-o/--output`（curl 同名）、`-n/--limit`（head 同款语义）、`-t/--template`、`-a/--artist`、`--json`、`--lrc-dir`、`--lrcs`、`--no-lrc`、`--only-lrc`、`--allow-trial`、`-V/-h` | 已符合惯例或无冲突 |

删除 `dl -S` 后：`ncm 关键词`、`ncm dl 关键词`、`ncm search 关键词 -d 1` 三条路都通，覆盖不降级。

## D. 新功能

### D1. 歌单浏览 + 多选下载
- `ncm list <目标>`：列出歌单/专辑/歌手的作品（`# 歌名 歌手 时长 ID` 表格），
  目标 = 歌单/专辑 URL 或 ID；`--json` 输出。
- **选择器统一扩展**：`-d` 选择器（`1,3,5-8/all/1-`）对 搜索/歌单/专辑 结果都生效：
  `ncm list <id> -d 1,3`、`ncm dl <歌单> -d 1-5`、`ncm search kw -d all`。
- **交互多选**：TTY 且给了列表但没给 `-d` 时，提示 `选择要下载的编号 (如 1,3,5-8/all): `
  读取一行；非 TTY 保持"全部下载"现状（脚本兼容），`--json` 下禁止 prompt。

### D2. pyncm 兼容登录（看全部歌单 / 我喜欢的音乐）
- 输入方式（三选一，优先级从高到低）：
  1. `--session <file>`：读 pyncm session dump，兼容两种格式：
     - 新：`PYNCM` + base64(zlib(json))，json 内 `cookies[]` 含 `MUSIC_U/__csrf`
     - 旧（legacy eapi 加密）：大写 hex → AES-128-ECB（key `e82ckenh8dichen8`，PKCS7）解密 →
       `pyncm-36cd479b6b5-{json}-36cd479b6b5-{md5}` → 取 json cookies
  2. `--cookie "MUSIC_U=xxx; __csrf=yyy"`（pyncm 1.7 `LoginViaCookie` 同款，浏览器 F12 直接粘）
  3. 环境变量 `NCM_SESSION` / `NCM_COOKIE`
- 注入 `Ncm` client 的 Cookie header（`MUSIC_U` 生效于所有明文 API）。
- `ncm playlists`：列出我创建/收藏的歌单（含"我喜欢的音乐"红心歌单），表格 + `--json`。
- 新增依赖：`aes`、`base64`、`flate2`（legacy 解密不需要 md5——只解不签）。

### D3. 漫游 / 每日推荐 / 播放
- `ncm daily`：每日推荐歌单（`/api/v1/discovery/recommend/songs`，需登录），
  表格列出，支持 `-d` 选择下载 —— 即"给出漫游列表"。
- `ncm play <目标|daily>`：
  1. 解析出曲目后取播放地址（复用 fetch_row，含第三方源）
  2. 播放：探测 `$NCM_PLAYER` → `mpv` → `ffplay`，找到哪个用哪个（URL 直接喂给播放器）；
     都没有则打印可播放 URL（`--json` 给结构化）
  3. `ncm play daily` = 播放每日推荐第一首（或 `--shuffle` 随机）

## E. 执行顺序

| Step | 内容 | 验收 |
|------|------|------|
| 0 | 确认 CI 结果（B2）、清 /tmp 测试产物 | CI 绿或已定位失败原因 |
| 1 | 本 MD | — |
| 2 | B1 嵌入封面+歌词、B3/B4 清理 | mutagen 读回：pictures=1、LYRICS 有值；clippy/fmt 绿 |
| 3 | C 惯例化（选项映射、quiet、no-color、help、dry-run） | `ncm help dl` 可用；`-q` 静默；NO_COLOR 生效 |
| 4 | D1 浏览+多选（含 dl 关键词自动搜索、selector 全面化、TTY 交互） | `ncm list <歌单>` 表格；`-d 1,3` 只下选中；TTY 提示选择 |
| 5 | D2 pyncm 登录 + `ncm playlists` | 解密真实 pyncm 文件或构造样本通过；登录后歌单列表含红心 |
| 6 | D3 daily / play | 列表可下；play 能喂 URL 给播放器或打印 |
| 7 | 收尾：README/Description 更新、全量手测、push | CI 绿 |

> 状态（2026-09-29）：Step 0-7 全部完成，CI success，clippy 0 warning，单测 3 passed。

## F. 明确不做

- weapi/eapi 请求加密（明文 `/api` 已够用；登录只靠 cookie）
- 内置音频播放器（调用外部播放器）
- TUI（留给后续独立评估）
