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

## G. v0.2.0 增强（2026-09-30）

| Step | 内容 | 状态 |
|------|------|------|
| A | 去重合：search/list/daily/playlists 改纯浏览（ViewArgs），`-d`/TTY 交互归 `dl` 独占，`DownloadArgs` 加 `-n`，删 `browse()`，`run_batch` 统一 dl 流程（daily 字面量/关键词/目标） | ✅ |
| B | 登录持久化：`ncm login/--logout/whoami`，`~/.config/ncm/cookie`(0600)，`resolve_auth` 自动回退；`whoami()` 返回 uid+昵称 | ✅ |
| C | `play` 重写：整队列喂 mpv（n/p/←→ 键位）、`-n`/`--shuffle`/`--save -o`（先下载播本地）/`--json`/`--no-lyric`、mpv IPC 歌词同步滚动（player.rs）、`parse_lrc_timeline`；ffplay 降级逐首、无播放器打印地址 | ✅ |
| D | `info <目标>` 详情、`completions <shell>`（clap_complete）、README/Description/PLAN 同步、v0.2.0 发布、远端二进制更新 | ✅ |

验收：`cargo fmt && cargo clippy --all-targets && cargo test` 全绿（13 tests），
play 无头实测（--json / mpv --ao=null 双曲 / --save 回落 / 无播放器）通过；
CI 四平台 success（Windows 构建修复：mpv IPC `cfg(unix)` 门控），
`v0.2.0` Release 7 资产，远端 8.162.6.112 `~/ncm` 已更新为 0.2.0。

## H. v0.2.1 修复与增强（2026-10-01）

用户反馈：VIP 曲（daily 拉取的歌单）非会员播不完需走 mirror；匹配/搜索有问题；播放界面和提示简陋。

| 项 | 内容 | 状态 |
|----|------|------|
| H1 | mirror 解锁音源（mirror.rs）：官方 `-110`/`404`/30s 试听时自动回退 UnblockNeteaseMusic-utils `GET /match?id=`；`--mirror <URL\|none>` + `NCM_MIRROR`（默认 `127.0.0.1:3000`，`none` 禁用，30s 超时，br=0 跳过体积校验，source 标 `mirror·xxx`） | ✅ |
| H2 | 错误文案细分：需 VIP(-110) / 无版权或已下架(404) / mirror 未配置给部署提示 / 不存在 ID 明确报因 | ✅ |
| H3 | 目标解析修复：`parse_target` 含 `.` 的 URL 曾误入 `list.xx` 分支（歌单链接报"无法识别"）；单曲描述带歌名 | ✅ |
| H4 | 搜索相关度稳定重排（完全相等>前缀>包含>未命中，同档保接口原序）；search 表格双提示去除；dl 空结果带目标上下文 | ✅ |
| H5 | 播放界面：队列卡片（歌名/来源/音质/⚠30s试听警告）、mpv 状态行抑制防刷屏、无歌词时底行歌名占位+`[i/N]` 切曲提示、结束「✔ 播放完毕」总结 | ✅ |

验收：`cargo fmt && cargo clippy --all-targets && cargo test` 全绿（15 tests、clippy 0）；
CI 四平台 success，`v0.2.1` Release（初版 tag 忘 bump 版本号已废弃重建）；
远端 43.136.171.164（Termux）实测：`dl 186016 --mirror` 下载 12.1MB `周杰伦 - 晴天.mp3`（源 mirror·unm）、
`play 29774171` VIP 曲走 mirror·byfuns 完整播放+歌词滚动、daily 队列卡片正常；
解锁服务 `~/UnblockNeteaseMusic-utils`（PORT=3100，启动脚本 `~/start-unm.sh`）。

## I. v0.2.2 输出视觉统一与体验增强（2026-10-01）

用户反馈："More to enhance and design better."（开放式——自主挑高价值项实施）

| 项 | 内容 | 状态 |
|----|------|------|
| I1 | ui 层：`display_width` 剥 ANSI（着色字符串可直接作表格单元格）、`table()` 底部收尾线、search 结果表着色（歌名 accent、时长/ID dim） | ✅ |
| I2 | info 卡片：标题行 `♪ 歌名 · 歌手`、费用值着色（免费 good、VIP/专辑购买 warn）、VIP/专辑曲尾提示 `--mirror/NCM_MIRROR` 解锁 | ✅ |
| I3 | dl 汇总升级：逐首失败错误截断 64 字、结果一览表（`#`/状态/曲目/来源/大小/备注，失败原因截断 30）、汇总失败/跳过只计数不重复全文、完成行加耗时、`--json` 加 `elapsed_secs` | ✅ |
| I4 | 播放进度条：mpv IPC observe `duration`，底行 `♪ [i/N] mm:ss/mm:ss [bar] 歌词`（每秒重绘，duration 未知显 `mm:ss` 无 bar）；非 TTY 全链路静默（overlay 与收尾清理的 `\x1b[2K` 均加 TTY 门控，管道输出零裸 ANSI） | ✅ |

验收：fmt/clippy/tests 全绿（15 tests、clippy 0）；管道采样无 `ESC[2K`、
`script -qec` 模拟 pty 实测进度条逐秒刷新；dl 成功/跳过/失败三态汇总表对齐；
info/search 着色+收尾线在 TTY 正常、管道自动退化为纯文本。

## J. v0.2.3 修复：我的歌单接口下线（2026-10-01）

| 项 | 内容 | 状态 |
|----|------|------|
| J1 | `ncm playlists` 返回空表：网易已下线 `/api/v1/user/playlist`（JSON `code=404`，对照 uid=1 同样 404），改用 `/api/user/playlist`（实测 200、11 个歌单） | ✅ |
| J2 | `user_playlists` 响应加 `code` 校验（非 200 报错带 message），不再静默成空——根因是 `get_json` 不校验业务 code 的显式化补丁（全局校验风险大，仅修此接口） | ✅ |

验收：clippy 0、15 tests、远端 `~/ncm playlists` 列出全部歌单。
背景：同日用户报 `playlists` 空输出 + DNS fallback 报错——DNS 为 Termux 瞬时断联（curl 0.4s 恢复），空表是本接口问题。

## K. v0.2.4 试听曲自动第三方直连 + mirror 多候选（2026-10-01）

用户反馈：`play 2088507144`（GODS）只有 30s 试听，"make it play from mirror auto
with a low quality"；澄清 "I don't need local mirror server, I mean the mirror is
the third-party provided downloading files."，且 "But remain the mirror feature now."

| 项 | 内容 | 状态 |
|----|------|------|
| K1 | fetch_row 兜底顺序改为：官方失败/试听 → ①第三方直连源（resolve_lossless 三源，无需本地服务、低质可接受、br=0）→ ②本地 mirror 服务（保留）；lossless 请求不重复试第三方 | ✅ |
| K2 | 本地 mirror 多候选：`--mirror`/`NCM_MIRROR` 支持逗号分隔，`match_url` 依次尝试聚合报错；默认 `3000,3100`（修复远端 unm 部署在 3100 而默认 3000 连不上的静默失败）；删无引用的 `base()` | ✅ |
| K3 | 全兜底失败时来源标 `官方·试听（无完整源）`（原 mirror 失败静默，用户看不出走没走兜底） | ✅ |

验收：clippy 0、15 tests；本地 `play 2088507144 --json` source=`kangqiovo` 完整直链、
`play 29774171`（VIP）source=`vincentzyu`、mirror 失败标注生效。

## L. v0.3.0 播放漫游 / 首页推荐 / 播放取址顺序（2026-10-02）

用户需求：「我需要添加漫游功能。并且如果从第三方API上下载来的歌曲，是无法支持到比较低的音质，
使得能够快速加载的话，那我建议的是还是先走mirror local。」「播放漫游」「以及我希望能够获取到
他推荐的歌单和歌曲，在首页上推荐的。」

| 项 | 内容 | 状态 |
|----|------|------|
| L1 | 取址顺序按场景：`fetch_row` 加 `mirror_first`——play 先本地 mirror（快起播低质可接受）再第三方；dl 保持第三方保音质再 mirror。提取 `tp_fallback`/`mirror_fallback`/`row_incomplete` helper | ✅ |
| L2 | `ncm recommend [-n N]`：每日推荐歌曲表（需登录，空则降级仅榜单）+ 官方榜单歌单表（`/api/toplist` 实测 200；`/api/v1/discovery/recommend/resource` 登录墙 301 放弃）；`SUBCOMMANDS` 加 `recommend`（修顶层快捷 dl 吞子命令的 bug） | ✅ |
| L3 | `ncm play --roam` 播放漫游：私人FM（`/api/v1/radio/get`，标准 song 结构）；`roam_loop` 2s 轮询 mpv `playlist-pos/count`，剩余 ≤3 首预取，`loadfile append` 追加（serde_json 构造防转义）+ 歌词/label 同步 push 进 `Arc<RwLock<Vec<LyricTrack>>>`（overlay 三处读点改 read 锁）；取址失败曲丢弃；启动期连续 3 次查询失败才退出；mpv 退出 abort task。`Ncm` 加 `#[derive(Clone)]`；`resolve_lossless` 的 `dyn Future` 补 `+ Send`（tokio::spawn 要求）；player.rs 新增 `ipc_send`/`ipc_get`（request_id 匹配） | ✅ |

验收：clippy 0、15 tests；本地 `NCM_PLAYER="mpv --ao=null --length=5" ncm play 晴天 -n 3 --roam`
45s 跑满 timeout（15s 队列被 FM 无限接力）证明 append 生效；`recommend -n 5` 榜单表 5 行正常
（每日推荐本地无登录态显示降级提示）。

## M. v0.3.1 分批取址防 403 / play roam 入口 / 底行宽度自适应（2026-10-02）

| # | 内容 | 状态 |
|---|------|------|
| M1 | 用户反馈①：「我需要添加漫游功能」→ `ncm play roam` 独立入口（FM 首曲开播，复用续播守护）；`let roam = roam \|\| target=="roam"` 自动开 | ✅ |
| M2 | 用户反馈②：每日推荐播到第 13 首起全部 `[ffmpeg] HTTP 403` —— 确诊网易直链 ~10 分钟过期，33 首播放前批量预取全老化；修：`PLAY_PREFETCH=6` 分批取址（仅 mpv+unix），`guard_loop`（roam_loop 泛化）队尾将尽时把 pending 续入：在线地址**现取**（`fetch_row` mirror_first=true），本地文件直 append；pending 空 + roam → FM 补充；`--ytdl=no` 去坏 yt-dlp traceback、`msg-level` 加 `ffmpeg=warn` 压掉与底行交错的 ffmpeg 消息 | ✅ |
| M3 | 用户反馈③：「lrc 行只显示前 N 字符+...且疑似叠影」—— `print_row` 预算写死 60 列；修：`terminal_size_of(stderr)` 实取宽度，预算 = 屏宽 - prefix - meta - 1（窄屏不 wrap 叠影，宽屏多显示） | ✅ |
| M4 | `PendingItem{label,src,track}`、slot 汇总加 cutoff（第 6 个在线地址后全部进 pending 保顺序）；`guard_needed = is_mpv && (!pending \|\| roam)` 独立于歌词（`--no-lyric` 下漫游也可用）；socket 提到 overlay 分支外 | ✅ |

验收：clippy 0、15 tests；本地 12 首队列只取址 6 首 + `分批加载` 提示，40s 跑满；
script 伪终端 100 列实测底行 `♪ [1/3] 00:01 / 04:39 [bar] 歌词` 正常无 wrap。

## N. v0.3.3 切曲歌名行（2026-10-02）

| # | 内容 | 状态 |
|---|------|------|
| N1 | 用户反馈：播放中底行只有歌词（`♪ [5/10] ... 歌词`），看不到当前曲名无法下载；修：overlay playlist-pos 切曲时先 `\r\x1b[2K` 清底行，再 eprintln 一行 `▶ [i/N] 歌名 - 歌手`（dim，进滚动历史可回翻），随后底行恢复歌词 | ✅ |

验收：clippy 0、15 tests；script 伪终端 100 列实测 `▶ [1/3] 晴天(深情版) - Lucky小爱` 独立行 + 底行歌词正常。

## O. v0.3.4 译文分行底行 / 探测重试（2026-10-02）

| # | 内容 | 状态 |
|---|------|------|
| O1 | 用户需求「And the translated line should bee another line, instead of show in the window time.」——`current_line` 改返回 `(原文, Option<译文>)`（网易 lrc 同刻成对、稳定排序后原文在前：`lines[i-1].0 == lines[i].0` 判对）；`print_row` 双行恒定输出（line1 前缀+时间+原文，line2 dim 译文），`ON_SECOND_LINE` atomic 管回移（`\x1b[1A\r`）；切曲歌名行与退出清理走 `clear_overlay_rows()`（两行清理） | ✅ |
| O2 | 探测失败加固：`probe_link` 失败重试 1 次（网络瞬断误杀整曲）；错误改 `{e:?}` 带 source 链（dns/timeout/connect 根因可见） | ✅ |
| O3 | 单测 +2：同刻对分行（主/副）、占位无副行（17 tests） | ✅ |

验收：clippy 0、17 tests；script 伪终端双行对结构 `\n\r\x1b[2K` 正常（line2 仅混译歌非空，Greed/Shape 抽测为单语空行属预期）。
