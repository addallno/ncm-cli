# PLAN — 播放期输出路由 + 防拼行（C3/C4/C6bc/C15）

> 来源：TODO C3/C4/C6/C15（m0216 反馈 + fqdt 45 号教训）。一步一验，全过三绿后中文 commit push。

## 背景与现状

- 播放 overlay（stderr）：行1 状态栏（含右侧 2s toast/FEEDBACK）+ 行2 歌词 + 行3 译文（可选）。
- 快捷下载 `quick_download`：先 `clear_overlay_rows()` 再走 `ui::step/fail` 输出进**滚动区**——歌词栏被清、输出与 overlay 抢行（C4 痛点）。
- 播放期错误（快捷下载失败等）走滚动区散落，不进 toast（C3）。
- `log.rs` `-v` 镜像 `eprintln!` 与 stdout spinner/Live 单行进度并发 → 拼行双行残留（C15，fqdt 45 号）。
- C6b：`concurrent()` 已禁 Live/Spinner（ui.rs live()/Spinner::start 均检查）——待实测确认后标完成。

## 设计

### 1. C15 — 动态行解耦 `ui::detach_line()`
- `ui.rs` 新增 `pub fn detach_line()`：TTY 下 `\r\x1b[K` 擦掉当前 stdout 动态行（即 clear_cur_line 导出，语义=「后续 stderr 输出前先解绑进度行」）。
- `log.rs` stderr 镜像写入前调用 `ui::detach_line()`：日志行固化进历史，spinner/Live 下一帧在新光标位重画，不拼行。

### 2. C3/C4/C6c — 播放期输出统一走 overlay（sink 分流）
- `ui.rs` 新增全局 sink：`type Sink = Arc<dyn Fn(&str, bool) + Send + Sync>`（msg, is_fail），`set_sink()/clear_sink()`。
  - `step/ok/skip/info` → sink(msg, false)
  - `fail` → sink(msg, true)
  - 无 sink 时行为不变（滚动区路径）。
- `player.rs` 新增：
  - `static PROCESS: Mutex<Option<String>>` + `PROCESS_DIRTY`——**常驻第三行**（歌词/译文之下，OVERLAY_ROWS 深度 +1），无 TTL、被下一条覆盖，overlay 退出时清。
  - `pub fn install_ui_sink()`：`ui::set_sink(|msg, is_fail| if is_fail { toast(msg) } else { process_line(msg) })`——错误进 toast（状态栏 2s），过程进第三行。
  - `print_row` 渲染时 PROCESS 有值 → 追加画过程行（dim 前缀 `↓`）。
  - overlay_loop 退出（OVERLAY_ALIVE=false）→ `ui::clear_sink()`。
- `quick_download`：删 `clear_overlay_rows()` 预清（sink 接管后不再进滚动区）；成功收据改 `toast("✓ 已下载 · label")`（C6c：结果走 status bar，不用 download 同款滚动区消息）；下载管线 `DownloadArgs.quiet=false` 不变——step/ok/fail 由 sink 分流。
- 安装点：播放分支 spawn overlay 前 `player::install_ui_sink()`（仅播放路径，非播放命令不受影响）。

### 3. C6b — 验证后标完成
- 实测并发下载（-j>1）：无 spinner/Live 残留、结果行完整 → `[x]`。

### 4. 验收（一步一验）
1. `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` 三绿。
2. 无 `-v` 运行：零日志行；`-v` 下模拟进度期日志（跑带进度的下载）：无拼行。
3. 播放冒烟（本地 mpv）：按快捷下载键 → 第三行出现「快捷下载 · …」、歌词不被清；下载失败 → 状态栏 toast ×…；播放退出后 sink 移除、下载命令输出照旧。
4. 非播放 `ncm download` 输出与改前一致（回归）。
