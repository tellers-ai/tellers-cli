use console::{style, Emoji};

static INFO_ICON: Emoji<'_, '_> = Emoji("ℹ ", "");
static WARNING_ICON: Emoji<'_, '_> = Emoji("⚠ ", "");
static ERROR_ICON: Emoji<'_, '_> = Emoji("✗ ", "");
static SUCCESS_ICON: Emoji<'_, '_> = Emoji("✓ ", "");
static ARROW: Emoji<'_, '_> = Emoji("→ ", "");

const MAX_LINE_WIDTH: usize = 120;

fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    format!("{}...", &s[..max_len.saturating_sub(3)])
}

fn format_message(msg: impl std::fmt::Display) -> String {
    let msg_str = msg.to_string();
    truncate_with_ellipsis(&msg_str, MAX_LINE_WIDTH)
}

pub fn info(msg: impl std::fmt::Display) {
    println!("{} {}", style(INFO_ICON).cyan(), style(format_message(msg)).white());
}

pub fn warning(msg: impl std::fmt::Display) {
    println!("{} {}", style(WARNING_ICON).yellow(), style(format_message(msg)).yellow());
}

pub fn error(msg: impl std::fmt::Display) {
    eprintln!("{} {}", style(ERROR_ICON).red(), style(format_message(msg)).red().bold());
}

pub fn success(msg: impl std::fmt::Display) {
    println!("{} {}", style(SUCCESS_ICON).green(), style(format_message(msg)).green().bold());
}

pub fn step(msg: impl std::fmt::Display) {
    println!("{} {}", style(ARROW).cyan(), style(format_message(msg)).cyan());
}

pub fn plain(msg: impl std::fmt::Display) {
    println!("{}", format_message(msg));
}

pub fn item(msg: impl std::fmt::Display) {
    println!("  {}", format_message(msg));
}

