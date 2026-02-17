use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Gauge, LineGauge, List, ListItem, Paragraph},
    Frame, Terminal, TerminalOptions, Viewport,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::stdout,
    sync::{Arc, Mutex},
    time::Instant,
};

pub type TaskId = usize;

pub struct InlineProgress {
    terminal: RefCell<Option<Terminal<CrosstermBackend<std::io::Stdout>>>>,
    state: Arc<Mutex<ProgressState>>,
}

#[derive(Clone)]
struct TaskProgress {
    label: String,
    started_at: Instant,
    progress: f64,
    total_bytes: u64,
    uploaded_bytes: u64,
    completed: bool,
}

#[derive(Clone)]
pub(crate) enum MessageType {
    Info,
    Warning,
    Error,
    Success,
}

#[derive(Clone)]
pub(crate) struct Message {
    pub(crate) text: String,
    pub(crate) msg_type: MessageType,
}

pub(crate) struct ProgressState {
    title: String,
    total_tasks: usize,
    completed: usize,
    in_progress: BTreeMap<TaskId, TaskProgress>,
    recent_messages: Vec<Message>,
    max_messages: usize,
}

impl InlineProgress {
    pub fn new(title: impl Into<String>, total_tasks: usize) -> Result<Self, String> {
        let terminal = Terminal::with_options(
            CrosstermBackend::new(stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(12),
            },
        )
        .map_err(|e| format!("failed to initialize terminal: {}", e))?;

        Ok(Self {
            terminal: RefCell::new(Some(terminal)),
            state: Arc::new(Mutex::new(ProgressState {
                title: title.into(),
                total_tasks,
                completed: 0,
                in_progress: BTreeMap::new(),
                recent_messages: Vec::new(),
                max_messages: 5,
            })),
        })
    }

    pub fn start_task(
        &self,
        task_id: TaskId,
        label: impl Into<String>,
        total_bytes: u64,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.in_progress.insert(
            task_id,
            TaskProgress {
                label: label.into(),
                started_at: Instant::now(),
                progress: 0.0,
                total_bytes,
                uploaded_bytes: 0,
                completed: false,
            },
        );
        drop(state);
        self.render()?;
        Ok(())
    }

    pub fn update_task(&self, task_id: TaskId, uploaded_bytes: u64) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if let Some(task) = state.in_progress.get_mut(&task_id) {
            task.uploaded_bytes = uploaded_bytes;
            if task.total_bytes > 0 {
                task.progress = (uploaded_bytes as f64 / task.total_bytes as f64) * 100.0;
            }
        }
        drop(state);
        self.render()?;
        Ok(())
    }

    pub fn finish_task(&self, task_id: TaskId, success: bool) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if let Some(task) = state.in_progress.get_mut(&task_id) {
            if success {
                task.completed = true;
                task.progress = 100.0;
                state.completed += 1;
                // Don't add to messages - task stays visible in the list with completion status
            }
        }
        drop(state);
        self.render()?;
        Ok(())
    }

    pub fn add_message(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Info)
    }

    pub fn add_info(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Info)
    }

    pub fn add_warning(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Warning)
    }

    pub fn add_error(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Error)
    }

    pub fn add_success(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Success)
    }

    fn add_typed_message(
        &self,
        msg: impl Into<String>,
        msg_type: MessageType,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.recent_messages.insert(
            0,
            Message {
                text: msg.into(),
                msg_type,
            },
        );
        if state.recent_messages.len() > state.max_messages {
            state.recent_messages.pop();
        }
        drop(state);
        self.render()?;
        Ok(())
    }

    pub fn clone_handle(&self) -> ProgressHandle {
        ProgressHandle {
            state: Arc::clone(&self.state),
        }
    }

    pub fn start_render_loop(
        &mut self,
        handle: ProgressHandle,
    ) -> tokio::task::JoinHandle<Result<(), String>> {
        use tokio::time::{interval, Duration};
        let terminal_opt = self.terminal.get_mut().take();
        let terminal_mutex = std::sync::Arc::new(std::sync::Mutex::new(terminal_opt));
        let handle_clone = handle.clone();
        let terminal_clone = std::sync::Arc::clone(&terminal_mutex);

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                if let Some(ref mut terminal) = *terminal_clone.lock().unwrap() {
                    let state = handle_clone.state.lock().unwrap();
                    if terminal.draw(|f| draw_ui_internal(f, &state)).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            }
            Ok(())
        })
    }

    /// Stop the render loop and ensure final state is displayed.
    /// This method handles the timing to ensure the last render completes before cleanup.
    pub async fn stop_render_loop(render_handle: tokio::task::JoinHandle<Result<(), String>>) {
        // Allow time for the render loop to process final state changes and display them.
        // The render loop runs asynchronously and polls state periodically. Without this
        // delay, we might abort it before it shows the final completed tasks and messages.
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        render_handle.abort();
        let _ = render_handle.await;
    }

    fn render(&self) -> Result<(), String> {
        if let Some(ref mut terminal) = *self.terminal.borrow_mut() {
            let state = self.state.lock().unwrap();
            terminal
                .draw(|frame| draw_ui_internal(frame, &state))
                .map_err(|e| format!("failed to render: {}", e))?;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if let Some(mut terminal) = self.terminal.get_mut().take() {
            let state = self.state.lock().unwrap();
            terminal
                .draw(|frame| draw_ui_internal(frame, &state))
                .map_err(|e| format!("failed to render: {}", e))?;
            // Terminal drop will restore the viewport, but we need to ensure it happens
            // Explicitly drop to restore terminal state
            drop(terminal);
        }
        Ok(())
    }
}

impl Drop for InlineProgress {
    fn drop(&mut self) {
        if let Some(mut terminal) = self.terminal.get_mut().take() {
            let state = self.state.lock().unwrap();
            let _ = terminal.draw(|frame| draw_ui_internal(frame, &state));
        }
    }
}

#[derive(Clone)]
pub struct ProgressHandle {
    pub(crate) state: Arc<Mutex<ProgressState>>,
}

impl ProgressHandle {
    pub fn start_task(
        &self,
        task_id: TaskId,
        label: impl Into<String>,
        total_bytes: u64,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.in_progress.insert(
            task_id,
            TaskProgress {
                label: label.into(),
                started_at: Instant::now(),
                progress: 0.0,
                total_bytes,
                uploaded_bytes: 0,
                completed: false,
            },
        );
        Ok(())
    }

    pub fn update_task(&self, task_id: TaskId, uploaded_bytes: u64) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if let Some(task) = state.in_progress.get_mut(&task_id) {
            task.uploaded_bytes = uploaded_bytes;
            if task.total_bytes > 0 {
                task.progress = (uploaded_bytes as f64 / task.total_bytes as f64) * 100.0;
            }
        }
        Ok(())
    }

    pub fn finish_task(&self, task_id: TaskId, success: bool) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if let Some(task) = state.in_progress.get_mut(&task_id) {
            if success {
                task.completed = true;
                task.progress = 100.0;
                state.completed += 1;
                // Don't add to messages - task stays visible in the list with completion status
            }
        }
        Ok(())
    }

    pub fn add_message(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Info)
    }

    pub fn add_info(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Info)
    }

    pub fn add_warning(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Warning)
    }

    pub fn add_error(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Error)
    }

    pub fn add_success(&self, msg: impl Into<String>) -> Result<(), String> {
        self.add_typed_message(msg, MessageType::Success)
    }

    fn add_typed_message(
        &self,
        msg: impl Into<String>,
        msg_type: MessageType,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.recent_messages.insert(
            0,
            Message {
                text: msg.into(),
                msg_type,
            },
        );
        if state.recent_messages.len() > state.max_messages {
            state.recent_messages.pop();
        }
        Ok(())
    }
}

pub fn draw_ui_internal(frame: &mut Frame, state: &ProgressState) {
    let area = frame.area();

    let block = Block::default().title(Line::from(state.title.as_str()).centered());
    frame.render_widget(block, area);

    let has_messages = !state.recent_messages.is_empty();
    let msg_space = if has_messages { 4 } else { 0 };

    let vertical = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(msg_space),
    ])
    .margin(1);

    let areas = vertical.split(area);
    let top_area = areas[0];
    let middle_area = areas[1];
    let bottom_area = areas[2];

    let horizontal_areas = Layout::horizontal([
        Constraint::Percentage(40),
        Constraint::Length(3),
        Constraint::Percentage(57),
    ])
    .split(middle_area);
    let list_area = horizontal_areas[0];
    let separator_area = horizontal_areas[1];
    let gauge_area = horizontal_areas[2];

    let progress_ratio = if state.total_tasks > 0 {
        state.completed as f64 / state.total_tasks as f64
    } else {
        0.0
    };

    let total_progress = LineGauge::default()
        .filled_style(Style::default().fg(Color::Blue))
        .label(format!("{}/{}", state.completed, state.total_tasks))
        .ratio(progress_ratio);

    frame.render_widget(total_progress, top_area);

    let max_items = list_area.height as usize;

    // Separate in-progress and completed tasks
    let mut in_progress_tasks: Vec<_> = state
        .in_progress
        .iter()
        .filter(|(_, task)| !task.completed)
        .collect();
    let mut completed_tasks: Vec<_> = state
        .in_progress
        .iter()
        .filter(|(_, task)| task.completed)
        .collect();

    // Sort by task ID to show in order
    in_progress_tasks.sort_by_key(|(id, _)| *id);
    completed_tasks.sort_by_key(|(id, _)| *id);

    // Show active tasks first, then recent completions if space allows
    let mut tasks_to_show: Vec<_> = in_progress_tasks.into_iter().take(max_items).collect();
    let remaining_space = max_items.saturating_sub(tasks_to_show.len());
    if remaining_space > 0 {
        // Show most recent completions (reverse order)
        tasks_to_show.extend(completed_tasks.iter().rev().take(remaining_space));
    }

    let items: Vec<ListItem> = tasks_to_show
        .iter()
        .map(|(_, task)| {
            let elapsed_ms = task.started_at.elapsed().as_millis();
            let label = truncate_string(&task.label, 28);
            let (icon, color) = if task.completed {
                ("✓", Color::Green)
            } else {
                ("●", Color::LightGreen)
            };

            ListItem::new(Line::from(vec![
                Span::raw(format!("{} ", icon)),
                Span::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" ({}ms)", elapsed_ms)),
            ]))
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, list_area);

    // Render separator " | " between file names and progress bars
    for (i, _) in tasks_to_show.iter().enumerate() {
        let y = list_area.top().saturating_add(i as u16);
        if y >= list_area.bottom() {
            break;
        }
        frame.render_widget(
            Paragraph::new(" | "),
            Rect {
                x: separator_area.left(),
                y,
                width: separator_area.width,
                height: 1,
            },
        );
    }

    for (i, (_, task)) in tasks_to_show.iter().enumerate() {
        let gauge_color = if task.completed {
            Color::Green
        } else {
            Color::Yellow
        };
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(gauge_color))
            .ratio(task.progress / 100.0)
            .label("");

        let y = list_area.top().saturating_add(i as u16);
        if y >= list_area.bottom() || y >= gauge_area.bottom() {
            break;
        }

        frame.render_widget(
            gauge,
            Rect {
                x: gauge_area.left(),
                y,
                width: gauge_area.width,
                height: 1,
            },
        );
    }

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

                let text = truncate_string(&msg.text, 65);
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
            let msg_list = List::new(messages);
            frame.render_widget(msg_list, bottom_area);
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
