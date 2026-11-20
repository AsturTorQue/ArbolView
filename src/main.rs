// On Windows the window opens without a console; commands attach to the one they were typed in.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod fsinfo;
mod i18n;
mod cli;
mod icon;
mod monitor;
mod platform;
mod report;
mod tools;
mod scan;
mod treemap;
mod ui_draw;

use eframe::egui;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui};
use fsinfo::{format_pct, format_size, format_time, group, Mount, Unit};
use i18n::{t, Lang};
use scan::{BigFile, Node, Progress, ScanMsg, ScanOptions, SortKey};
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui_draw::*;

const APP_NAME: &str = "ÁrbolView";
const ROW_H: f32 = 26.0;
const INDENT: f32 = 18.0;
const DEFAULT_WIDTHS: [f32; 9] = [300.0, 92.0, 92.0, 80.0, 74.0, 130.0, 128.0, 90.0, 108.0];
const COL_KEYS: [(&str, SortKey); 9] = [
    ("col_name", SortKey::Name),
    ("col_size", SortKey::Size),
    ("col_alloc", SortKey::Alloc),
    ("col_files", SortKey::Files),
    ("col_folders", SortKey::Folders),
    ("col_share", SortKey::Percent),
    ("col_modified", SortKey::Modified),
    ("col_owner", SortKey::Owner),
    ("col_perms", SortKey::Perms),
];

/// Owner (7) and permissions (8) are Unix ideas: Windows doesn't show them.
fn col_visible(i: usize) -> bool {
    platform::HAS_POSIX_PERMS || i < 7
}

/// The comparer's state: two folders scanned on their own thread, then diffed.
struct Compare {
    a: String,
    b: String,
    running: bool,
    rx: Option<Receiver<(Option<Node>, Option<Node>)>>,
    prog_a: Arc<Progress>,
    prog_b: Arc<Progress>,
    totals: Option<(u64, u64, u64, u64)>, // size A, size B, files A, files B
    rows: Vec<scan::DiffRow>,
    show: Option<scan::DiffKind>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Basis {
    Size,
    Alloc,
    Files,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Map,
    Types,
    Largest,
    Age,
    Changes,
    Disks,
}

#[derive(Clone)]
struct Row {
    path: PathBuf,
    depth: usize,
    name: String,
    is_dir: bool,
    is_link: bool,
    size: u64,
    alloc: u64,
    files: u64,
    folders: u64,
    mtime: i64,
    uid: u32,
    gid: u32,
    mode: u32,
    pct: f64,
    has_children: bool,
    denied: bool,
    other_fs: bool,
}

struct App {
    root_path: PathBuf,
    root: Node,
    scanning: bool,
    rx: Option<Receiver<ScanMsg>>,
    prog: Arc<Progress>,
    started: Instant,
    elapsed: Duration,
    top_files: Vec<BigFile>,
    message: Option<String>,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    unit: Unit,
    basis: Basis,
    show_files: bool,
    sort: (SortKey, bool),
    opts: ScanOptions,
    owners: HashMap<u32, String>,
    groups: HashMap<u32, String>,
    compare: Option<Compare>,
    tab: Tab,
    mounts: Vec<Mount>,
    show_pseudo: bool,
    theme: Theme,
    lang: Lang,
    browser: Option<PathBuf>,
    show_settings: bool,
    filter: String,
    filter_cache: Option<((String, u64, u64, u8), Vec<Row>)>,
    ext_cache: Option<(PathBuf, u64, Vec<(String, u64, u64)>)>,
    widths: [f32; 9],
    history: Vec<PathBuf>,
    screenshot: Option<PathBuf>,
    frame: u64,
    shot_requested: bool,
    filters: tools::Filters,
    show_filters: bool,
    confirm_trash: Option<PathBuf>,
    /// The last trash action (one or more entries), for "Undo".
    undo: Vec<(tools::TrashRecord, Option<Node>)>,
    dups: Option<DupState>,
    cleanup: Option<CleanState>,
    snap_saved: Option<PathBuf>,
    changes: ChangesState,
    last_export: Option<PathBuf>,
    /// (root, schedule) as last read from the user's timers; refreshed on change.
    schedule: Option<(PathBuf, Option<monitor::Every>)>,
}

struct DupState {
    min: u64,
    dir: PathBuf,
    running: bool,
    prog: Arc<tools::DupProgress>,
    rx: Option<Receiver<Vec<tools::DupGroup>>>,
    groups: Option<Vec<tools::DupGroup>>,
}

struct CleanState {
    items: tools::Shared<Vec<tools::CleanItem>>,
    measuring: Arc<std::sync::atomic::AtomicBool>,
    running: bool,
    confirm: bool,
    rx: Option<Receiver<(u64, Vec<&'static str>)>>,
    result: Option<String>,
}

#[derive(Default)]
struct ChangesState {
    pick: usize,
    depth: usize,
    cache: Option<(PathBuf, HashMap<String, u64>)>,
    rows_key: Option<(PathBuf, u64, u64, usize)>,
    rows: Vec<tools::ChangeRow>,
}

fn home() -> PathBuf {
    platform::home()
}

fn config_path() -> PathBuf {
    platform::config_dir().join("arbolview").join("settings")
}


fn key_cmp(k: SortKey, a: &Node, b: &Node) -> std::cmp::Ordering {
    match k {
        SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortKey::Alloc => a.alloc.cmp(&b.alloc),
        SortKey::Files => a.files.cmp(&b.files),
        SortKey::Folders => a.folders.cmp(&b.folders),
        SortKey::Modified => a.mtime.cmp(&b.mtime),
        SortKey::Owner => a.uid.cmp(&b.uid),
        SortKey::Perms => (a.mode & 0o7777).cmp(&(b.mode & 0o7777)),
        SortKey::Size | SortKey::Percent => a.size.cmp(&b.size),
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl App {
    fn new(cc: &eframe::CreationContext, start: PathBuf, screenshot: Option<PathBuf>) -> Self {
        let mut app = App {
            root_path: start.clone(),
            root: Node::new_dir(&start.to_string_lossy()),
            scanning: false,
            rx: None,
            prog: Arc::new(Progress::default()),
            started: Instant::now(),
            elapsed: Duration::ZERO,
            top_files: Vec::new(),
            message: None,
            expanded: HashSet::new(),
            selected: None,
            unit: Unit::Auto,
            basis: Basis::Size,
            show_files: true,
            sort: (SortKey::Size, false),
            opts: ScanOptions::default(),
            owners: fsinfo::users(),
            groups: fsinfo::groups(),
            compare: None,
            tab: Tab::Map,
            mounts: fsinfo::mounts(false),
            show_pseudo: false,
            theme: Theme::Light,
            lang: Lang::from_env(),
            browser: None,
            show_settings: false,
            filter: String::new(),
            filter_cache: None,
            ext_cache: None,
            widths: DEFAULT_WIDTHS,
            history: Vec::new(),
            screenshot,
            frame: 0,
            shot_requested: false,
            filters: tools::Filters::default(),
            show_filters: false,
            confirm_trash: None,
            undo: Vec::new(),
            dups: None,
            cleanup: None,
            snap_saved: None,
            changes: ChangesState { depth: 2, ..Default::default() },
            last_export: None,
            schedule: None,
        };
        app.load_settings();
        // a window started by "open as root" keeps the person's choices
        if let Some(l) = std::env::var("ARBOLVIEW_LANG").ok().and_then(|v| Lang::from_code(&v)) {
            app.lang = l;
        }
        if let Some(th) = std::env::var("ARBOLVIEW_THEME").ok().and_then(|v| Theme::from_code(&v)) {
            app.theme = th;
        }
        app.apply_theme(&cc.egui_ctx);
        app.start_scan(start, &cc.egui_ctx);
        app
    }

    fn tr(&self, key: &str) -> &'static str {
        t(self.lang, key)
    }

    fn pal(&self) -> Palette {
        Palette::new(self.theme)
    }

    fn load_settings(&mut self) {
        let Ok(text) = std::fs::read_to_string(config_path()) else { return };
        for line in text.lines() {
            match line.split_once('=') {
                Some(("lang", v)) => {
                    if let Some(l) = Lang::from_code(v.trim()) {
                        self.lang = l;
                    }
                }
                Some(("theme", v)) => {
                    if let Some(th) = Theme::from_code(v.trim()) {
                        self.theme = th;
                    }
                }
                Some(("show_files", v)) => self.show_files = v.trim() == "1",
                Some(("skip_links", v)) => self.opts.skip_symlinks = v.trim() == "1",
                _ => {}
            }
        }
    }

    fn save_settings(&self) {
        if self.screenshot.is_some() {
            return;
        }
        let p = config_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(
            &p,
            format!(
                "lang={}\ntheme={}\nshow_files={}\nskip_links={}\n",
                self.lang.code(),
                self.theme.code(),
                self.show_files as u8,
                self.opts.skip_symlinks as u8
            ),
        );
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let p = self.pal();
        let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill = p.bg;
        v.window_fill = p.card;
        v.extreme_bg_color = p.card;
        v.selection.bg_fill = p.accent.gamma_multiply(0.6);
        v.hyperlink_color = p.accent;
        v.override_text_color = Some(p.text);
        v.window_corner_radius = 10.into();
        v.widgets.inactive.corner_radius = 6.into();
        v.widgets.hovered.corner_radius = 6.into();
        v.widgets.active.corner_radius = 6.into();
        v.widgets.inactive.weak_bg_fill = p.bar_bg;
        v.widgets.hovered.weak_bg_fill = p.hover;
        ctx.set_visuals(v);
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = vec2(6.0, 4.0);
        style.spacing.button_padding = vec2(8.0, 4.0);
        // tooltips wide enough for a path or a size line without breaking it up
        style.spacing.tooltip_width = 640.0;
        style.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.0));
        style.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.0));
        ctx.set_style(style);
    }

    fn owner(&self, uid: u32) -> String {
        if cfg!(windows) {
            return String::new(); // Windows files have no numeric owner here
        }
        self.owners.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
    }

    fn start_scan(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.prog.cancel.store(true, Ordering::Relaxed);
        let path = platform::canonical(&path);
        if self.root_path != path && !self.root.children.is_empty() {
            self.history.retain(|h| *h != self.root_path);
            self.history.push(self.root_path.clone());
        }
        self.root_path = path.clone();
        self.root = Node::new_dir(&path.to_string_lossy());
        self.top_files.clear();
        self.message = None;
        self.ext_cache = None;
        self.filter_cache = None;
        self.expanded.clear();
        self.expanded.insert(path.clone());
        self.selected = Some(path.clone());
        self.prog = Arc::new(Progress::default());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.scanning = true;
        self.started = Instant::now();
        self.mounts = fsinfo::mounts(self.show_pseudo);
        scan::spawn(path, self.opts, self.prog.clone(), tx, ctx.clone());
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut changed = false;
        let mut done = false;
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ScanMsg::Root { mtime, uid, gid, mode } => {
                    self.root.mtime = mtime;
                    self.root.uid = uid;
                    self.root.gid = gid;
                    self.root.mode = mode;
                }
                ScanMsg::Child(n) => {
                    self.root.add_child_totals(&n);
                    self.root.children.push(n);
                    changed = true;
                }
                ScanMsg::Done { top_files, cancelled } => {
                    self.top_files = top_files;
                    done = true;
                    if cancelled {
                        self.message = Some(self.tr("stopped").into());
                    } else {
                        finished = true;
                    }
                }
                ScanMsg::Failed(e) => self.message = Some(e),
            }
        }
        if changed || done {
            let (k, asc) = self.sort;
            if done && self.sort != (SortKey::Size, false) {
                self.resort_all();
            } else {
                self.root.children.sort_by(|a, b| {
                    let o = key_cmp(k, a, b);
                    if asc { o } else { o.reverse() }
                });
            }
            self.ext_cache = None;
        }
        if done {
            self.scanning = false;
            self.elapsed = self.started.elapsed();
            self.rx = None;
        }
        if finished && self.screenshot.is_none() {
            // keep a light record of this scan so the Changes view can tell what grew
            let ts = now_secs();
            self.snap_saved = tools::save_snapshot(&self.root_path, &self.root, ts);
            monitor::append_history(&self.root_path, &self.root, ts);
            self.changes.cache = None;
            self.changes.pick = 0;
        }
    }

    fn resort_all(&mut self) {
        let owners = self.owners.clone();
        let f = move |u: u32| owners.get(&u).cloned().unwrap_or_else(|| u.to_string());
        scan::sort_tree(&mut self.root, self.sort.0, self.sort.1, &f);
        self.filter_cache = None;
    }

    fn apply_sort(&mut self, key: SortKey) {
        if self.sort.0 == key {
            self.sort.1 = !self.sort.1;
        } else {
            self.sort = (key, matches!(key, SortKey::Name | SortKey::Owner));
        }
        self.resort_all();
    }

    fn find(&self, path: &Path) -> Option<&Node> {
        let rel = path.strip_prefix(&self.root_path).ok()?;
        let mut n = &self.root;
        for c in rel.components() {
            let name = c.as_os_str().to_string_lossy();
            n = n.children.iter().find(|x| *x.name == *name)?;
        }
        Some(n)
    }

    fn basis_value(&self, n: &Node) -> u64 {
        match self.basis {
            Basis::Size => n.size,
            Basis::Alloc => n.alloc,
            Basis::Files => {
                if n.is_dir { n.files } else { 1 }
            }
        }
    }

    fn make_row(&self, n: &Node, path: PathBuf, name: String, depth: usize, parent: f64) -> Row {
        Row {
            path,
            depth,
            name,
            is_dir: n.is_dir,
            is_link: n.is_link,
            size: n.size,
            alloc: n.alloc,
            files: n.files,
            folders: n.folders,
            mtime: n.mtime,
            uid: n.uid,
            gid: n.gid,
            mode: n.mode,
            pct: if parent > 0.0 { self.basis_value(n) as f64 / parent * 100.0 } else { 0.0 },
            has_children: n.children.iter().any(|c| c.is_dir || self.show_files),
            denied: n.denied,
            other_fs: n.other_fs,
        }
    }

    fn rows(&mut self) -> Vec<Row> {
        let needle = self.filter.trim().to_lowercase();
        if !needle.is_empty() || self.filters.active() {
            let key = (needle.clone(), self.root.size ^ self.filters.stamp(), self.root.files, self.basis as u8);
            if let Some((k, rows)) = &self.filter_cache {
                if *k == key {
                    return rows.clone();
                }
            }
            let mut out = Vec::new();
            let total = self.basis_value(&self.root).max(1) as f64;
            self.collect_matches(&self.root, self.root_path.clone(), &needle, total, &mut out);
            out.sort_by(|a, b| b.size.cmp(&a.size));
            out.truncate(3000);
            self.filter_cache = Some((key, out.clone()));
            return out;
        }
        let mut out = Vec::new();
        let total = self.basis_value(&self.root).max(1) as f64;
        self.push_rows(&self.root, self.root_path.clone(), 0, total, &mut out, true);
        out
    }

    fn collect_matches(&self, n: &Node, path: PathBuf, needle: &str, total: f64, out: &mut Vec<Row>) {
        let now = now_secs();
        for c in &n.children {
            let p = path.join(&*c.name);
            let shown_kind = c.is_dir || self.show_files || self.filters.active();
            let name_ok = needle.is_empty() || c.name.to_lowercase().contains(needle);
            if shown_kind && name_ok && self.filters.pass(c, now) {
                let shown = p.strip_prefix(&self.root_path).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
                out.push(self.make_row(c, p.clone(), shown, 0, total));
            }
            if c.is_dir {
                self.collect_matches(c, p, needle, total, out);
            }
        }
    }

    fn push_rows(&self, n: &Node, path: PathBuf, depth: usize, parent: f64, out: &mut Vec<Row>, is_root: bool) {
        let name = if is_root { path.to_string_lossy().into_owned() } else { n.name.to_string() };
        let expanded = n.is_dir && self.expanded.contains(&path);
        out.push(self.make_row(n, path.clone(), name, depth, parent));
        if expanded {
            let me = (self.basis_value(n) as f64).max(1.0);
            for c in &n.children {
                if !c.is_dir && !self.show_files {
                    continue;
                }
                self.push_rows(c, path.join(&*c.name), depth + 1, me, out, false);
            }
        }
    }

    fn expand_level(&mut self, level: usize) {
        fn walk(n: &Node, p: PathBuf, d: usize, max: usize, set: &mut HashSet<PathBuf>) {
            if d >= max || !n.is_dir {
                return;
            }
            set.insert(p.clone());
            for c in &n.children {
                if c.is_dir {
                    walk(c, p.join(&*c.name), d + 1, max, set);
                }
            }
        }
        let mut set = HashSet::new();
        walk(&self.root, self.root_path.clone(), 0, level, &mut set);
        self.expanded = set;
    }

    fn select_and_reveal(&mut self, p: PathBuf) {
        let mut cur = p.parent().map(|x| x.to_path_buf());
        while let Some(c) = cur {
            if !c.starts_with(&self.root_path) {
                break;
            }
            self.expanded.insert(c.clone());
            cur = c.parent().map(|x| x.to_path_buf());
        }
        self.selected = Some(p);
    }

    fn selected_dir(&self) -> PathBuf {
        match &self.selected {
            Some(p) => match self.find(p) {
                Some(n) if n.is_dir => p.clone(),
                Some(_) => p.parent().map(|x| x.to_path_buf()).unwrap_or_else(|| self.root_path.clone()),
                None => self.root_path.clone(),
            },
            None => self.root_path.clone(),
        }
    }

    /// Write a report of the current scan (PDF, HTML, CSV or JSON) into the home folder.
    fn export_report(&mut self, f: report::Format) {
        let hist = monitor::read_history(&self.root_path);
        let rep = report::build(self.lang, &self.root_path, &self.root, &self.top_files, 3, 300, hist);
        let bytes = report::render(&rep, f);
        let file = report::default_path(&self.root_path, f);
        let res = std::fs::File::create(&file).and_then(|mut fh| fh.write_all(&bytes));
        self.message = Some(match res {
            Ok(_) => {
                self.last_export = Some(file.clone());
                format!("{} {}", self.tr("exported"), file.display())
            }
            Err(e) => format!("{}: {}", self.tr("export_fail"), e),
        });
    }

    // ---------------------------------------------------------------- top bar

    fn top_bar(&mut self, ctx: &egui::Context) {
        let pal = self.pal();
        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::new().fill(pal.card).inner_margin(egui::Margin::symmetric(12, 8)).stroke(Stroke::new(1.0, pal.grid)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(26.0, 30.0), Sense::hover());
                    logo(ui.painter(), r.center(), &pal);
                    ui.label(egui::RichText::new(APP_NAME).size(18.0).strong().color(pal.text));
                    ui.add_space(18.0);
                    if primary_button(ui, &pal, Glyph::Folder, self.tr("scan_folder")).clicked() {
                        self.browser = Some(self.root_path.clone());
                    }
                    ui.add_space(4.0);
                    if self.scanning {
                        if icon_button(ui, &pal, Glyph::Stop, self.tr("stop"), true).clicked() {
                            self.prog.cancel.store(true, Ordering::Relaxed);
                        }
                    } else if icon_button(ui, &pal, Glyph::Refresh, self.tr("refresh"), true).clicked() {
                        let p = self.root_path.clone();
                        self.start_scan(p, ctx);
                    }
                    let parent = self.root_path.parent().map(|p| p.to_path_buf());
                    if icon_button(ui, &pal, Glyph::Up, self.tr("up"), parent.is_some()).clicked() {
                        if let Some(p) = parent {
                            self.start_scan(p, ctx);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, &pal, Glyph::Gear, self.tr("settings"), true).clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        let export = icon_button(ui, &pal, Glyph::Export, self.tr("export"), !self.scanning);
                        let pop = egui::Id::new("export_menu");
                        if export.clicked() {
                            ui.memory_mut(|m| m.toggle_popup(pop));
                        }
                        let mut pick: Option<report::Format> = None;
                        egui::popup_below_widget(ui, pop, &export, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                            ui.set_min_width(190.0);
                            for (f, k) in [
                                (report::Format::Pdf, "export_pdf"),
                                (report::Format::Html, "export_html"),
                                (report::Format::Csv, "export_csv"),
                                (report::Format::Json, "export_json"),
                            ] {
                                if ui.button(t(self.lang, k)).clicked() {
                                    pick = Some(f);
                                }
                            }
                        });
                        if let Some(f) = pick {
                            self.export_report(f);
                        }
                        if icon_button(ui, &pal, Glyph::Compare, self.tr("compare"), true).clicked() {
                            self.open_compare();
                        }
                        if icon_button(ui, &pal, Glyph::Dups, self.tr("dups_title"), true).clicked() {
                            self.open_dups();
                        }
                        if icon_button(ui, &pal, Glyph::Broom, self.tr("cleanup"), true).clicked() {
                            self.open_cleanup();
                        }
                        if fsinfo::is_root_user() {
                            egui::Frame::new().fill(pal.warn).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
                                ui.label(egui::RichText::new(t(self.lang, "admin")).size(10.5).strong().color(Color32::WHITE));
                            });
                        } else if platform::can_elevate() && icon_button(ui, &pal, Glyph::Shield, &format!("{}\n{}", self.tr("run_root"), self.tr("run_root_hint")), true).clicked() {
                            self.relaunch_as_root(ctx);
                        }
                        ui.add_space(6.0);
                        let hint = self.tr("filter_hint");
                        egui::Frame::new().fill(pal.bar_bg).corner_radius(17.0).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
                            let r = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(hint).frame(false).desired_width(220.0));
                            if r.changed() {
                                self.filter_cache = None;
                            }
                        });
                        let r = icon_button(ui, &pal, Glyph::Filter, self.tr("filters"), true);
                        if self.filters.active() {
                            ui.painter().circle_filled(r.rect.right_top() + vec2(-7.0, 7.0), 4.0, pal.accent);
                        }
                        if r.clicked() {
                            self.show_filters = !self.show_filters;
                        }
                    });
                });
            });
    }

    // ---------------------------------------------------------------- sidebar

    fn sidebar(&mut self, ctx: &egui::Context) {
        let pal = self.pal();
        let mut go: Option<PathBuf> = None;
        egui::SidePanel::left("side")
            .resizable(false)
            .exact_width(232.0)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin::symmetric(10, 12)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().id_salt("side_scroll").show(ui, |ui| {
                    section(ui, &pal, self.tr("places"));
                    let mut places: Vec<(Glyph, &str, PathBuf)> = vec![(Glyph::Home, self.tr("home"), home())];
                    if let Some(p) = platform::documents() {
                        places.push((Glyph::Doc, self.tr("documents"), p));
                    }
                    if let Some(p) = platform::downloads() {
                        places.push((Glyph::Download, self.tr("downloads"), p));
                    }
                    places.push((Glyph::Globe, self.tr("root"), platform::system_root()));
                    for (g, label, p) in places {
                        let active = self.root_path == p;
                        if side_item(ui, &pal, |pp, c| glyph(pp, g, c, pal.weak, &pal), label, None, active).clicked() {
                            go = Some(p);
                        }
                    }
                    ui.add_space(10.0);
                    section(ui, &pal, self.tr("disks"));
                    for m in &self.mounts {
                        let active = self.root_path == Path::new(&m.mount_point);
                        let sub = format!("{} {} {}", format_size(m.avail, Unit::Auto), self.tr("free_of"), format_size(m.total, Unit::Auto));
                        let warn = m.used_fraction() > 0.9;
                        let resp = side_item(ui, &pal, |pp, c| disk_icon(pp, c, if warn { pal.warn } else { pal.weak }), &short_mount(&m.mount_point), Some((sub, m.used_fraction())), active);
                        let resp = resp.on_hover_text(format!("{}\n{} · {}", m.mount_point, m.fs_type, m.device));
                        if resp.clicked() {
                            go = Some(PathBuf::from(&m.mount_point));
                        }
                    }
                    ui.add_space(10.0);
                    section(ui, &pal, self.tr("recent"));
                    if self.history.is_empty() {
                        ui.label(egui::RichText::new(self.tr("none_recent")).color(pal.weak).size(12.0));
                    }
                    for h in self.history.iter().rev().take(5) {
                        let label = h.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| h.to_string_lossy().into_owned());
                        if side_item(ui, &pal, |pp, c| glyph(pp, Glyph::Clock, c, pal.weak, &pal), &label, None, false)
                            .on_hover_text(h.to_string_lossy())
                            .clicked()
                        {
                            go = Some(h.clone());
                        }
                    }
                    if let Some(tip) = self.tip() {
                        ui.add_space(14.0);
                        egui::Frame::new().fill(pal.card).corner_radius(10.0).inner_margin(10.0).stroke(Stroke::new(1.0, pal.grid)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(egui::RichText::new(self.tr("tip_title")).strong().color(pal.accent).size(12.0));
                            ui.label(egui::RichText::new(tip).size(12.0));
                        });
                    }
                });
            });
        if let Some(p) = go {
            self.start_scan(p, ctx);
        }
    }

    fn tip(&self) -> Option<String> {
        if let Some(m) = self.mounts.iter().find(|m| m.used_fraction() > 0.9) {
            return Some(format!("{} ({})", self.tr("tip_disk_full"), m.mount_point));
        }
        if !self.scanning && self.prog.errors.load(Ordering::Relaxed) > 50 {
            return Some(self.tr("tip_root").into());
        }
        if let Some(c) = self.root.children.iter().find(|c| cfg!(target_os = "linux") && &*c.name == ".cache") {
            if c.size > 1 << 30 {
                return Some(format!("{} ({})", self.tr("tip_cache"), format_size(c.size, Unit::Auto)));
            }
        }
        None
    }

    // ---------------------------------------------------------------- tree

    fn breadcrumb(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        let ctx = ui.ctx().clone();
        let mut rescan: Option<PathBuf> = None;
        let mut select: Option<PathBuf> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // (label, folder) per step; a Windows drive "C:" and its root "\" are one step, C:\
            let mut steps: Vec<(String, PathBuf)> = Vec::new();
            let mut acc = PathBuf::new();
            for c in self.root_path.components() {
                acc.push(c);
                match c {
                    std::path::Component::RootDir if !steps.is_empty() => {
                        if let Some(last) = steps.last_mut() {
                            last.1 = acc.clone();
                        }
                    }
                    _ => steps.push((c.as_os_str().to_string_lossy().into_owned(), acc.clone())),
                }
            }
            for (i, (label, path)) in steps.iter().enumerate() {
                let last = i + 1 == steps.len();
                let txt = egui::RichText::new(label.clone()).size(13.5).color(if last { pal.text } else { pal.weak });
                if ui.add(egui::Button::new(if last { txt.strong() } else { txt }).frame(false)).clicked() && !last {
                    rescan = Some(path.clone());
                }
                if !last && label != "/" {
                    ui.label(egui::RichText::new("›").color(pal.weak));
                }
            }
            if let Some(sel) = &self.selected {
                if let Ok(rel) = sel.strip_prefix(&self.root_path) {
                    let mut acc = self.root_path.clone();
                    for c in rel.components() {
                        acc.push(c);
                        ui.label(egui::RichText::new("›").color(pal.weak));
                        let name = c.as_os_str().to_string_lossy().into_owned();
                        if ui.add(egui::Button::new(egui::RichText::new(name).size(13.5).color(pal.accent)).frame(false)).clicked() {
                            select = Some(acc.clone());
                        }
                    }
                }
            }
        });
        if let Some(p) = rescan {
            self.start_scan(p, &ctx);
        }
        if let Some(p) = select {
            self.selected = Some(p);
        }
    }

    fn tree_toolbar(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(self.tr("measure")).color(pal.weak).size(12.0));
            let items = [self.tr("by_size"), self.tr("by_disk"), self.tr("by_count")];
            let cur = match self.basis {
                Basis::Size => 0,
                Basis::Alloc => 1,
                Basis::Files => 2,
            };
            if let Some(i) = segmented(ui, &pal, &items, cur) {
                self.basis = [Basis::Size, Basis::Alloc, Basis::Files][i];
                self.filter_cache = None;
            }
            ui.add_space(10.0);
            ui.label(egui::RichText::new(self.tr("unit")).color(pal.weak).size(12.0));
            let units = [Unit::Auto, Unit::KB, Unit::MB, Unit::GB];
            let names = [self.tr("auto"), "KB", "MB", "GB"];
            let cur = units.iter().position(|u| *u == self.unit).unwrap_or(0);
            if let Some(i) = segmented(ui, &pal, &names, cur) {
                self.unit = units[i];
            }
            ui.add_space(10.0);
            ui.label(egui::RichText::new(self.tr("expand")).color(pal.weak).size(12.0));
            if let Some(i) = segmented(ui, &pal, &["1", "2", "3", self.tr("collapse")], 9) {
                if i == 3 {
                    self.expanded.clear();
                    self.expanded.insert(self.root_path.clone());
                } else {
                    self.expand_level(i + 2);
                }
            }
            ui.add_space(10.0);
            let label = t(self.lang, "show_files");
            if ui.checkbox(&mut self.show_files, label).changed() {
                self.filter_cache = None;
                self.save_settings();
            }
        });
    }

    fn tree(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        let rows = self.rows();
        let filtering = !self.filter.trim().is_empty() || self.filters.active();
        let total_w: f32 = self.widths.iter().enumerate().filter(|(i, _)| col_visible(*i)).map(|(_, w)| w).sum();
        let mut clicked: Option<(usize, bool, bool)> = None;
        let mut ctx_action: Option<(usize, &'static str)> = None;
        let mut sort_click: Option<SortKey> = None;
        if filtering {
            ui.label(egui::RichText::new(format!("{} {}", group(rows.len() as u64), self.tr("results"))).color(pal.weak).size(12.0));
        }
        egui::Frame::new().fill(pal.card).corner_radius(10.0).stroke(Stroke::new(1.0, pal.grid)).inner_margin(egui::Margin::symmetric(4, 4)).show(ui, |ui| {
            egui::ScrollArea::horizontal().id_salt("tree_h").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let row_w = total_w.max(ui.available_width());
                let (hrect, _) = ui.allocate_exact_size(vec2(row_w, 28.0), Sense::hover());
                ui.painter().hline(hrect.x_range(), hrect.bottom() - 0.5, Stroke::new(1.0, pal.grid));
                let mut x = hrect.left();
                for (i, (key, sk)) in COL_KEYS.iter().enumerate() {
                    if !col_visible(i) {
                        continue;
                    }
                    let w = self.widths[i];
                    let r = Rect::from_min_size(pos2(x, hrect.top()), vec2(w, hrect.height()));
                    let resp = ui.interact(r.shrink2(vec2(4.0, 0.0)), ui.id().with(("hdr", i)), Sense::click());
                    let label = self.tr(key).to_uppercase();
                    let active = self.sort.0 == *sk;
                    let col = if active { pal.accent } else if resp.hovered() { pal.text } else { pal.weak };
                    let font = FontId::proportional(11.0);
                    let (align, tp) = if i == 0 { (Align2::LEFT_CENTER, pos2(r.left() + 12.0, r.center().y)) } else { (Align2::RIGHT_CENTER, pos2(r.right() - 10.0, r.center().y)) };
                    let tr = ui.painter().with_clip_rect(r).text(tp, align, label, font, col);
                    if active {
                        let ax = if i == 0 { tr.right() + 8.0 } else { tr.left() - 8.0 };
                        sort_arrow(ui.painter(), pos2(ax, r.center().y), self.sort.1, pal.accent);
                    }
                    if resp.clicked() {
                        sort_click = Some(*sk);
                    }
                    let hr = Rect::from_min_max(pos2(r.right() - 3.0, r.top()), pos2(r.right() + 3.0, r.bottom()));
                    let hresp = ui.interact(hr, ui.id().with(("hdr_sz", i)), Sense::drag());
                    if hresp.hovered() || hresp.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        ui.painter().vline(r.right(), r.y_range().shrink(6.0), Stroke::new(1.0, pal.grid));
                    }
                    if hresp.dragged() {
                        self.widths[i] = (self.widths[i] + hresp.drag_delta().x).clamp(40.0, 900.0);
                    }
                    x += w;
                }
                let selected = self.selected.clone();
                egui::ScrollArea::vertical().id_salt("tree_v").auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                    for i in range {
                        let row = &rows[i];
                        let (rect, resp) = ui.allocate_exact_size(vec2(row_w, ROW_H), Sense::click());
                        let painter = ui.painter_at(rect);
                        let is_sel = selected.as_deref() == Some(row.path.as_path());
                        let inner = rect.shrink2(vec2(2.0, 1.0));
                        if is_sel {
                            painter.rect_filled(inner, 6.0, pal.sel_bg);
                            painter.rect_filled(Rect::from_min_size(inner.min + vec2(0.0, 5.0), vec2(3.0, inner.height() - 10.0)), 2.0, pal.accent);
                        } else if resp.hovered() {
                            painter.rect_filled(inner, 6.0, pal.hover);
                        }
                        let mut x = rect.left();
                        let cy = rect.center().y;
                        let txt = if row.other_fs || row.denied { pal.weak } else { pal.text };
                        let w0 = self.widths[0];
                        let np = painter.with_clip_rect(Rect::from_min_max(pos2(x, rect.top()), pos2(x + w0 - 6.0, rect.bottom())));
                        let indent = 10.0 + row.depth as f32 * INDENT;
                        let exp_rect = Rect::from_center_size(pos2(x + indent + 5.0, cy), vec2(14.0, 14.0));
                        if row.is_dir && row.has_children && !filtering {
                            chevron(&np, exp_rect.center(), self.expanded.contains(&row.path), pal.weak);
                        }
                        let icon_x = x + indent + 22.0;
                        if row.is_dir {
                            folder_icon(&np, pos2(icon_x, cy), if row.other_fs { pal.weak } else { pal.folder });
                        } else {
                            file_icon(&np, pos2(icon_x, cy), pal.weak, row.is_link, pal.accent);
                        }
                        let mut nm = row.name.clone();
                        if row.denied {
                            nm.push_str(&format!("  · {}", t(self.lang, "denied")));
                        } else if row.other_fs {
                            nm.push_str(&format!("  · {}", t(self.lang, "other_fs")));
                        } else if row.is_link {
                            if let Ok(target) = std::fs::read_link(&row.path) {
                                nm.push_str(&format!("  → {}", target.display()));
                            }
                        }
                        let font = if row.depth == 0 && !filtering { FontId::proportional(13.5) } else { FontId::proportional(13.0) };
                        np.text(pos2(icon_x + 14.0, cy), Align2::LEFT_CENTER, nm, font, txt);
                        x += w0;
                        let right = |x: f32, w: f32, s: String, col: Color32| {
                            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w, ROW_H));
                            painter.with_clip_rect(r).text(pos2(x + w - 10.0, cy), Align2::RIGHT_CENTER, s, FontId::proportional(13.0), col);
                        };
                        right(x, self.widths[1], format_size(row.size, self.unit), txt);
                        x += self.widths[1];
                        right(x, self.widths[2], format_size(row.alloc, self.unit), pal.weak);
                        x += self.widths[2];
                        right(x, self.widths[3], if row.is_dir { group(row.files) } else { String::new() }, pal.weak);
                        x += self.widths[3];
                        right(x, self.widths[4], if row.is_dir { group(row.folders) } else { String::new() }, pal.weak);
                        x += self.widths[4];
                        let w5 = self.widths[5];
                        let track = Rect::from_min_size(pos2(x + 10.0, cy - 3.0), vec2((w5 - 62.0).max(10.0), 6.0));
                        painter.rect_filled(track, 3.0, pal.bar_bg);
                        let fw = track.width() * (row.pct as f32 / 100.0).clamp(0.0, 1.0);
                        if fw > 0.5 {
                            painter.rect_filled(Rect::from_min_size(track.min, vec2(fw.max(6.0), track.height())), 3.0, pal.bar);
                        }
                        let pr = Rect::from_min_size(pos2(x, rect.top()), vec2(w5, ROW_H));
                        painter.with_clip_rect(pr).text(pos2(x + w5 - 8.0, cy), Align2::RIGHT_CENTER, format!("{:.0} %", row.pct), FontId::proportional(12.0), pal.weak);
                        x += w5;
                        right(x, self.widths[6], format_time(row.mtime), pal.weak);
                        x += self.widths[6];
                        // owner and Unix permissions: Linux and macOS only
                        let resp = if platform::HAS_POSIX_PERMS {
                            right(x, self.widths[7], self.owner(row.uid), pal.weak);
                            x += self.widths[7];
                            let pr = Rect::from_min_size(pos2(x, rect.top()), vec2(self.widths[8], ROW_H));
                            painter.with_clip_rect(pr).text(pos2(x + self.widths[8] - 10.0, cy), Align2::RIGHT_CENTER, fsinfo::perm_string(row.mode), FontId::monospace(12.0), pal.weak);
                            resp.on_hover_text_at_pointer(format!(
                                "{}  {:04o}  {}:{}",
                                fsinfo::perm_string(row.mode),
                                row.mode & 0o7777,
                                self.owner(row.uid),
                                self.groups.get(&row.gid).cloned().unwrap_or_else(|| row.gid.to_string())
                            ))
                        } else {
                            resp
                        };

                        if resp.clicked() || resp.double_clicked() {
                            let on_exp = resp.interact_pointer_pos().map(|p| exp_rect.expand(3.0).contains(p)).unwrap_or(false);
                            clicked = Some((i, resp.double_clicked(), on_exp));
                        }
                        if resp.secondary_clicked() {
                            clicked = Some((i, false, false));
                        }
                        resp.context_menu(|ui| {
                            if row.is_dir && ui.button(t(self.lang, "scan_this")).clicked() {
                                ctx_action = Some((i, "scan"));
                                ui.close_menu();
                            }
                            if row.is_dir && !filtering && ui.button(t(self.lang, "expand_all")).clicked() {
                                ctx_action = Some((i, "expand"));
                                ui.close_menu();
                            }
                            if ui.button(t(self.lang, "open_fm")).clicked() {
                                ctx_action = Some((i, "open"));
                                ui.close_menu();
                            }
                            if ui.button(t(self.lang, "open_term")).clicked() {
                                ctx_action = Some((i, "term"));
                                ui.close_menu();
                            }
                            if !(row.depth == 0 && !filtering) {
                                ui.separator();
                                if ui.button(egui::RichText::new(t(self.lang, "trash")).color(pal.warn)).clicked() {
                                    ctx_action = Some((i, "trash"));
                                    ui.close_menu();
                                }
                                ui.separator();
                            }
                            if ui.button(t(self.lang, "copy_path")).clicked() {
                                ctx_action = Some((i, "copy"));
                                ui.close_menu();
                            }
                        });
                    }
                });
            });
        });
        if let Some(k) = sort_click {
            self.apply_sort(k);
        }
        if let Some((i, double, on_exp)) = clicked {
            let row = &rows[i];
            if filtering {
                if double {
                    self.filter.clear();
                    self.filter_cache = None;
                    self.select_and_reveal(row.path.clone());
                } else {
                    self.selected = Some(row.path.clone());
                }
            } else {
                self.selected = Some(row.path.clone());
                if row.is_dir && (double || on_exp) && !self.expanded.remove(&row.path) {
                    self.expanded.insert(row.path.clone());
                }
            }
        }
        if let Some((i, act)) = ctx_action {
            let p = rows[i].path.clone();
            let dir = if rows[i].is_dir { p.clone() } else { p.parent().map(|x| x.to_path_buf()).unwrap_or(p.clone()) };
            match act {
                "trash" => self.confirm_trash = Some(p.clone()),
                "scan" => {
                    let c = ui.ctx().clone();
                    self.start_scan(p, &c);
                }
                "expand" => {
                    fn walk(n: &Node, p: PathBuf, set: &mut HashSet<PathBuf>) {
                        if !n.is_dir {
                            return;
                        }
                        set.insert(p.clone());
                        for c in &n.children {
                            walk(c, p.join(&*c.name), set);
                        }
                    }
                    let mut set = HashSet::new();
                    if let Some(n) = self.find(&p) {
                        walk(n, p, &mut set);
                    }
                    self.expanded.extend(set);
                }
                "open" => open_in_file_manager(&dir),
                "term" => open_terminal(&dir),
                _ => ui.ctx().copy_text(p.to_string_lossy().into_owned()),
            }
        }
    }

    // ---------------------------------------------------------------- right panel

    fn details(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        let dir = self.selected_dir();
        if let Some(n) = self.find(&dir) {
            let name = if dir == self.root_path { dir.to_string_lossy().into_owned() } else { n.name.to_string() };
            ui.label(egui::RichText::new(self.tr("selection").to_uppercase()).size(11.0).color(pal.weak));
            ui.label(egui::RichText::new(name).size(16.0).strong());
            let sel_path = self.selected.clone().unwrap_or_else(|| dir.clone());
            let shown = self.find(&sel_path).unwrap_or(n);
            if platform::HAS_POSIX_PERMS {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(fsinfo::perm_string(shown.mode)).monospace().color(pal.accent));
                    ui.label(egui::RichText::new(format!("({:04o})", shown.mode & 0o7777)).monospace().color(pal.weak));
                    ui.label(egui::RichText::new(format!(
                        "· {} {} · {} {}",
                        t(self.lang, "col_owner"),
                        self.owner(shown.uid),
                        t(self.lang, "group_w"),
                        self.groups.get(&shown.gid).cloned().unwrap_or_else(|| shown.gid.to_string())
                    )).color(pal.weak).size(12.0));
                });
            } else {
                // Windows: the attributes people know (read-only, hidden/system)
                let mut attrs = Vec::new();
                if shown.mode & 0o200 == 0 {
                    attrs.push(t(self.lang, "attr_readonly"));
                }
                if shown.mode & 0o004 == 0 {
                    attrs.push(t(self.lang, "attr_hidden"));
                }
                if !attrs.is_empty() {
                    ui.label(egui::RichText::new(attrs.join(" · ")).color(pal.accent).size(12.0));
                }
            }
            if shown.is_link {
                if let Ok(target) = std::fs::read_link(&sel_path) {
                    ui.label(egui::RichText::new(format!("{} {}", t(self.lang, "points_to"), target.display())).size(12.0).color(pal.weak));
                }
            }
            ui.add_space(4.0);
            let cards = [
                (self.tr("col_size"), format_size(n.size, self.unit)),
                (self.tr("col_alloc"), format_size(n.alloc, self.unit)),
                (self.tr("col_files"), group(n.files)),
                (self.tr("col_folders"), group(n.folders)),
            ];
            ui.columns(4, |cols| {
                for (i, (k, v)) in cards.iter().enumerate() {
                    egui::Frame::new().fill(pal.card).corner_radius(8.0).stroke(Stroke::new(1.0, pal.grid)).inner_margin(8.0).show(&mut cols[i], |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(egui::RichText::new(*k).size(11.0).color(pal.weak));
                        ui.label(egui::RichText::new(v).size(14.0).strong().color(if i == 0 { pal.accent } else { pal.text }));
                    });
                }
            });
        }
        ui.add_space(8.0);
        let tabs = [Tab::Map, Tab::Types, Tab::Largest, Tab::Age, Tab::Changes, Tab::Disks];
        let names = [self.tr("tab_map"), self.tr("tab_types"), self.tr("tab_big"), self.tr("tab_age"), self.tr("tab_changes"), self.tr("tab_disks")];
        let cur = tabs.iter().position(|x| *x == self.tab).unwrap_or(0);
        if let Some(i) = segmented(ui, &pal, &names, cur) {
            self.tab = tabs[i];
        }
        ui.add_space(6.0);
        match self.tab {
            Tab::Map => self.chart(ui, &pal),
            Tab::Types => self.extensions(ui, &pal),
            Tab::Largest => self.largest(ui, &pal),
            Tab::Age => self.age_view(ui, &pal),
            Tab::Changes => self.changes_view(ui, &pal),
            Tab::Disks => self.disks(ui, &pal),
        }
    }

    fn chart(&mut self, ui: &mut Ui, pal: &Palette) {
        let dir = self.selected_dir();
        let mut clicked: Option<PathBuf> = None;
        {
            let Some(node) = self.find(&dir) else { return };
            let avail = ui.available_rect_before_wrap();
            let (rect, _) = ui.allocate_exact_size(avail.size(), Sense::hover());
            let painter = ui.painter_at(rect);
            let mut kids: Vec<&Node> = node.children.iter().filter(|c| self.basis_value(c) > 0).collect();
            kids.sort_by(|a, b| self.basis_value(b).cmp(&self.basis_value(a)));
            kids.truncate(300);
            if kids.is_empty() {
                painter.rect_filled(rect, 10.0, pal.bar_bg);
                let msg = if self.scanning { self.tr("scanning") } else { self.tr("empty") };
                painter.text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), pal.weak);
                return;
            }
            let vals: Vec<f64> = kids.iter().map(|c| self.basis_value(c) as f64).collect();
            let total: f64 = vals.iter().sum::<f64>().max(1.0);
            let rects = treemap::layout(&vals, rect);
            let hover = ui.ctx().pointer_hover_pos().filter(|h| rect.contains(*h));
            let primary = ui.input(|i| i.pointer.primary_clicked());
            let mut hovered: Option<(String, u64, f64)> = None;
            for (k, (c, r0)) in kids.iter().zip(rects.iter()).enumerate() {
                let r = r0.shrink(1.5);
                if r.width() < 1.0 || r.height() < 1.0 {
                    continue;
                }
                let col = pal.tile(k, 0);
                painter.rect_filled(r, 6.0, col);
                let header = r.height() > 36.0 && r.width() > 56.0;
                if header {
                    let tc = pal.on_tile(col);
                    let pc = painter.with_clip_rect(r.shrink(4.0));
                    pc.text(pos2(r.left() + 7.0, r.top() + 5.0), Align2::LEFT_TOP, &*c.name, FontId::proportional(12.5), tc);
                    pc.text(pos2(r.left() + 7.0, r.top() + 20.0), Align2::LEFT_TOP, format_size(c.size, self.unit), FontId::proportional(11.0), tc.gamma_multiply(0.8));
                }
                let mut inner_hit = false;
                if c.is_dir && r.width() > 50.0 && r.height() > 60.0 {
                    let inner = Rect::from_min_max(pos2(r.left() + 4.0, r.top() + if header { 36.0 } else { 4.0 }), pos2(r.right() - 4.0, r.bottom() - 4.0));
                    let mut gk: Vec<&Node> = c.children.iter().filter(|g| self.basis_value(g) > 0).collect();
                    gk.sort_by(|a, b| self.basis_value(b).cmp(&self.basis_value(a)));
                    gk.truncate(150);
                    let gv: Vec<f64> = gk.iter().map(|g| self.basis_value(g) as f64).collect();
                    let gr = treemap::layout(&gv, inner);
                    for (j, (g, rr0)) in gk.iter().zip(gr.iter()).enumerate() {
                        let rr = rr0.shrink(1.0);
                        if rr.width() < 1.0 || rr.height() < 1.0 {
                            continue;
                        }
                        let gc = pal.tile(k, 1 + (j % 3));
                        painter.rect_filled(rr, 4.0, gc);
                        if rr.width() > 60.0 && rr.height() > 18.0 {
                            painter.with_clip_rect(rr.shrink(2.0)).text(pos2(rr.left() + 4.0, rr.top() + 3.0), Align2::LEFT_TOP, &*g.name, FontId::proportional(10.5), pal.on_tile(gc));
                        }
                        if let Some(h) = hover {
                            if rr.contains(h) {
                                inner_hit = true;
                                hovered = Some((format!("{}/{}", c.name, g.name), g.size, self.basis_value(g) as f64 / total * 100.0));
                                if primary {
                                    clicked = Some(dir.join(&*c.name).join(&*g.name));
                                }
                            }
                        }
                    }
                }
                if let Some(h) = hover {
                    if r.contains(h) {
                        painter.rect_stroke(r, 6.0, Stroke::new(2.0, pal.text), StrokeKind::Inside);
                        if !inner_hit {
                            hovered = Some((c.name.to_string(), c.size, vals[k] / total * 100.0));
                            if primary {
                                clicked = Some(dir.join(&*c.name));
                            }
                        }
                    }
                }
            }
            if let Some((n, s, p)) = hovered {
                let unit = self.unit;
                // one line, never wrapped: size with its unit and the share
                let _ = n;
                egui::show_tooltip_at_pointer(ui.ctx(), ui.layer_id(), egui::Id::new("tm_tip"), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(format!("{}  ·  {}", format_size(s, unit), format_pct(p))).strong()).extend());
                });
            }
        }
        if let Some(p) = clicked {
            self.select_and_reveal(p);
        }
    }

    fn extensions(&mut self, ui: &mut Ui, pal: &Palette) {
        let dir = self.selected_dir();
        let stamp = self.find(&dir).map(|n| n.size ^ (n.files << 1)).unwrap_or(0);
        let fresh = matches!(&self.ext_cache, Some((p, s, _)) if *p == dir && *s == stamp);
        if !fresh {
            let mut m = HashMap::new();
            if let Some(n) = self.find(&dir) {
                scan::extensions(n, &mut m);
            }
            let mut v: Vec<(String, u64, u64)> = m.into_iter().map(|(k, (s, c))| (k, s, c)).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            self.ext_cache = Some((dir.clone(), stamp, v));
        }
        let list = self.ext_cache.as_ref().unwrap().2.clone();
        let total: u64 = list.iter().map(|e| e.1).sum::<u64>().max(1);
        ui.label(egui::RichText::new(self.tr("types_title")).strong());
        egui::ScrollArea::vertical().id_salt("ext").auto_shrink([false, false]).show(ui, |ui| {
            for (k, (ext, s, c)) in list.iter().take(200).enumerate() {
                let label = if ext.is_empty() { self.tr("no_ext").to_string() } else { format!(".{}", ext) };
                let p = *s as f64 / total as f64;
                bar_line(ui, pal, pal.tile(k, 0), &label, &format!("{} · {} {}", format_size(*s, self.unit), group(*c), self.tr("files_w")), p);
            }
        });
    }

    fn largest(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.label(egui::RichText::new(self.tr("largest")).strong());
        if self.top_files.is_empty() {
            ui.label(egui::RichText::new(self.tr("after_scan")).color(pal.weak));
            return;
        }
        let mut sel: Option<PathBuf> = None;
        let max = self.top_files.first().map(|f| f.size).unwrap_or(1).max(1);
        egui::ScrollArea::vertical().id_salt("top").auto_shrink([false, false]).show(ui, |ui| {
            for f in &self.top_files {
                let name = f.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let resp = bar_line(ui, pal, pal.accent, &name, &format_size(f.size, self.unit), f.size as f64 / max as f64);
                let resp = resp.on_hover_text(format!("{}\n{}", f.path.display(), format_time(f.mtime)));
                if resp.clicked() {
                    sel = Some(f.path.clone());
                }
            }
        });
        if let Some(p) = sel {
            self.show_files = true;
            self.filter.clear();
            self.select_and_reveal(p);
        }
    }

    fn age_view(&mut self, ui: &mut Ui, pal: &Palette) {
        let dir = self.selected_dir();
        let mut buckets = [(0u64, 0u64); 6];
        if let Some(n) = self.find(&dir) {
            scan::ages(n, now_secs(), &mut buckets);
        }
        let total = buckets.iter().map(|b| b.0).sum::<u64>().max(1);
        ui.label(egui::RichText::new(self.tr("age_title")).strong());
        ui.label(egui::RichText::new(self.tr("age_hint")).size(12.0).color(pal.weak));
        ui.add_space(6.0);
        let keys = ["age_0", "age_1", "age_2", "age_3", "age_4", "age_5"];
        for (i, (s, c)) in buckets.iter().enumerate() {
            let col = pal.tile(i, 0);
            let value = format!("{} · {} {}", format_size(*s, self.unit), group(*c), self.tr("files_w"));
            bar_line(ui, pal, col, self.tr(keys[i]), &value, *s as f64 / total as f64);
        }
    }

    fn disks(&mut self, ui: &mut Ui, pal: &Palette) {
        let ctx = ui.ctx().clone();
        let mut go: Option<String> = None;
        egui::ScrollArea::vertical().id_salt("fs").auto_shrink([false, false]).show(ui, |ui| {
            for m in &self.mounts {
                egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.grid)).corner_radius(10.0).inner_margin(10.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::hover());
                        disk_icon(ui.painter(), r.center(), pal.accent);
                        ui.label(egui::RichText::new(&m.mount_point).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(t(self.lang, "scan_here")).clicked() {
                                go = Some(m.mount_point.clone());
                            }
                        });
                    });
                    let extra = if m.read_only() { format!(" · {}", t(self.lang, "read_only")) } else { String::new() };
                    ui.label(egui::RichText::new(format!("{} · {}{}", m.fs_type, m.device, extra)).color(pal.weak).size(11.5));
                    let f = m.used_fraction();
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
                    ui.painter().rect_filled(r, 4.0, pal.bar_bg);
                    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2((r.width() * f).max(8.0), r.height())), 4.0, if f > 0.9 { pal.warn } else { pal.bar });
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} {} · {} {} · {} {}",
                            format_size(m.avail, Unit::Auto),
                            t(self.lang, "free_of"),
                            format_size(m.total, Unit::Auto),
                            format_pct(f as f64 * 100.0),
                            t(self.lang, "used"),
                            group(m.inodes_free),
                            t(self.lang, "inodes_free")
                        ))
                        .size(11.5),
                    );
                });
                ui.add_space(6.0);
            }
        });
        if let Some(p) = go {
            self.start_scan(PathBuf::from(p), &ctx);
        }
    }

    // ---------------------------------------------------------------- windows & status

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let pal = self.pal();
        let mut open = true;
        let mut theme_changed = false;
        let mut lang_changed = false;
        let mut run_root = false;
        egui::Window::new(self.tr("settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::RIGHT_TOP, vec2(-12.0, 60.0))
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(t(self.lang, "language")).strong());
                ui.horizontal_wrapped(|ui| {
                    for l in Lang::ALL {
                        if ui.selectable_label(self.lang == l, l.native_name()).clicked() && self.lang != l {
                            self.lang = l;
                            lang_changed = true;
                        }
                    }
                });
                ui.separator();
                ui.label(egui::RichText::new(t(self.lang, "theme")).strong());
                egui::Grid::new("themes").spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                    for (i, th) in Theme::ALL.into_iter().enumerate() {
                        let p = Palette::new(th);
                        let (r, resp) = ui.allocate_exact_size(vec2(84.0, 50.0), Sense::click());
                        ui.painter().rect_filled(r, 8.0, p.bg);
                        ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(22.0, r.height())), egui::CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, p.sidebar);
                        ui.painter().rect_filled(Rect::from_min_size(r.min + vec2(28.0, 8.0), vec2(48.0, 7.0)), 3.0, p.accent);
                        ui.painter().rect_filled(Rect::from_min_size(r.min + vec2(28.0, 19.0), vec2(32.0, 5.0)), 2.0, p.bar);
                        ui.painter().text(pos2(r.left() + 28.0, r.bottom() - 9.0), Align2::LEFT_CENTER, t(self.lang, th.key()), FontId::proportional(10.5), p.text);
                        let sel = self.theme == th;
                        ui.painter().rect_stroke(r, 8.0, Stroke::new(if sel { 2.5 } else { 1.0 }, if sel { pal.accent } else { pal.grid }), StrokeKind::Inside);
                        if resp.clicked() && !sel {
                            self.theme = th;
                            theme_changed = true;
                        }
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
                ui.separator();
                ui.label(egui::RichText::new(t(self.lang, "scan_opts")).strong());
                // only what this system has: drive letters on Windows already keep
                // disks apart, and it has neither /proc-like folders nor inode hard links here
                if cfg!(unix) {
                    ui.checkbox(&mut self.opts.one_filesystem, t(self.lang, "opt_onefs"));
                    ui.checkbox(&mut self.opts.hardlinks_once, t(self.lang, "opt_hardlinks"));
                    ui.checkbox(&mut self.opts.skip_virtual, t(self.lang, "opt_virtual"));
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new(t(self.lang, "links_label")).strong());
                let before = self.opts.skip_symlinks;
                ui.radio_value(&mut self.opts.skip_symlinks, false, t(self.lang, "links_own"));
                ui.radio_value(&mut self.opts.skip_symlinks, true, t(self.lang, "links_skip"));
                ui.label(egui::RichText::new(format!("{} {}", t(self.lang, "links_note"), t(self.lang, "links_apply"))).size(11.0).color(pal.weak));
                if before != self.opts.skip_symlinks {
                    lang_changed = true; // save it with the other settings
                }
                ui.separator();
                if fsinfo::is_root_user() {
                    ui.label(egui::RichText::new(t(self.lang, "admin")).strong().color(pal.warn));
                } else if platform::can_elevate() {
                    if ui.button(t(self.lang, "run_root")).clicked() {
                        run_root = true;
                    }
                    ui.label(egui::RichText::new(t(self.lang, "run_root_hint")).size(11.0).color(pal.weak));
                }
                if cfg!(unix) && ui.checkbox(&mut self.show_pseudo, t(self.lang, "opt_pseudo")).changed() {
                    self.mounts = fsinfo::mounts(self.show_pseudo);
                }
                ui.separator();
                ui.label(egui::RichText::new(format!("{} — {}", APP_NAME, t(self.lang, "about"))).size(11.5).color(pal.weak));
            });
        if !open {
            self.show_settings = false;
        }
        if theme_changed {
            self.apply_theme(ctx);
        }
        if theme_changed || lang_changed {
            self.save_settings();
        }
        if run_root {
            self.relaunch_as_root(ctx);
        }
    }

    /// Start a second ÁrbolView with administrator rights (pkexec on Linux, the
    /// UAC prompt on Windows), keeping the person's language and theme.
    fn relaunch_as_root(&mut self, _ctx: &egui::Context) {
        let Some(exe) = fsinfo::self_exe() else { return };
        let env = [("ARBOLVIEW_LANG".to_string(), self.lang.code().to_string()), ("ARBOLVIEW_THEME".to_string(), self.theme.code().to_string())];
        let res = platform::elevate(&exe, &[self.root_path.to_string_lossy().into_owned()], &env);
        self.message = Some(match res {
            Ok(_) => self.tr("root_opening").into(),
            Err(_) => self.tr("root_fail").into(),
        });
    }

    // ---------------------------------------------------------------- trash & undo

    fn rel_comps(&self, p: &Path) -> Option<Vec<String>> {
        let rel = p.strip_prefix(&self.root_path).ok()?;
        let v: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if v.is_empty() { None } else { Some(v) }
    }

    /// Move several paths to the trash as one action (undone together).
    fn trash_paths(&mut self, paths: &[PathBuf]) {
        let mut done = Vec::new();
        let mut errors = Vec::new();
        for p in paths {
            match tools::trash(p) {
                Ok(rec) => {
                    let node = self.rel_comps(p).and_then(|c| self.root.take(&c));
                    self.top_files.retain(|f| !f.path.starts_with(p));
                    if self.selected.as_ref().is_some_and(|s| s.starts_with(p)) {
                        self.selected = p.parent().map(|x| x.to_path_buf());
                    }
                    done.push((rec, node));
                }
                Err(e) => errors.push(format!("{}: {}", p.display(), e)),
            }
        }
        self.ext_cache = None;
        self.filter_cache = None;
        self.mounts = fsinfo::mounts(self.show_pseudo);
        let names: Vec<String> = done.iter().map(|(r, _)| r.original.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()).collect();
        self.message = Some(if errors.is_empty() {
            format!("{} {}", self.tr("trashed"), if names.len() == 1 { names[0].clone() } else { format!("{} {}", names.len(), self.tr("files_w")) })
        } else {
            format!("{}: {}", self.tr("trash_fail"), errors.join("; "))
        });
        if !done.is_empty() {
            self.undo = done;
        }
    }

    fn undo_last(&mut self) {
        let batch = std::mem::take(&mut self.undo);
        let mut ok = 0;
        let mut errors = Vec::new();
        for (rec, node) in batch.into_iter().rev() {
            match tools::restore(&rec) {
                Ok(()) => {
                    ok += 1;
                    if let (Some(node), Some(c)) = (node, self.rel_comps(&rec.original)) {
                        self.root.put(&c[..c.len() - 1], node);
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        self.ext_cache = None;
        self.filter_cache = None;
        self.mounts = fsinfo::mounts(self.show_pseudo);
        self.message = Some(if errors.is_empty() { format!("{} {}", self.tr("restored"), ok) } else { errors.join("; ") });
    }

    fn confirm_trash_window(&mut self, ctx: &egui::Context) {
        let Some(p) = self.confirm_trash.clone() else { return };
        let pal = self.pal();
        let size = self.find(&p).map(|n| format_size(n.size, self.unit)).unwrap_or_default();
        let mut decision: Option<bool> = None;
        egui::Window::new(self.tr("trash_q"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                    glyph(ui.painter(), Glyph::Trash, r.center(), pal.warn, &pal);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(p.to_string_lossy()).strong());
                        ui.label(egui::RichText::new(size).color(pal.weak));
                    });
                });
                ui.label(egui::RichText::new(t(self.lang, "trash_body")).size(12.0).color(pal.weak));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, &pal, Glyph::Trash, t(self.lang, "trash")).clicked() {
                        decision = Some(true);
                    }
                    if ui.button(t(self.lang, "cancel")).clicked() {
                        decision = Some(false);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                self.confirm_trash = None;
                self.trash_paths(&[p]);
            }
            Some(false) => self.confirm_trash = None,
            None => {}
        }
    }

    // ---------------------------------------------------------------- filters

    fn filters_window(&mut self, ctx: &egui::Context) {
        if !self.show_filters {
            return;
        }
        let pal = self.pal();
        let before = self.filters;
        let mut open = true;
        egui::Window::new(self.tr("filters"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::RIGHT_TOP, vec2(-12.0, 60.0))
            .show(ctx, |ui| {
                let lang = self.lang;
                ui.label(egui::RichText::new(format!("{} …", t(lang, "f_bigger"))).strong());
                let sizes = [0u64, 10 << 20, 100 << 20, 1 << 30];
                let labels = [t(lang, "f_any"), "10 MB", "100 MB", "1 GB"];
                let cur = sizes.iter().position(|s| *s == self.filters.min_size).unwrap_or(0);
                if let Some(i) = segmented(ui, &pal, &labels, cur) {
                    self.filters.min_size = sizes[i];
                }
                ui.add_space(6.0);
                ui.label(egui::RichText::new(t(lang, "f_age")).strong());
                let years = [0u32, 1, 2, 5];
                let labels = [t(lang, "f_any"), t(lang, "y1"), t(lang, "y2"), t(lang, "y5")];
                let cur = years.iter().position(|y| *y == self.filters.older_years).unwrap_or(0);
                if let Some(i) = segmented(ui, &pal, &labels, cur) {
                    self.filters.older_years = years[i];
                }
                ui.add_space(6.0);
                ui.label(egui::RichText::new(t(lang, "ext")).strong());
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.filters.kind.is_none(), t(lang, "f_any")).clicked() {
                        self.filters.kind = None;
                    }
                    for k in tools::Kind::ALL {
                        if ui.selectable_label(self.filters.kind == Some(k), t(lang, k.key())).clicked() {
                            self.filters.kind = Some(k);
                        }
                    }
                });
                ui.add_space(6.0);
                ui.label(egui::RichText::new(t(lang, "f_note")).size(11.0).color(pal.weak));
                if self.filters.active() && ui.button(t(lang, "f_clear")).clicked() {
                    self.filters = tools::Filters::default();
                }
            });
        if !open {
            self.show_filters = false;
        }
        if before != self.filters {
            self.filter_cache = None;
        }
    }

    fn filter_chips(&mut self, ui: &mut Ui) {
        if !self.filters.active() {
            return;
        }
        let pal = self.pal();
        let lang = self.lang;
        let mut parts = Vec::new();
        if self.filters.min_size > 0 {
            parts.push(format!("{} {}", t(lang, "f_bigger"), format_size(self.filters.min_size, Unit::Auto)));
        }
        if self.filters.older_years > 0 {
            let y = match self.filters.older_years {
                1 => t(lang, "y1"),
                2 => t(lang, "y2"),
                _ => t(lang, "y5"),
            };
            parts.push(format!("{} {}", t(lang, "f_age"), y));
        }
        if let Some(k) = self.filters.kind {
            parts.push(t(lang, k.key()).to_string());
        }
        ui.horizontal(|ui| {
            egui::Frame::new().fill(pal.sel_bg).corner_radius(12.0).inner_margin(egui::Margin::symmetric(10, 3)).show(ui, |ui| {
                ui.label(egui::RichText::new(format!("{}: {}", t(lang, "filters"), parts.join(" · "))).size(12.0).color(pal.accent));
            });
            if ui.small_button(t(lang, "f_clear")).clicked() {
                self.filters = tools::Filters::default();
                self.filter_cache = None;
            }
        });
    }

    // ---------------------------------------------------------------- duplicates

    fn open_dups(&mut self) {
        if self.dups.is_none() {
            self.dups = Some(DupState {
                min: 1 << 20,
                dir: self.selected_dir(),
                running: false,
                prog: Arc::new(tools::DupProgress::default()),
                rx: None,
                groups: None,
            });
        }
    }

    fn dups_window(&mut self, ctx: &egui::Context) {
        if self.dups.is_none() {
            return;
        }
        let pal = self.pal();
        let lang = self.lang;
        let unit = self.unit;
        let scanning = self.scanning;
        let sel_dir = self.selected_dir();
        let mut start = false;
        let mut trash: Vec<PathBuf> = Vec::new();
        let mut reveal: Option<PathBuf> = None;
        let mut open = true;
        {
            let d = self.dups.as_mut().unwrap();
            if let Some(rx) = &d.rx {
                if let Ok(g) = rx.try_recv() {
                    d.groups = Some(g);
                    d.running = false;
                    d.rx = None;
                }
            }
            if d.running {
                ctx.request_repaint_after(Duration::from_millis(150));
            }
            egui::Window::new(t(lang, "dups_title"))
                .open(&mut open)
                .default_size([720.0, 520.0])
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(egui::RichText::new(t(lang, "dups_hint")).size(12.0).color(pal.weak));
                    ui.horizontal(|ui| {
                        ui.label(format!("{}: {}", t(lang, "in_folder"), d.dir.display()));
                        if d.dir != sel_dir && ui.small_button(t(lang, "use_current")).clicked() {
                            d.dir = sel_dir.clone();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(t(lang, "dups_min"));
                        let sizes = [64u64 << 10, 1 << 20, 10 << 20, 100 << 20];
                        let labels = ["64 KB", "1 MB", "10 MB", "100 MB"];
                        let cur = sizes.iter().position(|s| *s == d.min).unwrap_or(1);
                        if let Some(i) = segmented(ui, &pal, &labels, cur) {
                            d.min = sizes[i];
                        }
                        ui.add_space(8.0);
                        if scanning {
                            ui.label(egui::RichText::new(t(lang, "wait_scan")).color(pal.weak));
                        } else if d.running {
                            ui.spinner();
                            ui.label(format!(
                                "{} {}/{}",
                                t(lang, "dups_running"),
                                group(d.prog.checked.load(Ordering::Relaxed)),
                                group(d.prog.total.load(Ordering::Relaxed))
                            ));
                        } else if primary_button(ui, &pal, Glyph::Dups, t(lang, "dups_search")).clicked() {
                            start = true;
                        }
                    });
                    let Some(groups) = &mut d.groups else { return };
                    ui.separator();
                    if groups.is_empty() {
                        ui.label(egui::RichText::new(t(lang, "dups_none")).color(pal.accent).strong());
                        return;
                    }
                    let wasted: u64 = groups.iter().map(|g| g.wasted()).sum();
                    ui.label(egui::RichText::new(format!("{} {} · {} {}", group(groups.len() as u64), t(lang, "dups_groups"), format_size(wasted, unit), t(lang, "can_free"))).strong());
                    egui::ScrollArea::vertical().id_salt("dups_list").auto_shrink([false, false]).show(ui, |ui| {
                        for (gi, g) in groups.iter().enumerate().take(300) {
                            egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.grid)).corner_radius(8.0).inner_margin(8.0).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(format!("{} × {} {}", format_size(g.size, unit), g.paths.len(), t(lang, "copies"))).strong());
                                    ui.label(egui::RichText::new(format!("· {} {}", format_size(g.wasted(), unit), t(lang, "can_free"))).color(pal.weak));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button(t(lang, "keep_first")).clicked() {
                                            trash.extend(g.paths.iter().skip(1).cloned());
                                        }
                                    });
                                });
                                for p in &g.paths {
                                    ui.horizontal(|ui| {
                                        if icon_button(ui, &pal, Glyph::Trash, t(lang, "trash"), true).clicked() {
                                            trash.push(p.clone());
                                        }
                                        if ui.link(p.to_string_lossy()).clicked() {
                                            reveal = Some(p.clone());
                                        }
                                    });
                                }
                            });
                            let _ = gi;
                            ui.add_space(4.0);
                        }
                    });
                });
        }
        if start {
            let d = self.dups.as_mut().unwrap();
            let mut files = Vec::new();
            let dir = d.dir.clone();
            let root_path = self.root_path.clone();
            if let Some(n) = {
                let rel = dir.strip_prefix(&root_path).ok();
                rel.and_then(|rel| {
                    let mut n = &self.root;
                    for c in rel.components() {
                        let name = c.as_os_str().to_string_lossy();
                        n = n.children.iter().find(|x| *x.name == *name)?;
                    }
                    Some(n)
                })
            } {
                tools::collect_files(n, &dir, d.min, &mut files);
            }
            let prog = Arc::new(tools::DupProgress::default());
            d.prog = prog.clone();
            d.groups = None;
            d.running = true;
            let (tx, rx) = channel();
            d.rx = Some(rx);
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let g = tools::find_duplicates(files, &prog);
                let _ = tx.send(g);
                repaint.request_repaint();
            });
        }
        if !trash.is_empty() {
            self.trash_paths(&trash);
            let gone: HashSet<PathBuf> = self.undo.iter().map(|(r, _)| r.original.clone()).collect();
            if let Some(groups) = self.dups.as_mut().and_then(|d| d.groups.as_mut()) {
                for g in groups.iter_mut() {
                    g.paths.retain(|p| !gone.contains(p));
                }
                groups.retain(|g| g.paths.len() > 1);
            }
        }
        if let Some(p) = reveal {
            self.show_files = true;
            self.filter.clear();
            self.filter_cache = None;
            self.select_and_reveal(p);
        }
        if !open {
            if let Some(d) = &self.dups {
                d.prog.cancel.store(true, Ordering::Relaxed);
            }
            self.dups = None;
        }
    }

    // ---------------------------------------------------------------- clean-up

    fn open_cleanup(&mut self) {
        if self.cleanup.is_some() {
            return;
        }
        let items = Arc::new(std::sync::Mutex::new(tools::cleanup_items()));
        let measuring = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (it, m) = (items.clone(), measuring.clone());
        std::thread::spawn(move || {
            let n = it.lock().map(|v| v.len()).unwrap_or(0);
            for i in 0..n {
                let paths = it.lock().map(|v| v[i].measure.clone()).unwrap_or_default();
                if paths.is_empty() {
                    continue;
                }
                let s: u64 = paths.iter().map(|p| tools::du(p)).sum();
                if let Ok(mut v) = it.lock() {
                    v[i].size = Some(s);
                }
            }
            m.store(false, Ordering::Relaxed);
        });
        self.cleanup = Some(CleanState { items, measuring, running: false, confirm: false, rx: None, result: None });
    }

    fn cleanup_window(&mut self, ctx: &egui::Context) {
        let Some(c) = self.cleanup.as_mut() else { return };
        let pal = Palette::new(self.theme);
        let lang = self.lang;
        let is_root = fsinfo::is_root_user();
        let mut open = true;
        let mut run = false;
        let mut finished = false;
        if let Some(rx) = &c.rx {
            if let Ok((freed, failed)) = rx.try_recv() {
                c.running = false;
                c.rx = None;
                let mut msg = format!("{} {}", t(lang, "cl_done"), format_size(freed, Unit::Auto));
                if !failed.is_empty() {
                    let names: Vec<&str> = failed.iter().map(|k| t(lang, k)).collect();
                    msg.push_str(&format!(" · {} {}", t(lang, "cl_failed"), names.join(", ")));
                }
                c.result = Some(msg);
                finished = true;
            }
        }
        if c.running || c.measuring.load(Ordering::Relaxed) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        egui::Window::new(t(lang, "cleanup"))
            .open(&mut open)
            .default_width(560.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(t(lang, "cleanup_hint")).size(12.0).color(pal.weak));
                ui.add_space(4.0);
                let mut total = 0u64;
                if let Ok(mut items) = c.items.lock() {
                    for it in items.iter_mut() {
                        egui::Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.grid)).corner_radius(8.0).inner_margin(8.0).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut it.selected, egui::RichText::new(t(lang, it.key)).strong());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let s = match it.size {
                                        Some(s) => format_size(s, Unit::Auto),
                                        None if it.measure.is_empty() => t(lang, "unknown_size").to_string(),
                                        None => t(lang, "cl_measuring").to_string(),
                                    };
                                    ui.label(egui::RichText::new(s).color(pal.accent).strong());
                                });
                            });
                            let mut detail = it.detail.clone();
                            if matches!(it.action, tools::CleanAction::Root(_)) && !is_root {
                                detail.push_str(&format!("  ·  {}", t(lang, "needs_root")));
                            }
                            ui.label(egui::RichText::new(detail).size(11.0).color(pal.weak));
                        });
                        if it.selected {
                            total += it.size.unwrap_or(0);
                        }
                        ui.add_space(3.0);
                    }
                }
                ui.add_space(4.0);
                if let Some(r) = &c.result {
                    ui.label(egui::RichText::new(r).color(pal.accent).strong());
                }
                ui.horizontal(|ui| {
                    if c.running {
                        ui.spinner();
                        ui.label(t(lang, "cl_running"));
                    } else if c.confirm {
                        ui.label(egui::RichText::new(format!("{} ({})", t(lang, "cl_confirm"), format_size(total, Unit::Auto))).color(pal.warn).strong());
                        if primary_button(ui, &pal, Glyph::Broom, t(lang, "cl_clean")).clicked() {
                            run = true;
                            c.confirm = false;
                        }
                        if ui.button(t(lang, "cancel")).clicked() {
                            c.confirm = false;
                        }
                    } else if primary_button(ui, &pal, Glyph::Broom, &format!("{} · {}", t(lang, "cl_clean"), format_size(total, Unit::Auto))).clicked() {
                        c.confirm = true;
                    }
                });
            });
        if run {
            let items = c.items.lock().map(|v| v.clone()).unwrap_or_default();
            let (tx, rx) = channel();
            c.rx = Some(rx);
            c.running = true;
            c.result = None;
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let r = tools::run_cleanup(&items);
                let _ = tx.send(r);
                repaint.request_repaint();
            });
        }
        if finished {
            // measure again so the list shows what is left
            let it = c.items.clone();
            let m = c.measuring.clone();
            m.store(true, Ordering::Relaxed);
            std::thread::spawn(move || {
                let n = it.lock().map(|v| v.len()).unwrap_or(0);
                for i in 0..n {
                    let paths = it.lock().map(|v| v[i].measure.clone()).unwrap_or_default();
                    if paths.is_empty() {
                        continue;
                    }
                    let s: u64 = paths.iter().map(|p| tools::du(p)).sum();
                    if let Ok(mut v) = it.lock() {
                        v[i].size = Some(s);
                        v[i].selected = false;
                    }
                }
                m.store(false, Ordering::Relaxed);
            });
            self.mounts = fsinfo::mounts(self.show_pseudo);
        }
        if !open {
            self.cleanup = None;
        }
    }

    // ---------------------------------------------------------------- changes since an earlier scan

    /// Scheduled scans for this folder and a chart of its size over time.
    fn history_block(&mut self, ui: &mut Ui, pal: &Palette) {
        let lang = self.lang;
        if self.schedule.as_ref().map(|(p, _)| p != &self.root_path).unwrap_or(true) {
            self.schedule = Some((self.root_path.clone(), monitor::schedule_for(&self.root_path)));
        }
        let cur = self.schedule.as_ref().and_then(|s| s.1);
        let choices = [None, Some(monitor::Every::Hourly), Some(monitor::Every::Daily), Some(monitor::Every::Weekly)];
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(t(lang, "mon_title")).strong());
            let labels = [t(lang, "mon_never"), t(lang, "mon_hourly"), t(lang, "mon_daily"), t(lang, "mon_weekly")];
            let idx = choices.iter().position(|c| *c == cur).unwrap_or(0);
            if let Some(i) = segmented(ui, pal, &labels, idx) {
                if i != idx {
                    let res = match choices[i] {
                        None => monitor::remove_schedule(&self.root_path).map(|_| t(lang, "mon_off").to_string()),
                        Some(e) => monitor::set_schedule(&self.root_path, e).map(|_| format!("{} {} · {}", t(lang, "mon_set"), t(lang, e.key()), self.root_path.display())),
                    };
                    self.message = Some(match res {
                        Ok(m) => m,
                        Err(e) => format!("{}: {}", t(lang, "mon_fail"), e),
                    });
                    self.schedule = None;
                }
            }
        });
        let mut hint = t(lang, "mon_hint").to_string();
        if fsinfo::is_root_user() {
            hint.push(' ');
            hint.push_str(t(lang, "mon_root_note"));
        }
        ui.label(egui::RichText::new(hint).size(11.0).color(pal.weak));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(t(lang, "hist_title")).strong());
        let h = monitor::read_history(&self.root_path);
        if h.len() < 2 {
            ui.label(egui::RichText::new(t(lang, "hist_none")).size(12.0).color(pal.weak));
            return;
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 130.0), Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 8.0, pal.card);
        p.rect_stroke(rect, 8.0, Stroke::new(1.0, pal.grid), StrokeKind::Inside);
        let plot = Rect::from_min_max(rect.min + vec2(64.0, 12.0), rect.max - vec2(12.0, 22.0));
        let (t0, t1) = (h[0].ts as f32, h[h.len() - 1].ts as f32);
        let max = h.iter().map(|q| q.size).max().unwrap_or(1).max(1) as f32;
        let min = h.iter().map(|q| q.size).min().unwrap_or(0) as f32;
        let lo = (min * 0.9).min(max - 1.0);
        let xy = |ts: i64, v: f32, vlo: f32, vhi: f32| {
            pos2(plot.left() + (ts as f32 - t0) / (t1 - t0).max(1.0) * plot.width(), plot.bottom() - (v - vlo) / (vhi - vlo).max(1e-6) * plot.height())
        };
        // disk used share, faint, on its own 0–100 % scale
        let disk: Vec<egui::Pos2> = h
            .iter()
            .filter(|q| q.fs_total > 0)
            .map(|q| xy(q.ts, 1.0 - q.fs_free as f32 / q.fs_total as f32, 0.0, 1.0))
            .collect();
        if disk.len() > 1 {
            p.add(egui::Shape::line(disk, Stroke::new(1.5, pal.weak.gamma_multiply(0.6))));
        }
        let pts: Vec<egui::Pos2> = h.iter().map(|q| xy(q.ts, q.size as f32, lo, max)).collect();
        p.add(egui::Shape::line(pts.clone(), Stroke::new(2.5, pal.accent)));
        for q in &pts {
            p.circle_filled(*q, 3.0, pal.accent);
        }
        p.text(pos2(rect.left() + 8.0, plot.top()), Align2::LEFT_CENTER, format_size(max as u64, Unit::Auto), FontId::proportional(10.5), pal.weak);
        p.text(pos2(rect.left() + 8.0, plot.bottom()), Align2::LEFT_CENTER, format_size(lo as u64, Unit::Auto), FontId::proportional(10.5), pal.weak);
        p.text(pos2(plot.left(), rect.bottom() - 10.0), Align2::LEFT_CENTER, format_time(h[0].ts), FontId::proportional(10.5), pal.weak);
        p.text(pos2(plot.right(), rect.bottom() - 10.0), Align2::RIGHT_CENTER, format_time(h[h.len() - 1].ts), FontId::proportional(10.5), pal.weak);
        if let Some(hp) = resp.hover_pos() {
            if let Some((i, q)) = pts.iter().enumerate().min_by(|a, b| (a.1.x - hp.x).abs().total_cmp(&(b.1.x - hp.x).abs())) {
                p.vline(q.x, plot.y_range(), Stroke::new(1.0, pal.grid));
                p.circle_stroke(*q, 5.0, Stroke::new(2.0, pal.accent));
                let pt = &h[i];
                let used = if pt.fs_total > 0 { format_pct((1.0 - pt.fs_free as f64 / pt.fs_total as f64) * 100.0) } else { "—".into() };
                egui::show_tooltip_at_pointer(ui.ctx(), ui.layer_id(), egui::Id::new("hist_tip"), |ui| {
                    ui.label(egui::RichText::new(format_time(pt.ts)).strong());
                    ui.label(format!("{} · {} {}", format_size(pt.size, self.unit), group(pt.files), t(lang, "files_w")));
                    ui.label(format!("{}: {}", t(lang, "hist_disk"), used));
                });
            }
        }
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 4.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, pal.accent);
            ui.label(egui::RichText::new(t(lang, "col_size")).size(11.0).color(pal.weak));
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 4.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, pal.weak.gamma_multiply(0.6));
            ui.label(egui::RichText::new(format!("{} (0–100 %)", t(lang, "hist_disk"))).size(11.0).color(pal.weak));
        });
    }

    fn changes_view(&mut self, ui: &mut Ui, pal: &Palette) {
        let lang = self.lang;
        self.history_block(ui, pal);
        ui.separator();
        ui.label(egui::RichText::new(t(lang, "ch_title")).strong());
        let list: Vec<(i64, PathBuf)> = tools::list_snapshots(&self.root_path).into_iter().filter(|(_, p)| Some(p) != self.snap_saved.as_ref()).collect();
        if list.is_empty() {
            ui.label(egui::RichText::new(t(lang, "ch_none")).size(12.0).color(pal.weak));
            return;
        }
        if self.scanning {
            ui.label(egui::RichText::new(t(lang, "wait_scan")).color(pal.weak));
            return;
        }
        self.changes.pick = self.changes.pick.min(list.len() - 1);
        ui.horizontal(|ui| {
            ui.label(t(lang, "ch_since"));
            egui::ComboBox::from_id_salt("snap_pick").selected_text(format_time(list[self.changes.pick].0)).show_ui(ui, |ui| {
                for (i, (ts, _)) in list.iter().enumerate() {
                    ui.selectable_value(&mut self.changes.pick, i, format_time(*ts));
                }
            });
            ui.label(t(lang, "depth"));
            let cur = self.changes.depth.saturating_sub(1).min(2);
            if let Some(i) = segmented(ui, pal, &["1", "2", "3"], cur) {
                self.changes.depth = i + 1;
            }
        });
        let file = list[self.changes.pick].1.clone();
        if self.changes.cache.as_ref().map(|(p, _)| p != &file).unwrap_or(true) {
            self.changes.cache = Some((file.clone(), tools::load_snapshot(&file)));
            self.changes.rows_key = None;
        }
        let key = (file.clone(), self.root.size, self.root.files, self.changes.depth);
        if self.changes.rows_key.as_ref() != Some(&key) {
            let old = &self.changes.cache.as_ref().unwrap().1;
            self.changes.rows = tools::changes(old, &self.root, self.changes.depth);
            self.changes.rows_key = Some(key);
        }
        let old_total = self.changes.cache.as_ref().and_then(|(_, m)| m.get("").copied()).unwrap_or(0);
        let delta = self.root.size as i128 - old_total as i128;
        let sign = if delta >= 0 { "+" } else { "−" };
        ui.label(egui::RichText::new(format!(
            "{}: {} → {}  ({}{})",
            t(lang, "ch_total"),
            format_size(old_total, self.unit),
            format_size(self.root.size, self.unit),
            sign,
            format_size(delta.unsigned_abs() as u64, self.unit)
        )).color(if delta > 0 { pal.warn } else { pal.accent }));
        if self.changes.rows.is_empty() {
            ui.label(egui::RichText::new(t(lang, "ch_same")).color(pal.accent));
            return;
        }
        let max = self.changes.rows.first().map(|r| r.delta().unsigned_abs()).unwrap_or(1).max(1);
        let mut reveal: Option<PathBuf> = None;
        egui::ScrollArea::vertical().id_salt("changes").auto_shrink([false, false]).show(ui, |ui| {
            for r in &self.changes.rows {
                let d = r.delta();
                let (tag, col) = match (r.old, r.new) {
                    (None, _) => (t(lang, "new_w"), pal.tile(2, 0)),
                    (_, None) => (t(lang, "gone"), pal.weak),
                    _ if d > 0 => (t(lang, "grew"), pal.warn),
                    _ => (t(lang, "shrank"), pal.accent),
                };
                let sign = if d >= 0 { "+" } else { "−" };
                let value = format!("{} {}{}", tag, sign, format_size(d.unsigned_abs() as u64, self.unit));
                let resp = bar_line(ui, pal, col, &r.rel, &value, d.unsigned_abs() as f64 / max as f64);
                let resp = resp.on_hover_text(format!(
                    "{} → {}",
                    r.old.map(|s| format_size(s, self.unit)).unwrap_or_else(|| "—".into()),
                    r.new.map(|s| format_size(s, self.unit)).unwrap_or_else(|| "—".into())
                ));
                if resp.clicked() && r.new.is_some() {
                    reveal = Some(self.root_path.join(&r.rel));
                }
            }
        });
        if let Some(p) = reveal {
            self.select_and_reveal(p);
        }
    }

    fn open_compare(&mut self) {
        if self.compare.is_none() {
            let a = self.root_path.to_string_lossy().into_owned();
            let b = self.root_path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            self.compare = Some(Compare {
                a,
                b,
                running: false,
                rx: None,
                prog_a: Arc::new(Progress::default()),
                prog_b: Arc::new(Progress::default()),
                totals: None,
                rows: Vec::new(),
                show: None,
                error: None,
            });
        }
    }

    fn compare_window(&mut self, ctx: &egui::Context) {
        let pal = self.pal();
        let lang = self.lang;
        let opts = self.opts;
        let unit = self.unit;
        let current = self.root_path.to_string_lossy().into_owned();
        let Some(cmp) = self.compare.as_mut() else { return };
        // collect a finished job
        if let Some(rx) = &cmp.rx {
            if let Ok((a, b)) = rx.try_recv() {
                cmp.running = false;
                cmp.rx = None;
                match (a, b) {
                    (Some(a), Some(b)) => {
                        let mut rows = Vec::new();
                        scan::diff(&a, &b, "", &mut rows);
                        rows.sort_by(|x, y| {
                            let dx = (x.size_a as i128 - x.size_b as i128).unsigned_abs();
                            let dy = (y.size_a as i128 - y.size_b as i128).unsigned_abs();
                            dy.cmp(&dx)
                        });
                        cmp.totals = Some((a.size, b.size, a.files, b.files));
                        cmp.rows = rows;
                    }
                    (a, _) => {
                        let which = if a.is_none() { &cmp.a } else { &cmp.b };
                        cmp.error = Some(format!("{}: {}", t(lang, "not_dir"), which));
                    }
                }
            }
        }
        if cmp.running {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        let mut open = true;
        let mut start = false;
        egui::Window::new(t(lang, "compare"))
            .open(&mut open)
            .default_size([760.0, 520.0])
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("cmp_paths").num_columns(3).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
                    ui.label(egui::RichText::new(t(lang, "folder_a")).strong().color(pal.tile(0, 0)));
                    ui.add(egui::TextEdit::singleline(&mut cmp.a).desired_width(520.0));
                    if ui.small_button(t(lang, "use_current")).clicked() {
                        cmp.a = current.clone();
                    }
                    ui.end_row();
                    ui.label(egui::RichText::new(t(lang, "folder_b")).strong().color(pal.tile(2, 0)));
                    ui.add(egui::TextEdit::singleline(&mut cmp.b).desired_width(520.0));
                    if ui.small_button(t(lang, "use_current")).clicked() {
                        cmp.b = current.clone();
                    }
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    if !cmp.running && primary_button(ui, &pal, Glyph::Compare, t(lang, "compare_run")).clicked() {
                        start = true;
                    }
                    if ui.button(t(lang, "swap")).clicked() {
                        std::mem::swap(&mut cmp.a, &mut cmp.b);
                    }
                    if cmp.running {
                        ui.spinner();
                        ui.label(format!(
                            "{} A: {} · B: {} {}",
                            t(lang, "comparing"),
                            group(cmp.prog_a.files.load(Ordering::Relaxed)),
                            group(cmp.prog_b.files.load(Ordering::Relaxed)),
                            t(lang, "files_w")
                        ));
                    }
                });
                if let Some(e) = &cmp.error {
                    ui.label(egui::RichText::new(e).color(pal.warn));
                }
                if let Some((sa, sb, fa, fb)) = cmp.totals {
                    ui.separator();
                    let delta = sb as i128 - sa as i128;
                    let sign = if delta >= 0 { "+" } else { "−" };
                    ui.label(format!(
                        "A: {} · {} {}    B: {} · {} {}    {}: {}{}",
                        format_size(sa, unit),
                        group(fa),
                        t(lang, "files_w"),
                        format_size(sb, unit),
                        group(fb),
                        t(lang, "files_w"),
                        t(lang, "difference"),
                        sign,
                        format_size(delta.unsigned_abs() as u64, unit)
                    ));
                    let count = |k: scan::DiffKind| cmp.rows.iter().filter(|r| r.kind == k).count() as u64;
                    let (na, nb, nc) = (count(scan::DiffKind::OnlyA), count(scan::DiffKind::OnlyB), count(scan::DiffKind::Changed));
                    let items = [
                        format!("{} ({})", t(lang, "all_w"), group(na + nb + nc)),
                        format!("{} ({})", t(lang, "only_a"), group(na)),
                        format!("{} ({})", t(lang, "only_b"), group(nb)),
                        format!("{} ({})", t(lang, "changed"), group(nc)),
                    ];
                    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
                    let cur = match cmp.show {
                        None => 0,
                        Some(scan::DiffKind::OnlyA) => 1,
                        Some(scan::DiffKind::OnlyB) => 2,
                        Some(scan::DiffKind::Changed) => 3,
                    };
                    if let Some(i) = segmented(ui, &pal, &refs, cur) {
                        cmp.show = [None, Some(scan::DiffKind::OnlyA), Some(scan::DiffKind::OnlyB), Some(scan::DiffKind::Changed)][i];
                    }
                    if cmp.rows.is_empty() {
                        ui.label(egui::RichText::new(t(lang, "same")).color(pal.accent).strong());
                    }
                    if cmp.rows.len() >= scan::MAX_DIFF_ROWS {
                        ui.label(egui::RichText::new(t(lang, "diff_limit")).color(pal.weak).size(11.0));
                    }
                    let shown: Vec<&scan::DiffRow> = cmp.rows.iter().filter(|r| cmp.show.is_none_or(|k| r.kind == k)).collect();
                    egui::ScrollArea::vertical().id_salt("cmp_rows").auto_shrink([false, false]).show_rows(ui, 24.0, shown.len(), |ui, range| {
                        for r in &shown[range] {
                            ui.horizontal(|ui| {
                                ui.set_height(22.0);
                                let (tag, col) = match r.kind {
                                    scan::DiffKind::OnlyA => (t(lang, "only_a"), pal.tile(0, 0)),
                                    scan::DiffKind::OnlyB => (t(lang, "only_b"), pal.tile(2, 0)),
                                    scan::DiffKind::Changed => (t(lang, "changed"), pal.tile(1, 0)),
                                };
                                egui::Frame::new().fill(col).corner_radius(8.0).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
                                    ui.set_min_width(78.0);
                                    ui.label(egui::RichText::new(tag).size(11.0).color(pal.on_tile(col)));
                                });
                                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                                if r.is_dir {
                                    folder_icon(ui.painter(), rect.center(), pal.folder);
                                } else {
                                    file_icon(ui.painter(), rect.center(), pal.weak, false, pal.accent);
                                }
                                let sizes = match r.kind {
                                    scan::DiffKind::OnlyA => format_size(r.size_a, unit),
                                    scan::DiffKind::OnlyB => format_size(r.size_b, unit),
                                    scan::DiffKind::Changed => format!("{} → {}", format_size(r.size_a, unit), format_size(r.size_b, unit)),
                                };
                                let perms = if platform::HAS_POSIX_PERMS && r.kind == scan::DiffKind::Changed && (r.mode_a & 0o7777) != (r.mode_b & 0o7777) {
                                    format!("  {} → {}", fsinfo::perm_string(r.mode_a), fsinfo::perm_string(r.mode_b))
                                } else {
                                    String::new()
                                };
                                ui.label(&r.rel);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(egui::RichText::new(format!("{sizes}{perms}")).color(pal.weak).monospace().size(11.5));
                                });
                            });
                        }
                    });
                }
            });
        if start {
            let a = PathBuf::from(cmp.a.trim());
            let b = PathBuf::from(cmp.b.trim());
            cmp.error = None;
            cmp.totals = None;
            cmp.rows.clear();
            cmp.prog_a = Arc::new(Progress::default());
            cmp.prog_b = Arc::new(Progress::default());
            let (pa, pb) = (cmp.prog_a.clone(), cmp.prog_b.clone());
            let (tx, rx) = channel();
            cmp.rx = Some(rx);
            cmp.running = true;
            let repaint = ctx.clone();
            let spawned = std::thread::Builder::new().name("compare".into()).stack_size(256 * 1024 * 1024).spawn(move || {
                let hb = std::thread::Builder::new().stack_size(256 * 1024 * 1024).spawn(move || scan::scan_blocking(&b, opts, &pb));
                let na = scan::scan_blocking(&a, opts, &pa);
                let nb = hb.ok().and_then(|h| h.join().ok()).flatten();
                let _ = tx.send((na, nb));
                repaint.request_repaint();
            });
            if spawned.is_err() {
                cmp.running = false;
            }
        }
        if !open {
            if let Some(c) = &self.compare {
                c.prog_a.cancel.store(true, Ordering::Relaxed);
                c.prog_b.cancel.store(true, Ordering::Relaxed);
            }
            self.compare = None;
        }
    }

    fn browser_window(&mut self, ctx: &egui::Context) {
        let Some(cur) = self.browser.clone() else { return };
        let pal = self.pal();
        let mut open = true;
        let mut next: Option<PathBuf> = None;
        let mut scan_it: Option<PathBuf> = None;
        egui::Window::new(self.tr("choose_folder"))
            .open(&mut open)
            .default_size([560.0, 420.0])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if icon_button(ui, &pal, Glyph::Up, t(self.lang, "up"), cur.parent().is_some()).clicked() {
                        next = cur.parent().map(|p| p.to_path_buf());
                    }
                    if icon_button(ui, &pal, Glyph::Home, t(self.lang, "home"), true).clicked() {
                        next = Some(home());
                    }
                    let mut s = cur.to_string_lossy().into_owned();
                    let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width(ui.available_width()));
                    if r.changed() {
                        let p = PathBuf::from(&s);
                        if p.is_dir() {
                            next = Some(p);
                        }
                    }
                });
                ui.separator();
                ui.columns(2, |cols| {
                    cols[0].label(egui::RichText::new(t(self.lang, "mounts")).strong());
                    egui::ScrollArea::vertical().id_salt("bm").max_height(300.0).show(&mut cols[0], |ui| {
                        for m in &self.mounts {
                            if ui.selectable_label(cur == Path::new(&m.mount_point), &m.mount_point).clicked() {
                                next = Some(PathBuf::from(&m.mount_point));
                            }
                        }
                    });
                    cols[1].label(egui::RichText::new(t(self.lang, "subfolders")).strong());
                    egui::ScrollArea::vertical().id_salt("bd").max_height(300.0).show(&mut cols[1], |ui| {
                        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&cur)
                            .map(|rd| rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.path()).collect())
                            .unwrap_or_default();
                        dirs.sort();
                        for d in dirs {
                            let n = d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            let r = ui.selectable_label(false, n);
                            if r.double_clicked() {
                                scan_it = Some(d.clone());
                            } else if r.clicked() {
                                next = Some(d);
                            }
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if primary_button(ui, &pal, Glyph::Folder, t(self.lang, "scan_here")).clicked() {
                        scan_it = Some(cur.clone());
                    }
                    ui.label(egui::RichText::new(t(self.lang, "dbl_hint")).size(11.0).color(pal.weak));
                });
            });
        if let Some(n) = next {
            self.browser = Some(n);
        }
        if let Some(p) = scan_it {
            self.browser = None;
            self.start_scan(p, ctx);
        } else if !open {
            self.browser = None;
        }
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        let pal = self.pal();
        let mut undo = false;
        self.status_bar_inner(ctx, &pal, &mut undo);
        if undo {
            self.undo_last();
        }
    }

    fn status_bar_inner(&mut self, ctx: &egui::Context, pal: &Palette, undo: &mut bool) {
        let pal = pal;
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin::symmetric(12, 4)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.scanning {
                        ui.spinner();
                        ui.label(format!(
                            "{} {} {} · {} {} · {}",
                            self.tr("scanning"),
                            group(self.prog.files.load(Ordering::Relaxed)),
                            self.tr("files_w"),
                            group(self.prog.folders.load(Ordering::Relaxed)),
                            self.tr("folders_w"),
                            format_size(self.prog.bytes.load(Ordering::Relaxed), Unit::Auto)
                        ));
                        let cur = self.prog.current.lock().map(|c| c.clone()).unwrap_or_default();
                        let n = cur.chars().count();
                        let short = if n > 60 { format!("…{}", cur.chars().skip(n - 59).collect::<String>()) } else { cur };
                        ui.label(egui::RichText::new(short).color(pal.weak).size(11.0));
                    } else {
                        ui.label(egui::RichText::new("●").color(pal.accent));
                        ui.label(self.message.clone().unwrap_or_else(|| self.tr("ready").into()));
                        if !self.undo.is_empty() && ui.small_button(format!("↶ {}", self.tr("undo"))).clicked() {
                            *undo = true;
                        }
                        if let Some(f) = &self.last_export {
                            if self.message.as_deref().is_some_and(|m| m.contains(&*f.to_string_lossy())) && ui.small_button(self.tr("open_file")).clicked() {
                                platform::open_path(f);
                            }
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "· {} · {} {} · {} {} · {:.1} s",
                                format_size(self.root.size, self.unit),
                                group(self.root.files),
                                self.tr("files_w"),
                                group(self.root.folders),
                                self.tr("folders_w"),
                                self.elapsed.as_secs_f32()
                            ))
                            .color(pal.weak),
                        );
                        let errs = self.prog.errors.load(Ordering::Relaxed);
                        if errs > 0 {
                            ui.label(egui::RichText::new(format!("· {} {}", group(errs), self.tr("no_access"))).color(pal.warn));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(m) = best_mount(&self.mounts, &self.root_path) {
                            ui.label(egui::RichText::new(format!("{}: {} {} {}", m.mount_point, format_size(m.avail, Unit::Auto), t(self.lang, "free_of"), format_size(m.total, Unit::Auto))).color(pal.weak));
                        }
                    });
                });
            });
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frame += 1;
        ctx.request_repaint();
        let ready = self.frame >= 120 && (!self.scanning || self.frame >= 900);
        if ready && !self.shot_requested {
            self.shot_requested = true;
            self.frame = 0;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            return;
        }
        if !self.shot_requested && self.frame == 100 {
            self.expand_level(2);
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let mut buf = Vec::with_capacity(w * h * 4);
            for p in &img.pixels {
                buf.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = image::save_buffer(&path, &buf, w as u32, h as u32, image::ColorType::Rgba8) {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.shot_requested && self.frame > 300 {
            self.frame = 0;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        DRAWN.store(true, Ordering::Relaxed);
        self.poll();
        if self.scanning {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) && !self.scanning {
            let p = self.root_path.clone();
            self.start_scan(p, ctx);
        }
        // Delete key: send the selected entry to the trash (asks first)
        if !ctx.wants_keyboard_input() && self.confirm_trash.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            if let Some(s) = self.selected.clone() {
                if s != self.root_path && s.starts_with(&self.root_path) {
                    self.confirm_trash = Some(s);
                }
            }
        }
        if !ctx.wants_keyboard_input() && !self.undo.is_empty() && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
            self.undo_last();
        }
        let pal = self.pal();
        self.top_bar(ctx);
        self.status_bar(ctx);
        self.sidebar(ctx);
        egui::SidePanel::right("details")
            .resizable(true)
            .default_width(400.0)
            .min_width(300.0)
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin { left: 6, right: 12, top: 12, bottom: 10 }))
            .show(ctx, |ui| self.details(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin { left: 12, right: 6, top: 10, bottom: 10 }))
            .show(ctx, |ui| {
                self.breadcrumb(ui);
                ui.add_space(4.0);
                self.tree_toolbar(ui);
                self.filter_chips(ui);
                ui.add_space(6.0);
                self.tree(ui);
            });
        self.settings_window(ctx);
        self.compare_window(ctx);
        self.filters_window(ctx);
        self.dups_window(ctx);
        self.cleanup_window(ctx);
        self.confirm_trash_window(ctx);
        self.browser_window(ctx);
        self.screenshot_step(ctx);
    }
}

/// Small caption above a sidebar section.
fn section(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.label(egui::RichText::new(text).size(10.5).strong().color(pal.weak));
    ui.add_space(2.0);
}

/// Sidebar entry: icon, label, optional sub-line with a usage bar.
fn side_item(ui: &mut Ui, pal: &Palette, icon: impl Fn(&egui::Painter, egui::Pos2), label: &str, sub: Option<(String, f32)>, active: bool) -> egui::Response {
    let h = if sub.is_some() { 46.0 } else { 30.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 8.0, pal.sel_bg);
    } else if resp.hovered() {
        p.rect_filled(rect, 8.0, pal.hover);
    }
    icon(p, pos2(rect.left() + 16.0, rect.top() + 15.0));
    p.with_clip_rect(rect).text(pos2(rect.left() + 34.0, rect.top() + 15.0), Align2::LEFT_CENTER, label, FontId::proportional(13.0), if active { pal.accent } else { pal.text });
    if let Some((s, f)) = sub {
        let bar = Rect::from_min_size(pos2(rect.left() + 34.0, rect.top() + 26.0), vec2(rect.width() - 44.0, 4.0));
        p.rect_filled(bar, 2.0, pal.bar_bg);
        p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(4.0), bar.height())), 2.0, if f > 0.9 { pal.warn } else { pal.bar });
        p.with_clip_rect(rect).text(pos2(rect.left() + 34.0, rect.top() + 38.0), Align2::LEFT_CENTER, s, FontId::proportional(10.5), pal.weak);
    }
    resp
}

/// A labelled horizontal bar row (types, largest files, age).
fn bar_line(ui: &mut Ui, pal: &Palette, col: Color32, label: &str, value: &str, frac: f64) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 6.0, pal.hover);
    }
    p.circle_filled(pos2(rect.left() + 9.0, rect.top() + 11.0), 4.5, col);
    let vg = p.layout_no_wrap(value.to_string(), FontId::proportional(11.5), pal.weak);
    let label_clip = Rect::from_min_max(rect.min, pos2(rect.right() - vg.size().x - 14.0, rect.bottom()));
    p.with_clip_rect(label_clip).text(pos2(rect.left() + 20.0, rect.top() + 11.0), Align2::LEFT_CENTER, label, FontId::proportional(12.5), pal.text);
    p.galley(pos2(rect.right() - 6.0 - vg.size().x, rect.top() + 11.0 - vg.size().y / 2.0), vg, pal.weak);
    let track = Rect::from_min_size(pos2(rect.left() + 20.0, rect.top() + 23.0), vec2(rect.width() - 26.0, 5.0));
    p.rect_filled(track, 2.5, pal.bar_bg);
    let w = (track.width() * frac.clamp(0.0, 1.0) as f32).max(if frac > 0.0 { 4.0 } else { 0.0 });
    p.rect_filled(Rect::from_min_size(track.min, vec2(w, track.height())), 2.5, col);
    resp
}

fn short_mount(m: &str) -> String {
    if m == "/" {
        return "/".into();
    }
    let last = m.rsplit('/').next().unwrap_or(m);
    if last.chars().count() > 18 {
        format!("{}…", last.chars().take(17).collect::<String>())
    } else {
        last.to_string()
    }
}

fn best_mount<'a>(mounts: &'a [Mount], p: &Path) -> Option<&'a Mount> {
    mounts.iter().filter(|m| p.starts_with(&m.mount_point)).max_by_key(|m| m.mount_point.len())
}

fn open_in_file_manager(p: &Path) {
    platform::open_path(p);
}

fn open_terminal(p: &Path) {
    platform::open_terminal(p);
}

/// Set once the first frame is drawn: a failure before that is a start-up
/// (graphics) problem, after it an ordinary crash.
static DRAWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn renderer_choice() -> &'static str {
    match std::env::var("ARBOLVIEW_RENDERER").as_deref() {
        Ok("glow") | Ok("opengl") => "glow",
        Ok("wgpu") | Ok("dx12") | Ok("vulkan") => "wgpu",
        _ if cfg!(windows) => "wgpu",
        _ => "glow",
    }
}

/// Start this program again with the OpenGL renderer (one window loop per
/// process, so a second try needs a new process). Returns whether it started.
#[cfg_attr(not(windows), allow(dead_code))]
fn relaunch_with_glow(first_error: &str) -> bool {
    if renderer_choice() != "wgpu" || std::env::var_os("ARBOLVIEW_FIRST_ERROR").is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    std::process::Command::new(exe)
        .args(std::env::args().skip(1))
        .env("ARBOLVIEW_RENDERER", "glow")
        .env("ARBOLVIEW_FIRST_ERROR", first_error)
        .spawn()
        .is_ok()
}

fn write_crash_log(text: &str) -> PathBuf {
    let p = platform::crash_log();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let body = format!(
        "ÁrbolView {} — {stamp}\n{} {} · renderer {}\n\n{text}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        renderer_choice()
    );
    let _ = std::fs::write(&p, body);
    p
}

/// A panic shows a message (there's no console on Windows) and leaves a crash
/// log; before the first frame on Windows it first retries with OpenGL.
fn install_crash_handler() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let what = format!("{info}\n\n{}", std::backtrace::Backtrace::force_capture());
        let started = DRAWN.load(Ordering::Relaxed);
        if cfg!(windows) && !started && relaunch_with_glow(&info.to_string()) {
            std::process::exit(0);
        }
        let log = write_crash_log(&what);
        let lang = Lang::from_env();
        let head = if started { t(lang, "crashed") } else { t(lang, "start_fail") };
        platform::error_dialog(APP_NAME, &format!("{head}\n\n{info}\n\n{} {}", t(lang, "crash_saved"), log.display()));
    }));
}

fn main() -> eframe::Result<()> {
    // command-line automation: `arbolview scan|history|schedule|help ...` never opens a window
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().is_some_and(|a| cli::COMMANDS.contains(&a.as_str())) {
        platform::attach_parent_console();
        std::process::exit(cli::run(all));
    }
    let mut args = std::env::args().skip(1);
    let mut screenshot: Option<PathBuf> = None;
    let mut start: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        // Windows passes the person's language and theme to an elevated window this way
        if let Some(v) = a.strip_prefix("--arbolview-lang=") {
            std::env::set_var("ARBOLVIEW_LANG", v);
        } else if let Some(v) = a.strip_prefix("--arbolview-theme=") {
            std::env::set_var("ARBOLVIEW_THEME", v);
        } else if a == "--screenshot" {
            screenshot = args.next().map(PathBuf::from);
        } else if a == "--write-icon" {
            // packaging helper: .png, .ico (Windows) or .icns (macOS) by the file's extension
            if let Some(p) = args.next() {
                let lower = p.to_lowercase();
                if lower.ends_with(".ico") {
                    let _ = std::fs::write(&p, icon::ico());
                } else if lower.ends_with(".icns") {
                    let _ = std::fs::write(&p, icon::icns());
                } else {
                    let _ = image::save_buffer(&p, &icon::rgba(), 256, 256, image::ColorType::Rgba8);
                }
            }
            return Ok(());
        } else if !a.starts_with("--") {
            start = Some(PathBuf::from(a));
        }
    }
    let start = start.unwrap_or_else(home);
    install_crash_handler();
    if screenshot.is_none() {
        icon::install_desktop_entry();
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id(icon::APP_ID)
        .with_icon(std::sync::Arc::new(icon::icon_data()))
        .with_inner_size([1360.0, 820.0])
        .with_min_inner_size([960.0, 560.0]);
    if screenshot.is_some() {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    if screenshot.is_some() {
        options.event_loop_builder = Some(Box::new(|b| {
            use egui_winit::winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    #[cfg(windows)]
    {
        options.renderer = if renderer_choice() == "wgpu" { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow };
    }
    let res = eframe::run_native(APP_NAME, options, Box::new(move |cc| Ok(Box::new(App::new(cc, start, screenshot)))));
    if let Err(e) = &res {
        let err = e.to_string();
        if cfg!(windows) && relaunch_with_glow(&err) {
            return Ok(());
        }
        let mut what = err.clone();
        if let Ok(first) = std::env::var("ARBOLVIEW_FIRST_ERROR") {
            what = format!("Direct3D/wgpu: {first}\nOpenGL: {err}");
        }
        let log = write_crash_log(&what);
        let lang = Lang::from_env();
        platform::error_dialog(APP_NAME, &format!("{}\n\n{what}\n\n{} {}", t(lang, "start_fail"), t(lang, "crash_saved"), log.display()));
    }
    res
}
