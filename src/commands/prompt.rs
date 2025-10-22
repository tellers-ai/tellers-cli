use crate::tellers_api::client::TellersClient;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use std::io::Stdout;

pub fn run_interactive(prompt_text: String, full_auto: bool) -> Result<(), String> {
    let client = TellersClient::new_from_env().map_err(|e| e.to_string())?;
    let chat_id = client
        .create_chat(&prompt_text, full_auto)
        .map_err(|e| e.to_string())?;
    let chunks = client.stream_chat(&chat_id).map_err(|e| e.to_string())?;

    let mut stdout = std::io::stdout();
    setup_terminal(&mut stdout)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let mut buffer = String::new();
    for part in chunks {
        match part {
            crate::tellers_api::models::ChatChunk::Text(text) => buffer.push_str(&text),
        }
        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Percentage(100)].as_ref())
                    .split(f.area());

                let block = Block::default().title("Tellers Chat").borders(Borders::ALL);
                let paragraph = Paragraph::new(buffer.as_str()).block(block);
                f.render_widget(paragraph, chunks[0]);
            })
            .map_err(|e| e.to_string())?;
    }

    restore_terminal()?;
    Ok(())
}

pub fn run_background(prompt_text: String, full_auto: bool) -> Result<String, String> {
    let client = TellersClient::new_from_env().map_err(|e| e.to_string())?;
    let chat_id = client
        .create_chat(&prompt_text, full_auto)
        .map_err(|e| e.to_string())?;
    Ok(chat_id)
}

fn setup_terminal(stdout: &mut Stdout) -> Result<(), String> {
    use crossterm::{execute, terminal};
    execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )
    .map_err(|e| e.to_string())?;
    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    Ok(())
}

fn restore_terminal() -> Result<(), String> {
    use crossterm::{execute, terminal};
    terminal::disable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
