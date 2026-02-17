# TUI Module - Inline Progress Display

This module provides a simple, reusable abstraction for displaying progress using Ratatui's inline viewport feature. It's designed to be easy to use in commands that need to show concurrent operations (like file uploads).

## Architecture

- **`InlineProgress`**: Main struct that owns the terminal and renders progress
- **`ProgressHandle`**: Thread-safe handle for updating progress from async contexts

## Usage

### Simple Synchronous Usage

Use `clone_handle()` and the handle's methods (render loop is optional for sync use):

```rust
use crate::tui::{InlineProgress, ProgressHandle};

let mut progress = InlineProgress::new("Uploading Files", total_files)?;
let progress_handle = progress.clone_handle();

for (i, file) in files.iter().enumerate() {
    let file_size = std::fs::metadata(file)?.len();
    let _ = progress_handle.start_task(i, file.display().to_string(), file_size);
    
    // Simulate upload progress
    for chunk in 0..100 {
        let uploaded = (chunk * file_size / 100) as u64;
        let _ = progress_handle.update_task(i, uploaded);
        std::thread::sleep(Duration::from_millis(10));
    }
    
    let _ = progress_handle.finish_task(i, true);
}

progress.finish()?;
```

### Async Usage (Recommended for Tokio)

```rust
use crate::tui::{InlineProgress, ProgressHandle};

// Create progress display
let progress = InlineProgress::new("Uploading Files", files.len())?;
let progress_handle = progress.clone_handle();

// Spawn render loop (runs every 100ms)
let render_handle = {
    let mut terminal = progress.terminal.take().unwrap();
    let handle = progress_handle.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            let state = handle.state.lock().unwrap();
            if terminal.draw(|f| draw_ui_internal(f, &state)).is_err() {
                break;
            }
        }
    })
};

// Use handle in async tasks
for (i, file) in files.iter().enumerate() {
    let file_size = std::fs::metadata(file)?.len();
    let handle = progress_handle.clone();
    
    tokio::spawn(async move {
        handle.start_task(i, file.display().to_string(), file_size)?;
        
        // Upload file...
        handle.update_task(i, uploaded_bytes)?;
        
        handle.finish_task(i, true)?;
        Ok::<(), String>(())
    });
}

// Clean up
render_handle.abort();
progress.finish()?;
```

## API

### `InlineProgress`

- `new(title, total_tasks)` - Create a new progress display
- `clone_handle()` - Get a thread-safe handle for updates
- `start_render_loop(handle)` - Start the periodic render task (async)
- `stop_render_loop(render_handle)` - Stop the render loop
- `finish()` - Finalize and cleanup

### `ProgressHandle`

- `start_task(task_id, label, total_bytes)` - Thread-safe version
- `update_task(task_id, uploaded_bytes)` - Thread-safe version
- `finish_task(task_id, success)` - Thread-safe version

