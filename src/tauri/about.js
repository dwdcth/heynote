import { invoke } from "@tauri-apps/api/core"

// The Electron version receives {version} via an ipc "init" event from the
// main process; here we pull the same data from the backend on load.

async function init() {
    try {
        const data = await invoke("get_about_data")
        document.getElementById("version").textContent = data.version ?? "?"
        if (data.currencyTimestamp) {
            document.getElementById("currencyTimestamp").textContent =
                new Date(data.currencyTimestamp).toLocaleString()
        }
    } catch (err) {
        console.error("failed to load about data:", err)
    }
}

init()
