<script>
    import { mapState } from 'pinia'
    import { useHeynoteStore } from "@/src/stores/heynote-store"

    export default {
        computed: {
            ...mapState(useHeynoteStore, [
                "showLeftPanel",
            ]),
        },
        methods: {
            onMainMenuClick(event) {
                const x = event.target.offsetLeft
                const y = event.target.offsetTop + event.target.offsetHeight
                window.heynote.mainProcess.invoke("showMainMenu", x, y)
            },
        },
    }
</script>

<template>
    <div class="main-menu-container">
        <button class="main-menu"
            @click="onMainMenuClick"
        ></button>
    </div>
</template>

<style lang="sass" scoped>
    .main-menu-container
        width: 37px
        flex-shrink: 0
        text-align: center
        button
            border: none
            padding: 0
            margin: 0
            width: 18px
            height: 20px
            margin-top: 6px
            background: none
            background-image: url("@/assets/icons/vertical-dots-light.svg")
            background-repeat: no-repeat
            background-position: center
            background-size: 14px
            border-radius: 3px
            +dark-mode
                background-image: url("@/assets/icons/vertical-dots-dark.svg")
            &:hover
                background-color: #ccc
                +dark-mode
                    background-color: #3a3a3a
        +platform-mac
            width: 80px
            button
                display: none
        +platform-mac-fullscreen
            width: 16px
        +platform-webapp
            button
                display: none
</style>
