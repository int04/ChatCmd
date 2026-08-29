#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(date '+%y.%m.%d.%H%M')"
export CHATCMD_BUILD_VERSION="$VERSION"
ARCH="$(uname -m)"
OUTPUT="$ROOT/release/ChatCMD-$VERSION-macos-$ARCH"
APP="$OUTPUT/ChatCMD.app"
CONTENTS="$APP/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"
ICONSET="$ROOT/target/chatcmd.iconset"
ICNS="$RESOURCES/ChatCMD.icns"

printf 'Building ChatCMD %s for macOS %s\n' "$VERSION" "$ARCH"

cd "$ROOT/web"
npm ci
npm run build
if find "$ROOT/web/dist" -type f -name '*.map' -print -quit | grep -q .; then
  echo "Source map files were generated in web/dist" >&2
  exit 1
fi
cd "$ROOT"

cargo build --release --features embedded-web

rm -rf "$OUTPUT"
mkdir -p "$MACOS" "$RESOURCES"
cp "$ROOT/target/release/chat-cmd-client" "$MACOS/ChatCMD"
chmod +x "$MACOS/ChatCMD"

if command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
  rm -rf "$ICONSET"
  mkdir -p "$ICONSET"
  SOURCE="$ROOT/assets/icons/logo-icon-master-1024.png"
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$SOURCE" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$SOURCE" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$ICNS"
fi

cat > "$CONTENTS/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key><string>ChatCMD</string>
  <key>CFBundleExecutable</key><string>ChatCMD</string>
  <key>CFBundleIdentifier</key><string>com.chatcmd.client</string>
  <key>CFBundleName</key><string>ChatCMD</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSUIElement</key><true/>
  <key>CFBundleIconFile</key><string>ChatCMD</string>
</dict>
</plist>
EOF

ARCHIVE="$OUTPUT.zip"
rm -f "$ARCHIVE"
cd "$ROOT/release"
zip -qry "$ARCHIVE" "$(basename "$OUTPUT")"

printf 'Build completed: %s\nArchive: %s\n' "$OUTPUT" "$ARCHIVE"
