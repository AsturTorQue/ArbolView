//! ÁrbolView's application icon, painted in code, and the desktop entry that
//! lets Wayland compositors show it (they look the icon up by app id, not
//! from the window).

use image::ImageEncoder;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "arbolview";
const SIZE: u32 = 256;

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// Coverage of a rounded rectangle at (x, y), anti-aliased over one pixel.
fn rounded(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r;
    (0.5 - d).clamp(0.0, 1.0)
}

fn circle(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r;
    (0.5 - d).clamp(0.0, 1.0)
}

/// 256×256 RGBA: a green rounded tile with a round tree and three size bars.
pub fn rgba() -> Vec<u8> {
    let s = SIZE as f32;
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let bg_top = [0.20, 0.62, 0.40, 1.0];
    let bg_bottom = [0.13, 0.45, 0.29, 1.0];
    let leaf = [0.72, 0.92, 0.66, 1.0];
    let leaf_dark = [0.55, 0.84, 0.52, 1.0];
    let trunk = [0.85, 0.64, 0.25, 1.0];
    let bar = [1.0, 1.0, 1.0, 0.92];
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let tile = rounded(x, y, 8.0, 8.0, s - 8.0, s - 8.0, 52.0);
            let mut c = mix(bg_top, bg_bottom, y / s);
            c[3] = tile;
            // canopy
            let canopy = circle(x, y, 92.0, 104.0, 36.0).max(circle(x, y, 132.0, 82.0, 44.0)).max(circle(x, y, 168.0, 108.0, 34.0));
            let shade = circle(x, y, 150.0, 120.0, 30.0) * canopy;
            c = mix(c, [leaf[0], leaf[1], leaf[2], c[3]], canopy * tile);
            c = mix(c, [leaf_dark[0], leaf_dark[1], leaf_dark[2], c[3]], shade * tile * 0.8);
            // trunk
            let tr = rounded(x, y, 120.0, 120.0, 140.0, 178.0, 6.0) * tile;
            c = mix(c, [trunk[0], trunk[1], trunk[2], c[3]], tr);
            // size bars (largest to smallest)
            for (i, w) in [118.0, 84.0, 52.0].iter().enumerate() {
                let y0 = 188.0 + i as f32 * 16.0;
                let b = rounded(x, y, 69.0, y0, 69.0 + w, y0 + 9.0, 4.5) * tile;
                c = mix(c, [bar[0], bar[1], bar[2], c[3]], b * bar[3]);
            }
            out.extend_from_slice(&[(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8]);
        }
    }
    out
}

fn png_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    let _ = image::codecs::png::PngEncoder::new(&mut out).write_image(&rgba(), SIZE, SIZE, image::ExtendedColorType::Rgba8);
    out
}

/// Windows .ico holding the 256×256 PNG (supported since Windows Vista).
pub fn ico() -> Vec<u8> {
    let png = png_bytes();
    let mut v = vec![0, 0, 1, 0, 1, 0]; // reserved, type icon, one image
    v.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0]); // 256×256 (0 means 256), no palette, 1 plane, 32 bpp
    v.extend_from_slice(&(png.len() as u32).to_le_bytes());
    v.extend_from_slice(&22u32.to_le_bytes()); // data right after this 22-byte header
    v.extend_from_slice(&png);
    v
}

/// macOS .icns holding the 256×256 PNG ("ic08").
pub fn icns() -> Vec<u8> {
    let png = png_bytes();
    let mut v = b"icns".to_vec();
    v.extend_from_slice(&((8 + 8 + png.len()) as u32).to_be_bytes());
    v.extend_from_slice(b"ic08");
    v.extend_from_slice(&((8 + png.len()) as u32).to_be_bytes());
    v.extend_from_slice(&png);
    v
}

pub fn icon_data() -> egui::IconData {
    egui::IconData { rgba: rgba(), width: SIZE, height: SIZE }
}

fn data_home() -> Option<PathBuf> {
    Some(crate::platform::data_dir())
}

/// Write the icon and a desktop entry for this program into the user's data
/// folder, so the panel and the window list show ÁrbolView's icon on Wayland.
/// Rewritten only when the program has moved or the files are missing.
pub fn install_desktop_entry() {
    // Windows and macOS take the icon from the program and its bundle
    if !cfg!(target_os = "linux") {
        return;
    }
    if crate::fsinfo::is_root_user() {
        return; // a root window must not write into root's or the user's folders
    }
    let (Some(base), Some(exe)) = (data_home(), crate::fsinfo::self_exe()) else { return };
    let icon_dir = base.join("icons/hicolor/256x256/apps");
    let icon = icon_dir.join(format!("{APP_ID}.png"));
    let apps = base.join("applications");
    let desktop = apps.join(format!("{APP_ID}.desktop"));
    let exec = exe.to_string_lossy().replace('"', "\\\"");
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=ÁrbolView\nGenericName=Disk usage\nComment=See which folders fill your disks\nComment[es]=Mira qué carpetas llenan tus discos\nExec=\"{exec}\" %f\nIcon={APP_ID}\nTerminal=false\nCategories=System;Utility;FileTools;\nStartupWMClass={APP_ID}\nKeywords=disk;space;size;folder;\n"
    );
    let current = std::fs::read_to_string(&desktop).unwrap_or_default();
    if current == entry && icon.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&icon_dir);
    let _ = std::fs::create_dir_all(&apps);
    let _ = image::save_buffer(&icon, &rgba(), SIZE, SIZE, image::ColorType::Rgba8);
    let _ = std::fs::write(&desktop, entry);
    refresh_caches(&base, &apps);
}

fn refresh_caches(base: &Path, apps: &Path) {
    let _ = std::process::Command::new("update-desktop-database").arg(apps).output();
    let _ = std::process::Command::new("gtk-update-icon-cache").arg("-f").arg("-t").arg(base.join("icons/hicolor")).output();
}
