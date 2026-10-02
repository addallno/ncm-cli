# ncm-cli — 网易云音乐统一命令行工具（Rust）

## 用途

Rust 重写版网易云 CLI，替代远端 `pyncm`（8.3 MB aarch64）与 `musicdl`（30+ 依赖包），
单二进制覆盖：搜索、下载（音频+歌词+封面）、浏览歌单、每日推荐、队列播放（mpv
交互+歌词同步）、登录持久化、歌曲详情、shell 补全。
CLI 选项与 Python 原型 `ncmt-local` 同名兼容。

## 架构

```
src/
├── main.rs      CLI 定义（clap derive）+ 命令分支 + run_batch（dl 统一入口）+ 播放队列编排
├── api.rs       明文接口封装（search/detail/playlist/lyric/player_url/daily/user_playlists/whoami），
│                Ncm::with_cookie 注入登录 Cookie
├── session.rs   pyncm session 双格式解析 + 凭据持久化（~/.config/ncm/cookie，0600；
│                resolve_auth 优先级 --session > --cookie > 持久化文件）
├── source.rs    第三方 FLAC 源（xuanluoge/kangqiovo/vincentzyu，429/5xx 熔断），失败回退官方
├── mirror.rs    解锁音源（UnblockNeteaseMusic-utils REST /match）：官方 -110/404/试听时兜底，
│                --mirror / NCM_MIRROR 配置（默认 127.0.0.1:3000，none 禁用），30s 超时
├── download.rs  下载核心：Range 探测、.part+长度比对、魔数/时长/STREAMINFO 校验、降级重试
├── model.rs     Track 模型、文件名模板、目标解析（ID/链接/list.123 → 曲目列表）
├── player.rs    mpv IPC 客户端：Unix socket observe time-pos/playlist-pos，
│                歌词二分定位 + 终端单行滚动（♪ [i/N]）
├── lrc.rs       LRC 时间轴解析（parse_lrc_timeline）、yrc 转 ASS、多类型歌词
├── tags.rs      lofty 写 ID3：封面嵌入、音轨编号、多类型歌词帧
├── lrc.txt      内嵌歌词资源
└── ui.rs        进度条/spinner/表格/颜色（NO_COLOR、TTY 检测、管道降级）
```

## 命令

| 命令 | 说明 |
|------|------|
| `ncm search <kw>` | 纯浏览：表格 / `--json`（下载走 dl） |
| `ncm dl <目标>` | 唯一下载入口：单曲/歌单/专辑/歌手/关键词/`daily`；`-d` 选择器、TTY 交互、`-n` 候选数；完成出结果一览表（状态/来源/大小/失败原因）+ 耗时汇总 |
| `ncm list <目标>` | 纯浏览歌单/专辑/歌手作品列表 |
| `ncm playlists` | 我创建/收藏的歌单（需登录） |
| `ncm daily` | 每日推荐浏览（需登录） |
| `ncm play <目标\|daily\|roam>` | 队列播放：`-n` 队列数、`--shuffle`、`--save -o` 先下载、`--json`、`--no-lyric`、`--roam` 播放漫游（队列将尽自动接私人FM续播）、`roam` 目标单开私人FM漫游歌单；mpv 下 <>/←→ 交互+歌词滚动（`--prefetch-playlist` 预开下首，跳过无缓冲等待），双行底行 `mm:ss/mm:ss [进度条] 原文` + `译文(dim)` 按终端宽度截行（非 TTY 静默）；切曲历史打一行 `▶ [i/N] 歌名`；**分批取址**：先播前 6 首，后续随播随取（防网易直链 ~10min 过期 403 雪崩）；取址先本地 mirror 再第三方（快起播） |
| `ncm recommend` | 首页推荐：每日推荐歌曲表（需登录）+ 官方榜单歌单表（`-n` 控制数量，`--json`） |
| `ncm login/--logout` | 保存/查看/清除登录凭据（持久化） |
| `ncm whoami` | 当前登录状态（昵称/uid/凭据来源） |
| `ncm info <目标>` | 歌曲详情（标题行/歌名/歌手/专辑/时长/费用着色/版权），VIP 曲提示 mirror 解锁，`--json` |
| `ncm completions <shell>` | 生成 bash/zsh/fish/powershell 补全 |
| `ncm <ID>` | 快捷下载（顶层无子命令即 dl） |

登录凭据优先级：`--session <file>` / `--cookie <str>` > 环境变量 `NCM_SESSION`/`NCM_COOKIE` >
`ncm login` 持久化文件。

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
