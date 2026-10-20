// Config persistence: a drop-in replacement for the `electron-store` config
// file used by the Electron build (config.json / config-dev.json in the
// Electron userData directory). Reads and writes the exact same file so both
// builds can operate on the same data.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub fn is_dev() -> bool {
    cfg!(debug_assertions)
}

/// Electron userData directory (derived from the "Heynote" app name):
/// Linux: ~/.config/Heynote, macOS: ~/Library/Application Support/Heynote,
/// Windows: %APPDATA%/Heynote
pub fn user_data_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    let base = dirs::config_dir().expect("could not determine user config dir");
    #[cfg(target_os = "macos")]
    let base = dirs::data_dir().expect("could not determine user data dir");
    #[cfg(windows)]
    let base = dirs::data_dir().expect("could not determine user data dir");
    base.join("Heynote")
}

pub fn config_file() -> PathBuf {
    user_data_dir().join(if is_dev() { "config-dev.json" } else { "config.json" })
}

pub const DEFAULT_LEFT_PANEL_WIDTH: i64 = 210;

/// Effective default settings, mirroring the merged schema defaults + defaults
/// object in electron/config.js.
fn default_settings() -> Value {
    let emacs_meta_key = if cfg!(target_os = "macos") { "meta" } else { "alt" };
    json!({
        "keymap": "default",
        "emacsMetaKey": emacs_meta_key,
        "keyBindings": [],
        "showLineNumberGutter": true,
        "showFoldGutter": true,
        "showTabs": true,
        "showTabsInFullscreen": true,
        "showLeftPanel": true,
        "leftPanelWidth": DEFAULT_LEFT_PANEL_WIDTH,
        "bufferTreeOpenFolders": [],
        "autoUpdate": true,
        "autoInstallUpdates": true,
        "allowBetaVersions": false,
        "enableGlobalHotkey": false,
        "globalHotkey": "CmdOrCtrl+Shift+H",
        "bufferPath": "",
        "showInDock": true,
        "showInMenu": false,
        "alwaysOnTop": false,
        "openAtLogin": false,
        "startHidden": false,
        "bracketClosing": false,
        "indentType": "space",
        "tabSize": 4,
        "spellcheckEnabled": false,
        "showWhitespace": false,
        "colorPreviewEnabled": true,
        "cursorBlinkRate": 1000,
        "searchSettings": {
            "onlyCurrentBlock": true,
            "caseSensitive": false,
            "wholeWord": false,
            "regexp": false,
        },
        "librarySearchSettings": {
            "caseSensitive": false,
            "wholeWord": false,
            "regexp": false,
        },
    })
}

/// Deep-assign `src` onto `target` (src values win; nested objects merged).
fn deep_assign(target: &mut Value, src: &Value) {
    if let (Value::Object(t), Value::Object(s)) = (target, src) {
        for (k, sv) in s {
            match t.get_mut(k) {
                Some(tv) if tv.is_object() && sv.is_object() => deep_assign(tv, sv),
                _ => {
                    t.insert(k.clone(), sv.clone());
                }
            }
        }
    }
}

/// Expand a leading `~` to the user's home directory (port of untildify()).
pub fn untildify(path: &str) -> PathBuf {
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// Write to a temp file first, then rename over the target, so a crash mid-write
/// can never corrupt user data.
pub fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp = dir.join(format!(".{}.tmp", file_name));
    fs::write(&tmp, contents).map_err(|e| format!("failed to write {}: {}", tmp.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path).map_err(|e| format!("failed to rename {} -> {}: {}", tmp.display(), path.display(), e))
}

/// Port of src/common/client-id.js: base62 of 16 random bytes, padded to 22 chars.
pub fn generate_client_id() -> String {
    use rand::RngCore;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut value = u128::from_be_bytes(bytes);
    let mut id = String::new();
    loop {
        id.insert(0, ALPHABET[(value % 62) as usize] as char);
        value /= 62;
        if value == 0 {
            break;
        }
    }
    while id.len() < 22 {
        id.insert(0, '0');
    }
    id
}

#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub width: i32,
    pub height: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub is_maximized: bool,
    pub is_fullscreen: bool,
    pub visible_on_quit: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig {
            width: 940,
            height: 720,
            x: None,
            y: None,
            is_maximized: false,
            is_fullscreen: false,
            visible_on_quit: true,
        }
    }
}

pub struct ConfigStore {
    path: PathBuf,
    root: Value,
}

impl ConfigStore {
    pub fn load() -> Result<Self, String> {
        let path = config_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {}", parent.display(), e))?;
        }
        let root: Value = if path.exists() {
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
            serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("Invalid JSON in {}, starting fresh: {}", path.display(), e);
                Value::Object(Map::new())
            })
        } else {
            Value::Object(Map::new())
        };
        let mut store = ConfigStore { path, root };
        store.ensure_client_id()?;
        Ok(store)
    }

    fn ensure_client_id(&mut self) -> Result<(), String> {
        let missing = self
            .root
            .get("clientId")
            .and_then(|v| v.as_str())
            .map_or(true, |s| s.is_empty());
        if missing {
            if let Some(obj) = self.root.as_object_mut() {
                obj.insert("clientId".to_string(), json!(generate_client_id()));
            }
            self.save()?;
        }
        Ok(())
    }

    pub fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&self.root)
            .map_err(|e| format!("failed to serialize config: {}", e))?;
        atomic_write(&self.path, &text)
    }

    /// Settings with defaults applied for missing keys (electron-store semantics).
    pub fn settings(&self) -> Value {
        let mut merged = default_settings();
        if let Some(stored) = self.root.get("settings") {
            deep_assign(&mut merged, stored);
        }
        merged
    }

    pub fn set_settings(&mut self, settings: Value) -> Result<(), String> {
        if let Some(obj) = self.root.as_object_mut() {
            obj.insert("settings".to_string(), settings);
        }
        self.save()
    }

    pub fn theme(&self) -> String {
        self.root
            .get("theme")
            .and_then(|v| v.as_str())
            .unwrap_or("system")
            .to_string()
    }

    pub fn set_theme(&mut self, theme: &str) -> Result<(), String> {
        if let Some(obj) = self.root.as_object_mut() {
            obj.insert("theme".to_string(), json!(theme));
        }
        self.save()
    }

    pub fn window_config(&self) -> WindowConfig {
        let mut config = WindowConfig::default();
        if let Some(wc) = self.root.get("windowConfig") {
            if let Some(v) = wc.get("width").and_then(|v| v.as_i64()) {
                config.width = v as i32;
            }
            if let Some(v) = wc.get("height").and_then(|v| v.as_i64()) {
                config.height = v as i32;
            }
            if let Some(v) = wc.get("x").and_then(|v| v.as_i64()) {
                config.x = Some(v as i32);
            }
            if let Some(v) = wc.get("y").and_then(|v| v.as_i64()) {
                config.y = Some(v as i32);
            }
            if let Some(v) = wc.get("isMaximized").and_then(|v| v.as_bool()) {
                config.is_maximized = v;
            }
            if let Some(v) = wc.get("isFullScreen").and_then(|v| v.as_bool()) {
                config.is_fullscreen = v;
            }
            if let Some(v) = wc.get("visibleOnQuit").and_then(|v| v.as_bool()) {
                config.visible_on_quit = v;
            }
        }
        config
    }

    pub fn set_window_config(&mut self, config: &WindowConfig) -> Result<(), String> {
        let mut wc = json!({
            "width": config.width,
            "height": config.height,
            "isMaximized": config.is_maximized,
            "isFullScreen": config.is_fullscreen,
            "visibleOnQuit": config.visible_on_quit,
        });
        if let Some(obj) = wc.as_object_mut() {
            if let Some(x) = config.x {
                obj.insert("x".to_string(), json!(x));
            }
            if let Some(y) = config.y {
                obj.insert("y".to_string(), json!(y));
            }
        }
        if let Some(obj) = self.root.as_object_mut() {
            obj.insert("windowConfig".to_string(), wc);
        }
        self.save()
    }

    pub fn open_tabs_state(&self) -> Option<Value> {
        self.root.get("openTabsState").cloned()
    }

    pub fn set_open_tabs_state(&mut self, state: Value) -> Result<(), String> {
        if let Some(obj) = self.root.as_object_mut() {
            obj.insert("openTabsState".to_string(), state);
        }
        self.save()
    }

    pub fn client_id(&self) -> String {
        self.root
            .get("clientId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    pub fn currency_cache(&self) -> Option<Value> {
        self.root.get("currency").cloned()
    }

    pub fn set_currency_cache(&mut self, data: Value, time_fetched: f64) -> Result<(), String> {
        if let Some(obj) = self.root.as_object_mut() {
            obj.insert(
                "currency".to_string(),
                json!({ "data": data, "timeFetched": time_fetched }),
            );
        }
        self.save()
    }
}
