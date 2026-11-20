# Packaging ÁrbolView

One program for Linux, Windows and macOS. Each system builds its own package:

| System  | Package                                   | How |
|---------|-------------------------------------------|-----|
| Linux   | `dist/ArbolView-<ver>-x86_64.AppImage`     | `sh packaging/build-appimage.sh` (needs `mksquashfs`) |
| Windows | `dist/ArbolView-<ver>-windows-x64.zip`     | `powershell -File packaging\build-windows.ps1` on Windows with Rust |
| macOS   | `dist/ArbolView-<ver>-macOS.dmg` (universal) | `sh packaging/build-macos.sh` on a Mac with Rust and both Apple targets |

Or let GitHub build all three: put this `app/` folder in a repository (as its root) and run
the **release** workflow from the Actions tab, or push a tag like `v0.3.0` to publish them
as a release.

Icons: `arbolview --write-icon file.png|file.ico|file.icns` writes the app's icon; the
`.ico` is built into the Windows `.exe` and the `.icns` goes into the macOS bundle.

What differs per system:

- **Root / administrator**: Linux asks through `pkexec`, Windows through the UAC prompt; on
  macOS the button is hidden (apps can't reopen themselves as root there).
- **Trash**: Linux uses the freedesktop trash with Undo; Windows the Recycle Bin with Undo;
  macOS the Trash (Undo there is Finder ▸ Put Back).
- **Scheduled scans**: systemd user timers (Linux), launchd agents (macOS), Task Scheduler
  (Windows).
- **Quick clean-up**: each system lists its own caches, trash and temporary files.
- **Permissions column**: on Windows it shows read-only/hidden attributes in the same style,
  and the Owner column stays empty.
