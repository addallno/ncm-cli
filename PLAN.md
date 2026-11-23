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
