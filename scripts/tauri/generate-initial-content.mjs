// Generates the initial scratch-note content templates consumed by the Rust
// backend (include_str!). The single source of truth stays
// src/common/initial-content.js; this script just renders it per platform at
// build time and replaces the generation timestamp with a sentinel that Rust
// substitutes at note-creation time.

import { writeFileSync, mkdirSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { getInitialContent } from "../../src/common/initial-content.js"

const __dirname = dirname(fileURLToPath(import.meta.url))
const OUT_DIR = join(__dirname, "..", "..", "src-tauri", "assets")
const SENTINEL = "@@CREATED@@"

mkdirSync(OUT_DIR, { recursive: true })

for (const platform of ["linux", "darwin", "win32"]) {
    for (const dev of [false, true]) {
        let content = getInitialContent(platform, dev)
        // The generation timestamp is embedded as `created=<ISO>`; replace all
        // occurrences of the (first, current-generation) timestamp with the sentinel.
        const m = /created=([^\n]*)/.exec(content)
        if (m) {
            content = content.split(m[1]).join(SENTINEL)
        }
        const suffix = dev ? "dev" : "prod"
        const outPath = join(OUT_DIR, `initial-content.${platform}.${suffix}.txt`)
        writeFileSync(outPath, content)
        console.log(`wrote ${outPath}`)
    }
}
