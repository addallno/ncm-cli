//! 终端动画：颜色、表格、spinner、原地刷新状态行、进度条
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// 终端列宽；取不到（非 TTY/异常）回退 80
fn term_width() -> usize {
    terminal_size::terminal_size_of(std::io::stdout())
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80)
}

/// JSON 模式：静默全部人类可读输出与动画，保证 stdout 是纯 JSON
static JSON_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_json(v: bool) {
    JSON_MODE.store(v, std::sync::atomic::Ordering::Relaxed);
}

pub fn json_mode() -> bool {
    JSON_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// 静默模式（-q/--quiet）：隐藏进度动画与过程行，保留结果/错误
static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(v: bool) {
    QUIET.store(v, Ordering::Relaxed);
}

pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// 并发下载模式：多任务并行时转圈/行内进度会互相覆盖，整批关闭动画（结果行照常输出）
static CONCURRENT: AtomicBool = AtomicBool::new(false);

pub fn set_concurrent(v: bool) {
    CONCURRENT.store(v, Ordering::Relaxed);
}

pub fn concurrent() -> bool {
    CONCURRENT.load(Ordering::Relaxed)
}

/// 行内刷新模式（批量下载单任务）：step/进度原地覆盖不占滚屏，只固化结果行
static LIVE: AtomicBool = AtomicBool::new(false);

pub fn set_live(v: bool) {
    LIVE.store(v, Ordering::Relaxed)
}

fn live() -> bool {
    LIVE.load(Ordering::Relaxed) && is_tty() && !json_mode() && !quiet() && !concurrent()
}

/// 颜色开关（--no-color / NO_COLOR）
static COLOR: AtomicBool = AtomicBool::new(true);

pub fn set_color(v: bool) {
    COLOR.store(v, Ordering::Relaxed);
}

pub fn color_on() -> bool {
    COLOR.load(Ordering::Relaxed)
}

pub fn paint(text: &str, code: &str) -> String {
    if is_tty() && color_on() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn accent(t: &str) -> String {
    paint(t, "36")
}
pub fn good(t: &str) -> String {
    paint(t, "32")
}
pub fn bad(t: &str) -> String {
    paint(t, "31")
}
pub fn warn(t: &str) -> String {
    paint(t, "33")
}
pub fn dim(t: &str) -> String {
    paint(t, "2")
}

/// 淡蓝（256 色亮天蓝），用于译文行
pub fn soft_blue(t: &str) -> String {
    paint(t, "38;5;117")
}

/// 清掉当前行（防 spinner 转圈占行时后续输出黏在行尾）；TTY 下才写
fn clear_cur_line() {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return;
    }
    let mut out = std::io::stdout();
    let _ = write!(out, "\r\x1b[K");
    let _ = out.flush();
}

pub fn step(msg: &str) {
    if json_mode() || quiet() {
        return;
    }
    if live() {
        // 行内刷新：原地覆盖当前行不换行，结果行由 ok/skip/fail 固化
        let line = truncate_width(
            &format!("{} {}", accent("▸"), msg),
            term_width().saturating_sub(1),
        );
        let mut out = std::io::stdout();
        let _ = write!(out, "\r\x1b[K{line}");
        let _ = out.flush();
        return;
    }
    clear_cur_line();
    println!("{} {}", accent("▸"), msg);
}

/// 行内刷新模式下清掉动态行（固化结果行前调用）；非 live 模式空操作
pub fn live_clear() {
    if live() {
        clear_cur_line();
    }
}
pub fn ok(msg: &str) {
    clear_cur_line();
    if json_mode() {
        return;
    }
    println!("  {} {}", good("✓"), msg);
}
pub fn skip(msg: &str) {
    clear_cur_line();
    if json_mode() {
        return;
    }
    println!("  {} {}", warn("◌"), msg);
}
pub fn fail(msg: &str) {
    clear_cur_line();
    eprintln!("  {} {}", bad("×"), msg);
}
pub fn info(msg: &str) {
    clear_cur_line();
    if json_mode() || quiet() {
        return;
    }
    println!("  {}", dim(msg));
}

/// 交互确认；非 TTY 或 quiet/json 模式下直接返回 false（安全默认）
/// default_yes=true 时空输入按 y 处理（提示 [Y/n]），false 提示 [y/N]
pub fn confirm(msg: &str, default_yes: bool) -> bool {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || json_mode() || quiet() {
        return false;
    }
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("  {msg} {hint} ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return default_yes;
    }
    match line.trim().to_lowercase().as_str() {
        "" => default_yes,
        s => !s.starts_with('n'),
    }
}

/// 中文/全角按 2 列计算的显示宽度
pub fn display_width(text: &str) -> usize {
    // 剥离 ANSI 转义序列（\x1b[...m），带色单元格也能正确对齐
    let mut w = 0usize;
    let mut esc = false;
    for c in text.chars() {
        if esc {
            if c.is_ascii_alphabetic() {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        w += char_width(c);
    }
    w
}

fn char_width(c: char) -> usize {
    let cp = c as u32;
    // 常用 CJK / 全角区段（够终端表格用，不引入 unicode-width 依赖）
    let wide = (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0x303E).contains(&cp)
        || (0x3041..=0x33FF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xA000..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE30..=0xFE6F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x1F300..=0x1F9FF).contains(&cp);
    if wide {
        2
    } else {
        1
    }
}

/// 按显示宽度截断，超出补省略号；ANSI 转义原样保留且不计宽，截断处补重置防串色
pub fn truncate_width(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    let mut esc = false;
    let mut had_esc = false;
    let budget = max - 1; // 预留 1 列给省略号
    for c in text.chars() {
        if esc {
            out.push(c);
            if c.is_ascii_alphabetic() {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            had_esc = true;
            out.push(c);
            continue;
        }
        if w + char_width(c) > budget {
            out.push('…');
            if had_esc {
                out.push_str("\x1b[0m"); // 截断可能切掉原重置码，补一个
            }
            return out;
        }
        out.push(c);
        w += char_width(c);
    }
    out
}

fn pad(text: &str, width: usize) -> String {
    let w = display_width(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(w)))
}

/// 把列宽总预算压进 budget（每列至少 4、至少保留 1 列给省略号空间时的下限 4）
/// 返回是否发生了收缩。纯函数便于测试。
fn fit_widths(widths: &mut [usize], budget: usize) -> bool {
    const MIN: usize = 4;
    let total = |ws: &[usize]| -> usize { ws.iter().map(|w| w + 2).sum() };
    let mut shrunk = false;
    while total(widths) > budget {
        // 缩最宽的一列（无符号，存在性由循环条件保证）
        let Some(i) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, &w)| w)
            .map(|(i, _)| i)
        else {
            break;
        };
        if widths[i] <= MIN {
            break;
        }
        widths[i] -= 1;
        shrunk = true;
    }
    shrunk
}

/// 对齐表格（中文算 2 列）；列宽超出终端宽时按比例收缩 + 单元格截断
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() || json_mode() {
        return;
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(display_width(cell));
        }
    }
    // 终端感知：仅 TTY 收缩到屏宽内（管道/重定向保持全宽不截内容），单元格随之截断
    if is_tty() {
        fit_widths(&mut widths, term_width().saturating_sub(4)); // 留余量防自动换行
    }
    let header_line: String = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("  {}", pad(&truncate_width(h, widths[i]), widths[i])))
        .collect();
    println!("{}", dim(&header_line));
    let rule: String = widths.iter().map(|w| "─".repeat(w + 2)).collect();
    println!("{}", dim(&format!("  {rule}")));
    for row in rows {
        let line: String = row
            .iter()
            .enumerate()
            .map(|(i, cell)| format!("  {}", pad(&truncate_width(cell, widths[i]), widths[i])))
            .collect();
        println!("{line}");
    }
    println!("{}", dim(&format!("  {rule}")));
}

pub fn human_size(size: u64) -> String {
    if size >= 1 << 30 {
        format!("{:.2} GB", size as f64 / (1 << 30) as f64)
    } else if size >= 1 << 20 {
        format!("{:.1} MB", size as f64 / (1 << 20) as f64)
    } else if size >= 1024 {
        format!("{:.0} KB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}

pub fn render_bar(ratio: f64, width: usize) -> String {
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = (ratio * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// 单行原地刷新状态（TTY 用 \r+清行；非 TTY 分段打印）
pub struct Live {
    tty: bool,
    shown: bool,
}

impl Live {
    pub fn new() -> Self {
        Self {
            tty: is_tty(),
            shown: false,
        }
    }

    pub fn update(&mut self, text: &str, force: bool) {
        if json_mode() || quiet() || concurrent() {
            return;
        }
        if self.tty {
            // 按终端列宽截断：整行不换行，\r+\x1b[K 单行刷新才有效（窄屏不刷屏）
            let clipped = truncate_width(text, term_width().saturating_sub(1));
            let mut out = std::io::stdout();
            let _ = write!(out, "\r\x1b[K{clipped}");
            let _ = out.flush();
            self.shown = true;
        } else if force {
            println!("{text}");
        }
    }

    pub fn finish(&mut self) {
        if json_mode() || quiet() {
            self.shown = false;
            return;
        }
        if self.tty && self.shown {
            let mut out = std::io::stdout();
            let _ = write!(out, "\r\x1b[K");
            let _ = out.flush();
            self.shown = false;
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.finish();
    }
}

/// 进度条文本：`██░░ 45.7% · 14.6/30.5 MB · 4.1 MB/s`
/// bar 宽度随终端列宽自适应（窄屏缩 bar，优先保百分比/大小/速率）
pub fn progress_line(done: u64, total: u64, started: Instant) -> String {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let ratio = if total > 0 {
        done as f64 / total as f64
    } else {
        0.0
    };
    let speed = (done as f64 / elapsed) as u64;
    let mut parts = vec![format!("{:5.1}%", ratio * 100.0)];
    if total > 0 {
        parts.push(format!("{}/{}", human_size(done), human_size(total)));
    }
    parts.push(format!("{}/s", human_size(speed)));
    let tail = parts.join(" ");
    // 预算 = 终端宽 - 1(Live 截断预留) - 2(前导) - tail宽 - 1(空格)；bar 封顶 20
    let avail = term_width().saturating_sub(1 + 2 + display_width(&tail) + 1);
    if avail >= 4 {
        format!("  {} {}", render_bar(ratio, avail.min(20)), tail)
    } else {
        // 极窄屏：丢 bar 保数据，超宽由 Live 截断兜底
        format!("  {tail}")
    }
}

// gh 风格 braille 帧（GitHub gh CLI 加载 Codespace 时同款）
const SPINNER_FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

/// 转圈动画：进度条接管时调用 stop() 收行，两者不抢行
pub struct Spinner {
    label: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    tty: bool,
}

impl Spinner {
    pub fn start(label: impl Into<String>) -> Self {
        let mut s = Self {
            label: label.into(),
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
            tty: is_tty() && !json_mode() && !quiet() && !concurrent(),
        };
        if json_mode() || quiet() || concurrent() {
            return s;
        }
        if s.tty {
            let stop = Arc::clone(&s.stop);
            let label = s.label.clone();
            s.handle = Some(std::thread::spawn(move || {
                let mut i = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let frame = SPINNER_FRAMES[i % SPINNER_FRAMES.len()];
                    let painted = if color_on() {
                        format!("\x1b[36m{frame}\x1b[0m")
                    } else {
                        frame.to_string()
                    };
                    let mut out = std::io::stdout();
                    let line = format!("  {painted} {label} ");
                    let line = truncate_width(&line, term_width().saturating_sub(1));
                    let _ = write!(out, "\r\x1b[K{line}");
                    let _ = out.flush();
                    i += 1;
                    std::thread::sleep(std::time::Duration::from_millis(90));
                }
            }));
        } else {
            println!("  {} ", dim(&s.label));
        }
        s
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    /// 供进度回调共享的停止开关（不产生借用冲突）
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    pub fn shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
            let mut out = std::io::stdout();
            let _ = write!(out, "\r\x1b[K");
            let _ = out.flush();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_ansi_keeps_width_and_reset() {
        let colored = format!("{}{}", accent("你好世界，很长的中文标题"), "的尾巴");
        let out = truncate_width(&colored, 8);
        assert!(display_width(&out) <= 8, "截断后显示宽超限: {out:?}");
        assert!(
            out.ends_with("\x1b[0m") || !out.contains('\x1b'),
            "截断色串需补重置码"
        );
    }

    #[test]
    fn truncate_plain_no_change_within_limit() {
        assert_eq!(truncate_width("short", 10), "short");
        assert_eq!(truncate_width("x", 0), "");
    }

    #[test]
    fn fit_widths_shrinks_widest_to_budget() {
        let mut ws = vec![2usize, 60, 8];
        // 总宽 = Σ(w+2) = 4+62+10 = 76 → 压到 40
        fit_widths(&mut ws, 40);
        let total: usize = ws.iter().map(|w| w + 2).sum();
        assert!(total <= 40, "收缩后仍超预算: {ws:?}");
        assert_eq!(ws[0], 2, "窄列不应被改");
        assert!(ws[1] >= 4, "最宽列不应低于下限");
    }

    #[test]
    fn fit_widths_noop_when_fits() {
        let mut ws = vec![6usize, 10];
        assert!(!fit_widths(&mut ws, 100));
        assert_eq!(ws, vec![6, 10]);
    }

    #[test]
    fn progress_line_fits_narrow_terminal() {
        // 非 TTY 环境 term_width 回退 80；构造大文件进度文本
        let started = Instant::now();
        let line = progress_line(3 * (1 << 30), 4 * (1 << 30), started);
        let w = display_width(&line);
        assert!(w <= 80, "进度行超 80 列: {w} — {line:?}");
        assert!(line.contains('%'));
    }
}
