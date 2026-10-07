# 设计：search --type / recommend --type / 严格 subcommand（v0.3.13）

日期：2026-10-06
状态：待用户审阅

## 背景

- `ncm search` 固定 `type=1` 只搜单曲，无法搜歌单/专辑/歌手等（接口 `/api/search/get/web` 本身支持多种 type）。
- `ncm recommend` 是纯浏览命令且不接播放参数；其内容（每日推荐 + 榜单）语义混乱，默认铺榜单表格。
- `inject_dl_subcommand()` 把首个非子命令参数自动注入 `dl`，help 从未提及该行为，`ncm <生词>` 会悄悄变成"搜索并下载"，违背 subcommand 直觉。
- 命令正式名应为 `download`，`dl` 只应作为短别名存在。

## 需求（已与用户逐条确认）

1. `search` 支持 `-t/--type`，逗号分隔多值（如 `1,1000`），数字与英文别名可混用。
2. type **全放开**：不做白名单，任何 type 数值都允许搜索并列出。
3. `-t` 的 long help 中附完整 type 取值含义表。
4. 多 type 输出用**一张统一表格** + 「类型」列区分，不分节。
5. `-n/--limit` 全局归一化为**该命令输出的总条数**；多 type 时按类平分配额。
6. `recommend` 同样支持 `-t/--type`，值为推荐位**词枚举**，**默认 `daily`（只显示每日推荐歌曲，不再默认铺榜单）**。
7. 删除自动注入，严格 subcommand 语义：未知首参由 clap 报 `unrecognized subcommand`。
8. `download` 为正式命令名，`dl` 保留为别名。
9. `recommend --type` 值域包含 `fm`（私人FM）与 `radar`（私人雷达）。

## 1. `search --type` 设计

### 语法

```
ncm search -t 1,1000 -n 10 周杰伦
ncm search -t song,playlist -n 10 周杰伦     # 英文别名
ncm search -t 1,song,album 周杰伦             # 数字与别名混用
```

### type 取值

| 数字 | 别名 | 含义 | 备注 |
|------|------|------|------|
| 1 | `song` | 单曲 | 默认 |
| 10 | `album` | 专辑 | |
| 100 | `artist` | 歌手 | |
| 1000 | `playlist` | 歌单 | |
| 1002 | `user` | 用户 | 仅列出 ID |
| 1004 | `mv` | MV | 仅列出 ID |
| 1006 | `lyric` | 歌词 | 仅列出 |
| 1009 | `radio` | 主播电台 | 仅列出 ID |
| 1014 | `video` | 视频 | 仅列出 ID |
| 1018 | `cloud` | 综合 | limit 无效（服务端行为），help 中注明 |
| 2000 | `sound` | 声音 | 仅列出 |

- 别名解析表实现为 `const` 映射（大小写不敏感），未识别的英文别名 → clap value_parser 报错并列出全部别名。
- 未收录的数字 type 直接透传（全放开）；其结果显示走通用降级提取（见 §3）。
- help（`-t` 的 `long_help`）附上表，`search --help` 里能直接看到。

### 请求与配额

- 每个 type 发一次 `/api/search/get/web` 请求，`limit` = 该类配额。
- 配额平分总 `-n`：`quota_i = n/k + (i < n%k ? 1 : 0)`（余数依次给前几类）。
- 某类结果不足配额时**不做二次补足**，缺口留空（YAGNI）。
- 按 `--type` 输入顺序分组排列；组内保持服务端返回顺序（单曲组继续走现有 `rank_songs` 相关度排序）。
- 不给 `-t` 时行为完全不变：`type=1`、现有输出。

## 2. `recommend --type` 设计

### 语法

```
ncm recommend                        # 等效 -t daily：只列每日推荐歌曲
ncm recommend -t daily,toplist -n 10
ncm recommend -t fm,radar
```

### 值域（词枚举，逗号分隔）

| 值 | 数据源 | 说明 |
|----|--------|------|
| `daily` | `daily_songs()` | 每日推荐歌曲，需登录；**默认** |
| `toplist` | `toplist()` | 官方榜单 |
| `fm` | `radio_songs()` | 私人FM 首批曲目 |
| `radar` | 见下 | 私人雷达歌单 |

- 未识别的值 → clap 报错并列出四个值。
- 默认值改为 `daily`：**不再默认列榜单**（行为变化，README 同步）。
- `-n` 同样平分配额（见 §1），统一表格输出（榜单行：名称=榜单名、艺术家列=更新频率、时长曲数列=曲数、类型=榜单；fm/radar/daily 行均为单曲行）。

### radar 数据源

1. 调 `/api/v1/discovery/recommend/resource`（每日推荐歌单列表，需登录；与已有 `/api/v1/discovery/recommend/songs` 同族，明文路径在实现时验证）。
2. 筛选 `name` 含「私人雷达」的歌单，取其 `id` 后按普通歌单解析曲目（复用 `resolve_list`）。
3. 接口失败或未命中 → 明确报错：`未找到私人雷达歌单（可 ncm list <私人雷达链接> 手动浏览）`。
   - 实现时若验证该接口不可用，则 radar 项报同一错误提示（不引入新接口调研，兜底路径明确）。

## 3. 统一表格规范

表头：`# / 名称 / 艺术家·创建者 / 时长·曲数 / 类型 / ID`

各类型行的字段提取：

| 类型 | 名称 | 艺术家·创建者 | 时长·曲数 | 类型列文案 |
|------|------|---------------|-----------|------------|
| 单曲(1) | `name` | `ar[0].name` | `dt`→`hms` | `单曲` |
| 专辑(10) | `name` | `artists[0].name` | `size` 首 | `专辑` |
| 歌手(100) | `name` | —（留空） | `musicSize` 首 | `歌手` |
| 歌单(1000) | `name` | `creator.nickname` | `trackCount` 首 | `歌单` |
| 用户(1002) | `nickname` | — | — | `用户` |
| MV(1004) | `name` | `artists[0].name` | `duration`→`hms` | `MV` |
| 歌词(1006) | 所属曲 `name` | `ar[0].name` | — | `歌词` |
| 电台(1009) | `name` | `dj.nickname` | — | `电台` |
| 视频(1014) | `name` | `creator[0].userName` | `duration`→`hms` | `视频` |
| 综合(1018) | 按其子类降级提取 | 同左 | 同左 | 取子类 |
| 声音(2000) | `name` | `creator.nickname` | — | `声音` |
| 榜单 | `name` | `updateFrequency` | `trackCount` 首 | `榜单` |

- 字段缺失一律显示 `-`，不 panic。
- **通用降级**：未收录的 type（全放开允许），先按 `result` 内第一个数组取，逐行尝试 `id`/`name`/常见副信息键，取到什么显示什么，类型列显示 `类型N`（N=数字）。
- JSON 模式：只指定一个 type（或默认）时输出数组（与现状同构）；指定多个 type 时输出对象 `{ "<type>": [...], ... }`，键为 type 数字字符串，按字符串字典序输出（serde_json Map 默认排序，非输入序，实测为准）。

## 4. 严格 subcommand

- 删除 `inject_dl_subcommand()` 与 `SUBCOMMANDS` 常量，`main` 恢复 `Cli::parse()`。
- 未知首参 → clap 默认错误：`unrecognized subcommand 'xxx'` + 候选列表 + `Usage`。
- 无参数 → 保持现状（打印 help，`run()` 的 `None` 分支不动）。
- `ncm 259138`、`ncm <链接>` 直下快捷方式**移除**：下载一律 `ncm download ...`。
- 删除/修正相关测试（含遍历子命令、断言注入行为的用例）；README/Description 中快捷方式描述删除。

## 5. `download` 别名

- `Commands::Dl` 重命名为 `Commands::Download`，clap 属性 `#[command(name = "download", visible_alias = "dl")]`。
- `ncm dl ...` 继续可用；`--help` 实测渲染 `download … [alias: dl]`（clap 别名展示格式）。
- 所有 `Commands::Dl` 引用、`section_of`、文档示例同步：文档主用 `download`，并注明别名 `dl`。
- `dl` 内部对非数字目标的搜索下载流程**保留**（`ncm download 周杰伦` 仍然工作——它是显式 download 命令，不是隐式注入）。

## 6. `-n` 归一化

- 全命令统一语义：**输出总条数**（见 §1 配额公式）。
- `daily`/`list`/`download`/`play` 现有语义本就是"最多 N 首"，无需改动，仅文档表述统一。
- README 增加一节说明该语义。

## 7. 错误处理

- type 解析失败（未知别名/未知推荐位值/空串）→ clap value_parser 错误，附全部合法值。
- 各类请求失败：单 type 失败则整体失败（不静默吞）；全部失败 → bail 原因；部分成功场景不出现（每类都是整体请求）。
- `daily` 未登录 → 沿用现有提示文案；`radar` 见 §2 兜底文案。
- 表格字段缺失 → `-`。

## 8. 测试计划

单元测试（`cargo test` 内）：

1. 别名解析：`song`→1、`PLAYLIST`→1000、`1,1000` 混用、未知别名报错。
2. 配额计算：`n=10,k=2` → (5,5)；`n=10,k=3` → (4,3,3)；`n=1,k=2` → (1,0)。
3. 统一行提取：单曲/歌单/榜单字段映射与缺失降级 `-`。
4. subcommand：`ncm 259138` 解析失败（unrecognized subcommand）；`ncm dl x` 与 `ncm download x` 均解析成功且为同一 variant。
5. `recommend -t` 解析：默认 `daily`、`fm,radar` 合法、未知值报错。
6. 通用降级提取对未知 type 的健壮性（任意 JSON 行不 panic）。

手测（远端/本地）：

- `ncm search -t song,playlist -n 10 周杰伦` 表格分组与类型列。
- `ncm recommend -t fm`、`-t toplist`、默认（仅每日推荐）。
- `ncm 123` 报 unrecognized subcommand；`ncm help search` 可见 type 取值表。
- `ncm dl x` / `ncm download x` 均可用。

## 9. 非目标

- 不做 MV/视频/电台的播放与下载消费。
- 不做交互式选中（列表后输序号进入歌单）。
- 不做 type=1018 的分页控制（服务端 limit 无效，help 注明即可）。
- 不改 `play`/`download` 对搜索结果的消费链路。

## 10. 交付

- 版本 v0.3.13；`cargo clippy -- -D warnings && cargo fmt --check && cargo test` 全绿。
- README.md、Description.md 同步（type 表、默认行为变化、严格 subcommand、download 别名）。
- commit → push CI → tag v0.3.13 → Release aarch64 → 远端部署验证（照 v0.3.12 流程）。
