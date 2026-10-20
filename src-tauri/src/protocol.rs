// heynote-file:// URI scheme handler (port of electron/main/protocol.js).
// Note content embeds heynote-file://image/<file> URLs, so this scheme must be
// preserved exactly. URL forms handled:
//   heynote-file://image/<file>       (host = "image")
//   heynote-file:///image/<file>      (path form some backends produce)
//   heynote-file://localhost/image/<file>

use tauri::http::Response;
use tauri::{Manager, UriSchemeContext, Wry};

pub fn handle(
    ctx: UriSchemeContext<'_, Wry>,
    request: tauri::http::Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let uri = request.uri().to_string();
    let rest = uri
        .strip_prefix("heynote-file://")
        .unwrap_or("");

    let filename_encoded = if let Some(f) = rest.strip_prefix("image/") {
        f
    } else if let Some(f) = rest.strip_prefix("/image/") {
        f
    } else if let Some(f) = rest.strip_prefix("localhost/image/") {
        f
    } else {
        return not_found();
    };
    // strip any query/fragment
    let filename_encoded = filename_encoded.split(['?', '#']).next().unwrap_or("");
    let filename = percent_decode(filename_encoded);
    if filename.is_empty()
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
    {
        return not_found();
    }

    let app = ctx.app_handle();
    let images_path = {
        let state = app.state::<crate::state::AppState>();
        let library = state.library.lock().unwrap();
        library.as_ref().map(|l| l.images_path.clone())
    };
    let Some(images_path) = images_path else {
        return service_unavailable();
    };
    let path = images_path.join(&filename);
    match std::fs::read(&path) {
        Ok(data) => {
            let content_type = mime_guess::from_path(&path)
                .first_or_octet_stream()
                .as_ref()
                .to_string();
            Response::builder()
                .header("Content-Type", content_type)
                .header("Access-Control-Allow-Origin", "*")
                .body(data)
                .unwrap_or_else(|_| not_found())
        }
        Err(_) => not_found(),
    }
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder()
        .status(404)
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn service_unavailable() -> Response<Vec<u8>> {
    Response::builder()
        .status(503)
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
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
