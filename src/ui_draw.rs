//! Themes, small painted icons and compact widgets.

use eframe::egui;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    Light,
    Dark,
    Forest,
    Ocean,
    Sunset,
    Contrast,
}

impl Theme {
    pub const ALL: [Theme; 6] = [Theme::Light, Theme::Dark, Theme::Forest, Theme::Ocean, Theme::Sunset, Theme::Contrast];

    pub fn key(self) -> &'static str {
        match self {
            Theme::Light => "theme_light",
            Theme::Dark => "theme_dark",
            Theme::Forest => "theme_forest",
            Theme::Ocean => "theme_ocean",
            Theme::Sunset => "theme_sunset",
            Theme::Contrast => "theme_contrast",
        }
    }

    pub fn code(self) -> &'static str {
        &self.key()[6..]
    }

    pub fn from_code(c: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|t| t.code() == c)
    }
}

pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    pub on_accent: Color32,
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub sel_bg: Color32,
    pub hover: Color32,
    pub grid: Color32,
    pub bar: Color32,
    pub bar_bg: Color32,
    pub folder: Color32,
    pub warn: Color32,
    pub tiles: [Color32; 8],
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Palette {
    pub fn new(t: Theme) -> Self {
        match t {
            Theme::Light => Palette {
                dark: false,
                accent: c(0x2e8b57),
                on_accent: Color32::WHITE,
                bg: c(0xfbfbf8),
                sidebar: c(0xf0f2ec),
                card: c(0xffffff),
                text: c(0x1f2421),
                weak: c(0x6b736d),
                sel_bg: c(0xd9efe1),
                hover: c(0xeaf3ed),
                grid: c(0xe2e6e0),
                bar: c(0x3fa66b),
                bar_bg: c(0xe9eee8),
                folder: c(0xd9a441),
                warn: c(0xc0392b),
                tiles: [c(0x7fc8a0), c(0xf2c46d), c(0x8fb8e8), c(0xef9a7a), c(0xb9a3e3), c(0x9ed9d3), c(0xe6a6c4), c(0xc9cf86)],
            },
            Theme::Dark => Palette {
                dark: true,
                accent: c(0x5cc48a),
                on_accent: c(0x0e1a13),
                bg: c(0x1b1e1c),
                sidebar: c(0x222724),
                card: c(0x262b28),
                text: c(0xe6ebe7),
                weak: c(0x9aa59d),
                sel_bg: c(0x2f4a3a),
                hover: c(0x2c332f),
                grid: c(0x343b36),
                bar: c(0x4fb57c),
                bar_bg: c(0x313833),
                folder: c(0xe0b25a),
                warn: c(0xff6f5e),
                tiles: [c(0x3f8f63), c(0xb38b3c), c(0x4a77a8), c(0xae5f45), c(0x7a64a8), c(0x3e8d86), c(0xa45f80), c(0x86894a)],
            },
            Theme::Forest => Palette {
                dark: false,
                accent: c(0x3d6b35),
                on_accent: Color32::WHITE,
                bg: c(0xf4f6ef),
                sidebar: c(0xe1e9d8),
                card: c(0xfbfcf7),
                text: c(0x23301f),
                weak: c(0x667560),
                sel_bg: c(0xcfe0c3),
                hover: c(0xe4ecdc),
                grid: c(0xd5ddcb),
                bar: c(0x6aa84f),
                bar_bg: c(0xe3e9da),
                folder: c(0xa9772f),
                warn: c(0xb5432a),
                tiles: [c(0x8cb369), c(0xd4a373), c(0x5b8e7d), c(0xbc6c25), c(0xa3b18a), c(0xe9c46a), c(0x6b9080), c(0xccd5ae)],
            },
            Theme::Ocean => Palette {
                dark: true,
                accent: c(0x4fc3f7),
                on_accent: c(0x07202e),
                bg: c(0x0f1b2b),
                sidebar: c(0x132539),
                card: c(0x172c44),
                text: c(0xe3eef8),
                weak: c(0x8ea6bf),
                sel_bg: c(0x1e4766),
                hover: c(0x1a3550),
                grid: c(0x24405d),
                bar: c(0x29a8e0),
                bar_bg: c(0x1c3450),
                folder: c(0xf2c14e),
                warn: c(0xff7b7b),
                tiles: [c(0x1f7a9c), c(0x2a9d8f), c(0x3d5a99), c(0x5e60ce), c(0x4ea8de), c(0x48bfe3), c(0x6930c3), c(0x56cfe1)],
            },
            Theme::Sunset => Palette {
                dark: false,
                accent: c(0xd9622b),
                on_accent: Color32::WHITE,
                bg: c(0xfff8f1),
                sidebar: c(0xfbe8d8),
                card: c(0xffffff),
                text: c(0x3a2418),
                weak: c(0x8a6a58),
                sel_bg: c(0xfbd9c2),
                hover: c(0xfdeadc),
                grid: c(0xf1dccb),
                bar: c(0xe8834f),
                bar_bg: c(0xf7e6d8),
                folder: c(0xe0a03a),
                warn: c(0xb3261e),
                tiles: [c(0xf4a261), c(0xe76f51), c(0xe9c46a), c(0xd88c9a), c(0xf2cc8f), c(0xc97c5d), c(0xb5838d), c(0xffb4a2)],
            },
            Theme::Contrast => Palette {
                dark: true,
                accent: c(0xffd400),
                on_accent: Color32::BLACK,
                bg: Color32::BLACK,
                sidebar: c(0x0d0d0d),
                card: c(0x111111),
                text: Color32::WHITE,
                weak: c(0xd0d0d0),
                sel_bg: c(0x3a3300),
                hover: c(0x262626),
                grid: c(0x777777),
                bar: c(0xffd400),
                bar_bg: c(0x333333),
                folder: c(0xffd400),
                warn: c(0xff5050),
                tiles: [c(0xffd400), c(0x00e5ff), c(0xff5ec4), c(0x7cff4f), c(0xff9100), c(0xb388ff), c(0xffffff), c(0x00c853)],
            },
        }
    }

    pub fn tile(&self, i: usize, level: usize) -> Color32 {
        let b = self.tiles[i % self.tiles.len()];
        let k = 1.0 - level as f32 * 0.09;
        Color32::from_rgb((b.r() as f32 * k) as u8, (b.g() as f32 * k) as u8, (b.b() as f32 * k) as u8)
    }

    /// Text colour that reads on a tile.
    pub fn on_tile(&self, col: Color32) -> Color32 {
        let l = 0.299 * col.r() as f32 + 0.587 * col.g() as f32 + 0.114 * col.b() as f32;
        if l > 140.0 { Color32::from_gray(25) } else { Color32::from_gray(240) }
    }
}

#[derive(Clone, Copy)]
pub enum Glyph {
    Folder,
    Refresh,
    Stop,
    Up,
    Export,
    Gear,
    Home,
    Globe,
    Download,
    Doc,
    Clock,
    Compare,
    Shield,
    Trash,
    Filter,
    Dups,
    Broom,
}

/// Square icon button with a tooltip.
pub fn icon_button(ui: &mut Ui, pal: &Palette, g: Glyph, tip: &str, enabled: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), sense);
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, pal.hover);
    }
    let col = if enabled { pal.text } else { pal.grid };
    glyph(ui.painter(), g, rect.center(), col, pal);
    resp.on_hover_text(tip)
}

/// Filled accent "pill" button with an icon and text.
pub fn primary_button(ui: &mut Ui, pal: &Palette, g: Glyph, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.5), pal.on_accent);
    let w = galley.size().x + 46.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let fill = if resp.hovered() { pal.accent.gamma_multiply(0.88) } else { pal.accent };
    ui.painter().rect_filled(rect, 17.0, fill);
    glyph(ui.painter(), g, pos2(rect.left() + 20.0, rect.center().y), pal.on_accent, pal);
    ui.painter().galley(pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0), galley, pal.on_accent);
    resp
}

/// Segmented control; returns the newly picked index.
pub fn segmented(ui: &mut Ui, pal: &Palette, items: &[&str], current: usize) -> Option<usize> {
    let mut picked = None;
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = items.iter().map(|s| ui.painter().layout_no_wrap(s.to_string(), font.clone(), pal.text).size().x + 20.0).collect();
    let total: f32 = widths.iter().sum();
    // the strip's own automatic id is unique per control, even when two
    // controls show the same first label ("Any" for size and for age)
    let (rect, strip) = ui.allocate_exact_size(vec2(total + 4.0, 26.0), Sense::hover());
    ui.painter().rect_filled(rect, 13.0, pal.bar_bg);
    let mut x = rect.left() + 2.0;
    for (i, s) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(widths[i], 22.0));
        let resp = ui.interact(r, strip.id.with(("seg", i)), Sense::click());
        let active = i == current;
        if active {
            ui.painter().rect_filled(r, 11.0, pal.card);
            ui.painter().rect_stroke(r, 11.0, Stroke::new(1.0, pal.grid), StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 11.0, pal.hover);
        }
        let col = if active { pal.accent } else { pal.weak };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *s, font.clone(), col);
        if resp.clicked() {
            picked = Some(i);
        }
        x += widths[i];
    }
    picked
}

/// The app's mark: a small round tree.
pub fn logo(p: &egui::Painter, c: Pos2, pal: &Palette) {
    p.rect_filled(Rect::from_center_size(c + vec2(0.0, 7.0), vec2(4.0, 10.0)), 1.0, pal.folder);
    p.circle_filled(c + vec2(-5.0, -1.0), 6.0, pal.accent.gamma_multiply(0.8));
    p.circle_filled(c + vec2(5.0, -1.0), 6.0, pal.accent.gamma_multiply(0.9));
    p.circle_filled(c + vec2(0.0, -7.0), 7.0, pal.accent);
}

pub fn glyph(p: &egui::Painter, g: Glyph, c: Pos2, col: Color32, pal: &Palette) {
    let s = Stroke::new(1.8, col);
    match g {
        Glyph::Folder => {
            p.rect_stroke(Rect::from_min_size(c + vec2(-8.0, -6.0), vec2(16.0, 12.0)), 2.0, s, StrokeKind::Inside);
            p.line_segment([c + vec2(-8.0, -3.0), c + vec2(-1.0, -3.0)], s);
            p.line_segment([c + vec2(-1.0, -3.0), c + vec2(1.0, -6.0)], s);
        }
        Glyph::Refresh => {
            // circular arrow: an open ring drawn clockwise, arrowhead at its end
            let r = 6.5;
            let a0 = (-60f32).to_radians();
            let a1 = (240f32).to_radians();
            let pts: Vec<Pos2> = (0..=28)
                .map(|i| {
                    let a = a0 + (a1 - a0) * i as f32 / 28.0;
                    c + vec2(r * a.cos(), r * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(2.0, col)));
            let end = c + vec2(r * a1.cos(), r * a1.sin());
            let tan = vec2(-a1.sin(), a1.cos()); // direction of travel at the end
            let nrm = vec2(a1.cos(), a1.sin());
            p.add(egui::Shape::convex_polygon(
                vec![end + tan * 4.5, end - tan * 1.5 + nrm * 4.0, end - tan * 1.5 - nrm * 4.0],
                col,
                Stroke::NONE,
            ));
        }
        Glyph::Trash => {
            p.line_segment([c + vec2(-7.0, -5.0), c + vec2(7.0, -5.0)], s);
            p.line_segment([c + vec2(-2.5, -5.0), c + vec2(-2.0, -8.0)], s);
            p.line_segment([c + vec2(-2.0, -8.0), c + vec2(2.0, -8.0)], s);
            p.line_segment([c + vec2(2.0, -8.0), c + vec2(2.5, -5.0)], s);
            p.add(egui::Shape::line(vec![c + vec2(-5.5, -4.0), c + vec2(-4.5, 8.0), c + vec2(4.5, 8.0), c + vec2(5.5, -4.0)], s));
            p.line_segment([c + vec2(-1.5, -1.0), c + vec2(-1.5, 5.0)], Stroke::new(1.2, col));
            p.line_segment([c + vec2(1.5, -1.0), c + vec2(1.5, 5.0)], Stroke::new(1.2, col));
        }
        Glyph::Filter => {
            p.add(egui::Shape::closed_line(vec![c + vec2(-8.0, -7.0), c + vec2(8.0, -7.0), c + vec2(2.0, 0.0), c + vec2(2.0, 7.0), c + vec2(-2.0, 5.0), c + vec2(-2.0, 0.0)], s));
        }
        Glyph::Dups => {
            p.rect_stroke(Rect::from_min_size(c + vec2(-8.0, -8.0), vec2(10.0, 12.0)), 2.0, s, StrokeKind::Inside);
            p.rect_filled(Rect::from_min_size(c + vec2(-2.0, -4.0), vec2(10.0, 12.0)), 2.0, pal.card);
            p.rect_stroke(Rect::from_min_size(c + vec2(-2.0, -4.0), vec2(10.0, 12.0)), 2.0, s, StrokeKind::Inside);
            p.line_segment([c + vec2(1.0, 2.0), c + vec2(5.0, 2.0)], Stroke::new(1.2, col));
        }
        Glyph::Broom => {
            p.line_segment([c + vec2(6.0, -8.0), c + vec2(-1.0, 2.0)], Stroke::new(2.2, col));
            p.add(egui::Shape::closed_line(vec![c + vec2(-4.0, 0.0), c + vec2(2.0, 4.0), c + vec2(-1.0, 9.0), c + vec2(-9.0, 4.0)], s));
            p.line_segment([c + vec2(-3.5, 3.5), c + vec2(-5.5, 7.0)], Stroke::new(1.2, col));
        }
        Glyph::Compare => {
            p.rect_stroke(Rect::from_min_size(c + vec2(-9.0, -7.0), vec2(7.0, 14.0)), 1.5, s, StrokeKind::Inside);
            p.rect_stroke(Rect::from_min_size(c + vec2(2.0, -7.0), vec2(7.0, 14.0)), 1.5, s, StrokeKind::Inside);
            p.line_segment([c + vec2(-6.5, -2.0), c + vec2(-4.5, -2.0)], Stroke::new(1.2, col));
            p.line_segment([c + vec2(4.5, 2.0), c + vec2(6.5, 2.0)], Stroke::new(1.2, col));
        }
        Glyph::Shield => {
            let pts = vec![c + vec2(0.0, -8.0), c + vec2(7.0, -5.0), c + vec2(6.0, 3.0), c + vec2(0.0, 8.0), c + vec2(-6.0, 3.0), c + vec2(-7.0, -5.0)];
            p.add(egui::Shape::closed_line(pts, s));
            p.line_segment([c + vec2(-3.0, 0.0), c + vec2(-0.5, 2.5)], s);
            p.line_segment([c + vec2(-0.5, 2.5), c + vec2(3.5, -2.5)], s);
        }
        Glyph::Stop => {
            p.rect_stroke(Rect::from_center_size(c, vec2(13.0, 13.0)), 3.0, s, StrokeKind::Inside);
            p.rect_filled(Rect::from_center_size(c, vec2(5.0, 5.0)), 1.0, col);
        }
        Glyph::Up => {
            p.line_segment([c + vec2(0.0, 7.0), c + vec2(0.0, -7.0)], s);
            p.line_segment([c + vec2(0.0, -7.0), c + vec2(-5.0, -2.0)], s);
            p.line_segment([c + vec2(0.0, -7.0), c + vec2(5.0, -2.0)], s);
        }
        Glyph::Export => {
            p.line_segment([c + vec2(-7.0, 2.0), c + vec2(-7.0, 7.0)], s);
            p.line_segment([c + vec2(-7.0, 7.0), c + vec2(7.0, 7.0)], s);
            p.line_segment([c + vec2(7.0, 7.0), c + vec2(7.0, 2.0)], s);
            p.line_segment([c + vec2(0.0, 3.0), c + vec2(0.0, -8.0)], s);
            p.line_segment([c + vec2(0.0, -8.0), c + vec2(-4.0, -4.0)], s);
            p.line_segment([c + vec2(0.0, -8.0), c + vec2(4.0, -4.0)], s);
        }
        Glyph::Gear => {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                p.line_segment([c + vec2(5.0 * a.cos(), 5.0 * a.sin()), c + vec2(8.5 * a.cos(), 8.5 * a.sin())], Stroke::new(2.6, col));
            }
            p.circle_stroke(c, 5.0, s);
            p.circle_filled(c, 1.8, col);
        }
        Glyph::Home => {
            p.line_segment([c + vec2(-8.0, -1.0), c + vec2(0.0, -8.0)], s);
            p.line_segment([c + vec2(0.0, -8.0), c + vec2(8.0, -1.0)], s);
            p.rect_stroke(Rect::from_min_max(c + vec2(-5.5, -2.0), c + vec2(5.5, 7.0)), 1.0, s, StrokeKind::Inside);
        }
        Glyph::Globe => {
            p.circle_stroke(c, 7.5, s);
            p.line_segment([c + vec2(-7.5, 0.0), c + vec2(7.5, 0.0)], Stroke::new(1.2, col));
            p.add(egui::Shape::ellipse_stroke(c, vec2(3.5, 7.5), Stroke::new(1.2, col)));
        }
        Glyph::Download => {
            p.line_segment([c + vec2(0.0, -8.0), c + vec2(0.0, 3.0)], s);
            p.line_segment([c + vec2(0.0, 3.0), c + vec2(-4.0, -1.0)], s);
            p.line_segment([c + vec2(0.0, 3.0), c + vec2(4.0, -1.0)], s);
            p.line_segment([c + vec2(-7.0, 7.0), c + vec2(7.0, 7.0)], s);
        }
        Glyph::Doc => {
            p.rect_stroke(Rect::from_center_size(c, vec2(12.0, 16.0)), 2.0, s, StrokeKind::Inside);
            p.line_segment([c + vec2(-3.0, -2.0), c + vec2(3.0, -2.0)], Stroke::new(1.2, col));
            p.line_segment([c + vec2(-3.0, 2.0), c + vec2(3.0, 2.0)], Stroke::new(1.2, col));
        }
        Glyph::Clock => {
            p.circle_stroke(c, 7.5, s);
            p.line_segment([c, c + vec2(0.0, -5.0)], s);
            p.line_segment([c, c + vec2(4.0, 2.0)], s);
        }
    }
    let _ = pal;
}

pub fn disk_icon(p: &egui::Painter, c: Pos2, col: Color32) {
    let r = Rect::from_center_size(c, vec2(18.0, 12.0));
    p.rect_stroke(r, 3.0, Stroke::new(1.6, col), StrokeKind::Inside);
    p.line_segment([pos2(r.left() + 2.0, r.center().y + 1.0), pos2(r.right() - 2.0, r.center().y + 1.0)], Stroke::new(1.0, col));
    p.circle_filled(pos2(r.right() - 4.5, r.bottom() - 3.0), 1.3, col);
}

/// Expander arrow of the tree: one joined stroke, so its tip is drawn whole.
pub fn chevron(p: &egui::Painter, c: Pos2, open: bool, col: Color32) {
    let pts = if open {
        vec![c + vec2(-4.5, -2.0), c + vec2(0.0, 2.5), c + vec2(4.5, -2.0)]
    } else {
        vec![c + vec2(-1.5, -4.5), c + vec2(3.0, 0.0), c + vec2(-1.5, 4.5)]
    };
    p.add(egui::Shape::line(pts, Stroke::new(2.0, col)));
}

pub fn sort_arrow(p: &egui::Painter, c: Pos2, up: bool, col: Color32) {
    let s = Stroke::new(1.5, col);
    let d = if up { -1.0 } else { 1.0 };
    p.line_segment([c + vec2(-3.5, -1.5 * d), c + vec2(0.0, 2.0 * d)], s);
    p.line_segment([c + vec2(0.0, 2.0 * d), c + vec2(3.5, -1.5 * d)], s);
}

pub fn folder_icon(p: &egui::Painter, c: Pos2, col: Color32) {
    p.rect_filled(Rect::from_min_size(pos2(c.x - 8.0, c.y - 5.5), vec2(7.0, 4.0)), 2.0, col);
    p.rect_filled(Rect::from_min_size(pos2(c.x - 8.0, c.y - 3.5), vec2(16.0, 10.5)), 2.5, col);
}

pub fn file_icon(p: &egui::Painter, c: Pos2, col: Color32, link: bool, accent: Color32) {
    let r = Rect::from_center_size(c, vec2(11.0, 14.0));
    p.rect_stroke(r, 2.0, Stroke::new(1.3, col), StrokeKind::Inside);
    if link {
        p.circle_filled(pos2(r.right() - 1.0, r.bottom() - 1.0), 3.0, accent);
    }
}
