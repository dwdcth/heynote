<script>
    // Native-feeling window control buttons for the Tauri build on
    // Windows/Linux, sitting in the 140px zone the tab bar already reserves
    // (platform-windows-linux sass mixin). macOS uses the native traffic
    // lights, and the webapp/Electron builds don't render this at all.
    // @tauri-apps/api is imported dynamically so it stays out of the
    // Electron/webapp bundles.
    import { mapState } from 'pinia'
    import { useHeynoteStore } from "@/src/stores/heynote-store"

    export default {
        computed: {
            ...mapState(useHeynoteStore, ["isFullscreen"]),
            visible() {
                const platform = window.heynote.platform
                return platform.isTauri && !platform.isMac && !this.isFullscreen
            },
        },

        methods: {
            callWindow(method) {
                import("@tauri-apps/api/window").then((m) => {
                    m.getCurrentWindow()[method]()
                }).catch((err) => {
                    console.error("window operation failed:", err)
                })
            },
            minimize() {
                this.callWindow("minimize")
            },
            toggleMaximize() {
                this.callWindow("toggleMaximize")
            },
            close() {
                this.callWindow("close")
            },
        },
    }
</script>

<template>
    <div v-if="visible" class="window-controls">
        <button class="wc-btn" title="Minimize" tabindex="-1" @click="minimize">
            <svg viewBox="0 0 10 10"><line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" stroke-width="1" /></svg>
        </button>
        <button class="wc-btn" title="Maximize" tabindex="-1" @click="toggleMaximize">
            <svg viewBox="0 0 10 10"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1" /></svg>
        </button>
        <button class="wc-btn wc-close" title="Close" tabindex="-1" @click="close">
            <svg viewBox="0 0 10 10"><line x1="0" y1="0" x2="10" y2="10" stroke="currentColor" stroke-width="1" /><line x1="10" y1="0" x2="0" y2="10" stroke="currentColor" stroke-width="1" /></svg>
        </button>
    </div>
</template>

<style lang="sass" scoped>
    .window-controls
        position: absolute
        top: 0
        right: 0
        display: flex
        z-index: 5

    .wc-btn
        width: 44px
        height: var(--tab-bar-height)
        display: flex
        align-items: center
        justify-content: center
        border: none
        background: none
        color: inherit
        cursor: default
        outline: none
        padding: 0

        &:hover
            background: rgba(128, 128, 128, 0.25)

        &.wc-close:hover
            background: #e81123
            color: #fff

        svg
            width: 10px
            height: 10px
</style>
