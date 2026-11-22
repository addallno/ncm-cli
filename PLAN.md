# PLAN：回退延迟改造 / 下一曲错乱 / lrc 时间对齐

任务（用户 m0196/m0198）：
1. 「为什么这个回退为了避免他的不断循环，而要加一个延迟呢。我觉得这个机制完全可以改一下，比如说就改成保持前排和后排的特定数列内的音乐链接不失效。」
2. 「当我选择切换下一曲时，它有概率使队列错乱，并且无法自动恢复。」
3. 「lrc歌词下载的时间对齐似乎还是不准。」

## 根因（已定位）
- ① `<` 走 nav→guard 2s 轮询→refresh_entry（fetch 慢则 6s 超时）→跳转，延迟=等刷新防过期回卷；保鲜只覆盖**前 3 首/60s**，无后排行。
- ② `>` 无专门处理，直接 keypress 转发 mpv（playlist-next）**不刷新下一首**→过期 403→mpv 播放失败→状态脱节错乱；与 `<` 不对称。
- ③ `parse_lrc_timeline` 对 `[offset:xx]` 元数据行直接跳过（lrc.rs:339），偏移未应用→整体对齐偏差。

## 步骤
1. **双向保鲜**（改回退预热块 main.rs:4553+）：范围从「前 3 首」扩为**前 N + 后 N 首**（N=5，跳过当前曲），60s 周期不变（链接 10min 过期，60s 覆盖绰绰有余）。
   - 验收：cargo check；预热范围含 pos+1..pos+5。
2. **跳转免延迟**（nav 消费 main.rs:4505+）：目标 `|target-pos| ≤ N` 时**直接 playlist-play-index**（保鲜窗口内链接必新鲜，免 refresh 等待）；窗口外保留原 refresh_entry 路径兜底。
   - 验收：cargo check + 单测（若抽纯函数）。
3. **`>` 对称接管**（key_reader player.rs）：`>` 同 `<` 读 pos→`nav.push_back(p+1)`（p+1 < count 才接管，队尾交 mpv 默认）反馈「▶ 下一曲」；guard 消费同一 nav 路径。
   - 验收：cargo check；按键行为走 nav。
4. **lrc offset 应用**（lrc.rs）：parse 时收集 `[offset:xxx]`（毫秒整数），应用 `t - offset/1000`（正=提前，业界惯例）；改测试 `parse_offset_tag_ignored` → `parse_offset_tag_applied` 断言偏移生效。
   - 验收：lrc 测试全绿。
5. **三绿**：fmt + clippy + test（基线 76，增改后核对）。
6. **中文 commit 直推 master**（不发版；0.3.53 拍板仍待用户）。

## 风险与边界
- 本地 mpv 0.37 拒 insert-at：保鲜 refresh 的 insert 路径本机 Unsupported（已有提示停用），远端 0.41 可验；本轮改动以范围/跳转逻辑为主。
- ②的「无法自动恢复」自愈（mpv 播失败检测 end-file）**本轮不做**——先消概率源头（保鲜+对称），自愈列后续；计划内明说。
- 队尾 `>`：交 mpv 默认（不回卷），与现行为一致。
