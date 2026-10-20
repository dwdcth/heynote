// Port of the window/tray/hotkey/lifecycle logic from electron/main/index.ts.

use crate::config::WindowConfig;
use crate::state::AppState;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use tauri::tray::TrayIconBuilder;
use tauri::webview::{WebviewWindow, WebviewWindowBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry, WebviewUrl};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};
use tauri_plugin_opener::OpenerExt;

/// The on_navigation callback has no context parameter, so keep a global
/// handle to reach the opener plugin.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn set_app_handle(app: AppHandle) {
    let _ = APP_HANDLE.set(app);
}

pub fn is_dev() -> bool {
    crate::config::is_dev()
}

// ---------------------------------------------------------------------------
// Accelerator parsing (Electron-style, e.g. "CmdOrCtrl+Shift+H")
// ---------------------------------------------------------------------------

fn parse_accelerator(accelerator: &str) -> Option<Shortcut> {
    let mut modifiers = Modifiers::empty();
    let mut code: Option<Code> = None;

    let is_mac = cfg!(target_os = "macos");
    for part in accelerator.split('+') {
        let key = part.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let modifier = match key.as_str() {
            "cmdorctrl" | "commandorcontrol" => {
                Some(if is_mac { Modifiers::META } else { Modifiers::CONTROL })
            }
            "ctrl" | "control" => Some(Modifiers::CONTROL),
            "shift" => Some(Modifiers::SHIFT),
            "alt" | "option" => Some(Modifiers::ALT),
            "cmd" | "command" | "meta" | "super" => Some(Modifiers::META),
            _ => None,
        };
        if let Some(m) = modifier {
            modifiers |= m;
            continue;
        }
        if code.is_some() {
            return None; // more than one non-modifier key
        }
        code = Some(parse_key_code(&key)?);
    }
    code.map(|c| Shortcut::new(Some(modifiers), c))
}

fn parse_key_code(key: &str) -> Option<Code> {
    use Code::*;
    Some(match key {
        "space" => Space,
        "tab" => Tab,
        "enter" | "return" => Enter,
        "backspace" => Backspace,
        "delete" => Delete,
        "insert" => Insert,
        "home" => Home,
        "end" => End,
        "pageup" => PageUp,
        "pagedown" => PageDown,
        "escape" => Escape,
        "up" | "arrowup" => ArrowUp,
        "down" | "arrowdown" => ArrowDown,
        "left" | "arrowleft" => ArrowLeft,
        "right" | "arrowright" => ArrowRight,
        _ => {
            if let Some(letter) = key.chars().next() {
                if key.len() == 1 && letter.is_ascii_alphabetic() {
                    return Some(match letter.to_ascii_uppercase() {
                        'A' => KeyA, 'B' => KeyB, 'C' => KeyC, 'D' => KeyD, 'E' => KeyE,
                        'F' => KeyF, 'G' => KeyG, 'H' => KeyH, 'I' => KeyI, 'J' => KeyJ,
                        'K' => KeyK, 'L' => KeyL, 'M' => KeyM, 'N' => KeyN, 'O' => KeyO,
                        'P' => KeyP, 'Q' => KeyQ, 'R' => KeyR, 'S' => KeyS, 'T' => KeyT,
                        'U' => KeyU, 'V' => KeyV, 'W' => KeyW, 'X' => KeyX, 'Y' => KeyY,
                        'Z' => KeyZ,
                        _ => return None,
                    });
                }
            }
            if let Some(num) = key.strip_prefix('f') {
                if let Ok(n) = num.parse::<u8>() {
                    if (1..=12).contains(&n) {
                        return Some([
                            F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
                        ][(n - 1) as usize].clone());
                    }
                }
            }
            if key.len() == 1 {
                if let Some(digit) = key.chars().next().and_then(|c| c.to_digit(10)) {
                    return Some([
                        Digit0, Digit1, Digit2, Digit3, Digit4,
                        Digit5, Digit6, Digit7, Digit8, Digit9,
                    ][digit as usize].clone());
                }
            }
            return None;
        }
    })
}

// ---------------------------------------------------------------------------
// Window creation
// ---------------------------------------------------------------------------

fn theme_option(theme: &str) -> Option<tauri::Theme> {
    match theme {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None, // "system"
    }
}

/// Port of getVisibleWindowBounds/ensureWindowBoundsVisible (window-bounds.ts):
/// if the window doesn't intersect any monitor, clamp it onto the nearest one.
fn clamp_bounds_to_monitors(app: &AppHandle, config: &mut WindowConfig) {
    let Ok(monitors) = app.available_monitors() else { return };
    if monitors.is_empty() {
        return;
    }
    let (Some(x), Some(y)) = (config.x, config.y) else { return };
    let w = config.width;
    let h = config.height;

    let intersects = |mx: i32, my: i32, mw: i32, mh: i32| {
        x < mx + mw && mx < x + w && y < my + mh && my < y + h
    };
    if monitors.iter().any(|m| {
        let size = m.size();
        let pos = m.position();
        intersects(pos.x, pos.y, size.width as i32, size.height as i32)
    }) {
        return;
    }
    // pick the monitor whose top-left is closest to the window position
    let nearest = monitors
        .iter()
        .min_by_key(|m| {
            let pos = m.position();
            let dx = (pos.x - x).abs();
            let dy = (pos.y - y).abs();
            dx + dy
        })
        .expect("monitors is non-empty");
    let pos = nearest.position();
    let size = nearest.size();
    let mw = size.width as i32;
    let mh = size.height as i32;
    let new_x = x.clamp(pos.x, (pos.x + mw - w).max(pos.x));
    let new_y = y.clamp(pos.y, (pos.y + mh - h).max(pos.y));
    config.x = Some(new_x);
    config.y = Some(new_y);
}

fn is_external_url(url: &tauri::Url) -> bool {
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    if let Some(host) = url.host_str() {
        // the dev server is internal
        !(host == "localhost" || host == "127.0.0.1" || host.ends_with(".localhost"))
    } else {
        true
    }
}

pub fn create_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow<Wry>> {
    let state = app.state::<AppState>();
    let (mut window_config, _settings, theme, start_hidden) = {
        let config = state.config.lock().unwrap();
        let settings = config.settings();
        let start_hidden = settings
            .get("startHidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        (config.window_config(), settings, config.theme(), start_hidden)
    };
    clamp_bounds_to_monitors(app, &mut window_config);
    let hide_on_startup = start_hidden;

    let dev = is_dev();
    let page = if dev { "index-tauri.html?dev=1" } else { "index-tauri.html" };

    let mut builder = WebviewWindowBuilder::new(
        app,
        "main",
        WebviewUrl::App(page.into()),
    )
    .title("")
    // On macOS a click on an inactive window is consumed just to activate it
    // (and WKWebView can drop to that state after IME/dialog interactions) —
    // acceptFirstMouse delivers the click through instead, fixing the
    // "have to click twice" behavior.
    .accept_first_mouse(true)
    // Disable Tauri's native drag-drop handler: on WKWebView it intercepts
    // the frontend's HTML5 drag events (tab reordering via Sortable, image
    // drag&drop) and swallows the whole drag session.
    .disable_drag_drop_handler()
    .inner_size(window_config.width as f64, window_config.height as f64)
    .visible(!hide_on_startup)
    .theme(theme_option(&theme))
    .on_navigation(move |url| {
        if is_external_url(url) {
            // open external links in the system browser; never navigate the app webview
            if let Some(app) = APP_HANDLE.get() {
                let _ = app.opener().open_url(url.to_string(), None::<&str>);
            }
            false
        } else {
            true
        }
    })
    .on_page_load(|webview, payload| {
        if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
            let app = webview.app_handle();
            if webview.label() == "main" {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = app.emit(
                        "window-fullscreen-state",
                        window.is_fullscreen().unwrap_or(false),
                    );
                    let _ = app.emit(
                        "window-focus-state",
                        window.is_focused().unwrap_or(false),
                    );
                }
            }
        }
    });

    // Windows/Linux: fully undecorated; the tab bar reserves 140px on the
    // right for our custom min/max/close buttons (WindowControls.vue).
    // macOS keeps native decorations with an overlay title bar (traffic
    // lights + rounded corners preserved, content under the bar — the
    // equivalent of Electron's titleBarStyle: "hidden").
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false);
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay);
    }

    if let (Some(x), Some(y)) = (window_config.x, window_config.y) {
        builder = builder.position(x as f64, y as f64);
    }
    if !hide_on_startup {
        if window_config.is_maximized {
            builder = builder.maximized(true);
        }
        if window_config.is_fullscreen {
            builder = builder.fullscreen(true);
        }
    }

    let window = builder.build()?;

    if hide_on_startup {
        // cannot create the window fullscreen+hidden at once; restore state on
        // first show instead (same workaround as the Electron version)
        if window_config.is_maximized {
            let _ = window.maximize();
        }
        if window_config.is_fullscreen {
            let _ = window.set_fullscreen(true);
        }
    }
    Ok(window)
}

// ---------------------------------------------------------------------------
// Show / hide / focus
// ---------------------------------------------------------------------------

/// Port of showWindow(): restore, ensure visible, raise (Linux needs the
/// hide+show dance on some WMs), focus, and tell the renderer to focus the
/// editor when the window was hidden.
pub fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let was_visible = window.is_visible().unwrap_or(false);
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    if !was_visible && cfg!(target_os = "linux") {
        // forces the window to the top of the stack on Linux WMs that don't
        // raise windows on a bare show() call
        let _ = window.hide();
    }
    let _ = window.show();
    let _ = window.set_focus();
    if !was_visible {
        let _ = app.emit("focus-editor", Value::Null);
    }
}

/// Global hotkey toggle (port of the registerGlobalHotkey callback).
pub fn hotkey_toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    if window.is_focused().unwrap_or(false) {
        // Note: Electron uses app.hide() on macOS so focus returns to the
        // previously active app; tauri has no equivalent. Toggling the
        // activation policy (Accessory) as an approximation turned out to
        // leave the app without a Dock icon, so we use a plain window hide.
        let _ = window.hide();
    } else {
        show_window(app);
    }
}

pub fn poll_fullscreen_state(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    let state = app.state::<AppState>();
    let mut cached = state.fullscreen_cached.lock().unwrap();
    if *cached != fullscreen {
        *cached = fullscreen;
        drop(cached);
        let _ = app.emit("window-fullscreen-state", fullscreen);
    }
}

/// Persist window bounds/state to config (port of the "close" handler part).
pub fn persist_window_config(app: &AppHandle, window: &tauri::Window<Wry>) {
    let is_maximized = window.is_maximized().unwrap_or(false);
    let is_fullscreen = window.is_fullscreen().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(false);

    // Approximation of getNormalBounds(): when maximized/fullscreen the current
    // outer bounds are the maximized ones; keep the configured size in that case.
    let (x, y, w, h) = if is_maximized || is_fullscreen {
        let state = app.state::<AppState>();
        let config = state.config.lock().unwrap();
        let wc = config.window_config();
        (wc.x, wc.y, wc.width, wc.height)
    } else {
        let pos = window.outer_position().ok();
        let size = window.outer_size().ok();
        match (pos, size) {
            (Some(p), Some(s)) => (
                Some((p.x as f64 / window.scale_factor().unwrap_or(1.0)).round() as i32),
                Some((p.y as f64 / window.scale_factor().unwrap_or(1.0)).round() as i32),
                (s.width as f64 / window.scale_factor().unwrap_or(1.0)).round() as i32,
                (s.height as f64 / window.scale_factor().unwrap_or(1.0)).round() as i32,
            ),
            _ => {
                let state = app.state::<AppState>();
                let config = state.config.lock().unwrap();
                let wc = config.window_config();
                (wc.x, wc.y, wc.width, wc.height)
            }
        }
    };

    let window_config = WindowConfig {
        width: w,
        height: h,
        x,
        y,
        is_maximized,
        is_fullscreen,
        visible_on_quit: visible,
    };
    let state = app.state::<AppState>();
    let mut config = state.config.lock().unwrap();
    if let Err(e) = config.set_window_config(&window_config) {
        eprintln!("failed to persist window config: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Registrations (hotkey / tray / always-on-top / autostart / dock)
// ---------------------------------------------------------------------------

pub fn register_global_hotkey(app: &AppHandle) {
    let global_shortcut = app.global_shortcut();
    let _ = global_shortcut.unregister_all();
    let state = app.state::<AppState>();
    let settings = state.config.lock().unwrap().settings();
    let enabled = settings
        .get("enableGlobalHotkey")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let accelerator = settings
        .get("globalHotkey")
        .and_then(|v| v.as_str())
        .unwrap_or("CmdOrCtrl+Shift+H")
        .to_string();
    match parse_accelerator(&accelerator) {
        Some(shortcut) => {
            if let Err(e) = global_shortcut.register(shortcut) {
                eprintln!("Could not register global hotkey {:?}: {}", accelerator, e);
            }
        }
        None => eprintln!("Could not parse global hotkey: {:?}", accelerator),
    }
}

pub fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = crate::menus::build_tray_menu(app)?;
    let mut builder = TrayIconBuilder::with_id("heynote-tray")
        .menu(&menu)
        .tooltip("Heynote");

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    if cfg!(target_os = "macos") {
        builder = builder.icon_as_template(true);
    }

    if cfg!(target_os = "linux") {
        // Linux tray implementations don't reliably emit right-click events, so
        // the menu is shown on any click (equivalent to Electron's setContextMenu).
        builder = builder.show_menu_on_left_click(true);
    } else {
        // macOS/Windows: left click toggles the window, right click shows the menu.
        builder = builder
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                if let tauri::tray::TrayIconEvent::Click {
                    button: tauri::tray::MouseButton::Left,
                    button_state: tauri::tray::MouseButtonState::Up,
                    ..
                } = event
                {
                    let app = tray.app_handle();
                    if let Some(window) = app.get_webview_window("main") {
                        if window.is_visible().unwrap_or(false) {
                            let _ = window.hide();
                        } else {
                            show_window(app);
                        }
                    }
                }
            });
    }

    builder.build(app)?;
    Ok(())
}

pub fn destroy_tray(app: &AppHandle) {
    app.remove_tray_by_id("heynote-tray");
}

pub fn register_show_in_menu(app: &AppHandle) {
    let state = app.state::<AppState>();
    let show_in_menu = state
        .config
        .lock()
        .unwrap()
        .settings()
        .get("showInMenu")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if show_in_menu {
        if let Err(e) = create_tray(app) {
            eprintln!("failed to create tray: {}", e);
        }
    } else {
        destroy_tray(app);
    }
}

pub fn register_show_in_dock(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let state = app.state::<AppState>();
        let show = state
            .config
            .lock()
            .unwrap()
            .settings()
            .get("showInDock")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let _ = app.set_activation_policy(if show {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

pub fn register_always_on_top(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let state = app.state::<AppState>();
    let enabled = state
        .config
        .lock()
        .unwrap()
        .settings()
        .get("alwaysOnTop")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let _ = window.set_always_on_top(enabled);
    let _ = window.set_visible_on_all_workspaces(enabled);
}

pub fn register_autostart(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let state = app.state::<AppState>();
    let enabled = state
        .config
        .lock()
        .unwrap()
        .settings()
        .get("openAtLogin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let autolaunch = app.autolaunch();
    let result = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    if let Err(e) = result {
        eprintln!("failed to update autostart (enabled={}): {}", enabled, e);
    }
}

// ---------------------------------------------------------------------------
// Quit
// ---------------------------------------------------------------------------

pub fn quit_app(app: &AppHandle) {
    // app.exit() bypasses the CloseRequested event where the window
    // config is normally persisted — save it here first.
    if let Some(window) = app.get_window("main") {
        persist_window_config(app, &window);
    }
    app.state::<AppState>().force_quit.store(true, Ordering::SeqCst);
    app.exit(0);
}
