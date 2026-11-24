# PLAN：FM 漫游升级（模式 API + trash 键 + 删 fm 子命令 + help 设计）

任务（用户 m0143/m0144/m0145 + 决策 m0065）：
1. help 消息设计学 `~/openwork/fqdt/HELP-DESIGN.md`，为 ncm-rs 做自己的规范。
2. fm 不必要：合并进 `play --roam`，播放中新增 like/trash 键位。
3. 搜索网易云其他 FM API——真实 app 漫游曲目已不同、推荐更贴喜好。

## 调研结论（已完成）

- **根因**：真实 app 走同一端点 `eapi /v1/radio/get` 但带模式参数（constself/NeteaseCloudMusicApi `personal_fm_mode.js`，2024-01 上线）：
  - `mode`: DEFAULT / FAMILIAR（熟悉）/ EXPLORE（探索）/ SCENE_RCMD（场景）
  - `subMode`: 场景子模式（EXERCISE / FOCUS / NIGHT_EMO …）
  - `limit`: app 默认 3；响应仍为 `data` 歌曲数组
- ncm-rs 现状：裸 GET `/api/v1/radio/get`（无 mode/limit）→ 推荐不准、曲目不同。
- `ncm fm` 子命令 = Like/Trash/Skip/Get；Get 已被 `recommend -t fm` 覆盖。
- 播放键位现有 d/l/u（config `play.download-key/like-key/unlike-key`），无 trash。
- help：全 clap 默认，无四层机制；UI 问题 a（`-u --user` 长行挤行）。

## 决策（用户已拍板）

- FM 默认模式 = **FAMILIAR**（熟悉模式，最贴喜好）
- trash 播放键默认 = **t**（config `play.trash-key` 可改）
- `ncm fm` 子命令 = **直接删**（breaking → 0.4.0）

## 步骤

1. **PLAN.md 本文件**（本步）。
2. **radio_songs 升级**（api.rs）：改 `post_eapi("/v1/radio/get", {e_r, mode, subMode?, limit})`；
   签名 `radio_songs(mode, scene, limit)`；config 新增 `fm.mode`（默认 FAMILIAR）/ `fm.scene`（默认空=不传）/ `fm.limit`（默认 10）。
   - 验收：cargo check；调用点全部改读 config。
3. **Quick::Trash + t 键**：player.rs enum/key_reader 接 trash 键（优先级 d > l > u > t）；main.rs dispatch → `fm_trash`；config `play.trash-key` 默认 t。
   - 验收：cargo check + 键位测试（config defaults 断言）。
4. **删 `ncm fm` 子命令**：Commands::Fm / FmAction / match 臂 / 别名与文档引用全清；检查 shell complete 与测试。
   - 验收：cargo check；`ncm fm` 变未知命令。
5. **E2E 实测**：`cargo build` 后对比裸 GET 与 `mode=FAMILIAR` 返回曲目差异（真 cookie）。
6. **HELP-DESIGN.md**：参考 fqdt 写 ncm 自己的规范（顶层分组/文案硬约束/机制/checklist，贴合 ncm 现状）；顺带修 UI 问题 a（`-u` 挤行，缩短文案）。
7. **三绿**：fmt + clippy + test（基线 76，增改后核对）。
8. **中文 commit 直推 master**（不发版；0.4.0 发版另拍板）。

## 风险与边界

- `post_eapi` 为写操作通道（不重试）——radio/get 是读，但 fm_like/trash/skip 已用同通道，先例成立；若实测不稳再拆读通道。
- breaking 范围仅 `ncm fm` 子命令；`recommend -t fm`、`play --roam` 不受影响。
- 场景模式 SCENE_RCMD 的 subMode 取值未穷举——scene 透传 config，空则不传。
- UI 小项 b/c/d（process line / download toast / 存储目录显示）不在本计划，另行处理。

---

# 后续任务队列（PLAN 主任务已完成于 b423c09）

1. **toast 状态源改读 mpv IPC**（用户 m0120）：observe speed/volume/mute/fullscreen → property-change 统一 toast（`[]`/`{ }` 倍速、音量、全屏、静音变化都显示）；key_reader 音量拦截去掉自 toast 改由 observe 统一；x/v 等 mpv 原生键保持转发。进行中。
2. **CI 自动推二进制到远端**（用户 m0146）：「like fqdt repo, on actions auto push the binary to remote」——参照 fqdt `.github/workflows/build.yml` release job（ssh/scp → 47.95.13.125:48022，deploy key 走 GH secret），tag v* 时自动推 aarch64 二进制。待做。
3. **`ncm version` 子命令**：已完成（b423c09 后本批，输出 `ncm <版本>`，section 归 default）。
4. **UI b/c/d**：b 多处 process line/spinner 不显示；c 播放中按 d 改 status bar/toast 不用 ncm download 同款消息；d download 完成后显示存储目录。
5. **CONFIG-INI.md**：用户稍后更新，到时重看。
6. **0.4.0 发版拍板**：含 `ncm fm` 删除 breaking + FM 模式 + version 子命令；发版时走新 CI 自动部署。

---

# PLAN：对账 v2 重写（m0242）

现行为四症：①没错却静默换（URL 文本当标识误判）②错了报错不修（无法归类短路）
③错了不吭声（unclass 静默放行）④没错却报错（URL 刷新差异误报）。
根因 = 用「URL 文本全等 + 两指针 + Err 放行」对账；URL 每次刷新都变，非稳定标识。

## 新思路：count 守门 + 集合差修复（guard 是唯一结构写者）

1. **每轮只读 count**（已有 IPC）：`count == srcs_len` → 对齐，跳过全量（99% 轮零额外 IPC，
   顺带修性能 #11/#1）；结构操作（insert/remove/add）必然改 count，纯换序不会自发产生 →
   count 守门即结构守门（信任边界写注释）。
2. **count 不等才拉全量** → `playlist_set_diff`（multiset 集合差，O(n) HashMap，**总可判定无 Err**）：
   phantoms=files 多出、missing=expect 缺的；重复 URL 计数正确处理；
   顺序错位 count 相等 → 差为空 → 不修（宁不修不乱）。
3. **状态机简化**：删 unclass/unclass_toasted；desynced = 派生（count!=srcs_len 轮首置位，
   修复成功清零）+ refresh Desync 即时置位保留；repair_fails 计数——失败 ≤5 每次报，
   ≥6 静默继续试（保自愈不吵屏）。
4. **toast 只在状态迁移**：进入漂移报一次「检测到漂移修复中」、成功报「✓ 已修复(删x补y)」、
   失败报「× 修复失败(第n次)」。

## 步骤与验收

1. `playlist_set_diff` 纯函数替换 `classify_playlist_diff` → cargo check
2. 单测改写（3 个 classify 测试 → set_diff 版：尾部残影/缺项、换序不修、重复 URL multiset）→ cargo test
3. guard 对账块重写（fast 路径 + 集合差修复 + 迁移 toast + repair_fails）→ cargo check
4. 三绿 fmt/clippy/test
5. 行为探针：正常播放 60s 零误报；python 人为 playlist-remove 制造漂移 → 自动修复 + 明确 toast
6. 中文 commit（与 C1 同批发版）

---

# PLAN：运行日志 ncm.run.log（m0245，100-500 首测试定位问题用）

位置 `~/.config/ncm/ncm.run.log`；格式 `2026-11-24T12:34:56.789Z [INFO ] [tag] msg`（UTC 毫秒）。
事件驱动、无逐轮心跳；启动 >5MB 归档 `.1`；std Mutex<File> 无新 crate。

1. `src/log.rs`：`init()`（config 目录+归档+启动头）/`info/warn/error(tag,msg)` 纯包装
   → cargo check
2. 接线：toast（所有提示的单点，非终端分支也记）、mpv stderr 过滤行、guard 对账
   （漂移/修复动作 phantom+missing 摘要/复读验证失败详情/失败计数）、IPC Err、
   nav 关键事件（pos 切换）、mpv 启动参数
   → cargo check
3. 单测：行格式纯函数 → 三绿
4. 探针：跑一轮确认文件生成、事件可读
