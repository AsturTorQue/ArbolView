#!/bin/sh
# Build ArbolView-<version>-<arch>.AppImage in app/dist/ for this machine
# (x86_64 or aarch64). Needs: cargo, mksquashfs, and the AppImage type-2
# runtime (downloaded once into packaging/ when missing, or pass RUNTIME=...).
set -eu

APP_DIR=$(cd "$(dirname "$0")/.." && pwd)
cd "$APP_DIR"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
# x86_64 or aarch64 (64-bit ARM): the machine's own unless ARCH is set
ARCH=${ARCH:-$(uname -m)}
case "$ARCH" in
    arm64) ARCH=aarch64 ;;
    amd64) ARCH=x86_64 ;;
esac
OUT="dist/ArbolView-$VERSION-$ARCH.AppImage"
RUNTIME=${RUNTIME:-packaging/runtime-$ARCH}
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-$ARCH"

# Keep the build folder's location out of the program (debug and panic paths
# then read "arbolview/src/..."). The 0x1f separator allows spaces in paths.
SEP=$(printf '\037')
CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$APP_DIR=arbolview${SEP}--remap-path-prefix=$(pwd -P)=arbolview"
export CARGO_ENCODED_RUSTFLAGS
cargo build --release

if [ ! -s "$RUNTIME" ]; then
    echo "Downloading the AppImage runtime..."
    curl -fL --retry 3 -o "$RUNTIME.part" "$RUNTIME_URL"
    mv "$RUNTIME.part" "$RUNTIME"
fi

APPDIR=$(mktemp -d "${TMPDIR:-/tmp}/arbolview-appdir.XXXXXX")
trap 'rm -rf "$APPDIR"' EXIT
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/256x256/apps"
install -m 755 target/release/arbolview "$APPDIR/usr/bin/arbolview"
strip "$APPDIR/usr/bin/arbolview" 2>/dev/null || true
target/release/arbolview --write-icon "$APPDIR/arbolview.png"
cp "$APPDIR/arbolview.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/arbolview.png"
ln -s arbolview.png "$APPDIR/.DirIcon"

cat > "$APPDIR/arbolview.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=ÁrbolView
GenericName=Disk usage
Comment=See which folders fill your disks
Comment[es]=Mira qué carpetas llenan tus discos
Exec=arbolview %f
Icon=arbolview
Terminal=false
Categories=System;Utility;FileTools;
StartupWMClass=arbolview
Keywords=disk;space;size;folder;
EOF
cp "$APPDIR/arbolview.desktop" "$APPDIR/usr/share/applications/arbolview.desktop"

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
exec "$HERE/usr/bin/arbolview" "$@"
EOF
chmod 755 "$APPDIR/AppRun"

mkdir -p dist
rm -f "$OUT" "$OUT.squashfs"
mksquashfs "$APPDIR" "$OUT.squashfs" -root-owned -noappend -comp gzip -quiet
cat "$RUNTIME" "$OUT.squashfs" > "$OUT"
rm -f "$OUT.squashfs"
chmod 755 "$OUT"
echo "$OUT"
