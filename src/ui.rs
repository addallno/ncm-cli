//! 终端动画：颜色、表格、spinner、原地刷新状态行、进度条
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub fn is_tty() -> bool {
    std::io::stdout().is_terminal()
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

pub fn step(msg: &str) {
    if json_mode() || quiet() {
        return;
    }
    println!("{} {}", accent("▸"), msg);
}
pub fn ok(msg: &str) {
    if json_mode() {
        return;
    }
    println!("  {} {}", good("✔"), msg);
}
pub fn skip(msg: &str) {
    if json_mode() {
        return;
    }
    println!("  {} {}", warn("◌"), msg);
}
pub fn fail(msg: &str) {
    eprintln!("  {} {}", bad("✘"), msg);
}
pub fn info(msg: &str) {
    if json_mode() || quiet() {
        return;
    }
    println!("  {}", dim(msg));
}

/// 中文/全角按 2 列计算的显示宽度
pub fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
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

/// 按显示宽度截断，超出补省略号
pub fn truncate_width(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in text.chars() {
        let cw = char_width(c);
        if w + cw > max - 1 {
            out.push('…');
            return out;
        }
        out.push(c);
        w += cw;
    }
    out
}

fn pad(text: &str, width: usize) -> String {
    let w = display_width(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(w)))
}

/// 对齐表格（中文算 2 列）
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
    let header_line: String = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("  {}", pad(h, widths[i])))
        .collect();
    println!("{}", dim(&header_line));
    let rule: String = widths.iter().map(|w| "─".repeat(w + 2)).collect();
    println!("{}", dim(&format!("  {rule}")));
    for row in rows {
        let line: String = row
            .iter()
            .enumerate()
            .map(|(i, cell)| format!("  {}", pad(cell, widths[i])))
            .collect();
        println!("{line}");
    }
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
        if json_mode() || quiet() {
            return;
        }
        if self.tty {
            let mut out = std::io::stdout();
            let _ = write!(out, "\r\x1b[K{text}");
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
pub fn progress_line(done: u64, total: u64, started: Instant) -> String {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let ratio = if total > 0 {
        done as f64 / total as f64
    } else {
        0.0
    };
    let speed = (done as f64 / elapsed) as u64;
    let mut parts = vec![render_bar(ratio, 20), format!("{:5.1}%", ratio * 100.0)];
    if total > 0 {
        parts.push(format!("{}/{}", human_size(done), human_size(total)));
    }
    parts.push(format!("{}/s", human_size(speed)));
    format!("  {}", parts.join(" "))
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
            tty: is_tty() && !json_mode() && !quiet(),
        };
        if json_mode() || quiet() {
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
                    let _ = write!(out, "\r\x1b[K  {painted} {label}");
                    let _ = out.flush();
                    i += 1;
                    std::thread::sleep(std::time::Duration::from_millis(90));
                }
            }));
        } else {
            println!("  {}", dim(&s.label));
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
