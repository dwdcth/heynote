#!/bin/bash
# Assemble a minimal macOS .app bundle around the cross-compiled arm64 binary
# and pack it as tar.gz. The bundle gives the app a proper Dock/Finder icon,
# Info.plist metadata and friendlier Gatekeeper behavior compared to a bare
# executable.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$REPO_ROOT/src-tauri/target/aarch64-apple-darwin/release/heynote-tauri"
ICNS="$REPO_ROOT/src-tauri/icons/icon.icns"
PLIST="$REPO_ROOT/scripts/tauri/mac-bundle/Info.plist"
# output path (default: one level above the repo, matching the original
# local workflow); pass an argument to override, e.g. CI uses the repo root
OUT_TGZ="${1:-$REPO_ROOT/../heynote-tauri-macos-arm64.tar.gz}"
# resolve to an absolute path: the tarball is created from inside a temp dir,
# a relative OUT_TGZ would land there and be deleted by the cleanup below
case "$OUT_TGZ" in
  /*) ;;
  *) OUT_TGZ="$PWD/$OUT_TGZ" ;;
esac

[ -f "$BIN" ] || { echo "binary not found: $BIN (run cargo zigbuild first)" >&2; exit 1; }
[ -f "$ICNS" ] || { echo "icns not found: $ICNS" >&2; exit 1; }

STAGE="$(mktemp -d)"
APP="$STAGE/Heynote.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/heynote-tauri"
cp "$ICNS" "$APP/Contents/Resources/icon.icns"
cp "$PLIST" "$APP/Contents/Info.plist"
chmod +x "$APP/Contents/MacOS/heynote-tauri"

cat > "$STAGE/README-macos.txt" <<'EOF'
Heynote (Tauri) — macOS ARM64 验证版 v10（.app bundle + 焦点修复）
==================================================================

安装/运行：
  tar -xzf heynote-tauri-macos-arm64.tar.gz
  # 方式一：直接运行
  xattr -d com.apple.quarantine Heynote.app 2>/dev/null
  open Heynote.app
  # 方式二：拖入 /Applications 后从启动台打开（首次右键->打开绕过 Gatekeeper）

说明：
  - 标准 .app 结构（Info.plist + icns）：Dock/Finder 显示 Heynote 图标
  - 数据目录：~/Library/Application Support/Heynote（与 Electron 版共用）
  - 未签名：被 Gatekeeper 拦截时右键->打开，或提前 xattr 清除

v10 焦点修复（点两次/快捷键失灵）：
  1. 窗口激活时自动将 webview 设为 first responder
  2. 弹窗关闭时立即请求原生 webview 级聚焦（不再等 JS 重试失败）
  3. focus_webview 命令升级为 webview 级 set_focus
  验证：左下角 Buffers/Search 切换一次点击生效；
  Cmd+S 移动块连续多次均弹框；弹窗操作后快捷键不失效。
EOF

(cd "$STAGE" && tar -czf "$OUT_TGZ" Heynote.app README-macos.txt)
rm -rf "$STAGE"
echo "bundle written: $OUT_TGZ"
