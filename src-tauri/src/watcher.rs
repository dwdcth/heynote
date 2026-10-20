// Port of the fs.watch-based watcher in electron/main/file-library.js, using
// the notify crate with debouncing. Self-writes are suppressed by content
// comparison inside Library::reload_if_changed (the same trick the Electron
// version uses), so a save -> watch-event -> reload loop cannot occur.

use crate::state::{AppState, FileDebouncer};
use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub fn stop(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut watcher = state.watcher.lock().unwrap();
    *watcher = None;
}

pub fn start(app: &AppHandle, base_path: PathBuf) -> Result<(), String> {
    stop(app);

    let app_handle = app.clone();
    let watch_root = base_path.clone();
    let mut debouncer: FileDebouncer = new_debouncer(
        Duration::from_millis(200),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                let state = app_handle.state::<AppState>();
                let mut library_guard = state.library.lock().unwrap();
                let Some(library) = library_guard.as_mut() else {
                    return;
                };
                let mut changed: Vec<String> = Vec::new();
                for event in &events {
                    for path in &event.paths {
                        if let Ok(rel) = path.strip_prefix(&watch_root) {
                            let rel = rel.to_string_lossy().replace('\\', "/");
                            if !rel.is_empty() && !changed.contains(&rel) {
                                changed.push(rel);
                            }
                        }
                    }
                }
                for rel in changed {
                    if let Some(content) = library.reload_if_changed(&rel) {
                        let _ = app_handle.emit(
                            "buffer:change",
                            serde_json::json!({ "path": rel, "content": content }),
                        );
                    }
                }
            }
            Err(errors) => {
                eprintln!("file watcher errors: {:?}", errors);
            }
        },
    )
    .map_err(|e| format!("failed to create watcher: {}", e))?;

    debouncer
        .watch(&base_path, RecursiveMode::Recursive)
        .map_err(|e| format!("failed to watch {}: {}", base_path.display(), e))?;

    let state = app.state::<AppState>();
    *state.watcher.lock().unwrap() = Some(debouncer);
    Ok(())
}

/// Reload all open buffers and emit buffer:change for the ones that changed.
/// Called when the window regains focus (fs watching is unreliable e.g. on
/// OneDrive-mounted directories — Electron parity).
pub fn reload_all_open(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut library_guard = state.library.lock().unwrap();
    let Some(library) = library_guard.as_mut() else {
        return;
    };
    for rel in library.open_paths() {
        if let Some(content) = library.reload_if_changed(&rel) {
            let _ = app.emit(
                "buffer:change",
                serde_json::json!({ "path": rel, "content": content }),
            );
        }
    }
}
