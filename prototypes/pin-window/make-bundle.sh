#!/usr/bin/env bash
# 打成真正的 .app bundle，并以 LSUIElement=1 在启动时固化 accessory 身份。
# 裸二进制没有 Info.plist，macOS 对其 Space 参与行为的处理与正常 App 不同。
set -euo pipefail
cd "$(dirname "$0")"
APP="PinWallProto.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/pin-window "$APP/Contents/MacOS/"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>pin-window</string>
  <key>CFBundleIdentifier</key><string>dev.pinwall.proto</string>
  <key>CFBundleName</key><string>PinWallProto</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <!-- 关键：启动即为 accessory 身份，不占 Dock，不强制切换 Space -->
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
PLIST
echo '</plist>' >> "$APP/Contents/Info.plist"
echo "已生成 $APP"
