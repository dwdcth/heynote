// All #[tauri::command] handlers exposed to the bridge. Channel mapping is
// documented in src/tauri/bridge.js; the Rust side uses snake_case command
// names (Tauri converts camelCase JS args automatically).

use crate::config;
use crate::library;
use crate::search::{self, SearchOptions};
use crate::state::AppState;
use crate::watcher;
use crate::window_ctl;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State, Wry, WindowEvent};
use tauri_plugin_opener::OpenerExt;

const CURRENCY_RATES_URL: &str = "https://currencies.heynote.com/rates.json";
const CURRENCY_STALE_MS: f64 = 1000.0 * 3600.0 * 12.0;

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Port of initFileLibrary(): legacy migration, library init, watcher start,
/// image GC.
pub fn init_library(app: &AppHandle<Wry>) -> Result<(), String> {
    let state = app.state::<AppState>();

    // stop the current watcher and search (if any)
    watcher::stop(app);
    state.search_gen.fetch_add(1, Ordering::SeqCst);

    let custom_buffer_path: Option<std::path::PathBuf> = {
        let config_store = state.config.lock().unwrap();
        config_store
            .settings()
            .get("bufferPath")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(config::untildify)
    };

    library::migrate_buffer_file_to_library(custom_buffer_path.as_deref());

    let base_path = match &custom_buffer_path {
        Some(custom) => custom.clone(),
        None => {
            let dir = config::user_data_dir().join(library::notes_dir_name());
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create {}: {}", dir.display(), e))?;
            dir
        }
    };

    match library::Library::new(base_path.clone()) {
        Ok(lib) => {
            watcher::start(app, base_path.clone())?;
            *state.library.lock().unwrap() = Some(lib);
            // garbage-collect stale images in the background
            let images_path = base_path.join(".images");
            std::thread::spawn(move || {
                library::remove_unreferenced_images(&base_path, &images_path);
            });
        }
        Err(e) => {
            eprintln!("failed to initialize file library: {}", e);
            state.init_errors.lock().unwrap().push(format!("Error: {}", e));
            *state.library.lock().unwrap() = None;
        }
    }
    Ok(())
}

#[tauri::command]
async fn get_initial_state(app: AppHandle<Wry>) -> Result<Value, String> {
    eprintln!("[cmd] get_initial_state");
    let state = app.state::<AppState>();
    let config_store = state.config.lock().unwrap();
    Ok(json!({
        "settings": config_store.settings(),
        "platform": {
            "isMac": cfg!(target_os = "macos"),
            "isWindows": cfg!(windows),
            "isLinux": cfg!(target_os = "linux"),
        },
        "isDev": config::is_dev(),
        "theme": config_store.theme(),
        "pathSeparator": std::path::MAIN_SEPARATOR,
    }))
}

// Diagnostic: frontend error/rejection/console forwarding (see bridge.js hooks).
#[tauri::command]
fn log_frontend(message: String) {
    eprintln!("[frontend] {}", message);
}

#[tauri::command]
fn get_version(app: AppHandle<Wry>, state: State<'_, AppState>) -> String {
    let mut version = app.package_info().version.to_string();
    let beta = state
        .config
        .lock()
        .unwrap()
        .settings()
        .get("allowBetaVersions")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if beta {
        version.push_str(" (beta channel)");
    }
    version
}

#[tauri::command]
fn get_init_errors(state: State<'_, AppState>) -> Vec<String> {
    state.init_errors.lock().unwrap().clone()
}

#[tauri::command]
fn get_system_locale() -> String {
    sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string())
}

#[tauri::command]
fn set_window_title(_app: AppHandle<Wry>, _title: String) {
    // The Electron version shows the current buffer name in the window
    // title; all Tauri platforms use chromeless/overlay title bars where a
    // title would either be invisible (Linux/Windows) or overlap the tab
    // bar (macOS Overlay), so we keep the title empty.
}

/// Native-level focus restore. When keyboard focus has passed through a
/// dialog input, WebKit's input-context routing can end up stuck and JS
/// focus() calls no longer revive key event delivery; a native webview
/// focus event does. (WebviewWindow::set_focus only focuses the OS window —
/// we need the webview-level focus to make it the first responder.)
#[tauri::command]
fn focus_webview(app: AppHandle<Wry>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_focus();
    }
    if let Some(webview) = app.get_webview("main") {
        let _ = webview.set_focus();
    }
}

#[tauri::command]
async fn open_external_url(app: AppHandle<Wry>, url: String) -> Result<(), String> {
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_about_data(app: AppHandle<Wry>, state: State<'_, AppState>) -> Result<Value, String> {
    let version = app.package_info().version.to_string();
    let currency_timestamp = {
        let config_store = state.config.lock().unwrap();
        config_store
            .currency_cache()
            .and_then(|c| c.get("timeFetched").and_then(|v| v.as_f64()))
    };
    Ok(json!({
        "version": version,
        "currencyTimestamp": currency_timestamp,
    }))
}

// ---------------------------------------------------------------------------
// Settings / theme
// ---------------------------------------------------------------------------

#[tauri::command]
async fn set_settings(
    app: AppHandle<Wry>,
    state: State<'_, AppState>,
    settings: Value,
) -> Result<(), String> {
    let (hotkey_changed, dock_changed, menu_changed, ontop_changed, login_changed, buffer_path_changed) = {
        let config_store = state.config.lock().unwrap();
        let old = config_store.settings();
        let cmp = |key: &str| old.get(key) != settings.get(key);
        (
            cmp("enableGlobalHotkey") || cmp("globalHotkey"),
            cmp("showInDock"),
            cmp("showInMenu"),
            cmp("alwaysOnTop"),
            cmp("openAtLogin"),
            cmp("bufferPath"),
        )
    };

    state.config.lock().unwrap().set_settings(settings.clone())?;

    // The settings store in the renderer updates itself from this event
    // (mirrors index.ts:690 — setSettings' return value is not used).
    let _ = app.emit("settings-change", settings);

    if hotkey_changed {
        window_ctl::register_global_hotkey(&app);
    }
    if dock_changed {
        window_ctl::register_show_in_dock(&app);
    }
    if menu_changed {
        window_ctl::register_show_in_menu(&app);
    }
    if ontop_changed {
        window_ctl::register_always_on_top(&app);
    }
    if login_changed {
        window_ctl::register_autostart(&app);
    }
    if buffer_path_changed {
        init_library(&app)?;
        let _ = app.emit("library:pathChanged", Value::Null);
    }
    Ok(())
}

#[tauri::command]
fn dark_mode_set(app: AppHandle<Wry>, state: State<'_, AppState>, mode: String) -> Result<(), String> {
    state.config.lock().unwrap().set_theme(&mode)?;
    if let Some(window) = app.get_webview_window("main") {
        let theme = match mode.as_str() {
            "dark" => Some(tauri::Theme::Dark),
            "light" => Some(tauri::Theme::Light),
            _ => None,
        };
        let _ = window.set_theme(theme);
    }
    Ok(())
}

#[tauri::command]
fn dark_mode_get(state: State<'_, AppState>) -> String {
    state.config.lock().unwrap().theme()
}

// ---------------------------------------------------------------------------
// Buffer / library operations
// ---------------------------------------------------------------------------

macro_rules! with_library {
    ($state:expr, $lib:ident => $body:expr) => {{
        let mut guard = $state.library.lock().unwrap();
        match guard.as_mut() {
            Some($lib) => $body,
            None => Err("File library is not initialized".to_string()),
        }
    }};
}

#[tauri::command]
fn buffer_exists(state: State<'_, AppState>, path: String) -> Result<bool, String> {
    with_library!(state, lib => lib.exists(&path))
}

#[tauri::command]
fn buffer_get_list(state: State<'_, AppState>) -> Result<Value, String> {
    eprintln!("[cmd] buffer_get_list");
    with_library!(state, lib => lib.get_list())
}

#[tauri::command]
fn buffer_get_directory_list(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    with_library!(state, lib => lib.get_directory_list())
}

#[tauri::command]
fn buffer_load(state: State<'_, AppState>, path: String) -> Result<String, String> {
    eprintln!("[cmd] buffer_load path={}", path);
    with_library!(state, lib => lib.load(&path))
}

#[tauri::command]
fn buffer_save(state: State<'_, AppState>, path: String, content: String) -> Result<(), String> {
    eprintln!("[cmd] buffer_save path={} len={}", path, content.len());
    with_library!(state, lib => lib.save(&path, &content))
}

#[tauri::command]
fn buffer_delete(state: State<'_, AppState>, path: String) -> Result<(), String> {
    with_library!(state, lib => lib.delete(&path))
}

#[tauri::command]
fn buffer_is_directory_empty(state: State<'_, AppState>, path: String) -> Result<bool, String> {
    with_library!(state, lib => lib.is_directory_empty(&path))
}

#[tauri::command]
fn buffer_delete_directory(state: State<'_, AppState>, path: String) -> Result<(), String> {
    with_library!(state, lib => lib.delete_directory(&path))
}

#[tauri::command]
fn buffer_move(state: State<'_, AppState>, path: String, new_path: String) -> Result<(), String> {
    with_library!(state, lib => lib.move_file(&path, &new_path))
}

#[tauri::command]
fn buffer_create(state: State<'_, AppState>, path: String, content: String) -> Result<(), String> {
    with_library!(state, lib => lib.create(&path, &content))
}

#[tauri::command]
fn buffer_create_directory(state: State<'_, AppState>, path: String) -> Result<(), String> {
    with_library!(state, lib => lib.create_directory(&path))
}

#[tauri::command]
fn buffer_close(state: State<'_, AppState>, path: String) -> Result<(), String> {
    with_library!(state, lib => {
        lib.close_file(&path);
        Ok(())
    })
}

#[tauri::command]
fn buffer_save_and_quit(app: AppHandle<Wry>, state: State<'_, AppState>, contents: Vec<(String, String)>) -> Result<(), String> {
    watcher::stop(&app);
    {
        let mut guard = state.library.lock().unwrap();
        if let Some(lib) = guard.as_mut() {
            for (path, content) in contents {
                if let Err(e) = lib.save(&path, &content) {
                    eprintln!("failed to save {}: {}", path, e);
                }
            }
        }
    }
    state.content_saved.store(true, Ordering::SeqCst);
    window_ctl::quit_app(&app);
    Ok(())
}

#[tauri::command]
fn library_save_image(state: State<'_, AppState>, mime: String, data: Vec<u8>) -> Result<Option<String>, String> {
    with_library!(state, lib => lib.save_image(&mime, &data))
}

#[tauri::command]
async fn library_select_location(app: AppHandle<Wry>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    // async command: blocking dialog must not run on the main thread
    let folder = app.dialog().file().blocking_pick_folder();
    Ok(folder.and_then(|f| f.as_path().map(|p| p.display().to_string())))
}

// ---------------------------------------------------------------------------
// Tabs state
// ---------------------------------------------------------------------------

#[tauri::command]
fn save_tabs_state(state: State<'_, AppState>, tabs_state: Value) -> Result<(), String> {
    state.config.lock().unwrap().set_open_tabs_state(tabs_state)
}

#[tauri::command]
fn load_tabs_state(state: State<'_, AppState>) -> Option<Value> {
    eprintln!("[cmd] load_tabs_state");
    state.config.lock().unwrap().open_tabs_state()
}

// ---------------------------------------------------------------------------
// Currency
// ---------------------------------------------------------------------------

#[tauri::command]
async fn currency_get(app: AppHandle<Wry>, state: State<'_, AppState>) -> Result<Option<Value>, String> {
    let (client_id, cached) = {
        let config_store = state.config.lock().unwrap();
        (config_store.client_id(), config_store.currency_cache())
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);

    if let Some(cache) = &cached {
        if let Some(time_fetched) = cache.get("timeFetched").and_then(|v| v.as_f64()) {
            if now - time_fetched < CURRENCY_STALE_MS {
                return Ok(cache.get("data").cloned());
            }
        }
    }

    let version = app.package_info().version.to_string();
    // blocking HTTP on the thread pool (sync commands run on the main thread)
    let fetch_result = tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let response = ureq::get(CURRENCY_RATES_URL)
            .set("x-client-id", &client_id)
            .set("x-client-version", &version)
            .set("cache-control", "no-cache")
            .timeout(std::time::Duration::from_secs(30))
            .call()
            .map_err(|e| e.to_string())?;
        response
            .into_json::<Value>()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    match fetch_result {
        Ok(data) => {
            state.config.lock().unwrap().set_currency_cache(data.clone(), now)?;
            Ok(Some(data))
        }
        Err(e) => {
            eprintln!("failed to fetch currency rates: {}", e);
            stale_fallback(cached)
        }
    }
}

fn stale_fallback(cached: Option<Value>) -> Result<Option<Value>, String> {
    Ok(cached.and_then(|c| c.get("data").cloned()))
}

// ---------------------------------------------------------------------------
// Library search
// ---------------------------------------------------------------------------

#[tauri::command]
fn library_search_start(
    app: AppHandle<Wry>,
    state: State<'_, AppState>,
    options: SearchOptions,
) -> Result<(), String> {
    search::start(app, &state, options)
}

#[tauri::command]
fn library_search_cancel(state: State<'_, AppState>) -> Result<Value, String> {
    // bump generation: in-flight searches observe the mismatch and stop
    state.search_gen.fetch_add(1, Ordering::SeqCst);
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Menus (must stay sync commands: muda popups run on the main thread)
// ---------------------------------------------------------------------------

#[tauri::command]
fn show_editor_context_menu(app: AppHandle<Wry>) -> Result<(), String> {
    crate::menus::show_editor_context_menu(&app).map_err(|e| e.to_string())
}

#[tauri::command]
fn show_main_menu(app: AppHandle<Wry>, x: f64, y: f64) -> Result<(), String> {
    crate::menus::show_main_menu(&app, x, y).map_err(|e| e.to_string())
}

#[tauri::command]
fn show_tab_context_menu(app: AppHandle<Wry>, tab_path: String) -> Result<(), String> {
    crate::menus::show_tab_context_menu(&app, &tab_path).map_err(|e| e.to_string())
}

#[tauri::command]
fn show_buffer_tree_context_menu(app: AppHandle<Wry>, buffer_path: String) -> Result<(), String> {
    crate::menus::show_buffer_tree_context_menu(&app, &buffer_path).map_err(|e| e.to_string())
}

#[tauri::command]
fn show_buffer_tree_directory_context_menu(
    app: AppHandle<Wry>,
    directory_path: String,
) -> Result<(), String> {
    let is_empty = {
        let state = app.state::<AppState>();
        let guard = state.library.lock().unwrap();
        guard
            .as_ref()
            .map(|lib| lib.is_directory_empty(&directory_path).unwrap_or(false))
            .unwrap_or(false)
    };
    crate::menus::show_buffer_tree_directory_context_menu(&app, &directory_path, is_empty)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn show_buffer_tree_background_context_menu(app: AppHandle<Wry>) -> Result<(), String> {
    crate::menus::show_buffer_tree_background_context_menu(&app).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Window events (wired from lib.rs)
// ---------------------------------------------------------------------------

pub fn handle_window_event(window: &tauri::Window<Wry>, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    let app = window.app_handle();
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            let state = app.state::<AppState>();
            let show_in_menu = state
                .config
                .lock()
                .unwrap()
                .settings()
                .get("showInMenu")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let force_quit = state.force_quit.load(Ordering::SeqCst);

            if !force_quit && show_in_menu {
                // menu-bar mode: hide instead of closing
                api.prevent_close();
                let _ = window.hide();
                #[cfg(windows)]
                {
                    let _ = window.set_skip_taskbar(true);
                }
                return;
            }

            window_ctl::persist_window_config(app, window);

            let has_library = state.library.lock().unwrap().is_some();
            if has_library && !state.content_saved.load(Ordering::SeqCst) {
                // Prevent closing and let the renderer flush all open buffers
                // (it will call buffer_save_and_quit when done).
                api.prevent_close();
                let _ = app.emit("window-close", Value::Null);
            }
        }
        WindowEvent::Focused(focused) => {
            let _ = app.emit("window-focus-state", focused);
            if *focused {
                // Make the webview the first responder on (re)activation so
                // the first click/keystroke isn't swallowed by the OS layer
                // (WKWebView can sit unfocused inside a focused window after
                // dialogs/IME interactions).
                if let Some(webview) = app.get_webview("main") {
                    let _ = webview.set_focus();
                }
                // fs.watch is unreliable in some setups (e.g. OneDrive), so
                // reload open buffers when the window regains focus
                watcher::reload_all_open(app);
                window_ctl::poll_fullscreen_state(app);
            }
        }
        WindowEvent::Resized(_) => {
            window_ctl::poll_fullscreen_state(app);
        }
        _ => {}
    }
}

pub fn invoke_handler() -> Box<dyn Fn(tauri::ipc::Invoke<Wry>) -> bool + Send + Sync + 'static> {
    Box::new(tauri::generate_handler![
        get_initial_state,
        log_frontend,
        focus_webview,
        get_version,
        get_init_errors,
        get_system_locale,
        set_window_title,
        open_external_url,
        get_about_data,
        set_settings,
        dark_mode_set,
        dark_mode_get,
        buffer_exists,
        buffer_get_list,
        buffer_get_directory_list,
        buffer_load,
        buffer_save,
        buffer_delete,
        buffer_is_directory_empty,
        buffer_delete_directory,
        buffer_move,
        buffer_create,
        buffer_create_directory,
        buffer_close,
        buffer_save_and_quit,
        library_save_image,
        library_select_location,
        save_tabs_state,
        load_tabs_state,
        currency_get,
        library_search_start,
        library_search_cancel,
        show_editor_context_menu,
        show_main_menu,
        show_tab_context_menu,
        show_buffer_tree_context_menu,
        show_buffer_tree_directory_context_menu,
        show_buffer_tree_background_context_menu,
    ])
}
