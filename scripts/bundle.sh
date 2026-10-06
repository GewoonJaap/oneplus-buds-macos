#!/usr/bin/env bash
# Build target/release/Buds.app (macOS ties Bluetooth permission to the bundle).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --bin buds-daemon
swiftc -O -parse-as-library -o target/release/buds-ui swift/BudsApp.swift
APP=target/release/Buds.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/buds-ui "$APP/Contents/MacOS/buds-ui"
cp target/release/buds-daemon "$APP/Contents/MacOS/buds-daemon"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>local.buds.menubar</string>
  <key>CFBundleName</key><string>Buds</string>
  <key>CFBundleDisplayName</key><string>Buds</string>
  <key>CFBundleExecutable</key><string>buds-ui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSBluetoothAlwaysUsageDescription</key><string>Buds uses Bluetooth to read and change the noise control mode of your earbuds.</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist"
codesign --force --deep -s - "$APP"
codesign -v "$APP"
echo "Built $APP"
