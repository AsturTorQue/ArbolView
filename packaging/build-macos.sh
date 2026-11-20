#!/bin/sh
# Build ArbolView-<version>-macOS.dmg in app/dist/ — run on a Mac with Rust
# (rustup target add aarch64-apple-darwin x86_64-apple-darwin).
# Makes one universal program (Apple Silicon + Intel) inside ÁrbolView.app.
set -eu

APP_DIR=$(cd "$(dirname "$0")/.." && pwd)
cd "$APP_DIR"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$APP_DIR=arbolview"
export CARGO_ENCODED_RUSTFLAGS

cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

BUNDLE="dist/ÁrbolView.app"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
lipo -create -output "$BUNDLE/Contents/MacOS/arbolview" \
    target/aarch64-apple-darwin/release/arbolview \
    target/x86_64-apple-darwin/release/arbolview
cp packaging/arbolview.icns "$BUNDLE/Contents/Resources/arbolview.icns"

cat > "$BUNDLE/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>ÁrbolView</string>
  <key>CFBundleDisplayName</key><string>ÁrbolView</string>
  <key>CFBundleIdentifier</key><string>app.arbolview</string>
  <key>CFBundleExecutable</key><string>arbolview</string>
  <key>CFBundleIconFile</key><string>arbolview.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
</dict></plist>
EOF

# ad-hoc signature so Apple Silicon runs it (Gatekeeper still asks on first open)
codesign --force --deep --sign - "$BUNDLE" 2>/dev/null || true

DMG="dist/ArbolView-$VERSION-macOS.dmg"
rm -f "$DMG"
hdiutil create -volname "ÁrbolView" -srcfolder "$BUNDLE" -ov -format UDZO "$DMG" >/dev/null
echo "$DMG"
