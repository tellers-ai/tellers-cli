use console::{style, Emoji};
use crossterm::terminal;

static INFO_ICON: Emoji<'_, '_> = Emoji("ℹ ", "");
static WARNING_ICON: Emoji<'_, '_> = Emoji("⚠ ", "");
#[allow(dead_code)]
static ERROR_ICON: Emoji<'_, '_> = Emoji("✗ ", "");
static SUCCESS_ICON: Emoji<'_, '_> = Emoji("✓ ", "");
#[allow(dead_code)]
static ARROW: Emoji<'_, '_> = Emoji("→ ", "");

const DEFAULT_MAX_WIDTH: usize = 120;
const MIN_WIDTH: usize = 40;
const ICON_AND_SPACE_WIDTH: usize = 4; // Icon (2) + space (1) + buffer (1)

fn get_terminal_width() -> usize {
    terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(DEFAULT_MAX_WIDTH)
}

fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    format!("{}...", &s[..max_len.saturating_sub(3)])
}

fn format_message(msg: &impl std::fmt::Display) -> String {
    let msg_str = msg.to_string();
    let terminal_width = get_terminal_width().max(MIN_WIDTH);
    let max_width = terminal_width.saturating_sub(ICON_AND_SPACE_WIDTH);
    truncate_with_ellipsis(&msg_str, max_width)
}

pub fn info(msg: impl std::fmt::Display) {
    let formatted = format_message(&msg);
    log::info!("{}", msg); // Log raw message to file
    println!("{} {}", style(INFO_ICON).cyan(), style(formatted).white());
}

pub fn warning(msg: impl std::fmt::Display) {
    let formatted = format_message(&msg);
    log::warn!("{}", msg);
    println!("{} {}", style(WARNING_ICON).yellow(), style(formatted).yellow());
}

#[allow(dead_code)]
pub fn error(msg: impl std::fmt::Display) {
    let formatted = format_message(&msg);
    log::error!("{}", msg);
    eprintln!("{} {}", style(ERROR_ICON).red(), style(formatted).red().bold());
}

pub fn success(msg: impl std::fmt::Display) {
    let formatted = format_message(&msg);
    log::info!("✓ {}", msg);
    println!("{} {}", style(SUCCESS_ICON).green(), style(formatted).green().bold());
}

#[allow(dead_code)]
pub fn step(msg: impl std::fmt::Display) {
    let formatted = format_message(&msg);
    log::info!("→ {}", msg);
    println!("{} {}", style(ARROW).cyan(), style(formatted).cyan());
}

pub fn plain(msg: impl std::fmt::Display) {
    let formatted = format_message(&msg);
    log::info!("{}", msg);
    println!("{}", formatted);
}

pub fn item(msg: impl std::fmt::Display) {
    let formatted = format_message(&msg);
    log::info!("  {}", msg);
    println!("  {}", formatted);
}

#[allow(dead_code)]
pub fn debug(msg: impl std::fmt::Display) {
    let _formatted = format_message(&msg);
    log::debug!("{}", msg);
    // Debug messages not printed to terminal by default
}

#[allow(dead_code)]
pub fn trace(msg: impl std::fmt::Display) {
    let _formatted = format_message(&msg);
    log::trace!("{}", msg);
    // Trace messages not printed to terminal by default
}

