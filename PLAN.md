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

## P. v0.3.5 播放器参数透传（2026-10-02）

| # | 内容 | 状态 |
|---|------|------|
| P1 | 用户需求「I need it allow to pass args to mpv, like --volume, --ao, --ar.」——`Play` 加 `player_args: Vec<OsString>`（clap `last = true`，经 `--` 分隔）；UTF-8 校验后 `args.extend` 置于内置参数之后（mpv 同名后者生效）；ffplay 选项体系不同 → 提示忽略 | ✅ |
| P2 | 单测 +1：`--` 后参数全部归 `player_args` 不被 ncm 选项拦截（18 tests） | ✅ |

验收：clippy 0、18 tests；实跑 `ncm play <id> --no-lyric -- --ao=null --volume=5` mpv 正常启动静默播放（mpv 只认 `=` 形式，help/文档已注明）。

## Q. v0.3.6 播放杂散日志与播放计数修复（2026-10-03）

| # | 内容 | 状态 |
|---|------|------|
| Q1 | 用户贴远端输出报告行错乱（`▶`/`♪` 同行拼接）与 `shared memfd open() failed: Unknown error 38`、`[ipc_N] Write error (Broken pipe)` 刷屏——根因：mpv 杂散 stderr 日志不带 ANSI 光标协议，插进 overlay 双行打乱行状态（`ipc=no` 匹配不到带编号的 `ipc_2348` module）；修：`--msg-level=all=fatal`（排障可经 `--` 透传覆盖） | ✅ |
| Q2 | `播放完毕 · N 首` 用首批 `entries.len()`，guard 续喂未计入（报告 3 首实播到 32）——修：`Arc<AtomicUsize>` fed 计数，成功 append 才累加，退出 `n_play + fed` | ✅ |
| Q3 | guard 续取失败 `ui::skip` 直接打印会插进 overlay 行——修：TTY 下先 `player::clear_overlay_rows()`（下秒 time-pos 自动重刷底行） | ✅ |

验收：clippy 0、18 tests；远端实测无 memfd/ipc 噪音、`▶`/`♪` 各自成行、播放计数含续喂。

## R. v0.3.7 封面压缩与标签健壮性（2026-10-04）

| # | 内容 | 状态 |
|---|------|------|
| R1 | 用户报告「The music file it download have a large picture header, that normal player cannot load the picture regularly.」远端扫描实证：Dehors 单首 PICTURE 1MB/3000x3000、Ari Abdul 0x0 尺寸、10 月 8 首无图（标签字段俱在=当时封面 fetch 瞬断，非 bail）。根因：`write_tags` 原样内嵌网易原图（可达 8MB 上限）。修：`prepare_cover` 压到最长边 ≤1000 + JPEG q85（image crate），lofty 从规范 JPEG 解出正确尺寸 → 实证 1.5MB→176KB/1000x1000 | ✅ |
| R2 | 封面 MIME 无法识别时旧代码 `bail` 中断整个标签写入（标题/歌词全丢）——修：识别失败仅跳过封面；`image_mime` 删除，webp 等交由 image 解码器，全格式统一转 JPEG | ✅ |
| R3 | 单测 +1：`prepare_cover` 大图压缩断言（≤1000px + JPEG 魔数）与垃圾输入 None（19 tests） | ✅ |
| R4 | 远端存量修复（不 pull 本地）：Dehors/Ari Abdul 用 ffmpeg 抽图缩放 + `~/fixcover.py`（python 原地重建 metadata，帧数据/SEEKTABLE 不动，SOF 头解析真尺寸）；8 首无图 `~/fix8.py`（VORBIS TITLE/ARTIST → search+song/detail 拿 picUrl → 下载缩放嵌入） | 🔶 远端网络抖动进行中 |

验收：clippy 0、19 tests；新下载 PICTURE ≤~200KB/1000x1000；远端重扫无 BIG/0x0/none。

## S. v0.3.8 关键词下载封面 + 网络退避重试（2026-10-04）

| # | 内容 | 状态 |
|---|------|------|
| S1 | 用户定位：「when ncm download music, it will actually not download and embed it at the while」——本地复现坐实：`ncm dl 关键词` 静默无图。根因：`search()` 返回的 json 不带 picUrl，封面段三处取值全 None 直接跳过且无提示。修：`pick_pic_url` 纯函数抽离，None 时二跳 `song_detail([track.id])` 补 `album.picUrl`；仍无 → `ui::info("该曲无封面信息，跳过嵌入")` 不再静默。单测 +1（三形状/空串/缺失，20 tests） | ✅ |
| S2 | 用户要求「if it cannot connect to DNS or others, silently retry 5 times then report if still failing」+「not just try 5 times instantly」——新增 `src/retry.rs`：`retry_reqwest` 指数退避 1/2/4/8/16s，共 5 次重试，全程无输出；`transient()` 只重试连接/超时/请求/传输类错误，HTTP 状态码直接返回不空耗。接入 `api::get_json`、`api::fetch_bytes`、`download::probe_once`、`download` 下载 send、`mirror::try_match` 五处 | ✅ |
| S3 | 复测：`ncm dl 关键词` → PICTURE 嵌入成功（封面最长边上限按用户建议 1600） | ✅ |

验收：clippy 0、20 tests；关键词路径出图；远端跑 fix8.py 补 8 首 + 部署本版。

## T. v0.3.9 下载嵌入 ncm-id 反查键（2026-10-04）

| # | 内容 | 状态 |
|---|------|------|
| T1 | 用户要求「In ncm-cli you should add to embed ncm-id.」——`write_tags` 增参 `ncm_id: Option<u64>`，写入键 `ncm-id`（`src/tags.rs` 常量 `NCM_ID_KEY`），main.rs 下载路径传 `Some(track.id)`。关键坑：lofty `Tag::insert_text` 内部 `re_map(tag_type, allow_unknown=false)` 会直接拒掉 `ItemKey::Unknown`，必须 `insert_unchecked`。FLAC→Vorbis 键（`verify_key` 校验 ASCII 0x20~0x7D 非 `=`，通过）；MP3→`ItemKey::Unknown` 经 `Tag→Id3v2Tag` 转换落成 TXXX 描述帧（读回走 description≠4字符分支 round-trip） | ✅ |
| T2 | 单测 +3（23 tests）：`writes_ncm_id_as_vorbis_key`（极简 FLAC fixture：fLaC+STREAMINFO+尾 PADDING，覆盖写+读回）、`skips_ncm_id_when_absent`、`writes_ncm_id_as_txxx_on_mp3`（Tag→Id3v2Tag→`get_user_text`）。另以 ffmpeg 生成真 mp3 端到端临时验证通过后删除 | ✅ |

验收：clippy 0、23 tests、fmt clean。

## U. v0.3.10 播放译文行回归修复（2026-10-04）

| # | 内容 | 状态 |
|---|------|------|
| U1 | 用户报「现在他歌词又不显示翻一行了。」——根因：`fetch_lyrics`(main.rs:2007) 与 `guard_loop` 续歌取词(main.rs:2103) 都只读 `v["lrc"]["lyric"]`，**从未合并 `tlyric`**（curl 实测 API 返回里 tlyric 有完整译文，keys 含 lrc/tlyric/romalrc/yrc/ytlrc）。v0.3.4 的分行显示只在 lrc 恰好同刻自带译文时才可见，故表现为「又不显示」。修：`lrc.rs` 新增 `merge_translation`（译文插到同刻原文之后、**沿用原文时间戳**以满足 `current_line` 的 `lines[i-1].0 == lines[i].0` 精确相等判定，容差 ≤0.05s 吸收微偏移，无匹配译文丢弃防错位成主行）与 `timeline_from_lyric(&Value)`（lrc 缺失返回空不 panic）；两处取词点改调 `timeline_from_lyric` | ✅ |
| U2 | 单测 +5（28 tests）：`merge_translation_pairs_same_timestamp`、`merge_translation_snaps_slight_offset`、`timeline_from_lyric_{without_tlyric,merges_tlyric,missing_lrc}`；另用真实 API（id=1817576399）临时 ignored 测试验证 `lines=120 pairs=59`、样例配对输出正确，验证后删除 | ✅ |

验收：clippy 0、28 tests、fmt clean；真实 API 译文成对。

## V. v0.3.11 译文行对齐与配色（2026-10-04）

| # | 内容 | 状态 |
|---|------|------|
| V1 | 用户反馈：「译文行应当与原文行对齐头才对。颜色就不要用灰色了吧，也不要用白色。你不要用绿色，用一个淡蓝色吧。」——`print_row` line2 由顶格改为跳过 `display_width(prefix)+display_width(meta)` 空格，与 line1 原文起始列对齐；颜色 `ui::dim` → 新增 `ui::soft_blue`（256 色 `38;5;117` 亮天蓝） | ✅ |

验收：fmt clean、clippy 0、28 tests。

## W. v0.3.12 config 配置 / 多会话板块绑定 / 播放快捷下载（2026-10-06）

| # | 内容 | 状态 |
|---|------|------|
| W1 | 用户需求：「我需要一个快捷键，当我在play的时候能够快捷下载，能够支持config下载到指定的目录。可以通过命令行直接配置，而不是改配置文件。希望能支持多个用户会话，并且希望能为不同板块分别设置不同的用户会话。比如说我希望为推荐和漫游设置一个用户的cookie，而下载时使用另一个VIP的cookie。」——新增 `src/config.rs`：`~/.config/ncm/config.json` 白名单键（`download-dir` / `play.download-key` / `profile.<板块>`，板块=default/download/play/recommend/roam/search/list），`ncm config set/get/list/unset` 子命令，0700/0600 权限；**事故：`SUBCOMMANDS` 注入白名单缺 `config` 导致 `ncm config ...` 被注入 `dl` 真实误下载 123MB×2，修数组长度 12→13 并补词，误下文件已全部清理** | ✅ |
| W2 | 多会话与板块绑定：`session.rs` 新增 `cookie_path/load_profile/save_profile/clear_profile`（`login --profile <名>` 存 `cookie.<名>`，default 复用旧 `cookie` 文件），`resolve_auth` → `resolve_auth_for(section, …)`（CLI/env > 板块 profile 映射 > 默认凭据）；`main.rs` 新增 `section_of()` 板块判定（dl=download、play roam=roam、play daily/daily/recommend=recommend、search=list/list=list…）接入 run() | ✅ |
| W3 | 下载目录配置：`DownloadArgs.output` 与 `play -o` 改 `Option<PathBuf>`，回退链 `-o` > `config download-dir` > `.`（dl / play --save / 快捷下载共用） | ✅ |
| W4 | 播放快捷下载：mpv 临时 input-conf（`{key} script-message ncm-download`，键取 `play.download-key` 默认 `d`）+ `player::spawn_msg_listener` 独立 IPC 客户端置 `DOWNLOAD_REQ` 原子量 + `guard_loop` 扩参（queue/dl_ncm/dl_dir，`#[allow(too_many_arguments)]`）消费请求 → `quick_download`（song_detail 复用 download_songs，download 板块凭据，先 clear_overlay_rows 防乱行）；`guard_needed=is_mpv`、pending 耗尽非漫游由 return 改 continue（监听至 mpv 退出）；`download_audio` 进度回调 `dyn FnMut` → `impl FnMut + Send`（future 可 spawn）；mpv 键位提示行加「{key} 下载当前曲」 | ✅ |
| W5 | 测试 28→34：config 3 个（set/get/unset 回转、非法键拒、默认值）、session `profile_paths_isolated`（多 profile 隔离+CLI 优先）、cli `subcommand_whitelist_covers_all`（clap 子命令名 vs SUBCOMMANDS 防 inject 回归）+ `section_of_commands`；持久化测试共用 `config::tests::LOCK` 防 XDG_CONFIG_HOME 互踩 | ✅ |

验收：fmt clean、clippy 0（`--all-targets -- -D warnings`）、34 tests、`--help` 正常列出 config；config/login 冒烟通过。
