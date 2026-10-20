// Application assembly: plugins, state, window, events.

mod about;
mod commands;
mod config;
mod library;
mod menus;
mod protocol;
mod search;
mod state;
mod watcher;
mod window_ctl;

use state::AppState;
use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

pub fn run() {
    tauri::Builder::default()
        // single-instance must be the first registered plugin
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            window_ctl::show_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        window_ctl::hotkey_toggle(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .register_uri_scheme_protocol("heynote-file", protocol::handle)
        .setup(|app| {
            let config_store =
                config::ConfigStore::load().map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(AppState::new(config_store));

            window_ctl::set_app_handle(app.handle().clone());
            commands::init_library(app.handle())
                .map_err(|e| Box::<dyn std::error::Error>::from(e))?;
            window_ctl::create_main_window(app.handle())?;
            menus::set_mac_app_menu(app.handle())?;
            window_ctl::register_global_hotkey(app.handle());
            window_ctl::register_show_in_dock(app.handle());
            window_ctl::register_show_in_menu(app.handle());
            window_ctl::register_always_on_top(app.handle());
            window_ctl::register_autostart(app.handle());
            Ok(())
        })
        .on_menu_event(|app, event| {
            menus::handle_menu_event(app, event.id().as_ref());
        })
        .on_window_event(commands::handle_window_event)
        .invoke_handler(commands::invoke_handler())
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {});
}
