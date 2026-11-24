# ncm help 设计规范

> 母版：`~/openwork/fqdt/HELP-DESIGN.md`（243 行，2026-11-24 更新）——四层结构、去括号、
> 不教 help、不列配置键、动词化摘要 ≤20 字、flag help ≤1 行、占位符小写等硬约束全部以母版为准。
> 输出哲学：`fqdt/TUI.md`「人看的信息要少而准，机器用的信息要纯而稳，每一行都挣得位置」。
> 本文件只记录 ncm 侧的实现差异与现状（旧裁剪版已废弃，m0116）。

## 实现（src/help.rs，手写两层）

| 层 | 载体 | 说明 |
|---|---|---|
| 顶层 help | `render_top_help` + `TOP_GROUPS`/`TOP_OPTIONS` | 标题 + 用法行 + 分组平铺（**组名不打印**，只定顺序）+ Options；**无优先级尾行**（m0125 拍板不加） |
| 子命令 help | `render_sub_help`（about/Usage/Arguments/Subcommands/Options 两列对齐） | 内省 clap doc；global flags 自动追加在专属 flags 之后 |
| clap 兜底 | `Cli::parse` 原生 | 带值全局 flag 后跟 `--help` 等边角场景（样式为 clap 默认） |

- 拦截链：`main()` 开头 `help::try_help_intercept()` 先于 `Cli::parse()`；
  命中 `裸调用 / --help / help / <cmd> --help / help <cmd>` 即渲染退出。
- **段标题用 `Subcommands`**（m0120 拍板：no `Sub commands`——与 fqdt 母版的 `Sub commands:` 不同，
  若统一需改 fqdt 侧）。
- 对齐：`ui::display_width`（中文 2 列）；管道 EPIPE 静默（writeln + let _，不 SIGPIPE panic）；
  颜色仅 `color_on && is_tty`（管道零转义）。
- **无** FLAG_HELP / HELP_TAILS / 示例段 / help.txt（未引入；需要时按母版加）。

## 分组（TOP_GROUPS，组内顺序 = 平铺顺序）

```
播放      play
查询      search, info, recommend, daily
下载      download
管理      playlist, like, login, whoami, sign, config
其他      completions, version, help
```

合并候选（C14，等拍板）：`daily` ≡ `recommend -t daily`（删）；`whoami` → `login` 无参（删）。

## -v / verbose（m0116、m0121）

- 默认：日志只落盘 `~/.config/ncm/ncm.run.log`，正常输出零日志行、**不打印日志产出位置**。
- `-v`：`log::set_console(true)` → 每行 stderr 镜像 + init 打印 `日志 → <路径>`。

## checklist（改 help 时过一遍）

- [ ] 变体 doc 单行 ≤20 字动词化、无括号（多段注释触发 long_about 破坏紧凑布局）
- [ ] flag help ≤1 行、无括号；`grep -n '///.*（' src/main.rs` 中 clap 消费项为零
- [ ] 段标题 `Subcommands`；别名不进列表（手写渲染天然满足）
- [ ] `TOP_GROUPS`/`TOP_OPTIONS` 与新命令、新 global flag 同步
- [ ] 冒烟五路：裸调用 / `--help` / `help <cmd>` / `<cmd> --help` / `version`（不被拦截）
- [ ] 管道 `ncm --help | head -3` 无 panic、`grep -c $'\x1b'` 为 0
- [ ] `-v` 见日志与位置；无 `-v` 一行日志不见（TUI.md 检查清单 75）
