# ncm-cli — 网易云音乐统一命令行工具（Rust）

## 用途

Rust 重写版网易云 CLI，替代远端 `pyncm`（8.3 MB aarch64）与 `musicdl`（30+ 依赖包），
单二进制覆盖：搜索、下载（音频+歌词+封面）、浏览歌单、每日推荐、播放。
CLI 选项与 Python 原型 `ncmt-local` 同名兼容。

## 架构

```
src/
├── main.rs      CLI 定义（clap derive）+ 命令分支 + 批量流程 run_batch/browse + 播放器探测
├── api.rs       明文接口封装（search/detail/playlist/lyric/player_url/daily/user_playlists/whoami），
│                Ncm::with_cookie 注入登录 Cookie
├── session.rs   pyncm session 双格式解析：PYNCM(魔数+base64+zlib+json) / legacy(hex+AES-128-ECB+PKCS7)
├── source.rs    第三方 FLAC 源（xuanluoge/kangqiovo/vincentzyu，429/5xx 熔断），失败回退官方
├── download.rs  下载核心：Range 探测、.part+长度比对、魔数/时长/STREAMINFO 校验、降级重试
├── model.rs     Track 模型、文件名模板、目标解析（ID/链接/list.123 → 曲目列表）
├── tags.rs      lofty 写 ID3：封面嵌入、音轨编号、多类型歌词帧
├── lrc.txt      内嵌歌词资源（yrc 转 ASS 等）
└── ui.rs        进度条/spinner/表格/颜色（NO_COLOR、TTY 检测、管道降级）
```

## 命令

| 命令 | 说明 |
|------|------|
| `ncm search <kw>` | 搜索表格；`-d/--download 1,3/all` 选择下载 |
| `ncm dl <目标>` | 单曲/歌单/专辑/歌手/关键词下载；默认 FLAC+歌词 |
| `ncm list <目标>` | 浏览歌单/专辑/歌手；TTY 交互选编号；`-d` 选择 |
| `ncm playlists` | 我创建/收藏的歌单（需登录） |
| `ncm daily` | 每日推荐（需登录），`-d` 选择下载 |
| `ncm play <目标\|daily>` | 播放：`--player`→`$NCM_PLAYER`→mpv→ffplay，否则打印 URL |
| `ncm <ID>` | 快捷下载（顶层无子命令即 dl） |

统一选择器：`-d 1,3,5-8/all/1-`；登录凭据：`--session <file>` / `--cookie <str>` /
环境变量 `NCM_SESSION`/`NCM_COOKIE`。

## 构建与验证

```bash
cargo build --release
cargo test && cargo clippy --all-targets && cargo fmt --check
```

CI（GitHub Actions，push 触发）：fmt/clippy/test → 四平台产物
（linux-x86_64 musl 静态 / linux-aarch64 / windows / macos），`v*` tag 发 Release。

## 依赖

clap、tokio、reqwest(rustls)、lofty、serde_json、aes/base64/flate2；无 jq、无 Python。
