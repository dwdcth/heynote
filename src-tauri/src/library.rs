// Port of electron/main/file-library.js: note file CRUD, metadata listing,
// image storage/GC, and the legacy buffer.txt -> library migration.
// All writes go through config::atomic_write (temp file + rename).

use crate::config;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

pub const SCRATCH_FILE_NAME: &str = "scratch.txt";

pub fn notes_dir_name() -> &'static str {
    if config::is_dev() {
        "notes-dev"
    } else {
        "notes"
    }
}

fn ext_for_mime(mime: &str) -> Option<&'static str> {
    // mirrors the mime-types extension() lookup for IMAGE_MIME_TYPES
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/apng" => "apng",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => return None,
    })
}

/// Convert a frontend (posix-style) relative path to an OS path.
fn rel_to_os(rel: &str) -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(rel.replace('/', "\\"));
    #[cfg(not(windows))]
    PathBuf::from(rel)
}

/// Convert an OS path under `base` to a frontend (posix-style) relative path.
fn os_to_rel(path: &Path, base: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

fn initial_content(dev: bool) -> String {
    #[cfg(target_os = "linux")]
    let platform = "linux";
    #[cfg(target_os = "macos")]
    let platform = "darwin";
    #[cfg(windows)]
    let platform = "win32";
    let template = match (platform, dev) {
        ("linux", false) => include_str!("../assets/initial-content.linux.prod.txt"),
        ("linux", true) => include_str!("../assets/initial-content.linux.dev.txt"),
        ("darwin", false) => include_str!("../assets/initial-content.darwin.prod.txt"),
        ("darwin", true) => include_str!("../assets/initial-content.darwin.dev.txt"),
        ("win32", false) => include_str!("../assets/initial-content.win32.prod.txt"),
        ("win32", true) => include_str!("../assets/initial-content.win32.dev.txt"),
        _ => unreachable!(),
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    template.replace("@@CREATED@@", &now)
}

/// Port of readNoteMetadata(): read the first ~4000 bytes and parse the JSON
/// metadata head before the first "\n∞∞∞" separator.
/// Returns None if there is no separator, an empty object on parse error.
fn read_note_metadata(path: &Path) -> Option<Value> {
    let mut file = fs::File::open(path).ok()?;
    let mut bytes = vec![0u8; 4001];
    let n = file.read(&mut bytes).ok()?;
    bytes.truncate(n);
    let head = String::from_utf8_lossy(&bytes);
    let first_separator = head.find("\n∞∞∞")?;
    let metadata_str = head[..first_separator].trim();
    let parsed: Value = match serde_json::from_str(metadata_str) {
        Ok(v) => v,
        Err(_) => return Some(json!({})),
    };
    let mut obj = Map::new();
    if let Some(name) = parsed.get("name") {
        if !name.is_null() {
            obj.insert("name".to_string(), name.clone());
        }
    }
    if let Some(tags) = parsed.get("tags") {
        if !tags.is_null() {
            obj.insert("tags".to_string(), tags.clone());
        }
    }
    Some(Value::Object(obj))
}

/// Port of NoteFormat.load + serialize: ensure the note has a metadata head
/// with name "Scratch" (used by the legacy migration).
fn ensure_buffer_file_metadata(file_path: &Path) -> Result<(), String> {
    let data = fs::read_to_string(file_path)
        .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;
    let needs_fix = match data.find("\n∞∞∞") {
        None => true,
        Some(idx) => {
            let head = data[..idx].trim();
            match serde_json::from_str::<Value>(head) {
                Ok(v) => v.get("name").and_then(|n| n.as_str()).is_none(),
                Err(_) => true,
            }
        }
    };
    if !needs_fix {
        return Ok(());
    }
    let (mut metadata, content) = match data.find("\n∞∞∞") {
        Some(idx) => {
            let head = data[..idx].trim();
            let mut m = serde_json::from_str::<Value>(head).unwrap_or(json!({}));
            if !m.is_object() {
                m = json!({});
            }
            (m, data[idx..].to_string())
        }
        None => (json!({}), data),
    };
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert("name".to_string(), json!("Scratch"));
        obj.insert("formatVersion".to_string(), json!("2.0.0"));
    }
    let fixed = format!("{}{}", serde_json::to_string(&metadata).unwrap(), content);
    config::atomic_write(file_path, &fixed)
}

fn get_backup_file(file_path: &Path) -> Result<PathBuf, String> {
    let mut backup = PathBuf::from(format!("{}.bak", file_path.display()));
    for i in 1..1000 {
        if !backup.is_file() {
            return Ok(backup);
        }
        backup = PathBuf::from(format!("{}.bak.{}", file_path.display(), i));
    }
    Err(format!("Unable to find an available file path after 1000 attempts for base path: {}", file_path.display()))
}

pub struct OpenBuffer {
    pub full_path: PathBuf,
    pub last_content: String,
}

pub struct Library {
    pub base_path: PathBuf,
    pub images_path: PathBuf,
    /// relative (frontend-style) path -> open buffer
    pub open: HashMap<String, OpenBuffer>,
}

impl Library {
    pub fn new(base_path: PathBuf) -> Result<Self, String> {
        if !base_path.is_dir() {
            return Err(format!("Path directory does not exist: {}", base_path.display()));
        }
        let base = base_path
            .canonicalize()
            .map_err(|e| format!("failed to canonicalize {}: {}", base_path.display(), e))?;
        let library = Library {
            images_path: base.join(".images"),
            base_path: base,
            open: HashMap::new(),
        };
        let scratch = library.base_path.join(SCRATCH_FILE_NAME);
        if !scratch.exists() {
            config::atomic_write(&scratch, &initial_content(config::is_dev()))?;
        }
        Ok(library)
    }

    fn full_path(&self, rel: &str) -> PathBuf {
        self.base_path.join(rel_to_os(rel))
    }

    pub fn exists(&self, rel: &str) -> Result<bool, String> {
        Ok(self.full_path(rel).is_file())
    }

    pub fn load(&mut self, rel: &str) -> Result<String, String> {
        let full = self.full_path(rel);
        let content = fs::read_to_string(&full)
            .map_err(|e| format!("failed to read {}: {}", full.display(), e))?;
        self.open.insert(
            rel.to_string(),
            OpenBuffer { full_path: full, last_content: content.clone() },
        );
        Ok(content)
    }

    pub fn save(&mut self, rel: &str, content: &str) -> Result<(), String> {
        let full = {
            let buffer = self
                .open
                .get(rel)
                .ok_or_else(|| format!("File not loaded: {}", rel))?;
            buffer.full_path.clone()
        };
        config::atomic_write(&full, content)?;
        if let Some(buffer) = self.open.get_mut(rel) {
            buffer.last_content = content.to_string();
        }
        Ok(())
    }

    pub fn create(&self, rel: &str, content: &str) -> Result<(), String> {
        let full = self.full_path(rel);
        if full.exists() {
            return Err(format!("File already exists: {}", rel));
        }
        config::atomic_write(&full, content)
    }

    pub fn create_directory(&self, rel: &str) -> Result<(), String> {
        if rel.is_empty() {
            return Err("Directory path is empty".to_string());
        }
        let full = self.full_path(rel);
        if full.is_file() {
            return Err(format!("A file already exists at path: {}", rel));
        }
        fs::create_dir_all(&full).map_err(|e| format!("failed to create {}: {}", full.display(), e))
    }

    pub fn move_file(&mut self, rel: &str, new_rel: &str) -> Result<(), String> {
        if self.exists(new_rel).unwrap_or(false) {
            return Err(format!("File already exists: {}", new_rel));
        }
        let old_full = self.full_path(rel);
        let new_full = self.full_path(new_rel);
        fs::rename(&old_full, &new_full)
            .map_err(|e| format!("failed to move {} -> {}: {}", old_full.display(), new_full.display(), e))?;
        if let Some(buffer) = self.open.remove(rel) {
            self.open.insert(
                new_rel.to_string(),
                OpenBuffer { full_path: new_full, last_content: buffer.last_content },
            );
        }
        Ok(())
    }

    pub fn delete(&self, rel: &str) -> Result<(), String> {
        if rel == SCRATCH_FILE_NAME {
            return Err("Can't delete scratch file".to_string());
        }
        let full = self.full_path(rel);
        if full.is_file() {
            fs::remove_file(&full).map_err(|e| format!("failed to delete {}: {}", full.display(), e))?;
        } else if full.is_dir() {
            fs::remove_dir_all(&full).map_err(|e| format!("failed to delete {}: {}", full.display(), e))?;
        }
        Ok(())
    }

    pub fn is_directory_empty(&self, rel: &str) -> Result<bool, String> {
        if rel.is_empty() {
            return Ok(false);
        }
        let full = self.full_path(rel);
        if !full.is_dir() {
            return Ok(false);
        }
        match fs::read_dir(&full) {
            Ok(mut entries) => Ok(entries.next().is_none()),
            Err(_) => Ok(false),
        }
    }

    pub fn delete_directory(&self, rel: &str) -> Result<(), String> {
        if rel.is_empty() {
            return Err("Can't delete root directory".to_string());
        }
        let full = self.full_path(rel);
        if !full.is_dir() {
            return Err(format!("Directory does not exist: {}", rel));
        }
        if !self.is_directory_empty(rel)? {
            return Err(format!("Directory is not empty: {}", rel));
        }
        fs::remove_dir(&full).map_err(|e| format!("failed to delete {}: {}", full.display(), e))
    }

    pub fn get_list(&self) -> Result<Value, String> {
        let mut notes = Map::new();
        for entry in WalkDir::new(&self.base_path).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            if !entry.file_name().to_string_lossy().ends_with(".txt") {
                continue;
            }
            let rel = os_to_rel(entry.path(), &self.base_path);
            let metadata = read_note_metadata(entry.path()).unwrap_or(Value::Null);
            notes.insert(rel, metadata);
        }
        Ok(Value::Object(notes))
    }

    /// List of all directories below the root (frontend filters dot-directories).
    pub fn get_directory_list(&self) -> Result<Vec<String>, String> {
        let mut dirs = Vec::new();
        for entry in WalkDir::new(&self.base_path)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_dir() {
                dirs.push(os_to_rel(entry.path(), &self.base_path));
            }
        }
        dirs.sort();
        Ok(dirs)
    }

    pub fn close_file(&mut self, rel: &str) {
        self.open.remove(rel);
    }

    /// Watcher/focus hook: re-read the buffer and return the new content if it
    /// changed on disk, None otherwise (content-compare suppression of our own
    /// atomic writes — port of NoteBuffer.loadIfChanged()).
    pub fn reload_if_changed(&mut self, rel: &str) -> Option<String> {
        let buffer = self.open.get_mut(rel)?;
        let content = fs::read_to_string(&buffer.full_path).ok()?;
        if content != buffer.last_content {
            buffer.last_content = content.clone();
            Some(content)
        } else {
            None
        }
    }

    pub fn open_paths(&self) -> Vec<String> {
        self.open.keys().cloned().collect()
    }

    pub fn save_image(&self, mime: &str, data: &[u8]) -> Result<Option<String>, String> {
        let Some(ext) = ext_for_mime(mime) else {
            return Ok(None);
        };
        let timestamp = chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            .replace(':', ".");
        let filename = format!("{}.{}", timestamp, ext);
        fs::create_dir_all(&self.images_path)
            .map_err(|e| format!("failed to create {}: {}", self.images_path.display(), e))?;
        fs::write(self.images_path.join(&filename), data)
            .map_err(|e| format!("failed to write image: {}", e))?;
        Ok(Some(filename))
    }
}

/// Port of getImgReferences(): scan all *.txt files for `<∞img;...∞>` blocks
/// and collect the referenced image filenames.
pub fn get_img_references(base_path: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let pattern = "<∞img;";
    for entry in WalkDir::new(base_path).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || !entry.file_name().to_string_lossy().ends_with(".txt") {
            continue;
        }
        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut rest = content.as_str();
        let terminator = "∞>";
        while let Some(start) = rest.find(pattern) {
            let attrs = &rest[start + pattern.len()..];
            match attrs.find(terminator) {
                Some(end) => {
                    let attr_str = &attrs[..end];
                    // attributes are semicolon-separated key=value pairs; we want file=
                    for pair in attr_str.split(';') {
                        if let Some(value) = pair.strip_prefix("file=") {
                            let decoded = percent_decode(value);
                            if let Some(filename) = decoded.strip_prefix("heynote-file://image/") {
                                files.push(filename.to_string());
                            }
                        }
                    }
                    // NB: "∞>" is 4 bytes (∞ is 3-byte UTF-8) — slicing by
                    // byte length, never by char count.
                    rest = &attrs[end + terminator.len()..];
                }
                None => break,
            }
        }
    }
    files
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Port of removeUnreferencedImages(): delete images older than 24h that are
/// no longer referenced, but only if at least one referenced image was found
/// (safety precaution against a scanning failure wiping the whole directory).
pub fn remove_unreferenced_images(base_path: &Path, images_path: &Path) {
    if !images_path.is_dir() {
        return;
    }
    let referenced = get_img_references(base_path);

    let mut referenced_found = false;
    let mut to_delete = Vec::new();
    let entries = match fs::read_dir(images_path) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let filename = entry.file_name().to_string_lossy().to_string();
        if referenced.contains(&filename) {
            referenced_found = true;
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO)
            > Duration::from_secs(24 * 3600)
        {
            to_delete.push(path);
        }
    }

    if !referenced_found {
        eprintln!("No referenced images found, so as a precaution, we won't do any removal of unreferenced images");
        return;
    }
    for path in to_delete {
        let _ = fs::remove_file(path);
    }
}

/// Port of migrateBufferFileToLibrary(): move the pre-2.0 single buffer file to
/// <library>/scratch.txt, taking a numbered .bak backup first.
pub fn migrate_buffer_file_to_library(custom_buffer_path: Option<&Path>) {
    let old_buffer_file = if config::is_dev() { "buffer-dev.txt" } else { "buffer.txt" };
    match custom_buffer_path {
        Some(custom) => {
            if custom.join(SCRATCH_FILE_NAME).is_file() {
                return;
            }
            let old_full = custom.join(old_buffer_file);
            if old_full.is_file() {
                if let Ok(backup) = get_backup_file(&old_full) {
                    if fs::copy(&old_full, &backup).is_ok() {
                        let new_full = custom.join(SCRATCH_FILE_NAME);
                        if fs::rename(&old_full, &new_full).is_ok() {
                            let _ = ensure_buffer_file_metadata(&new_full);
                        }
                    }
                }
            }
        }
        None => {
            let default_library = config::user_data_dir().join(notes_dir_name());
            if default_library.join(SCRATCH_FILE_NAME).is_file() {
                return;
            }
            let old_full = config::user_data_dir().join(old_buffer_file);
            if old_full.is_file() && !default_library.is_dir() {
                if let Ok(backup) = get_backup_file(&old_full) {
                    if fs::copy(&old_full, &backup).is_ok() {
                        if fs::create_dir_all(&default_library).is_ok() {
                            let new_full = default_library.join(SCRATCH_FILE_NAME);
                            if fs::rename(&old_full, &new_full).is_ok() {
                                let _ = ensure_buffer_file_metadata(&new_full);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn img_reference_scan_handles_multibyte_and_multiple_blocks() {
        // Regression test: advancing past "∞>" by 2 (instead of its 4-byte
        // length) sliced into the ∞ char and panicked (panic=abort killed
        // the whole app during the startup image GC).
        let content = "{\"name\":\"Scratch\"}\n∞∞∞text\nhello\n∞∞∞markdown\n\
            <∞img;width=100;height=50;file=heynote-file://image/%E5%9B%BE.png∞>\n\
            between\n\
            <∞img;file=heynote-file://image/b.png∞>\n\
            <∞img;file=other-scheme://x∞>\n";
        let dir = std::env::temp_dir().join("heynote-imgscan-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), content).unwrap();
        let refs = get_img_references(&dir);
        assert_eq!(
            refs,
            vec!["图.png".to_string(), "b.png".to_string()],
            "should extract percent-decoded filenames from consecutive image blocks"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
