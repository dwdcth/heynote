// Port of electron/main/menu.js: editor/tab/buffer-tree context menus, the
// popup main menu, the tray menu, and the macOS app menu.
//
// Menu items get sequential string ids ("m<n>") registered in
// AppState::menu_registry; the single app-level on_menu_event handler (wired in
// lib.rs) looks the id up and performs the action. Predefined items
// (cut/copy/paste) are handled natively by the webview.

use crate::state::{AppState, MenuAction};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tauri::menu::{ContextMenu, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_opener::OpenerExt;

pub const SCRATCH_FILE_NAME: &str = "scratch.txt";

fn get_parent_directory(path: &str) -> String {
    match path.rfind(['/', '\\']) {
        Some(idx) => path[..idx].to_string(),
        None => String::new(),
    }
}

fn register_action(app: &AppHandle<Wry>, action: MenuAction) -> String {
    let state = app.state::<AppState>();
    let n = state.next_menu_id.fetch_add(1, Ordering::Relaxed);
    state.menu_registry.lock().unwrap().insert(n, action);
    format!("m{}", n)
}

fn emit_item(app: &AppHandle<Wry>, label: &str, channel: &str, payload: Option<Value>) -> MenuItem<Wry> {
    let id = register_action(app, MenuAction::Emit {
        channel: channel.to_string(),
        payload,
    });
    MenuItem::with_id(app, id, label, true, None::<&str>).expect("failed to create menu item")
}

fn emit_item_accel(app: &AppHandle<Wry>, label: &str, accelerator: &str, channel: &str) -> MenuItem<Wry> {
    let id = register_action(app, MenuAction::Emit {
        channel: channel.to_string(),
        payload: None,
    });
    MenuItem::with_id(app, id, label, true, Some(accelerator)).expect("failed to create menu item")
}

fn url_item(app: &AppHandle<Wry>, label: &str, url: &str) -> MenuItem<Wry> {
    let id = register_action(app, MenuAction::OpenUrl(url.to_string()));
    MenuItem::with_id(app, id, label, true, None::<&str>).expect("failed to create menu item")
}

fn quit_item(app: &AppHandle<Wry>) -> MenuItem<Wry> {
    let id = register_action(app, MenuAction::Quit);
    MenuItem::with_id(app, id, "Quit", true, None::<&str>).expect("failed to create menu item")
}

fn action_item(app: &AppHandle<Wry>, label: &str, action: MenuAction) -> MenuItem<Wry> {
    let id = register_action(app, action);
    MenuItem::with_id(app, id, label, true, None::<&str>).expect("failed to create menu item")
}

fn separator(app: &AppHandle<Wry>) -> PredefinedMenuItem<Wry> {
    PredefinedMenuItem::separator(app).expect("failed to create separator")
}

/// The shared editor-ish items (subset of the Electron template).
struct EditItems {
    undo: MenuItem<Wry>,
    redo: MenuItem<Wry>,
    delete_block: MenuItem<Wry>,
    move_block: MenuItem<Wry>,
    select_all: MenuItem<Wry>,
}

fn edit_items(app: &AppHandle<Wry>) -> EditItems {
    EditItems {
        undo: emit_item_accel(app, "Undo", "CommandOrControl+z", "undo"),
        redo: emit_item_accel(app, "Redo", "CommandOrControl+Shift+z", "redo"),
        delete_block: emit_item_accel(app, "Delete block", "CommandOrControl+Shift+D", "delete-block"),
        move_block: emit_item_accel(app, "Move block to another buffer…", "CommandOrControl+S", "move-block"),
        select_all: emit_item_accel(app, "Select All", "CommandOrControl+a", "select-all"),
    }
}

fn emit_to_main(app: &AppHandle<Wry>, channel: &str, payload: Option<Value>) {
    // The bridge only listens in the main window; broadcasting is fine.
    match payload {
        Some(p) => {
            let _ = app.emit(channel, p);
        }
        None => {
            // emitting `null` — the bridge converts null to a missing positional arg
            let _ = app.emit(channel, Value::Null);
        }
    }
}


/// After a native menu closes, focus can be left on the menu layer; the
/// first webview click would then be consumed just to restore focus. Give
/// the webview its first-responder status back immediately.
fn refocus_webview(app: &AppHandle<Wry>) {
    if let Some(webview) = app.get_webview("main") {
        let _ = webview.set_focus();
    }
}

/// Dispatch a menu event id (called from the app-level on_menu_event in lib.rs).
pub fn handle_menu_event(app: &AppHandle<Wry>, id: &str) {
    let Some(num) = id.strip_prefix('m').and_then(|s| s.parse::<u32>().ok()) else {
        return;
    };
    // lookup without removing: persistent menus (app menu, tray) must keep
    // responding; per-popup menus simply leak a few entries, which is fine.
    let action = app
        .state::<AppState>()
        .menu_registry
        .lock()
        .unwrap()
        .get(&num)
        .cloned();
    match action {
        Some(MenuAction::Emit { channel, payload }) => emit_to_main(app, &channel, payload),
        Some(MenuAction::OpenUrl(url)) => {
            let _ = app.opener().open_url(url, None::<&str>);
        }
        Some(MenuAction::ShowWindow) => crate::window_ctl::show_window(app),
        Some(MenuAction::ToggleFullscreen) => {
            if let Some(window) = app.get_webview_window("main") {
                let fullscreen = window.is_fullscreen().unwrap_or(false);
                let _ = window.set_fullscreen(!fullscreen);
            }
        }
        Some(MenuAction::ShowAbout) => {
            let _ = crate::about::open_about_window(app);
        }
        Some(MenuAction::Quit) => crate::window_ctl::quit_app(app),
        None => {}
    }
}

// ---------------------------------------------------------------------------
// Context menus
// ---------------------------------------------------------------------------

pub fn show_editor_context_menu(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let edit = edit_items(app);
    let cut = PredefinedMenuItem::cut(app, None)?;
    let copy = PredefinedMenuItem::copy(app, None)?;
    let paste = PredefinedMenuItem::paste(app, None)?;
    let sep1 = separator(app);
    let sep2 = separator(app);
    let sep3 = separator(app);
    let menu = Menu::with_items(
        app,
        &[
            &edit.undo,
            &edit.redo,
            &sep1,
            &cut,
            &copy,
            &paste,
            &sep2,
            &edit.select_all,
            &sep3,
            &edit.delete_block,
            &edit.move_block,
        ],
    )?;
    if let Some(window) = app.get_window("main") {
        menu.popup(window)?;
        refocus_webview(app);
    }
    Ok(())
}

pub fn show_main_menu(app: &AppHandle<Wry>, x: f64, y: f64) -> tauri::Result<()> {
    let file_menu = build_file_submenu(app)?;
    let edit_menu = build_edit_submenu(app)?;
    let window_menu = build_window_submenu(app)?;
    let help_menu = build_help_submenu(app)?;
    let sep = separator(app);
    let quit = quit_item(app);
    let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &window_menu, &help_menu, &sep, &quit])?;
    if let Some(window) = app.get_window("main") {
        let scale = window.scale_factor().unwrap_or(1.0);
        let position = tauri::PhysicalPosition::new(
            (x * scale).round() as i32,
            (y * scale).round() as i32,
        );
        menu.popup_at(window, position)?;
        refocus_webview(app);
    }
    Ok(())
}

fn build_file_submenu(app: &AppHandle<Wry>) -> tauri::Result<Submenu<Wry>> {
    let change_buffer =
        emit_item_accel(app, "Switch buffer…", "CommandOrControl+P", "change-buffer");
    let settings = emit_item(app, "Settings", "open-settings", None);
    let about = action_item(app, "About", MenuAction::ShowAbout);
    Submenu::with_items(app, "File", true, &[&change_buffer, &settings, &about])
}

fn build_edit_submenu(app: &AppHandle<Wry>) -> tauri::Result<Submenu<Wry>> {
    let edit = edit_items(app);
    let cut = PredefinedMenuItem::cut(app, None)?;
    let copy = PredefinedMenuItem::copy(app, None)?;
    let paste = PredefinedMenuItem::paste(app, None)?;
    let sep1 = separator(app);
    let sep2 = separator(app);
    Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &edit.undo,
            &edit.redo,
            &sep1,
            &edit.delete_block,
            &edit.move_block,
            &sep2,
            &cut,
            &copy,
            &paste,
            &edit.select_all,
        ],
    )
}

fn build_window_submenu(app: &AppHandle<Wry>) -> tauri::Result<Submenu<Wry>> {
    let minimize = PredefinedMenuItem::minimize(app, None)?;
    let fullscreen = action_item(app, "Toggle Full Screen", MenuAction::ToggleFullscreen);
    let close = PredefinedMenuItem::close_window(app, None)?;
    Submenu::with_items(app, "Window", true, &[&minimize, &fullscreen, &close])
}

fn build_help_submenu(app: &AppHandle<Wry>) -> tauri::Result<Submenu<Wry>> {
    let docs = url_item(app, "Documentation", "https://heynote.com/docs/");
    let website = url_item(app, "Website", "https://heynote.com");
    Submenu::with_items(app, "Help", true, &[&docs, &website])
}

pub fn show_tab_context_menu(app: &AppHandle<Wry>, tab_path: &str) -> tauri::Result<()> {
    let is_scratch = tab_path == SCRATCH_FILE_NAME;
    let mut items: Vec<Box<dyn tauri::menu::IsMenuItem<Wry>>> = Vec::new();

    if is_scratch {
        items.push(Box::new(emit_item(app, "Archive...", "tab:archiveScratch", None)));
    } else {
        items.push(Box::new(emit_item(app, "Edit Buffer…", "tab:editBuffer", Some(json!(tab_path)))));
        items.push(Box::new(emit_item(app, "Delete Buffer", "tab:deleteBuffer", Some(json!(tab_path)))));
    }
    // Note: "Open Buffer…" is sent with *no* payload (the frontend branches on
    // undefined vs. a parent directory string).
    items.push(Box::new(emit_item(app, "Open Buffer…", "tab:openNew", Some(Value::Null))));
    items.push(Box::new(emit_item(
        app,
        "New Buffer…",
        "tab:openNew",
        Some(json!(get_parent_directory(tab_path))),
    )));
    items.push(Box::new(separator(app)));
    items.push(Box::new(emit_item(app, "Close Tab", "tab:close", Some(json!(tab_path)))));

    let menu = build_menu_from_boxed(app, &items)?;
    if let Some(window) = app.get_window("main") {
        menu.popup(window)?;
        refocus_webview(app);
    }
    Ok(())
}

fn build_menu_from_boxed(
    app: &AppHandle<Wry>,
    items: &[Box<dyn tauri::menu::IsMenuItem<Wry>>],
) -> tauri::Result<Menu<Wry>> {
    let refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = items.iter().map(|b| b.as_ref()).collect();
    Menu::with_items(app, &refs)
}

pub fn show_buffer_tree_context_menu(app: &AppHandle<Wry>, buffer_path: &str) -> tauri::Result<()> {
    let is_scratch = buffer_path == SCRATCH_FILE_NAME;
    let parent_directory = get_parent_directory(buffer_path);
    let mut items: Vec<Box<dyn tauri::menu::IsMenuItem<Wry>>> = Vec::new();

    if is_scratch {
        items.push(Box::new(emit_item(app, "Archive...", "tab:archiveScratch", None)));
    } else {
        items.push(Box::new(emit_item(
            app,
            "Edit Buffer…",
            "tab:editBuffer",
            Some(json!(buffer_path)),
        )));
        items.push(Box::new(emit_item(
            app,
            "Delete Buffer",
            "tab:deleteBuffer",
            Some(json!(buffer_path)),
        )));
    }
    items.push(Box::new(separator(app)));
    items.push(Box::new(emit_item(app, "New Buffer…", "tab:openNew", Some(json!(parent_directory)))));
    items.push(Box::new(emit_item(
        app,
        "New Folder…",
        "bufferTree:createFolder",
        Some(json!(parent_directory)),
    )));

    let menu = build_menu_from_boxed(app, &items)?;
    if let Some(window) = app.get_window("main") {
        menu.popup(window)?;
        refocus_webview(app);
    }
    Ok(())
}

pub fn show_buffer_tree_directory_context_menu(
    app: &AppHandle<Wry>,
    directory_path: &str,
    is_empty_directory: bool,
) -> tauri::Result<()> {
    let new_buffer = emit_item(
        app,
        "New Buffer…",
        "tab:openNew",
        Some(json!(directory_path)),
    );
    let new_folder = emit_item(
        app,
        "New Folder…",
        "bufferTree:createFolder",
        Some(json!(directory_path)),
    );
    let delete_folder = MenuItem::with_id(
        app,
        register_action(
            app,
            MenuAction::Emit {
                channel: "bufferTree:deleteDirectory".to_string(),
                payload: Some(json!(directory_path)),
            },
        ),
        "Delete Folder",
        is_empty_directory,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&new_buffer, &new_folder, &delete_folder])?;
    if let Some(window) = app.get_window("main") {
        menu.popup(window)?;
        refocus_webview(app);
    }
    Ok(())
}

pub fn show_buffer_tree_background_context_menu(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let new_buffer = emit_item(app, "New Buffer…", "tab:openNew", Some(json!("")));
    let new_folder = emit_item(app, "New Folder…", "bufferTree:createFolder", Some(json!("")));
    let menu = Menu::with_items(app, &[&new_buffer, &new_folder])?;
    if let Some(window) = app.get_window("main") {
        menu.popup(window)?;
        refocus_webview(app);
    }
    Ok(())
}

/// Tray menu: "Open Heynote" + the app menu sections + Quit
/// (port of getTrayMenu: template + Quit with an Open item prepended).
pub fn build_tray_menu(app: &AppHandle<Wry>) -> tauri::Result<Menu<Wry>> {
    let open = action_item(app, "Open Heynote", MenuAction::ShowWindow);
    let sep0 = separator(app);
    let file_menu = build_file_submenu(app)?;
    let edit_menu = build_edit_submenu(app)?;
    let help_menu = build_help_submenu(app)?;
    let sep = separator(app);
    let quit = quit_item(app);
    Menu::with_items(
        app,
        &[&open, &sep0, &file_menu, &edit_menu, &help_menu, &sep, &quit],
    )
}

/// macOS application menu (menubar). Linux/Windows use the popup main menu
/// from the tab bar button instead. The first submenu acts as the app menu.
pub fn set_mac_app_menu(app: &AppHandle<Wry>) -> tauri::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let app_submenu = build_mac_app_submenu(app)?;
    let edit_menu = build_edit_submenu(app)?;
    let window_menu = build_window_submenu(app)?;
    let help_menu = build_help_submenu(app)?;
    let menu = Menu::with_items(app, &[&app_submenu, &edit_menu, &window_menu, &help_menu])?;
    app.set_menu(menu)?;
    Ok(())
}

fn build_mac_app_submenu(app: &AppHandle<Wry>) -> tauri::Result<Submenu<Wry>> {
    let about = action_item(app, "About Heynote", MenuAction::ShowAbout);
    let sep1 = separator(app);
    let settings = emit_item(app, "Settings…", "open-settings", None);
    let sep2 = separator(app);
    let quit = {
        let id = register_action(app, MenuAction::Quit);
        MenuItem::with_id(app, id, "Quit Heynote", true, Some("CommandOrControl+q"))
            .expect("failed to create menu item")
    };
    Submenu::with_items(app, "Heynote", true, &[&about, &sep1, &settings, &sep2, &quit])
}
