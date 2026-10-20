// About window (port of electron/main/about.js): a small fixed-size child
// window loading about-tauri.html, which pulls version/currency info via the
// get_about_data command.

use tauri::{AppHandle, Manager, Wry};

pub fn open_about_window(app: &AppHandle<Wry>) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window("about") {
        let _ = existing.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        app,
        "about",
        tauri::WebviewUrl::App("about-tauri.html".into()),
    )
    .title("About Heynote")
    .inner_size(400.0, 320.0)
    .resizable(false)
    .minimizable(false)
    .maximizable(false)
    .fullscreen(false)
    .build()?;
    Ok(())
}
