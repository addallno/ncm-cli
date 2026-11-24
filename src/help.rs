//! 手写 help 渲染（借鉴 fqdt 分层：顶层分组平铺 + 子命令两列对齐，段标题 Subcommands）
//!
//! 拦截链（main() 开头，Cli::parse 之前）：
//!   top_help_requested → render_top_help（裸调用 / --help / help）
//!   sub_help_requested → render_sub_help（<cmd> --help / help <cmd>）
//!   其余交 clap（带值全局 flag 后跟 --help 等边角场景）

use crate::Cli;
use clap::{Arg, ArgAction, Command, CommandFactory};
use std::io::Write;

const TITLE: &str = "\x1b[1;4m"; // 加粗 + 下划线（与 ui 颜色开关联动）
const RST: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

fn t(s: &str) -> String {
    if crate::ui::color_on() && crate::ui::is_tty() {
        format!("{TITLE}{s}{RST}")
    } else {
        s.to_string()
    }
}

fn dim(s: &str) -> String {
    if crate::ui::color_on() && crate::ui::is_tty() {
        format!("{DIM}{s}{RST}")
    } else {
        s.to_string()
    }
}

fn emit(line: &str) {
    // writeln + let _：管道对端提前关闭时静默退出，不 SIGPIPE panic（`ncm --help | head`）
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// 顶层分组：(组名, 子命令)。组名不打印，只决定平铺顺序（HELP-DESIGN 规则 9）。
const TOP_GROUPS: &[(&str, &[&str])] = &[
    ("播放", &["play"]),
    ("查询", &["search", "info", "recommend", "daily"]),
    ("下载", &["download"]),
    (
        "管理",
        &["playlist", "like", "login", "whoami", "sign", "config"],
    ),
    ("其他", &["completions", "version", "help"]),
];

/// 顶层选项段（手写；子命令摘要从 Cli::command() 内省，不双份维护）。
const TOP_OPTIONS: &[(&str, &str)] = &[
    ("      --no-color", "禁用彩色输出，NO_COLOR 同效"),
    ("      --session <FILE>", "pyncm 会话文件，NCM_SESSION 同效"),
    (
        "      --cookie <STR>",
        "登录 Cookie 字符串，NCM_COOKIE 同效",
    ),
    ("      --mirror <URL>", "解锁音源服务地址，none 禁用"),
    ("  -u, --user <NAME>", "指定 profile 名"),
    ("  -v, --verbose", "详细日志输出到终端"),
    ("  -h, --help", "查看帮助"),
    ("  -V, --version", "显示版本"),
];

/// 顶层 help 请求判定：裸调用 / 单独 help / 首个 --help|-h 前无所见子命令。
/// `ncm play --help`、`ncm help play` 走 sub_help_requested。
fn top_help_requested(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }
    if args.len() == 1 && args[0] == "help" {
        return true;
    }
    let mut saw_sub = false;
    for a in args {
        if a == "--help" || a == "-h" {
            return !saw_sub;
        }
        if !a.starts_with('-') {
            saw_sub = true;
        }
    }
    false
}

/// 子命令 help 路径判定：`help <路径>` 或 `<路径...> --help|-h`。
fn sub_help_requested(args: &[String]) -> Option<Vec<String>> {
    let toks: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let path: &[&str] = if toks.first() == Some(&"help") {
        &toks[1..]
    } else {
        match toks.last() {
            Some(&"--help") | Some(&"-h") => &toks[..toks.len() - 1],
            _ => return None,
        }
    };
    if path.is_empty() {
        return None;
    }
    let mut cur = Cli::command();
    for t in path {
        let next = cur.find_subcommand(t)?.clone();
        cur = next;
    }
    Some(path.iter().map(|s| s.to_string()).collect())
}

/// flag 右列说明：help 首行 + `[default: ...]` 后缀；-h/-V 中文文案。
fn flag_help_text(a: &Arg) -> String {
    match a.get_long() {
        Some("help") => return "查看帮助".to_string(),
        Some("version") => return "显示版本".to_string(),
        _ => {}
    }
    let help = a
        .get_help()
        .or_else(|| a.get_long_help())
        .map(|h| h.to_string())
        .unwrap_or_default();
    let first = help.lines().next().unwrap_or("").trim().to_string();
    match a.get_default_values().first() {
        Some(d) if !first.is_empty() => format!("{first} [default: {}]", d.to_string_lossy()),
        Some(d) => format!("[default: {}]", d.to_string_lossy()),
        None => first,
    }
}

/// flag 左列签名：`-r, --range <RANGE> `（尾随单空格）；无短名用 6 空格对齐。
fn arg_left(a: &Arg) -> String {
    let mut left = String::new();
    match (a.get_short(), a.get_long()) {
        (Some(s), Some(l)) => left.push_str(&format!("  -{}, --{} ", s, l)),
        (None, Some(l)) => left.push_str(&format!("      --{} ", l)),
        (Some(s), None) => left.push_str(&format!("  -{} ", s)),
        (None, None) => left.push_str("  "),
    }
    let takes_val = !matches!(
        a.get_action(),
        ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Count
    );
    if takes_val {
        if let Some(v) = a.get_value_names().and_then(|v| v.first()) {
            left.push_str(&format!("<{}> ", v));
        }
    }
    left
}

/// 两列对齐输出行集：按显示宽度（中文 2 列）取最大左列 + 2 为说明列。
fn emit_rows(rows: &[(String, String)]) {
    if rows.is_empty() {
        return;
    }
    let width = rows
        .iter()
        .map(|(l, _)| crate::ui::display_width(l))
        .max()
        .unwrap_or(0)
        + 2;
    for (left, help) in rows {
        let pad = " ".repeat(width.saturating_sub(crate::ui::display_width(left)));
        emit(&format!("{left}{pad}{help}"));
    }
}

/// Options 段：本命令 flags + 顶层 global flags，补 -h/-V。
fn render_options(cmd: &Command) {
    let mut rows: Vec<(String, String)> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set())
        .filter(|a| !matches!(a.get_action(), ArgAction::Help | ArgAction::Version))
        .map(|a| (arg_left(a), flag_help_text(a)))
        .collect();
    // 顶层 global flags（clap 子命令 get_arguments 不含父级，手动补）
    for a in Cli::command().get_arguments() {
        let Some(long) = a.get_long() else { continue };
        if !a.is_global_set() {
            continue;
        }
        if rows.iter().any(|(l, _)| l.contains(&format!("--{long}"))) {
            continue;
        }
        rows.push((arg_left(a), flag_help_text(a)));
    }
    if rows.iter().all(|(l, _)| !l.contains("--help")) {
        rows.push(("  -h, --help ".to_string(), "查看帮助".to_string()));
    }
    emit(&t("Options:"));
    emit_rows(&rows);
}

/// Arguments 段：位置参数两列对齐。
fn render_pos_args(cmd: &Command) {
    let rows: Vec<(String, String)> = cmd
        .get_positionals()
        .filter(|a| !a.is_hide_set())
        .map(|a| {
            let name = a
                .get_value_names()
                .and_then(|v| v.first())
                .map(|s| s.to_string())
                .unwrap_or_else(|| a.get_id().to_string());
            (format!("  <{name}> "), flag_help_text(a))
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    emit(&t("Arguments:"));
    emit_rows(&rows);
}

/// Subcommands 段：子命令名 + 一句话目的（跳过 clap 自动 help 子命令）。
fn render_commands(cmd: &Command) {
    let subs: Vec<&Command> = cmd
        .get_subcommands()
        .filter(|s| s.get_name() != "help" && !s.is_hide_set())
        .collect();
    if subs.is_empty() {
        return;
    }
    emit(&t("Subcommands:"));
    let width = subs
        .iter()
        .map(|s| s.get_name().chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    for s in subs {
        let about = s.get_about().map(|a| a.to_string()).unwrap_or_default();
        let first = about.lines().next().unwrap_or("").trim();
        emit(&format!("  {:<width$}{}", s.get_name(), first));
    }
}

/// 格式行：取 clap usage，去掉 `Usage: ` 前缀并补全父命令路径。
fn usage_of(cmd: &Command, full: &str) -> String {
    let u = cmd.clone().render_usage().to_string();
    let u = u
        .lines()
        .find(|l| l.contains("Usage:"))
        .map(|l| l.trim_start().trim_start_matches("Usage: ").to_string())
        .unwrap_or_else(|| format!("ncm {}", cmd.get_name()));
    let u = if u.starts_with("ncm ") {
        u
    } else {
        format!("ncm {u}")
    };
    let name = format!("ncm {} ", cmd.get_name());
    match u.strip_prefix(&name) {
        Some(rest) => format!("ncm {full} {rest}"),
        None => u,
    }
}

/// 自定义子命令 help：about / Usage / Arguments / Subcommands / Options。
fn render_sub_help(path: &[String]) {
    let mut cmd = Cli::command();
    for p in path {
        cmd = cmd.find_subcommand(p).expect("path validated").clone();
    }
    let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
    emit(about.lines().next().unwrap_or("").trim());
    emit("");
    let usage = usage_of(&cmd, &path.join(" "));
    emit(&format!("{} {usage}", t("Usage:")));
    emit("");
    let pos: Vec<&Arg> = cmd.get_positionals().filter(|a| !a.is_hide_set()).collect();
    if !pos.is_empty() {
        render_pos_args(&cmd);
        emit("");
    }
    if cmd.has_subcommands() {
        render_commands(&cmd);
        emit("");
    }
    render_options(&cmd);
}

/// 手写顶层 help：子命令按分组顺序平铺（不打印组名）+ Options。
fn render_top_help() {
    let cmd = Cli::command();
    emit(&format!(
        "{} — 网易云音乐搜索/下载/歌词工具",
        crate::ui::accent("ncm")
    ));
    emit("");
    emit(&dim("  用法: ncm [选项] <命令>"));
    emit("");
    emit(&t("Subcommands:"));
    for (_, subs) in TOP_GROUPS {
        for s in *subs {
            emit(&format!("  {:<12} {}", s, sub_about(&cmd, s)));
        }
    }
    emit("");
    emit(&t("Options:"));
    let rows: Vec<(String, String)> = TOP_OPTIONS
        .iter()
        .map(|(f, a)| (format!("  {f} "), a.to_string()))
        .collect();
    emit_rows(&rows);
}

/// 子命令 about 内省（取首行）；clap 自动 help 子命令 build 前取不到，兜底中文。
fn sub_about(cmd: &Command, name: &str) -> String {
    let about = cmd
        .find_subcommand(name)
        .and_then(|c| c.get_about())
        .map(|a| a.to_string())
        .unwrap_or_default();
    let first = about.lines().next().unwrap_or("").to_string();
    if first.is_empty() && name == "help" {
        "查看帮助".into()
    } else {
        first
    }
}


/// main() 开头的 help 拦截；命中则渲染并返回 true。
pub(crate) fn try_help_intercept() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if top_help_requested(&args) {
        render_top_help();
        return true;
    }
    if let Some(path) = sub_help_requested(&args) {
        render_sub_help(&path);
        return true;
    }
    false
}
