//! A simple TUI checkbox list for multi-select. Space toggles, Enter confirms.

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::io;

/// Run a checkbox list TUI. Returns the selected items (those with checkbox checked).
/// - Space: toggle current item
/// - a: toggle all
/// - Enter: confirm and return selected
/// - q / Esc: confirm and return current selection (same as Enter)
///
/// `default_checked`: initial checkbox state per item (true = checked). If shorter than
/// `items`, remaining items are unchecked; if longer, extra values are ignored.
pub fn run_checkbox_list(
    title: &str,
    items: Vec<String>,
    default_checked: Option<Vec<bool>>,
) -> Result<Vec<String>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| format!("terminal: {}", e))?;
    execute!(stdout, EnterAlternateScreen).map_err(|e| format!("terminal: {}", e))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| format!("terminal: {}", e))?;

    let mut checked: Vec<bool> = if let Some(d) = default_checked {
        (0..items.len())
            .map(|i| d.get(i).copied().unwrap_or(false))
            .collect()
    } else {
        items.iter().map(|_| false).collect()
    };
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let result = loop {
        terminal
            .draw(|f| draw_checkbox_list(f, title, &items, &checked, &mut list_state))
            .map_err(|e| format!("draw: {}", e))?;

        if let Event::Key(key) = event::read().map_err(|e| format!("event: {}", e))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break collect_selected(&items, &checked),
                KeyCode::Enter => break collect_selected(&items, &checked),
                KeyCode::Up => {
                    let i = list_state.selected().unwrap_or(0);
                    list_state.select(Some(i.saturating_sub(1)));
                }
                KeyCode::Down => {
                    let i = list_state.selected().unwrap_or(0);
                    let next = (i + 1).min(items.len().saturating_sub(1));
                    list_state.select(Some(next));
                }
                KeyCode::Char(' ') => {
                    if let Some(i) = list_state.selected() {
                        if i < checked.len() {
                            checked[i] = !checked[i];
                        }
                    }
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    let all = checked.iter().all(|&c| c);
                    for c in &mut checked {
                        *c = !all;
                    }
                }
                _ => {}
            }
        }
    };

    execute!(io::stdout(), LeaveAlternateScreen).map_err(|e| format!("terminal: {}", e))?;
    disable_raw_mode().map_err(|e| format!("terminal: {}", e))?;
    terminal.show_cursor().map_err(|e| format!("terminal: {}", e))?;

    Ok(result)
}

fn draw_checkbox_list(
    f: &mut Frame,
    title: &str,
    items: &[String],
    checked: &[bool],
    list_state: &mut ListState,
) {
    let area = f.area();
    let chunks = Layout::default()
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let mark = if checked.get(i).copied().unwrap_or(false) {
                "[x]"
            } else {
                "[ ]"
            };
            ListItem::new(Line::from(format!("{} {}", mark, label)))
        })
        .collect();

    let list = List::new(list_items)
        .block(Block::default().title(title).borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, chunks[0], list_state);

    let help = Paragraph::new("↑/↓ move  Space toggle  a toggle all  Enter confirm  q quit");
    f.render_widget(help, chunks[1]);
}

fn collect_selected(items: &[String], checked: &[bool]) -> Vec<String> {
    items
        .iter()
        .zip(checked.iter().copied())
        .filter_map(|(id, c)| if c { Some(id.clone()) } else { None })
        .collect()
}
