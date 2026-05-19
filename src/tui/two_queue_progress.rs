//! Two-queue progress UI: downscale queue and upload queue, each showing queue size and current file.

use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, Paragraph},
    Frame, Terminal, TerminalOptions, Viewport,
};
use std::{
    cell::RefCell,
    io::stdout,
    sync::{Arc, Mutex},
};

use super::inline_progress::{Message, MessageType};

const PENDING_DISPLAY: usize = 5;

#[derive(Clone, Default)]
pub(crate) struct TwoQueueState {
    pub downscale_queued: usize,
    pub downscale_current: Option<String>,
    pub downscale_current_pct: Option<f64>,
    pub downscale_pending: Vec<String>,
    pub upload_queued: usize,
    pub upload_current: Option<String>,
    pub upload_current_pct: Option<f64>,
    pub upload_pending: Vec<String>,
    pub recent_messages: Vec<Message>,
    pub max_messages: usize,
}

impl TwoQueueState {
    fn downscale_display(&self) -> String {
        let current = self.downscale_current.as_deref().unwrap_or("—").to_string();
        let s = truncate_string(&current, 35);
        if let Some(pct) = self.downscale_current_pct {
            format!("{} ({:.0}%)", s, pct)
        } else {
            s
        }
    }

    fn upload_display(&self) -> String {
        let current = self.upload_current.as_deref().unwrap_or("—").to_string();
        let s = truncate_string(&current, 35);
        if let Some(pct) = self.upload_current_pct {
            format!("{} ({:.0}%)", s, pct)
        } else {
            s
        }
    }

    fn downscale_pending_next(&self) -> impl Iterator<Item = &str> {
        self.downscale_pending
            .iter()
            .take(PENDING_DISPLAY)
            .map(|s| s.as_str())
    }

    fn upload_pending_next(&self) -> impl Iterator<Item = &str> {
        self.upload_pending
            .iter()
            .take(PENDING_DISPLAY)
            .map(|s| s.as_str())
    }
}

pub struct TwoQueueProgress {
    terminal: RefCell<Option<Terminal<CrosstermBackend<std::io::Stdout>>>>,
    state: Arc<Mutex<TwoQueueState>>,
}

impl TwoQueueProgress {
    pub fn new() -> Result<Self, String> {
        let terminal = Terminal::with_options(
            CrosstermBackend::new(stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(14),
            },
        )
        .map_err(|e| format!("failed to initialize terminal: {}", e))?;

        Ok(Self {
            terminal: RefCell::new(Some(terminal)),
            state: Arc::new(Mutex::new(TwoQueueState {
                max_messages: 100,
                ..Default::default()
            })),
        })
    }

    pub fn without_terminal() -> Self {
        Self {
            terminal: RefCell::new(None),
            state: Arc::new(Mutex::new(TwoQueueState {
                max_messages: 100,
                ..Default::default()
            })),
        }
    }

    pub fn clone_handle(&self) -> TwoQueueProgressHandle {
        TwoQueueProgressHandle {
            state: Arc::clone(&self.state),
        }
    }

    pub fn start_render_loop(
        &mut self,
        handle: TwoQueueProgressHandle,
    ) -> tokio::task::JoinHandle<Result<(), String>> {
        use tokio::time::{interval, Duration};
        let terminal_opt = self.terminal.get_mut().take();
        let terminal_mutex = Arc::new(Mutex::new(terminal_opt));
        let handle_clone = handle.clone();
        let terminal_clone = Arc::clone(&terminal_mutex);

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                if let Some(ref mut terminal) = *terminal_clone.lock().unwrap() {
                    let state = handle_clone.state.lock().unwrap();
                    if terminal.draw(|f| draw_two_queue_ui(f, &state)).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            }
            Ok(())
        })
    }

    pub async fn stop_render_loop(render_handle: tokio::task::JoinHandle<Result<(), String>>) {
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        render_handle.abort();
        let _ = render_handle.await;
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if let Some(mut terminal) = self.terminal.get_mut().take() {
            let state = self.state.lock().unwrap();
            terminal
                .draw(|f| draw_two_queue_ui(f, &state))
                .map_err(|e| format!("failed to render: {}", e))?;
            drop(terminal);
        }
        Ok(())
    }

    /// Print all error and warning messages to stderr so the user has a persistent log
    /// after the TUI is closed. Call this after `finish()` when the run is done.
    pub fn print_messages_to_stderr(&self) {
        let state = self.state.lock().unwrap();
        for msg in state.recent_messages.iter().rev() {
            match msg.msg_type {
                MessageType::Error => eprintln!("Error: {}", msg.text),
                MessageType::Warning => eprintln!("Warning: {}", msg.text),
                _ => {}
            }
        }
    }
}

#[derive(Clone)]
pub struct TwoQueueProgressHandle {
    pub(crate) state: Arc<Mutex<TwoQueueState>>,
}

impl TwoQueueProgressHandle {
    pub fn set_downscale_queued(&self, n: usize) {
        let mut s = self.state.lock().unwrap();
        s.downscale_queued = n;
    }

    pub fn decrement_downscale_queued(&self) {
        let mut s = self.state.lock().unwrap();
        s.downscale_queued = s.downscale_queued.saturating_sub(1);
    }

    pub fn set_downscale_current(&self, label: Option<impl Into<String>>) {
        let mut s = self.state.lock().unwrap();
        s.downscale_current = label.map(Into::into);
        s.downscale_current_pct = None;
    }

    pub fn set_downscale_current_pct(&self, pct: Option<f64>) {
        let mut s = self.state.lock().unwrap();
        s.downscale_current_pct = pct;
    }

    pub fn set_downscale_pending(&self, names: Vec<String>) {
        let mut s = self.state.lock().unwrap();
        s.downscale_pending = names;
    }

    pub fn pop_downscale_pending(&self) {
        let mut s = self.state.lock().unwrap();
        if !s.downscale_pending.is_empty() {
            s.downscale_pending.remove(0);
        }
    }

    pub fn increment_upload_queued(&self) {
        let mut s = self.state.lock().unwrap();
        s.upload_queued = s.upload_queued.saturating_add(1);
    }

    pub fn decrement_upload_queued(&self) {
        let mut s = self.state.lock().unwrap();
        s.upload_queued = s.upload_queued.saturating_sub(1);
    }

    pub fn set_upload_current(&self, label: Option<impl Into<String>>) {
        let mut s = self.state.lock().unwrap();
        s.upload_current = label.map(Into::into);
        s.upload_current_pct = None;
    }

    pub fn set_upload_current_pct(&self, pct: Option<f64>) {
        let mut s = self.state.lock().unwrap();
        s.upload_current_pct = pct;
    }

    /// Add a file name to the upload pending list (when enqueueing an upload).
    pub fn push_upload_pending(&self, name: impl Into<String>) {
        let mut s = self.state.lock().unwrap();
        s.upload_pending.push(name.into());
    }

    /// Remove the first pending upload (the one now being uploaded). Call when starting an upload.
    pub fn pop_upload_pending(&self) {
        let mut s = self.state.lock().unwrap();
        if !s.upload_pending.is_empty() {
            s.upload_pending.remove(0);
        }
    }

    pub fn add_info(&self, msg: impl Into<String>) {
        self.add_typed_message(msg, MessageType::Info);
    }

    pub fn add_warning(&self, msg: impl Into<String>) {
        self.add_typed_message(msg, MessageType::Warning);
    }

    pub fn add_error(&self, msg: impl Into<String>) {
        self.add_typed_message(msg, MessageType::Error);
    }

    pub fn add_success(&self, msg: impl Into<String>) {
        self.add_typed_message(msg, MessageType::Success);
    }

    fn add_typed_message(&self, msg: impl Into<String>, msg_type: MessageType) {
        let mut s = self.state.lock().unwrap();
        s.recent_messages.insert(
            0,
            Message {
                text: msg.into(),
                msg_type,
            },
        );
        if s.recent_messages.len() > s.max_messages {
            s.recent_messages.pop();
        }
    }
}

pub fn draw_two_queue_ui(frame: &mut Frame, state: &TwoQueueState) {
    let area = frame.area();
    let block = Block::default().title(" Downscale │ Upload ");
    frame.render_widget(block, area);

    let has_messages = !state.recent_messages.is_empty();
    let msg_space = if has_messages { 4 } else { 0 };

    let vertical = Layout::vertical([Constraint::Min(3), Constraint::Length(msg_space)]).margin(1);

    let areas = vertical.split(area);
    let main_area = areas[0];
    let bottom_area = areas[1];

    // Two columns: each shows "Queue: N" and "Current: <file>"
    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_area);

    let downscale_block = Block::default().title(Span::styled(
        " Downscale ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(downscale_block, cols[0]);

    let downscale_inner = Rect::new(
        cols[0].x + 1,
        cols[0].y + 1,
        cols[0].width.saturating_sub(2),
        cols[0].height.saturating_sub(2),
    );
    let mut downscale_lines: Vec<Line<'_>> = vec![
        Line::from(vec![
            Span::raw("Queue: "),
            Span::styled(
                state.downscale_queued.to_string(),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::raw("Current: "),
            Span::styled(
                state.downscale_display(),
                Style::default().fg(Color::LightGreen),
            ),
        ]),
    ];
    let pending_down: Vec<String> = state
        .downscale_pending_next()
        .map(|s| truncate_string(s, 32))
        .collect();
    if !pending_down.is_empty() {
        downscale_lines.push(Line::from(Span::styled(
            "Next:",
            Style::default().fg(Color::DarkGray),
        )));
        for name in &pending_down {
            downscale_lines.push(Line::from(Span::styled(
                format!("  {}", name),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    let downscale_para = Paragraph::new(downscale_lines);
    frame.render_widget(downscale_para, downscale_inner);

    let upload_block = Block::default().title(Span::styled(
        " Upload ",
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(upload_block, cols[1]);

    let upload_inner = Rect::new(
        cols[1].x + 1,
        cols[1].y + 1,
        cols[1].width.saturating_sub(2),
        cols[1].height.saturating_sub(2),
    );
    let mut upload_lines: Vec<Line<'_>> = vec![
        Line::from(vec![
            Span::raw("Queue: "),
            Span::styled(
                state.upload_queued.to_string(),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::raw("Current: "),
            Span::styled(
                state.upload_display(),
                Style::default().fg(Color::LightGreen),
            ),
        ]),
    ];
    let pending_up: Vec<String> = state
        .upload_pending_next()
        .map(|s| truncate_string(s, 32))
        .collect();
    if !pending_up.is_empty() {
        upload_lines.push(Line::from(Span::styled(
            "Next:",
            Style::default().fg(Color::DarkGray),
        )));
        for name in &pending_up {
            upload_lines.push(Line::from(Span::styled(
                format!("  {}", name),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    let upload_para = Paragraph::new(upload_lines);
    frame.render_widget(upload_para, upload_inner);

    if has_messages && bottom_area.height > 0 {
        let messages: Vec<ListItem> = state
            .recent_messages
            .iter()
            .take(bottom_area.height as usize)
            .map(|msg| {
                let (icon, color) = match msg.msg_type {
                    MessageType::Info => ("ℹ", Color::Cyan),
                    MessageType::Warning => ("⚠", Color::Yellow),
                    MessageType::Error => ("✗", Color::Red),
                    MessageType::Success => ("✓", Color::Green),
                };
                let text = truncate_string(&msg.text, 70);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{} ", icon),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text, Style::default().fg(color)),
                ]))
            })
            .collect();
        if !messages.is_empty() {
            frame.render_widget(List::new(messages), bottom_area);
        }
    }
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
