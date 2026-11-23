# ncm help 设计与重写规范

> 面向修改 ncm CLI 帮助文案的 agent。改任何 help/输出前必读。
> 依据：`~/openwork/fqdt/HELP-DESIGN.md`（母版，v0.26.0 五层结构）+ clig.dev / Vercel CLI Guidelines / bettercli.org，
> 按 ncm 现状裁剪——本文件是 ncm 的唯一规范，与 fqdt 有差异处以本文件为准。

---

## 1. 结构（现状两层，按需增层）

| 层 | 触发 | 载体 | 状态 |
|---|---|---|---|
| 顶层 help | `ncm` / `ncm --help` / `ncm -h` | clap derive（`Cli` 的 `about` + 全局 `Args` doc） | ✅ 现状 |
| 子命令 help | `ncm <cmd> --help` | clap derive 变体/flag doc + `help_heading` 分组 | ✅ 现状 |
| flag 详情 | `ncm <cmd> <flag> --help` | 未做 | ⬜ 需要时再立 `FLAG_HELP` 表 |
| config 键详情 | `ncm config get <key>` | `config.rs` doc 注释 + `config list` 展示 | ✅ 现状（config 是 JSON 非 ini） |

**分层原则**（同 fqdt）：顶层只教「有哪些命令、干什么」；子命令层教「这个命令怎么用」；每层不越界——顶层不列 flag 详情，子命令 help 不教 help、不列配置键。

### 1.1 顶层

- `about` 一行 = 标题，动词化；`long_about = None`（防多段 doc 触发两行长格式）。
- 子命令摘要 = 变体 doc 注释**单行**（多段会触发 clap `long_about`，`--help` 落入两行式破坏紧凑）。
- **暂不做**手写 `render_top_help`/`TOP_GROUPS`——ncm 子命令 ≤15 个，clap 平铺可读；子命令多到需要分组时再立项（届时照 fqdt §1.1：`display_order` 排序、摘要内省不双份维护、选项段手写）。

### 1.2 子命令 help（clap + help_heading）

- 变体 doc = 摘要行，写**动词化一句话**（"搜索并列出表格"、"播放歌曲"），不写功能清单；**只留一行**。
- flag 的 `help` 属性：短、无括号、写取值语义；**≤1 行**。
- flag 用 `help_heading` 分组（先例：`队列` / `播放器` / `输出与保存`）——这是 ncm 的分组载体，等价 fqdt 的 Options 紧凑布局。
- 从属 flag 用多行 help 缩进表达从属关系，声明加 `hide = true` 保解析（先例：`--tone` 属 `--ai`）。
- 顶部 `-u/--user` 这类**全局长文案必须压到一行内**（v0.3.53 修过挤行：两行 help 会与相邻 flag 挤在同一行渲染）。

---

## 2. 文案规则（硬约束）

1. **去括号**——help 里禁止括号及括号内注释；补充信息改换行缩进或独立行。代码注释、运行输出不受此限。
2. **不教 help**——不写 `用 ncm xxx --help 查看` 类尾行。
3. **不列配置键**——help 不写 `配置: config.json [x] y`；配置说明唯一来源 = `config list` / `config get` + `config.rs` doc 注释。
4. **不打迁移提示**——命令移除后直接未知命令 err，不打"已移除/请改用 xxx"（先例：`ncm fm` 删除即走 clap `unrecognized subcommand`）。
5. **动词化、短**——摘要行 ≤20 字；flag help ≤1 行；占位符用 `<ID>` / `<FILE>` 形式（clap 惯例）。
6. **示例真实可跑**——改子命令/flag 当次同步 README/Description 示例；不用 `[options]` 占位。
7. **简体中文**，错误前缀 `  err `、成功 `  ok `，不用 emoji；clap 自带英文（`Print this message...`）暂保留，中文化需包装 `after_help`/自渲染时一并做。
8. **一行一记录**——help 与运行输出同守：进度走 stderr、结果单行、指引仅未登录保留单行。
9. **按键/键位文案进 help 时单行**——播放键位说明（d/l/u/t 等）保持一行，长了进 `?` 帮助（运行时帮助与 help 文案是两套，后者可长）。

---

## 3. 机制（现状与落点）

### 3.1 现状

- 全部 help = clap derive doc 注释，`help_heading` 分组；无 `HELP_TAILS`、无 `FLAG_HELP`、无手写顶层。
- 配置键清单 = `config.rs::all_keys()`（`config list` 展示默认值）；键校验 = `validate_key` + `set` 内值校验（新增键三处同步：白名单、校验、all_keys + 测试）。

### 3.2 新增子命令/flag 的 checklist

- [ ] 变体 doc = **单行**动词化摘要
- [ ] flag help ≤1 行、无括号、取值语义明确
- [ ] 会分组的 flag 设 `help_heading`（复用已有组名）
- [ ] 会取值的全局 flag 检查是否挤行（`ncm --help` 目测两列对齐）
- [ ] 新配置键 → `validate_key` + `set` 校验 + `all_keys()` 默认值 + 测试四处同步
- [ ] README.md / Description.md 命令表与示例同步
- [ ] 移除命令不留迁移提示；`completions` 重新生成
- [ ] 冒烟：`ncm --help`、`ncm <cmd> --help`、`ncm <cmd> <flag> --help`、`ncm help <cmd>` 四路
- [ ] 三绿：`cargo fmt` + `cargo clippy --all-targets` + `cargo test`

### 3.3 未来增层的触发条件

| 何时 | 立什么 | 照抄 fqdt |
|---|---|---|
| 子命令 >20 个 | 手写顶层 `render_top_help` + `TOP_GROUPS` | fqdt §1.1 |
| `ncm <cmd> <flag> --help` 退化为粗 help 的投诉 | `FLAG_HELP` 表 + `try_flag_help` 拦截链 | fqdt §1.3 / §3.2 |
| 需要解释性长参考（API 请求格式等） | `help.txt` 自动生成 | fqdt §3.1.1 |

---

## 4. 与 fqdt 的差异（有意为之）

| 项 | fqdt | ncm | 原因 |
|---|---|---|---|
| 配置载体 | `config.ini` 分节模板 + `CONFIG_HELP` | `config.json` + `all_keys()`/`config list` | ncm 是 JSON 键值，无节/注释模板 |
| 顶层 help | 手写 `render_top_help` 分组 | clap 平铺 | 子命令少，未到分组阈值 |
| help.txt | 自动生成 | 无 | 暂无需求，触发条件见 3.3 |
| examples 开关 | `ui.help_examples` 默认关 | 无示例段 | ncm 示例集中在 README，help 不放示例段 |
| help 语言 | 中文（clap 英文标题保留） | 同 | clap 自带文案未中文化，同守 |

---

## 5. 优秀 CLI help 要点（调研摘录，同 fqdt）

- **clig.dev**：`-h`/`--help` 全深度可用；lead with examples；常用排前；stdout=数据、stderr=消息。
- **Vercel**：help 走 stdout、exit 0、离线可用；叶子 help 是参考手册不是教程；未知命令点名未知段。
- **bettercli**：help 三问——这是什么 / 从哪开始 / 怎么深入。
