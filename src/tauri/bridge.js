// Tauri implementation of the `window.heynote` API.
//
// The shape of this bridge mirrors `electron/preload/index.js` (the contract the
// renderer consumes) and `webapp/bridge.js` (the precedent for implementing it
// without Electron). The bridge must be initialized (initBridge()) before any
// app module evaluates, because e.g. src/editor/keymap.js reads
// window.heynote.platform at module top-level.

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import {
    SETTINGS_CHANGE_EVENT,
    OPEN_SETTINGS_EVENT,
    SAVE_TABS_STATE,
    LOAD_TABS_STATE,
    WINDOW_CLOSE_EVENT,
    WINDOW_FULLSCREEN_STATE,
    WINDOW_FOCUS_STATE,
    FOCUS_EDITOR_EVENT,
    REDO_EVENT,
    UNDO_EVENT,
    MOVE_BLOCK_EVENT,
    DELETE_BLOCK_EVENT,
    CHANGE_BUFFER_EVENT,
    SELECT_ALL_EVENT,
    CONTEXT_MENU_CLOSED,
    LIBRARY_SEARCH_START,
    LIBRARY_SEARCH_CANCEL,
    LIBRARY_SEARCH_MATCH,
    LIBRARY_SEARCH_DONE,
    LIBRARY_SEARCH_ERROR,
} from "@/src/common/constants"

// ---------------------------------------------------------------------------
// Minimal ipcRenderer emulation: callbacks receive (event, ...args) like
// Electron's ipcRenderer, with a null stand-in for the event object.
// ---------------------------------------------------------------------------

class IpcRenderer {
    constructor() {
        this.callbacks = {}
    }

    on(event, callback) {
        if (!this.callbacks[event]) {
            this.callbacks[event] = []
        }
        this.callbacks[event].push(callback)
    }

    off(event, callback) {
        if (this.callbacks[event]) {
            this.callbacks[event] = this.callbacks[event].filter(cb => cb !== callback)
        }
    }

    send(event, ...args) {
        if (this.callbacks[event]) {
            for (const callback of this.callbacks[event]) {
                callback(null, ...args)
            }
        }
    }
}

const ipcRenderer = new IpcRenderer()

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

const mediaMatch = window.matchMedia("(prefers-color-scheme: dark)")
let themeModeChangeListener = null
const getComputedTheme = () => mediaMatch.matches ? "dark" : "light"

// ---------------------------------------------------------------------------
// External links: Electron intercepts window.open calls and routes them to
// shell.openExternal. In Tauri an unintercepted navigation would replace the
// app webview, so intercept anchor clicks targeting external URLs here (the
// Rust side has an on_navigation fallback as second line of defense).
// ---------------------------------------------------------------------------

function installExternalLinkHandler() {
    document.addEventListener("click", (ev) => {
        const anchor = ev.target?.closest?.("a")
        if (!anchor) {
            return
        }
        const href = anchor.getAttribute("href") || ""
        const isExternalHttp = /^(https?|mailto):/i.test(href)
        if (anchor.target === "_blank" || isExternalHttp) {
            ev.preventDefault()
            if (isExternalHttp) {
                invoke("open_external_url", { url: href }).catch((err) => {
                    console.error("Failed to open external URL:", err)
                })
            }
        }
    }, true)
}

// ---------------------------------------------------------------------------
// Diagnostics: forward JS errors to the Rust backend so they show up in
// stderr when running headless (no devtools available in Xvfb).
// ---------------------------------------------------------------------------

function installErrorForwarding() {
    const log = (message) => {
        invoke("log_frontend", { message }).catch(() => {})
    }
    window.addEventListener("error", (e) => {
        log(`JS ERROR: ${e.message} @ ${e.filename}:${e.lineno}:${e.colno}\n${e.error?.stack || ""}`)
    })
    window.addEventListener("unhandledrejection", (e) => {
        const reason = e.reason?.stack || e.reason?.message || String(e.reason)
        log(`PROMISE REJECT: ${reason}`)
    })
    for (const level of ["error", "warn"]) {
        const original = console[level].bind(console)
        console[level] = (...args) => {
            log(`console.${level}: ${args.map((a) => {
                try {
                    return typeof a === "string" ? a : JSON.stringify(a)
                } catch {
                    return String(a)
                }
            }).join(" ")}`)
            original(...args)
        }
    }

    // DOM geometry self-check (diagnosing the blank-editor issue on WebKit)
    const domCheck = (tag) => {
        const q = (sel) => document.querySelector(sel)
        // pick the first *visible* editor instance (hidden buffers are
        // display:none and would report 0-height false alarms)
        const qVisible = (sel) => {
            for (const el of document.querySelectorAll(sel)) {
                if (el.offsetParent !== null || el.offsetHeight > 0) return el
            }
            return null
        }
        const geom = (el) => el ? {
            clientH: el.clientHeight,
            scrollH: el.scrollHeight,
            offsetH: el.offsetHeight,
            styleH: getComputedStyle(el).height,
        } : null
        const cmEditorEl = qVisible(".cm-editor")
        const scrollerEl = cmEditorEl?.querySelector(".cm-scroller") || null
        const content = cmEditorEl?.querySelector(".cm-content") || null
        let childInfo = []
        if (content) {
            for (const child of content.children) {
                if (child.offsetHeight > 10000 || child.scrollHeight > 10000) {
                    childInfo.push({
                        tag: child.tagName,
                        cls: (child.className || "").toString().slice(0, 40),
                        offsetH: child.offsetHeight,
                        scrollH: child.scrollHeight,
                        styleH: getComputedStyle(child).height,
                        margin: getComputedStyle(child).marginBottom,
                    })
                }
            }
            childInfo = childInfo.slice(0, 5)
        }
        let inlineInfo = {}
        if (scrollerEl) {
            const cs = getComputedStyle(scrollerEl)
            inlineInfo = {
                scrollerInlineAttr: scrollerEl.getAttribute("style"),
                scrollerPaddingTop: cs.paddingTop,
                scrollerMinHeight: cs.minHeight,
                scrollerMaxHeight: cs.maxHeight,
                scrollerPosition: cs.position,
                scrollerDisplay: cs.display,
                editorDisplay: cmEditorEl ? getComputedStyle(cmEditorEl).display : null,
                editorPosition: cmEditorEl ? getComputedStyle(cmEditorEl).position : null,
                contentPaddingTop: content ? getComputedStyle(content).paddingTop : null,
                contentInlineAttr: content?.getAttribute("style"),
            }
        }
        let sheetInfo = { adopted: null, styles: [], cmRuleFound: false, inlineSels: [], gutterComputed: null }
        try {
            sheetInfo.adopted = (document.adoptedStyleSheets || []).length
        } catch (e) { sheetInfo.adopted = "ERR " + e }
        try {
            for (const ss of document.styleSheets) {
                let count = -1, hasCm = false, err = null
                try {
                    const rules = ss.cssRules
                    count = rules.length
                    for (const r of rules) {
                        const sel = r.selectorText || ""
                        if (sel.includes("cm-scroller") || sel.includes("cm-gutters")) { hasCm = true; break }
                    }
                    if (hasCm) sheetInfo.cmRuleFound = true
                    if (!ss.href && count <= 200) {
                        for (const r of rules) {
                            if (r.selectorText) sheetInfo.inlineSels.push(r.selectorText.slice(0, 70))
                        }
                        sheetInfo.inlineSels = sheetInfo.inlineSels.slice(0, 40)
                    }
                } catch (e) { err = String(e).slice(0, 60) }
                if (count > 0 || hasCm || err) {
                    sheetInfo.styles.push({
                        href: ss.href ? ss.href.split("/").pop().slice(0, 40) : "inline",
                        count, hasCm, err,
                    })
                }
            }
        } catch (e) { sheetInfo.styles.push({fatal: String(e)}) }
        const guttersEl = cmEditorEl?.querySelector(".cm-gutters") || null
        if (guttersEl) {
            const gcs = getComputedStyle(guttersEl)
            sheetInfo.gutterComputed = {
                display: gcs.display, flexDirection: gcs.flexDirection,
                height: gcs.height, width: guttersEl.offsetWidth,
                position: gcs.position, zIndex: gcs.zIndex,
            }
        }
        let styleTags = []
        for (const st of document.querySelectorAll("style")) {
            styleTags.push({
                parent: st.parentElement?.tagName || "?",
                textLen: (st.textContent || "").length,
                sheetRules: (() => { try { return st.sheet ? st.sheet.cssRules.length : -1 } catch (e) { return "ERR" } })(),
                disabled: st.disabled,
                media: st.getAttribute("media"),
                first40: (st.textContent || "").slice(0, 40),
            })
        }
        // probe: does a dynamically-created <style> get a stylesheet at all?
        let probe = null
        try {
            const t = document.createElement("style")
            t.textContent = ".probe-x { color: red }"
            document.head.appendChild(t)
            probe = {
                dynamicSheetRules: t.sheet ? t.sheet.cssRules.length : null,
                cspMeta: document.querySelector('meta[http-equiv="Content-Security-Policy"]')?.content?.slice(0, 200) || null,
            }
            t.remove()
        } catch (e) { probe = { err: String(e) } }
        log(`DOMCHECK[${tag}] ` + JSON.stringify({
            cmEditor: geom(cmEditorEl),
            cmScroller: geom(scrollerEl),
            cmContent: content ? {
                ...geom(content),
                children: content.childElementCount,
                textLen: (content.textContent || "").length,
                display: getComputedStyle(content).display,
            } : null,
            styleTags,
            probe,
            gutterComputed: sheetInfo.gutterComputed,
        }))
    }
    setTimeout(() => domCheck("3s"), 3000)
    setTimeout(() => domCheck("8s"), 8000)
    // manual dump: press F9 anywhere (also works when the editor lost focus)
    window.addEventListener("keydown", (e) => {
        if (e.key === "F9") {
            domCheck("manual")
            const popup = document.querySelector(".overlay .popup")
            const active = document.activeElement
            log("FOCUS STATE: " + JSON.stringify({
                popupExists: !!popup,
                popupVisible: popup ? getComputedStyle(popup).display !== "none" : false,
                activeTag: active?.tagName,
                activeClass: (active?.className || "").toString().slice(0, 50),
                inEditor: !!active?.closest?.(".cm-content"),
                hasFocus: document.hasFocus(),
            }))
        }
    }, true)

    // click diagnostics: log every click with its target + focus state, to
    // distinguish "click swallowed by the native layer" (no DOM event) from
    // "click delivered but handler did nothing".
    const fmtPath = (e) => e.composedPath?.().slice(0, 4).map((el) => {
        if (el instanceof Element) {
            const cls = (el.className || "").toString().slice(0, 30)
            return `${el.tagName.toLowerCase()}${cls ? "." + cls.split(/\s+/)[0] : ""}`
        }
        return ""
    }).join(" < ")

    window.addEventListener("mousedown", (e) => {
        // only log clicks in the tab-bar area (y < 40) to keep noise down
        if (e.clientY < 40) {
            log("MOUSEDOWN: " + JSON.stringify({
                target: fmtPath(e), x: e.clientX, y: e.clientY,
                button: e.button, detail: e.detail,
            }))
        }
    }, true)
    window.addEventListener("mouseup", (e) => {
        if (e.clientY < 40) {
            log("MOUSEUP: " + JSON.stringify({ x: e.clientX, y: e.clientY }))
        }
    }, true)
    window.addEventListener("click", (e) => {
        log("CLICK: " + JSON.stringify({
            target: fmtPath(e),
            x: e.clientX, y: e.clientY,
            detail: e.detail,
            activeEl: (document.activeElement?.className || document.activeElement?.tagName || "?").toString().slice(0, 30),
            hasFocus: document.hasFocus(),
        }))
    }, true)
}

// ---------------------------------------------------------------------------
// Context menus: muda (the menu library Tauri uses) has no menu-will-close
// callback, so we emit context-menu-closed locally once the menu command
// resolves (menus close before popup() returns on GTK; and focusing the editor
// at that point is the desired behavior anyway).
// ---------------------------------------------------------------------------

async function invokeMenu(command, args) {
    const result = await invoke(command, args)
    ipcRenderer.send(CONTEXT_MENU_CLOSED)
    return result
}

// ---------------------------------------------------------------------------
// Bridge initialization
// ---------------------------------------------------------------------------

let libraryPathChangedForwarded = false

export async function initBridge() {
    const state = await invoke("get_initial_state")
    // state: {
    //   settings: object, platform: {isMac, isWindows, isLinux}, isDev: bool,
    //   version: string, theme: "dark"|"light"|"system", pathSeparator: string,
    // }

    // Forward backend events to the ipcRenderer emulation. Payload-unpack
    // functions map the single Tauri payload to Electron-style positional args.
    const forwardedEvents = [
        [SETTINGS_CHANGE_EVENT, (settings) => [settings]],
        [WINDOW_CLOSE_EVENT, null],
        [WINDOW_FULLSCREEN_STATE, (isFullscreen) => [isFullscreen]],
        [WINDOW_FOCUS_STATE, (focused) => [focused]],
        [FOCUS_EDITOR_EVENT, null],
        [OPEN_SETTINGS_EVENT, null],
        [UNDO_EVENT, null],
        [REDO_EVENT, null],
        [SELECT_ALL_EVENT, null],
        [DELETE_BLOCK_EVENT, null],
        [MOVE_BLOCK_EVENT, null],
        [CHANGE_BUFFER_EVENT, null],
        ["tab:close", (tabPath) => [tabPath]],
        ["tab:openNew", (parentDir) => [parentDir]],
        ["tab:createNew", null],
        ["tab:editBuffer", (tabPath) => [tabPath]],
        ["tab:deleteBuffer", (tabPath) => [tabPath]],
        ["tab:archiveScratch", null],
        ["bufferTree:createFolder", (parentDir) => [parentDir]],
        ["bufferTree:deleteDirectory", (directoryPath) => [directoryPath]],
        [LIBRARY_SEARCH_MATCH, (match) => [match]],
        [LIBRARY_SEARCH_DONE, (payload) => [payload]],
        [LIBRARY_SEARCH_ERROR, (payload) => [payload]],
    ]
    for (const [name, unpack] of forwardedEvents) {
        listen(name, (event) => {
            ipcRenderer.send(name, ...(unpack ? unpack(event.payload) : []))
        }).catch(err => console.error(`Failed to listen to ${name}:`, err))
    }
    if (!libraryPathChangedForwarded) {
        libraryPathChangedForwarded = true
        listen("library:pathChanged", () => ipcRenderer.send("library:pathChanged"))
            .catch(err => console.error("Failed to listen to library:pathChanged:", err))
    }

    installExternalLinkHandler()
    installErrorForwarding()

    const Heynote = {
        defaultFontFamily: "Hack",
        defaultFontSize: 12,

        platform: {
            isMac: !!state.platform.isMac,
            isWindows: !!state.platform.isWindows,
            isLinux: !!state.platform.isLinux,
            isWebApp: false,
            // non-standard extension, used by WindowControls.vue only
            isTauri: true,
        },

        isDev: !!state.isDev,

        themeMode: {
            set: (mode) => invoke("dark_mode_set", { mode }),
            get: async () => {
                const theme = await invoke("dark_mode_get")
                return {
                    theme,
                    computed: getComputedTheme(),
                }
            },
            onChange: (callback) => {
                themeModeChangeListener = (event) => {
                    callback(event.matches ? "dark" : "light")
                }
                mediaMatch.addEventListener("change", themeModeChangeListener)
                return mediaMatch
            },
            removeListener() {
                mediaMatch.removeEventListener("change", themeModeChangeListener)
            },
            initial: getComputedTheme(),
        },

        init() {
            if (state.platform.isMac) {
                document.documentElement.setAttribute("platform", "mac")
            } else if (state.platform.isWindows) {
                document.documentElement.setAttribute("platform", "windows")
            } else if (state.platform.isLinux) {
                document.documentElement.setAttribute("platform", "linux")
            }
            // called on all changes to open buffer files; go through all
            // registered callbacks for this path and call them
            listen("buffer:change", (event) => {
                const { path, content } = event.payload
                if (this.buffer._onChangeCallbacks[path]) {
                    this.buffer._onChangeCallbacks[path].forEach(callback => callback(content))
                }
            }).catch(err => console.error("Failed to listen to buffer:change:", err))
        },

        mainProcess: {
            on(event, callback) {
                ipcRenderer.on(event, callback)
            },

            off(event, callback) {
                ipcRenderer.off(event, callback)
            },

            invoke(event, ...args) {
                switch (event) {
                    case SAVE_TABS_STATE:
                        return invoke("save_tabs_state", { tabsState: args[0] })
                    case LOAD_TABS_STATE:
                        return invoke("load_tabs_state")
                    case LIBRARY_SEARCH_START:
                        return invoke("library_search_start", { options: args[0] })
                    case LIBRARY_SEARCH_CANCEL:
                        return invoke("library_search_cancel")
                    case "showEditorContextMenu":
                        return invokeMenu("show_editor_context_menu")
                    case "showMainMenu":
                        return invokeMenu("show_main_menu", { x: args[0], y: args[1] })
                    case "showTabContextMenu":
                        return invokeMenu("show_tab_context_menu", { tabPath: args[0] })
                    case "showBufferTreeContextMenu":
                        return invokeMenu("show_buffer_tree_context_menu", { bufferPath: args[0] })
                    case "showBufferTreeDirectoryContextMenu":
                        return invokeMenu("show_buffer_tree_directory_context_menu", { directoryPath: args[0] })
                    case "showBufferTreeBackgroundContextMenu":
                        return invokeMenu("show_buffer_tree_background_context_menu")
                    case "showSpellcheckingContextMenu":
                        // Electron session spellchecker API has no Tauri equivalent
                        return Promise.resolve()
                    default:
                        return Promise.reject(new Error(`Unknown IPC channel: ${event}`))
                }
            },
        },

        buffer: {
            async exists(path) {
                return await invoke("buffer_exists", { path })
            },

            async getList() {
                return await invoke("buffer_get_list")
            },

            async getDirectoryList() {
                return await invoke("buffer_get_directory_list")
            },

            async load(path) {
                return await invoke("buffer_load", { path })
            },

            async save(path, content) {
                return await invoke("buffer_save", { path, content })
            },

            async delete(path) {
                return await invoke("buffer_delete", { path })
            },

            async isDirectoryEmpty(path) {
                return await invoke("buffer_is_directory_empty", { path })
            },

            async deleteDirectory(path) {
                return await invoke("buffer_delete_directory", { path })
            },

            async move(path, newPath) {
                return await invoke("buffer_move", { path, newPath })
            },

            async create(path, content) {
                return await invoke("buffer_create", { path, content })
            },

            async createDirectory(path) {
                return await invoke("buffer_create_directory", { path })
            },

            async saveAndQuit(contents) {
                return await invoke("buffer_save_and_quit", { contents })
            },

            async close(path) {
                return await invoke("buffer_close", { path })
            },

            async saveImage(blob) {
                // blob.data may be a Uint8Array (which JSON-serializes badly);
                // convert to a plain array for the IPC layer
                return await invoke("library_save_image", {
                    mime: blob.mime,
                    data: blob.data ? Array.from(blob.data) : [],
                })
            },

            _onChangeCallbacks: {},
            addOnChangeCallback(path, callback) {
                if (!this._onChangeCallbacks[path]) {
                    this._onChangeCallbacks[path] = []
                }
                this._onChangeCallbacks[path].push(callback)
            },
            removeOnChangeCallback(path, callback) {
                if (this._onChangeCallbacks[path]) {
                    this._onChangeCallbacks[path] = this._onChangeCallbacks[path].filter(cb => cb !== callback)
                }
            },

            async selectLocation() {
                return await invoke("library_select_location")
            },

            setLibraryPathChangeCallback(callback) {
                ipcRenderer.on("library:pathChanged", callback)
            },

            pathSeparator: state.pathSeparator,
        },

        settings: state.settings,

        setSettings(settings) {
            invoke("set_settings", { settings })
        },

        async getCurrencyData() {
            return await invoke("currency_get")
        },

        // autoUpdate intentionally omitted (no updater in the Tauri build);
        // StatusBar.vue guards with !!window.heynote.autoUpdate

        async getVersion() {
            return await invoke("get_version")
        },

        async getInitErrors() {
            return await invoke("get_init_errors")
        },

        setWindowTitle(title) {
            invoke("set_window_title", { title })
        },

        async getSystemLocale() {
            return await invoke("get_system_locale")
        },
    }

    return Heynote
}
