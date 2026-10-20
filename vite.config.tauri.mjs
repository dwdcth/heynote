import fs from 'node:fs'
import path from 'path'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

import { LANGUAGES } from "./src/editor/languages"

const updateGuesslangLanguagesInWebWorker = async () => {
    const langDetectWorkerPath = path.resolve(__dirname, 'public', 'langdetect-worker.js')
    let workerFileContents = fs.readFileSync(langDetectWorkerPath, 'utf8')
    const langListRegex = /^GUESSLANG_LANGUAGES = \[[^\]]+\]/gms
    const newLangList = JSON.stringify(LANGUAGES.map(l => l.guesslang).filter(l => l !== null))
    workerFileContents = workerFileContents.replace(langListRegex, `GUESSLANG_LANGUAGES = ${newLangList}`)
    fs.writeFileSync(langDetectWorkerPath, workerFileContents)
}

const resolveConfig = {
    alias: {
        '@': path.resolve(__dirname),
    },
}

// Tauri build config: plain Vite (no electron plugins), separate output
// directory and dev-server port so the Electron build stays untouched and
// both can run side by side.
export default defineConfig({
    resolve: resolveConfig,
    plugins: [
        vue(),
        updateGuesslangLanguagesInWebWorker(),
    ],
    css: {
        preprocessorOptions: {
            sass: {
                api: "modern-compiler",
                additionalData: `
    @use "@/src/css/include.sass" as *
`
            }
        }
    },
    build: {
        outDir: 'dist-tauri',
        rollupOptions: {
            input: {
                main: path.resolve(__dirname, 'index-tauri.html'),
                about: path.resolve(__dirname, 'about-tauri.html'),
            },
        },
    },
    server: {
        port: 3345,
        strictPort: true,
    },
    clearScreen: false,
})
