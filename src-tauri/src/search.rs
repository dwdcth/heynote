// Port of the ripgrep-backed library search (electron/main/ripgrep.js) as a
// pure-Rust search engine (walkdir + fancy-regex). Emits the exact same event
// protocol the frontend consumes: library-search:match / :done / :error with
// payloads carrying searchId and UTF-16 (JS string) indices.

use crate::state::AppState;
use fancy_regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager};
use walkdir::WalkDir;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub search_id: f64,
    #[serde(default)]
    pub regexp: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
}

const MIN_QUERY_BYTES: usize = 3;

pub fn start(app: AppHandle, state: &AppState, options: SearchOptions) -> Result<(), String> {
    // Bumping the generation cancels any in-flight search (Electron kills the
    // ripgrep process; we just make the thread observe a mismatch).
    let my_gen = state.search_gen.fetch_add(1, Ordering::SeqCst) + 1;

    let trimmed = options.query.trim();
    if trimmed.len() < MIN_QUERY_BYTES {
        return Err("Search query is too short".to_string());
    }

    let base_path = {
        let library = state.library.lock().unwrap();
        library
            .as_ref()
            .map(|l| l.base_path.clone())
            .ok_or_else(|| "File library is not initialized".to_string())?
    };

    let pattern = if options.regexp {
        options.query.clone()
    } else {
        fancy_regex::escape(&options.query).to_string()
    };
    let pattern = if options.whole_word {
        format!(r"\b{}\b", pattern)
    } else {
        pattern
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|e| format!("Invalid regular expression: {}", e))?;

    let app = app.clone();
    std::thread::spawn(move || {
        run_search(&app, &base_path, regex, options.search_id, my_gen);
    });
    Ok(())
}

fn run_search(app: &AppHandle, base_path: &Path, regex: Regex, search_id: f64, my_gen: u64) {
    let emit = |channel: &str, payload: Value| {
        let _ = app.emit(channel, payload);
    };

    for entry in WalkDir::new(base_path)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
    {
        // ripgrep skips hidden files by default; mirror that (also excludes .images)
        if entry
            .path()
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if !entry.file_type().is_file()
            || !entry.file_name().to_string_lossy().ends_with(".txt")
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let buffer = entry
            .path()
            .strip_prefix(base_path)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        for (lineno, raw_line) in content.split('\n').enumerate() {
            if app.state::<AppState>().search_gen.load(Ordering::SeqCst) != my_gen {
                // superseded by a newer search (or cancel): emit done(cancelled)
                // like the killed ripgrep process would.
                emit(
                    "library-search:done",
                    json!({ "searchId": search_id, "cancelled": true }),
                );
                return;
            }
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let mut submatches: Vec<(usize, usize, &str)> = Vec::new();
            for m in regex.find_iter(line) {
                match m {
                    Ok(m) => submatches.push((m.start(), m.end(), m.as_str())),
                    Err(e) => {
                        emit(
                            "library-search:error",
                            json!({ "searchId": search_id, "message": e.to_string() }),
                        );
                        return;
                    }
                }
            }
            if submatches.is_empty() {
                continue;
            }

            // Convert byte offsets to UTF-16 (JS string) indices — the frontend
            // uses them to slice/highlight, and CJK chars are 3 bytes but 1 unit.
            let units: Vec<u16> = line.encode_utf16().collect();
            let byte_to_u16 = utf16_boundary_map(line);
            let to_u16 = |byte_off: usize| -> usize {
                match byte_to_u16.binary_search_by_key(&byte_off, |&(b, _)| b) {
                    Ok(i) => byte_to_u16[i].1,
                    Err(i) => byte_to_u16[i.saturating_sub(1)].1,
                }
            };
            let submatches: Vec<(usize, usize, &str)> = submatches
                .into_iter()
                .map(|(bs, be, t)| (to_u16(bs), to_u16(be), t))
                .collect();

            if let Some(mut normalized) = normalize_match(line, &units, &submatches, lineno + 1, &byte_to_u16) {
                if let Some(obj) = normalized.as_object_mut() {
                    obj.insert("searchId".to_string(), json!(search_id));
                    obj.insert("type".to_string(), json!("match"));
                    obj.insert("buffer".to_string(), json!(buffer));
                }
                emit("library-search:match", normalized);
            }
        }
    }
    emit("library-search:done", json!({ "searchId": search_id }));
}

/// Parallel (byte offset, utf16 offset) pairs for each char boundary of `line`.
fn utf16_boundary_map(line: &str) -> Vec<(usize, usize)> {
    let mut map = Vec::with_capacity(line.len() + 1);
    let mut u16len = 0usize;
    for (byte_off, ch) in line.char_indices() {
        map.push((byte_off, u16len));
        u16len += ch.len_utf16();
    }
    map.push((line.len(), u16len));
    map
}

fn is_metadata_line(line: &str) -> bool {
    let trimmed = line.trim();
    if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return false;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v) => {
            let Some(obj) = v.as_object() else { return false };
            const METADATA_KEYS: [&str; 5] =
                ["formatVersion", "name", "tags", "cursors", "foldedRanges"];
            obj.keys().any(|k| METADATA_KEYS.contains(&k.as_str()))
        }
        Err(_) => false,
    }
}

/// Syntax ranges (in UTF-16 index space) hidden from the display line.
fn syntax_ranges_for_line(
    line: &str,
    line_number: usize,
    byte_to_u16: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    if line_number == 1 && is_metadata_line(line) {
        return vec![(0, line.encode_utf16().count())];
    }
    if line.starts_with("∞∞∞") {
        // ^∞∞∞[^\r\n]*$ — a block delimiter line
        return vec![(0, line.encode_utf16().count())];
    }
    let tag_regex = Regex::new("<∞.*?∞>").expect("static regex");
    let to_u16 = |byte_off: usize| -> usize {
        match byte_to_u16.binary_search_by_key(&byte_off, |&(b, _)| b) {
            Ok(i) => byte_to_u16[i].1,
            Err(i) => byte_to_u16[i.saturating_sub(1)].1,
        }
    };
    let mut ranges = Vec::new();
    for m in tag_regex.find_iter(line).flatten() {
        ranges.push((to_u16(m.start()), to_u16(m.end())));
    }
    ranges
}

/// Port of normalizeLibrarySearchMatch() (src/common/library-search-match.js).
/// All indices are UTF-16 code unit offsets, matching JS string semantics.
fn normalize_match(
    line: &str,
    units: &[u16],
    submatches: &[(usize, usize, &str)],
    line_number: usize,
    byte_to_u16: &[(usize, usize)],
) -> Option<Value> {
    let syntax_ranges = syntax_ranges_for_line(line, line_number, byte_to_u16);
    let overlaps = |start: usize, end: usize| {
        syntax_ranges
            .iter()
            .any(|&(rs, re)| start < re && rs < end)
    };
    let visible: Vec<&(usize, usize, &str)> = submatches
        .iter()
        .filter(|&&(s, e, _)| !overlaps(s, e))
        .collect();
    if visible.is_empty() {
        return None;
    }

    let raw_to_display = |mut index: usize| -> usize {
        for &(rs, re) in &syntax_ranges {
            if re <= index {
                index -= re - rs;
            }
        }
        index
    };
    let display_line = {
        let mut result: Vec<u16> = Vec::with_capacity(units.len());
        let mut cursor = 0usize;
        for &(rs, re) in &syntax_ranges {
            result.extend_from_slice(&units[cursor..rs]);
            cursor = re;
        }
        result.extend_from_slice(&units[cursor..]);
        String::from_utf16_lossy(&result)
    };

    let submatches_json: Vec<Value> = visible
        .iter()
        .map(|&&(s, e, t)| json!({ "start": s, "end": e, "text": t }))
        .collect();
    let display_submatches: Vec<Value> = visible
        .iter()
        .map(|&&(s, e, t)| {
            json!({
                "start": raw_to_display(s),
                "end": raw_to_display(e),
                "text": t,
            })
        })
        .collect();

    Some(json!({
        "line": line,
        "displayLine": display_line,
        "lineNumber": line_number,
        "submatches": submatches_json,
        "displaySubmatches": display_submatches,
    }))
}
