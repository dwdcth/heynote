# 维护指南：同步上游更新

上游 `heyman/heynote` 更新后，把改动合进来并保持 Tauri 构建正常。

## 同步流程

```bash
git fetch upstream
git rebase upstream/main
# 冲突解决见下文；package-lock.json 冲突直接:
#   git checkout --theirs package-lock.json && npm install
npm run tauri:vite-build                    # 前端构建验证
(cd src-tauri && cargo build --release)     # Rust 编译验证
git push -f origin tauri                    # rebase 后强推自己的分支
```

推送后 GitHub Actions 自动跑三平台构建（`.github/workflows/build-tauri.yml`），
产物在 Actions 页面的 Artifacts 里；打 `v*` tag 会自动发布 Release。

## 冲突预期

约 90% 的迁移代码是**纯新增文件**（`src-tauri/`、`src/tauri/`、入口/配置/脚本），
永不冲突。可能冲突的上游文件及处理原则：

| 文件 | 我们改了什么 | 处理原则 |
|---|---|---|
| `src/components/tabs/TabBar.vue` | 事件重构（mousedown 切换/JS 拖窗/Sortable 参数） | **保双方**：先取上游新功能，再套用我们的事件绑定模式 |
| `src/components/App.vue` | focusEditor 重试 + 原生聚焦兜底 | 保我们的 focusEditor 逻辑，合上游其余改动 |
| `src/components/LeftPanel.vue` | 删 app-region + JS 拖窗 | 同 TabBar 模式 |
| `package.json` / `package-lock.json` | tauri 脚本与依赖 | 手动合 package.json；lock 冲突取上游后 `npm install` 重生成 |

**禁止回归**：无论冲突怎么解，不要重新引入
`-webkit-app-region: drag`、`data-tauri-drag-region`、`csp` 配置、
`forceFallback: true`——这四个都会复发 WebKit 交互 bug（见 docs/tauri.md）。

## 契约检查（git 不会提醒的部分）

bridge 是独立实现，上游 preload 改了 API **不会产生冲突，只会静默缺功能**。
每次 rebase 后跑：

```bash
git diff ORIG_HEAD upstream/main -- electron/preload/index.js src/common/constants.js
```

有变化时，对照更新 `src/tauri/bridge.js`：

1. 新增 `ipcMain.handle` 频道 → bridge `mainProcess.invoke` 的 switch 加映射 + Rust 新命令
2. 新增 `webContents.send` 事件 → bridge 的 `forwardedEvents` 表加转发
3. preload API 签名变化 → bridge 对应方法同步

## 其它同步点

- 上游改了 `src/common/initial-content.js` → 重跑 `npm run tauri:gen-initial-content`
  （重新生成 `src-tauri/assets/initial-content.*.txt` 并提交）
- 上游改了 IPC 常量名（`src/common/constants.js`）→ bridge 的 import 与映射同步
- 上游新增 npm 依赖 → 确认 `npm install` 后 vite 构建仍通过（renderer 保持零 Electron 依赖）

## 本机环境备注（Linux 开发机）

- 项目已配置 `.mise.local.toml` 关闭 mr_boxington（mbx 缓存与 tauri build script
  metadata 有冲突，详见 https://github.com/jdx/mr-boxington）；**不要删这个文件**
- 交叉编译 mac 前置：`~/.cache/MacOSX-SDKs/MacOSX11.3.sdk` + cargo-zigbuild + zig
- CI 环境无 mbx，不受此影响

## 诊断通道（保留勿删）

`[cmd]` 命令日志（commands.rs 的 eprintln）、`[frontend]` 错误转发、
DOMCHECK（3s/8s/fn+F9）、CLICK/MOUSEDOWN/MOUSEUP 日志——用户要求长期保留，
排查线上问题靠它们（终端运行二进制即可看到）。
