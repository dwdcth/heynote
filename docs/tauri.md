# Tauri v2 Port

This branch is a Tauri v2 port of Heynote, living alongside the original
Electron build — both run from the same working tree and share the same data
directory (`~/Library/Application Support/Heynote` on macOS,
`~/.config/Heynote` on Linux), so you can switch between them freely.

The renderer is untouched: a bridge layer (`src/tauri/bridge.js`) implements
the exact `window.heynote` surface the Electron preload used to provide, on
top of Tauri `invoke`/`listen`.

## Layout

```
index-tauri.html          Tauri entry page (async bridge bootstrap → src/main.js)
src/tauri/                bridge / bootstrap / about-window modules
src/components/tabs/WindowControls.vue   min/max/close for Windows/Linux
vite.config.tauri.mjs     separate Vite config (port 3345, dist-tauri/)
src-tauri/                Rust backend
  src/config.rs           electron-store compatible config (atomic writes)
  src/library.rs          notes library CRUD, image storage/GC, legacy migration
  src/watcher.rs          notify-based file watching (content-compare debouncing)
  src/search.rs           library search (walkdir + fancy-regex, ripgrep protocol)
  src/menus.rs            context menus / app menu / tray menu
  src/window_ctl.rs       window creation, hotkey, tray, lifecycle
  src/protocol.rs         heynote-file:// image protocol
scripts/tauri/            initial-content generator + macOS .app packaging
```

## Build & Run

```bash
npm install
npm run tauri:dev        # dev (Vite on :3345 + Rust debug)
npm run tauri:build      # release bundle (use the tauri CLI)
```

For plain `cargo build --release` (without the tauri CLI), the
`custom-protocol` feature is already declared in `src-tauri/Cargo.toml` —
without it the binary embeds no frontend assets and shows a blank window.

### Cross-compiling for macOS (ARM64) from Linux

```bash
rustup target add aarch64-apple-darwin
cargo install cargo-zigbuild --locked
# fetch a MacOSX SDK once (zig ships no framework stubs):
curl -L -o sdk.tar.xz \
  https://github.com/phracker/MacOSX-SDKs/releases/download/11.3/MacOSX11.3.sdk.tar.xz
mkdir -p ~/.cache/MacOSX-SDKs && tar -xf sdk.tar.xz -C ~/.cache/MacOSX-SDKs

cd src-tauri
SDKROOT=$HOME/.cache/MacOSX-SDKs/MacOSX11.3.sdk \
MACOSX_DEPLOYMENT_TARGET=11.0 \
  cargo zigbuild --target aarch64-apple-darwin --release

# assemble a proper .app bundle (Dock icon, Info.plist):
../scripts/tauri/package-macos.sh
```

## Notes / known differences

- `app.security.csp` is `null`: Tauri's header-injected CSP silently blocks
  dynamically created `<style>` elements on WebKit (CodeMirror/PrimeVue
  themes are injected that way).
- Window dragging is JS-driven (`startDragging()` on empty title-bar areas);
  neither `-webkit-app-region: drag` nor `data-tauri-drag-region` are used —
  both disturb the mousedown pipeline on WKWebView.
- The updater is not wired; spellcheck dictionary selection (an Electron
  session API) is stubbed.
- Debug helpers are intentionally left in place: `[cmd]` command logs,
  `[frontend]` error/console forwarding, `DOMCHECK` dumps (3s/8s/fn+F9) and
  mouse-event logs — run the binary from a terminal to see them.

License remains MIT + Commons Clause (see LICENSE): no commercial selling.
