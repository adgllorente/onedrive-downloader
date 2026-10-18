#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --release

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
APP="dist/Copiar de OneDrive.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 755 target/release/onedrive-copier "$APP/Contents/MacOS/onedrive-copier"
install -m 644 assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

RCLONE=${RCLONE_BIN:-$(command -v rclone || true)}
if [ -n "$RCLONE" ]; then
  install -m 755 "$RCLONE" "$APP/Contents/MacOS/rclone"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Copiar de OneDrive</string>
  <key>CFBundleDisplayName</key><string>Copiar de OneDrive</string>
  <key>CFBundleIdentifier</key><string>io.github.adgllorente.onedrive-copier</string>
  <key>CFBundleExecutable</key><string>onedrive-copier</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"

echo "Aplicación creada en: $APP (versión $VERSION)"
