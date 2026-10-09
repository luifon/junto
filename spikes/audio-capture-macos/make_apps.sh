#!/bin/sh
# Builds throwaway application bundles under out/apps so each test player is a
# separate application for macOS (own bundle identifier, started by `open`).
#   WindowedA, WindowedB: copy of ffplay (opens a small window)
#   Windowless:           copy of this spike binary (`spike tone`, no window)
#   ChildHost:            shell script that starts afplay as a child process
#   CaptureApp:           copy of this spike binary, used to run the capture as
#                         its own application instead of under the terminal
set -eu
APPS=out/apps
rm -rf "$APPS"
make_app() {
    name=$1
    source_binary=$2
    app="$APPS/$name.app"
    mkdir -p "$app/Contents/MacOS"
    cp "$source_binary" "$app/Contents/MacOS/$name"
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>spike.audiocapture.$name</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundleExecutable</key><string>$name</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
}
ffplay_binary=$(python3 -c 'import os,shutil;print(os.path.realpath(shutil.which("ffplay")))')
make_app WindowedA "$ffplay_binary"
make_app WindowedB "$ffplay_binary"
make_app Windowless target/release/spike
make_app CaptureApp target/release/spike
make_app ChildHost /bin/echo
printf '#!/bin/sh\n/usr/bin/afplay "$1"\n' > "$APPS/ChildHost.app/Contents/MacOS/ChildHost"
