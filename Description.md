# ncm-cli — 网易云音乐统一命令行工具（Rust）

## 用途

Rust 重写版网易云 CLI，替代远端 `pyncm`（8.3 MB aarch64）与 `musicdl`（30+ 依赖包），
单二进制覆盖：多类型搜索（`search -t` 逗号分隔）、下载（音频+歌词+封面）、浏览歌单、
每日推荐与首页推荐（`recommend -t daily,toplist,fm,radar`）、队列播放（mpv
交互+歌词同步）、登录持久化、歌曲详情、shell 补全。严格子命令：`ncm <未知命令>`
报 `unrecognized subcommand`（不再自动当下载）。
CLI 选项与 Python 原型 `ncmt-local` 同名兼容。

## 架构

```
src/
├── main.rs      CLI 定义（clap derive）+ 命令分支 + run_batch（download 统一入口）+ 播放队列编排
├── searchtype.rs search/recommend 的 type 解析（英文别名表、配额 quotas、统一表行提取 extract_row）
├── api.rs       接口封装（读走明文 /api/，写走 weapi/eapi 加密通道），
│                Ncm::with_cookie 注入登录 Cookie 并解析 __csrf
├── crypto.rs    weapi/eapi 加密（AES-128-CBC/ECB 手写、RSA 教科书加密、
│                eapi 响应解密；参数对齐 pyncm 1.8.1）
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
| `ncm download <目标>`（别名 `ncm dl`） | 唯一下载入口：单曲/歌单/专辑/歌手/关键词 + `--daily` 每日推荐；`-d` 选择器、TTY 交互、`-n` 候选数、`-j/--jobs 2..16` 并发下载（>1 关闭逐曲动画）、`--m3u <文件>` 导出 M3U（成功曲目，路径相对列表）；完成出结果一览表（状态/来源/大小/失败原因）+ 耗时汇总；多目标部分解析失败只报该目标并继续，全部失败才整体报错 |
| `ncm list <目标>` | 纯浏览歌单/专辑/歌手作品列表 |
| `ncm pl list` | 我创建/收藏的歌单（需登录） |
| `ncm daily` | 每日推荐浏览（需登录） |
| `ncm play <目标...>` | 多目标按序拼接（`ncm play "A" "B"`），`-f <文件>` 每行一个目标（`#` 注释跳过，可与命令行混用）； 队列播放：`-n` 队列数、`--shuffle`、`--save -o` 先下载、`--json`、`--no-lyric`、`--daily` 每日推荐队列、`--roam` 播放漫游（队列将尽自动接私人FM续播；`play --roam` 且无目标时单开FM歌单）；mpv 下 <>/←→ 交互+歌词滚动（`--prefetch-playlist` 预开下首，跳过无缓冲等待），双行底行 `mm:ss/mm:ss [进度条] 原文` + `译文(dim)` 按终端宽度截行（非 TTY 静默）；切曲历史打一行 `▶ [i/N] 歌名`；**参数透传** `-- --volume=60 --ao=alsa`（经 `--` 分隔置于最后，同名以用户为准，仅 mpv，ffplay 忽略）；mpv stderr 管道过滤（`--msg-level=all=error,ao=warn,ad=warn`）：错误/音频设备等有用行打成 dim toast，音量/速度变化经 IPC 轮询打 `♪ 音量 60%`/`♪ 速度 1.50x`，memfd/ipc 杂散日志不打乱底行，播放计数含 guard 续喂；**分批取址**：先播前 6 首，后续随播随取（防网易直链 ~10min 过期 403 雪崩）；取址先本地 mirror 再第三方（快起播）；mpv 键位：空格 暂停、←/→ 快退/快进、</> 上/下曲、/ 搜索、`d` 下载当前曲、`l` 收藏当前曲（`play.download-key`/`play.like-key` 可配）；播放队列标 `★N` 显示歌曲喜欢人数（likeCount） |
| `ncm recommend` | 首页推荐：`-t/--type` 推荐位逗号分隔（`daily` 每日推荐·默认 / `toplist` 官方榜单 / `fm` 私人FM / `radar` 私人雷达），默认只列每日推荐歌曲；`-n` 总条数按位平分（0 = 全部），统一表 / `--json` |
| `ncm login/--logout` | 保存/查看/清除登录凭据（持久化）；`--profile <名>` 多账号另存 `cookie.<名>` |
| `ncm config set/get/list/unset` | 持久化配置：`download-dir` 默认下载目录、`play.download-key` 播放快捷下载键（默认 `d`）、`play.like-key` 播放快捷收藏键（默认 `l`）、`profile.<板块>` 板块绑定会话（板块：default/download/play/recommend/roam/search/list） |
| `ncm whoami` | 当前登录状态（昵称/uid/凭据来源） |
| `ncm like [IDS...] [--unlike]` | 收藏/取消收藏歌曲到"我喜欢的音乐"（需登录；ID/链接均可，可多个） |
| `ncm pl create/delete/rename/add/remove` | 歌单管理（需登录；create 新建、delete 删除含全部歌曲不可恢复、rename 改名、add 添加歌曲、remove 移除歌曲） |
| `ncm info <目标>` | 歌曲详情（标题行/歌名/歌手/专辑/时长/费用着色/版权），VIP 曲提示 mirror 解锁，`--json` |
| `ncm comment <ID>` | 歌曲热门评论（明文读接口可匿名；`-n` 条数、`--json`），含昵称/时间/赞数 |
| `ncm sign [--web]` | 每日签到（需登录；默认移动端 +4 经验，`--web` 网页端 +1） |
| `ncm subs [-t album,artist]` | 收藏的专辑/歌手列表（需登录；`-t` 逗号分隔选类型，默认两类、`-n` 每类条数） |
| `ncm mv <ID> [--res 1080] [--play]` | MV 详情 + 播放地址（240/480/720/1080）；`--play` 交 mpv 播放，`--json` 取链接 |
| `ncm cloud [-n] [--offset]` | 云盘歌曲列表（需登录；标准表，下载用 `ncm download <ID>`，暂不支持上传） |
| `ncm fm like/trash/skip/get` | 私人FM 操作（需登录）：`like <ID> [--unlike]` 红心、`trash <ID>` 踩、`skip <ID>` 跳过并拉下批、`get -n` 拉取 |
| `ncm completions <shell>` | 生成 bash/zsh/fish/powershell 补全 |

严格子命令：未知首参由 clap 报 `unrecognized subcommand 'xxx'`（`ncm 123` 不再
自动当下载），无参数打印 help。
`-n/--limit` 统一语义 = 输出总条数，多类型/多推荐位平分，`0` = 全部
（`search`/`recommend`/`list`/`daily`/`pl list`/`play`；`download` 为候选数）。

登录凭据优先级：`--session <file>` / `--cookie <str>` > 环境变量 `NCM_SESSION`/`NCM_COOKIE` >
`-u/--user <名>` 指定 profile（优先级高于板块绑定）> 板块绑定的 profile（`ncm config set profile.<板块> <名>`）> `ncm login` 默认持久化文件。
板块绑定的 profile 无凭据时报错（不静默回退默认账号）；profile 名仅限 `[A-Za-z0-9._-]`（防路径穿越）；凭据文件以 0600 直接创建。
播放时自动检测 VIP 账号（若已配置多个 profile）。
板块判定（`section_of`）：download（别名 dl）=download、play --roam=roam、play --daily/daily/recommend=recommend、
search=search、list/playlist=list、其余=default；播放中按下载键用 download 板块凭据下载，按收藏键用当前播放凭据。
旧目标别名 `daily/today/roam` 已移除：字面输入按关键词搜索并打一行迁移提示（改用 `--daily`/`--roam`）。
`ncm help play`/`help download` 选项按「选择/输出/歌词/执行/队列/播放器」分组显示。

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

## TUI 输出规范（v0.3.32 起）

### 符号语义表（Unicode）

| 符号 | 码点 | 名称 | 语义 | 颜色 | 出现位置 |
|------|------|------|------|------|----------|
| ▸ | U+25B8 | BLACK RIGHT-POINTING SMALL TRIANGLE | 过程步骤 | 青 36 | `ui::step` 行首 |
| ▶ | U+25B6 | BLACK RIGHT-POINTING TRIANGLE | 播放中 / 歌词占位行 | 青 36 | 播放反馈、overlay 无词占位 |
| ▷ | U+25B7 | WHITE RIGHT-POINTING TRIANGLE | 已暂停（空心播放三角） | 黄 33 | 播放反馈（第二行） |
| ♪ | U+266A | EIGHTH NOTE | 歌曲相关（音量/热度/歌词行） | 黄 33 / 青 36 | 反馈、overlay 歌词行 |
| ✔ | U+2714 | HEAVY CHECK MARK | 成功 | 绿 32 | `ui::ok`、反馈 |
| ✘ | U+2718 | HEAVY BALLOT X | 失败/错误 | 红 31 | `ui::fail`、mpv 错误反馈 |
| ◌ | U+25CC | DOTTED CIRCLE | 跳过/已存在（非错误） | 黄 33 | `ui::skip`、反馈 |
| ⚠ | U+26A0 | WARNING SIGN | 警告 | 黄 33 | 试听 30s 等 |
| ★ | U+2605 | BLACK STAR | 热度（喜欢人数） | 青 36 | 播放队列卡片、热度反馈 |
| ♥ | U+2665 | BLACK HEART SUIT | 已喜欢状态 | — | 热度反馈 |
| · | U+00B7 | MIDDLE DOT | 字段分隔符 | — | 所有消息内部字段 |
| 「」 | U+300C/U+300D | CJK corner brackets | 引用歌名/歌单名 | — | 所有消息 |
| ░ / █ | U+2591 / U+2588 | LIGHT SHADE / FULL BLOCK | 播放进度条 | — | overlay |
| ⣾ 等 8 帧 | U+2800–U+28FF | BRAILLE PATTERN | 加载 spinner | 青 36 | `ui::Spinner` |
| … | U+2026 | HORIZONTAL ELLIPSIS | 省略/截断 | — | 队列省略行 |

### 消息通道

- **滚动区行**（step/ok/skip/fail/info）：命令过程与结果，留历史
- **第二行反馈**（`player::toast` → FEEDBACK 槽，2s 消失）：播放期一切单行反馈——音量、暂停/播放、喜欢结果、热度/评论、mpv 错误（前缀 ✘），不留屏

### 语言规范

- 歌曲红心统一称「喜欢」（`like` 命令、l 键、热度状态）；歌单/专辑/歌手加入保持「收藏」
- 句式：动宾短语，无句号、无语气词；字段用 `·` 分隔，歌名/歌单用 `「」`
- 音量一律 `%`；中文标点用全角（：），数字与单位半角
