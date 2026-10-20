use crate::config::ConfigStore;
use crate::library::Library;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Mutex;

/// What to do when a registered menu item is activated.
#[derive(Clone)]
pub enum MenuAction {
    /// Emit an event to the main window (channel, optional payload).
    Emit {
        channel: String,
        payload: Option<serde_json::Value>,
    },
    /// Open a URL in the system browser.
    OpenUrl(String),
    /// Show the main window (tray "Open Heynote").
    ShowWindow,
    /// Toggle window fullscreen (main/tray menu).
    ToggleFullscreen,
    /// Open the About window.
    ShowAbout,
    /// Quit the application (tray Quit item).
    Quit,
}

// notify-debouncer-full's new_debouncer() returns Debouncer<RecommendedWatcher,
// RecommendedCache> on every platform (the cache resolves to the right backend
// internally) — keep the alias platform-independent.
pub type FileDebouncer = notify_debouncer_full::Debouncer<
    notify_debouncer_full::notify::RecommendedWatcher,
    notify_debouncer_full::RecommendedCache,
>;

pub struct AppState {
    pub config: Mutex<ConfigStore>,
    pub library: Mutex<Option<Library>>,
    pub watcher: Mutex<Option<FileDebouncer>>,
    pub init_errors: Mutex<Vec<String>>,
    pub force_quit: AtomicBool,
    /// Set once the renderer has flushed all buffers via buffer:saveAndQuit.
    pub content_saved: AtomicBool,
    /// Last known fullscreen state of the main window (Tauri has no fullscreen
    /// window event; we poll on Resized/Focused and emit on change).
    pub fullscreen_cached: Mutex<bool>,
    /// menu item id -> action
    pub menu_registry: Mutex<HashMap<u32, MenuAction>>,
    pub next_menu_id: AtomicU32,
    /// Current search generation; bumping it aborts in-flight searches.
    pub search_gen: AtomicU64,
}

impl AppState {
    pub fn new(config: ConfigStore) -> Self {
        AppState {
            config: Mutex::new(config),
            library: Mutex::new(None),
            watcher: Mutex::new(None),
            init_errors: Mutex::new(Vec::new()),
            force_quit: AtomicBool::new(false),
            content_saved: AtomicBool::new(false),
            fullscreen_cached: Mutex::new(false),
            menu_registry: Mutex::new(HashMap::new()),
            next_menu_id: AtomicU32::new(1),
            search_gen: AtomicU64::new(0),
        }
    }
}
