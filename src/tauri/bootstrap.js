// Tauri bootstrap: initialize the window.heynote bridge (which requires a
// round-trip to the Rust backend for the initial state) *before* the app
// modules evaluate, then load the regular app entry.
// src/editor/keymap.js and src/editor/links.js read window.heynote.platform
// at module top-level, and the Pinia store factories read window.heynote.settings
// synchronously, so window.heynote must be fully populated before this import.

async function boot() {
    const { initBridge } = await import("./bridge.js")
    try {
        window.heynote = await initBridge()
    } catch (err) {
        console.error("Failed to initialize Tauri bridge:", err)
        const loading = document.getElementById("app-loading")
        if (loading) {
            loading.innerHTML =
                '<div style="font: 13px sans-serif; color: #888; text-align: center;">' +
                "Failed to initialize Heynote backend:<br>" + String(err) + "</div>"
        }
        return
    }
    await import("../main.js")
    const loading = document.getElementById("app-loading")
    if (loading) {
        loading.remove()
    }
}

boot()
