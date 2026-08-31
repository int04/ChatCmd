#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${CHATCMD_BUILD_VERSION:-$(date '+%y.%m.%d.%H%M')}"
if [[ ! "$VERSION" =~ ^[0-9A-Za-z][0-9A-Za-z._+-]{0,79}$ ]]; then
  echo "Invalid ChatCMD version: $VERSION" >&2
  exit 1
fi
export CHATCMD_BUILD_VERSION="$VERSION"
ICON_SOURCE="$ROOT/assets/icons/logo-icon-master-1024.png"
ICONSET="$ROOT/target/chatcmd.iconset"
EXTENSION_SOURCE="$ROOT/chatgpt-extension"

TARGETS=(
  "aarch64-apple-darwin|silicon"
  "x86_64-apple-darwin|intel"
)

printf 'Building ChatCMD %s for macOS Apple Silicon + Intel\n' "$VERSION"

if [[ ! -d "$EXTENSION_SOURCE" ]]; then
  echo "ChatGPT extension folder not found: $EXTENSION_SOURCE" >&2
  exit 1
fi

cd "$ROOT/web"
npm ci
npm run build
npm run obfuscate -- dist
if find "$ROOT/web/dist" -type f -name '*.map' -print -quit | grep -q .; then
  echo "Source map files were generated in web/dist" >&2
  exit 1
fi
cd "$ROOT"

if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup is required to install both macOS Rust targets" >&2
  exit 1
fi

rustup target add aarch64-apple-darwin x86_64-apple-darwin

create_icns() {
  local destination="$1"
  if ! command -v sips >/dev/null 2>&1 || ! command -v iconutil >/dev/null 2>&1; then
    echo "sips/iconutil not found; macOS app icon cannot be generated" >&2
    return 0
  fi

  rm -rf "$ICONSET"
  mkdir -p "$ICONSET"
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$ICON_SOURCE" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    local double=$((size * 2))
    sips -z "$double" "$double" "$ICON_SOURCE" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$destination"
}

package_target() {
  local target="$1"
  local label="$2"
  local output="$ROOT/release/${VERSION}_${label}"
  local app="$output/ChatCMD.app"
  local contents="$app/Contents"
  local macos="$contents/MacOS"
  local resources="$contents/Resources"
  local binary="$ROOT/target/$target/release/chat-cmd-client"
  local archive="$output.zip"

  printf '\nBuilding Rust target %s (%s)...\n' "$target" "$label"
  cargo build --release --features embedded-web --target "$target"

  rm -rf "$output"
  mkdir -p "$macos" "$resources"
  cp "$binary" "$macos/ChatCMD"
  chmod +x "$macos/ChatCMD"
  mkdir -p "$output/chatgpt-extension"
  cp -R "$EXTENSION_SOURCE/." "$output/chatgpt-extension/"
  cd "$ROOT/web"
  npm run obfuscate -- "$output/chatgpt-extension"
  cd "$ROOT"
  create_icns "$resources/ChatCMD.icns"

  cat > "$contents/Info.plist" <<EOF
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

  rm -f "$archive"
  cd "$ROOT/release"
  zip -qry "$archive" "$(basename "$output")"
  cd "$ROOT"

  printf 'Build completed: %s\nArchive: %s\n' "$output" "$archive"
}

for entry in "${TARGETS[@]}"; do
  IFS='|' read -r target label <<< "$entry"
  package_target "$target" "$label"
done

rm -rf "$ICONSET"
printf '\nAll macOS builds completed.\n'
