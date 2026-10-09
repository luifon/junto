#!/bin/sh
# Builds out/JuntoSpike.app around the release binary and signs it ad hoc. Notifications need a bundle.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
app="$here/out/JuntoSpike.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$here/target/release/spike" "$app/Contents/MacOS/spike"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>spike</string>
    <key>CFBundleIdentifier</key><string>junto.spike.gpui-client</string>
    <key>CFBundleName</key><string>Junto gpui spike</string>
    <key>CFBundleDisplayName</key><string>Junto gpui spike</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.0.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$app"
codesign -dv "$app" 2>&1 | grep -E "Identifier|Signature|TeamIdentifier"
