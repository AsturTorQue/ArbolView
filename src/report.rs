//! Reports of a scan: plain text, CSV, JSON, a self-contained HTML page and a
//! PDF (written directly, no library), in the interface language.

use crate::fsinfo::{format_pct, format_size, format_time, group, Unit};
use crate::i18n::{t, Lang};
use crate::monitor::HistPoint;
use crate::scan::{BigFile, Node};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Text,
    Csv,
    Json,
    Html,
    Pdf,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        match s.to_lowercase().as_str() {
            "text" | "txt" => Some(Format::Text),
            "csv" => Some(Format::Csv),
            "json" => Some(Format::Json),
            "html" | "htm" => Some(Format::Html),
            "pdf" => Some(Format::Pdf),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Html => "html",
            Format::Pdf => "pdf",
        }
    }
}

pub struct FolderRow {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub size: u64,
    pub alloc: u64,
    pub files: u64,
    pub pct: f64,
}

pub struct Report<'a> {
    pub lang: Lang,
    pub root_path: PathBuf,
    pub root: &'a Node,
    pub ts: i64,
    pub disk: Option<(String, String, u64, u64)>, // mount, type, total, available
    pub folders: Vec<FolderRow>,
    pub types: Vec<(String, u64, u64)>,
    pub largest: Vec<(PathBuf, u64, i64)>,
    pub ages: [(u64, u64); 6],
    pub history: Vec<HistPoint>,
    pub owners: HashMap<u32, String>,
}

const PER_LEVEL: usize = 12;

pub fn build<'a>(lang: Lang, root_path: &Path, root: &'a Node, top: &[BigFile], depth: usize, max_rows: usize, history: Vec<HistPoint>) -> Report<'a> {
    fn walk(n: &Node, rel: &str, depth: usize, max: usize, limit: usize, out: &mut Vec<FolderRow>) {
        if depth >= max {
            return;
        }
        let mut kids: Vec<&Node> = n.children.iter().filter(|c| c.is_dir).collect();
        kids.sort_by(|a, b| b.size.cmp(&a.size));
        for c in kids.into_iter().take(PER_LEVEL) {
            if out.len() >= limit {
                return;
            }
            let r = if rel.is_empty() { c.name.to_string() } else { format!("{}/{}", rel, c.name) };
            out.push(FolderRow {
                rel: r.clone(),
                name: c.name.to_string(),
                depth,
                size: c.size,
                alloc: c.alloc,
                files: c.files,
                pct: if n.size > 0 { c.size as f64 / n.size as f64 * 100.0 } else { 0.0 },
            });
            walk(c, &r, depth + 1, max, limit, out);
        }
    }
    let mut folders = Vec::new();
    walk(root, "", 0, depth.max(1), max_rows, &mut folders);
    let mut m = HashMap::new();
    crate::scan::extensions(root, &mut m);
    let mut types: Vec<(String, u64, u64)> = m.into_iter().map(|(k, (s, c))| (k, s, c)).collect();
    types.sort_by(|a, b| b.1.cmp(&a.1));
    types.truncate(20);
    let mut ages = [(0u64, 0u64); 6];
    let now = now();
    crate::scan::ages(root, now, &mut ages);
    // free space straight from the scanned folder (works on overlay/composefs roots too);
    // the mount list only names it
    let disk = crate::fsinfo::statvfs(&root_path.to_string_lossy()).map(|(total, _free, avail, ..)| {
        let mt = crate::fsinfo::mounts(true).into_iter().filter(|mt| root_path.starts_with(&mt.mount_point)).max_by_key(|mt| mt.mount_point.len());
        let (mp, ty) = mt.map(|m| (m.mount_point, m.fs_type)).unwrap_or_else(|| ("/".into(), String::new()));
        (mp, ty, total, avail)
    });
    Report {
        lang,
        root_path: root_path.to_path_buf(),
        root,
        ts: now,
        disk,
        folders,
        types,
        largest: top.iter().take(25).map(|f| (f.path.clone(), f.size, f.mtime)).collect(),
        ages,
        history,
        owners: crate::fsinfo::users(),
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn sz(b: u64) -> String {
    format_size(b, Unit::Auto)
}

fn age_keys() -> [&'static str; 6] {
    ["age_0", "age_1", "age_2", "age_3", "age_4", "age_5"]
}

fn ext_label(lang: Lang, e: &str) -> String {
    if e.is_empty() { t(lang, "no_ext").to_string() } else { format!(".{e}") }
}

pub fn render(r: &Report, f: Format) -> Vec<u8> {
    match f {
        Format::Text => text(r).into_bytes(),
        Format::Csv => csv_all(r.lang, &r.root_path, r.root, &r.owners).into_bytes(),
        Format::Json => json(r).into_bytes(),
        Format::Html => html(r).into_bytes(),
        Format::Pdf => pdf(r),
    }
}

// ------------------------------------------------------------------- text

pub fn text(r: &Report) -> String {
    let l = r.lang;
    let n = r.root;
    let mut s = format!("ÁrbolView — {}\n{}\n{} {}\n\n", t(l, "rep_title"), r.root_path.display(), t(l, "rep_generated"), format_time(r.ts));
    s.push_str(&format!(
        "{}: {}   {}: {}   {}: {}   {}: {}\n",
        t(l, "col_size"),
        sz(n.size),
        t(l, "col_alloc"),
        sz(n.alloc),
        t(l, "col_files"),
        group(n.files),
        t(l, "col_folders"),
        group(n.folders)
    ));
    if let Some((mp, ty, total, avail)) = &r.disk {
        s.push_str(&format!("{} {} ({}): {} {} {}\n", t(l, "disk_w"), mp, ty, sz(*avail), t(l, "free_of"), sz(*total)));
    }
    s.push_str(&format!("\n{}\n", t(l, "rep_folders")));
    for f in &r.folders {
        s.push_str(&format!("{:>12}  {:>6}  {}{}\n", sz(f.size), format!("{:.1}%", f.pct), "  ".repeat(f.depth), f.name));
    }
    s.push_str(&format!("\n{}\n", t(l, "types_title")));
    for (e, b, c) in r.types.iter().take(10) {
        s.push_str(&format!("{:>12}  {:>10} {}  {}\n", sz(*b), group(*c), t(l, "files_w"), ext_label(l, e)));
    }
    s.push_str(&format!("\n{}\n", t(l, "largest")));
    for (p, b, _) in r.largest.iter().take(10) {
        s.push_str(&format!("{:>12}  {}\n", sz(*b), p.display()));
    }
    if r.history.len() > 1 {
        s.push_str(&format!("\n{}\n", t(l, "rep_history")));
        for h in &r.history {
            s.push_str(&format!("{}  {:>12}  {} {}\n", format_time(h.ts), sz(h.size), group(h.files), t(l, "files_w")));
        }
    }
    s
}

// ------------------------------------------------------------------- csv / json

pub fn csv_all(lang: Lang, root_path: &Path, root: &Node, owners: &HashMap<u32, String>) -> String {
    fn walk(n: &Node, p: &Path, out: &mut String, owners: &HashMap<u32, String>) {
        let q = p.to_string_lossy().replace('"', "\"\"");
        out.push_str(&format!(
            "\"{}\",{},{},{},{},{},{},{}\n",
            q,
            n.size,
            n.alloc,
            n.files,
            n.folders,
            format_time(n.mtime),
            owners.get(&n.uid).cloned().unwrap_or_else(|| n.uid.to_string()),
            crate::fsinfo::perm_string(n.mode)
        ));
        for c in &n.children {
            if c.is_dir {
                walk(c, &p.join(&*c.name), out, owners);
            }
        }
    }
    let mut s = format!(
        "{},{} (bytes),{} (bytes),{},{},{},{},{}\n",
        t(lang, "path"),
        t(lang, "col_size"),
        t(lang, "col_alloc"),
        t(lang, "col_files"),
        t(lang, "col_folders"),
        t(lang, "col_modified"),
        t(lang, "col_owner"),
        t(lang, "col_perms")
    );
    walk(root, root_path, &mut s, owners);
    s
}

fn js(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn json(r: &Report) -> String {
    let n = r.root;
    let folders: Vec<String> = r
        .folders
        .iter()
        .map(|f| format!("{{\"path\":{},\"depth\":{},\"size\":{},\"allocated\":{},\"files\":{},\"percent\":{:.2}}}", js(&f.rel), f.depth, f.size, f.alloc, f.files, f.pct))
        .collect();
    let types: Vec<String> = r.types.iter().map(|(e, s, c)| format!("{{\"extension\":{},\"size\":{},\"files\":{}}}", js(e), s, c)).collect();
    let largest: Vec<String> = r.largest.iter().map(|(p, s, m)| format!("{{\"path\":{},\"size\":{},\"modified\":{}}}", js(&p.to_string_lossy()), s, m)).collect();
    let hist: Vec<String> = r.history.iter().map(|h| format!("{{\"time\":{},\"size\":{},\"files\":{},\"disk_total\":{},\"disk_free\":{}}}", h.ts, h.size, h.files, h.fs_total, h.fs_free)).collect();
    let disk = match &r.disk {
        Some((mp, ty, total, avail)) => format!("{{\"mount\":{},\"type\":{},\"total\":{},\"available\":{}}}", js(mp), js(ty), total, avail),
        None => "null".into(),
    };
    format!(
        "{{\"root\":{},\"time\":{},\"size\":{},\"allocated\":{},\"files\":{},\"folders\":{},\"disk\":{},\"top_folders\":[{}],\"types\":[{}],\"largest_files\":[{}],\"history\":[{}]}}\n",
        js(&r.root_path.to_string_lossy()),
        r.ts,
        n.size,
        n.alloc,
        n.files,
        n.folders,
        disk,
        folders.join(","),
        types.join(","),
        largest.join(","),
        hist.join(",")
    )
}

// ------------------------------------------------------------------- html

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn svg_history(h: &[HistPoint], w: f32, hgt: f32) -> String {
    if h.len() < 2 {
        return String::new();
    }
    let (t0, t1) = (h[0].ts as f32, h[h.len() - 1].ts as f32);
    let max = h.iter().map(|p| p.size).max().unwrap_or(1).max(1) as f32;
    let min = h.iter().map(|p| p.size).min().unwrap_or(0) as f32;
    let lo = (min * 0.9).min(max - 1.0);
    let x = |ts: i64| 40.0 + (ts as f32 - t0) / (t1 - t0).max(1.0) * (w - 60.0);
    let y = |s: u64| hgt - 24.0 - (s as f32 - lo) / (max - lo).max(1.0) * (hgt - 44.0);
    let pts: Vec<String> = h.iter().map(|p| format!("{:.1},{:.1}", x(p.ts), y(p.size))).collect();
    let dots: String = h.iter().map(|p| format!("<circle cx='{:.1}' cy='{:.1}' r='3'><title>{} — {}</title></circle>", x(p.ts), y(p.size), format_time(p.ts), sz(p.size))).collect();
    format!(
        "<svg viewBox='0 0 {w} {hgt}' class='chart'><line x1='40' y1='{b}' x2='{r}' y2='{b}' class='axis'/><text x='4' y='16'>{}</text><text x='4' y='{b}'>{}</text><polyline points='{}' class='line'/>{}<text x='40' y='{hgt}'>{}</text><text x='{r}' y='{hgt}' text-anchor='end'>{}</text></svg>",
        esc(&sz(max as u64)),
        esc(&sz(lo as u64)),
        pts.join(" "),
        dots,
        esc(&format_time(h[0].ts)),
        esc(&format_time(h[h.len() - 1].ts)),
        b = hgt - 24.0,
        r = w - 20.0
    )
}

pub fn html(r: &Report) -> String {
    let l = r.lang;
    let n = r.root;
    let mut s = String::new();
    s.push_str(&format!(
        "<!doctype html><html lang='{}'><head><meta charset='utf-8'><meta name='viewport' content='width=device-width,initial-scale=1'><title>ÁrbolView — {}</title><style>
:root{{--a:#2e8b57;--t:#1f2421;--w:#6b736d;--g:#e2e6e0;--b:#e9eee8}}
body{{font-family:system-ui,-apple-system,'Segoe UI',Ubuntu,sans-serif;color:var(--t);background:#fbfbf8;margin:0}}
header{{background:var(--a);color:#fff;padding:28px 40px}}header h1{{margin:0 0 4px;font-size:24px}}header p{{margin:2px 0;opacity:.9}}
main{{max-width:1000px;margin:0 auto;padding:24px 40px 60px}}
.cards{{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin-bottom:8px}}.card{{background:#fff;border:1px solid var(--g);border-radius:10px;padding:12px 14px}}
.card small{{color:var(--w)}}.card b{{display:block;font-size:20px;margin-top:4px}}
h2{{font-size:17px;margin:28px 0 10px}}table{{width:100%;border-collapse:collapse;background:#fff;border:1px solid var(--g);border-radius:10px;overflow:hidden}}
th,td{{padding:6px 10px;text-align:left;font-size:13px;border-bottom:1px solid var(--g)}}th{{color:var(--w);font-weight:600;text-transform:uppercase;font-size:11px}}
td.n{{text-align:right;white-space:nowrap;font-variant-numeric:tabular-nums}}.bar{{background:var(--b);border-radius:4px;height:8px;width:160px}}.bar i{{display:block;height:8px;border-radius:4px;background:var(--a)}}
.muted{{color:var(--w)}}.chart{{width:100%;background:#fff;border:1px solid var(--g);border-radius:10px}}.chart text{{font-size:11px;fill:var(--w)}}
.chart .line{{fill:none;stroke:var(--a);stroke-width:2.5}}.chart circle{{fill:var(--a)}}.chart .axis{{stroke:var(--g)}}footer{{color:var(--w);font-size:12px;margin-top:30px}}
@media print{{header{{-webkit-print-color-adjust:exact;print-color-adjust:exact}}}}
</style></head><body>",
        l.code(),
        esc(&r.root_path.to_string_lossy())
    ));
    s.push_str(&format!(
        "<header><h1>{}</h1><p>{}</p><p>{} {}</p></header><main>",
        esc(t(l, "rep_title")),
        esc(&r.root_path.to_string_lossy()),
        esc(t(l, "rep_generated")),
        esc(&format_time(r.ts))
    ));
    s.push_str("<div class='cards'>");
    for (k, v) in [("col_size", sz(n.size)), ("col_alloc", sz(n.alloc)), ("col_files", group(n.files)), ("col_folders", group(n.folders))] {
        s.push_str(&format!("<div class='card'><small>{}</small><b>{}</b></div>", esc(t(l, k)), esc(&v)));
    }
    s.push_str("</div>");
    if let Some((mp, ty, total, avail)) = &r.disk {
        s.push_str(&format!("<p class='muted'>{} {} ({}): {} {} {}</p>", esc(t(l, "disk_w")), esc(mp), esc(ty), esc(&sz(*avail)), esc(t(l, "free_of")), esc(&sz(*total))));
    }
    if r.history.len() > 1 {
        s.push_str(&format!("<h2>{}</h2>{}", esc(t(l, "rep_history")), svg_history(&r.history, 920.0, 220.0)));
    }
    s.push_str(&format!(
        "<h2>{}</h2><table><tr><th>{}</th><th></th><th>{}</th><th>{}</th><th>{}</th></tr>",
        esc(t(l, "rep_folders")),
        esc(t(l, "col_name")),
        esc(t(l, "col_size")),
        esc(t(l, "col_files")),
        esc(t(l, "col_share"))
    ));
    for f in &r.folders {
        s.push_str(&format!(
            "<tr><td style='padding-left:{}px' title='{}'>📁 {}</td><td><div class='bar'><i style='width:{:.1}%'></i></div></td><td class='n'>{}</td><td class='n'>{}</td><td class='n'>{}</td></tr>",
            10 + f.depth * 18,
            esc(&f.rel),
            esc(&f.name),
            f.pct.clamp(0.0, 100.0),
            esc(&sz(f.size)),
            group(f.files),
            esc(&format_pct(f.pct))
        ));
    }
    s.push_str("</table>");
    let ttotal = r.types.iter().map(|x| x.1).sum::<u64>().max(1);
    s.push_str(&format!("<h2>{}</h2><table><tr><th>{}</th><th></th><th>{}</th><th>{}</th></tr>", esc(t(l, "types_title")), esc(t(l, "ext")), esc(t(l, "col_size")), esc(t(l, "col_files"))));
    for (e, b, c) in &r.types {
        s.push_str(&format!(
            "<tr><td>{}</td><td><div class='bar'><i style='width:{:.1}%'></i></div></td><td class='n'>{}</td><td class='n'>{}</td></tr>",
            esc(&ext_label(l, e)),
            *b as f64 / ttotal as f64 * 100.0,
            esc(&sz(*b)),
            group(*c)
        ));
    }
    s.push_str("</table>");
    s.push_str(&format!("<h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th></tr>", esc(t(l, "largest")), esc(t(l, "path")), esc(t(l, "col_size")), esc(t(l, "col_modified"))));
    for (p, b, m) in &r.largest {
        s.push_str(&format!("<tr><td>{}</td><td class='n'>{}</td><td class='n'>{}</td></tr>", esc(&p.to_string_lossy()), esc(&sz(*b)), esc(&format_time(*m))));
    }
    s.push_str("</table>");
    let atotal = r.ages.iter().map(|a| a.0).sum::<u64>().max(1);
    s.push_str(&format!("<h2>{}</h2><table>", esc(t(l, "age_title"))));
    for (i, (b, c)) in r.ages.iter().enumerate() {
        s.push_str(&format!(
            "<tr><td>{}</td><td><div class='bar'><i style='width:{:.1}%'></i></div></td><td class='n'>{}</td><td class='n'>{} {}</td></tr>",
            esc(t(l, age_keys()[i])),
            *b as f64 / atotal as f64 * 100.0,
            esc(&sz(*b)),
            group(*c),
            esc(t(l, "files_w"))
        ));
    }
    s.push_str("</table>");
    s.push_str("<footer>ÁrbolView</footer></main></body></html>\n");
    s
}

// ------------------------------------------------------------------- pdf

/// A4 pages, built from text, filled rectangles and lines.
struct Pdf {
    pages: Vec<Vec<u8>>,
    cur: Vec<u8>,
    y: f32,
}

const PW: f32 = 595.0;
const PH: f32 = 842.0;
const MX: f32 = 46.0;
const ACCENT: (f32, f32, f32) = (0.18, 0.545, 0.341);
const INK: (f32, f32, f32) = (0.12, 0.14, 0.13);
const WEAK: (f32, f32, f32) = (0.42, 0.45, 0.43);
const TRACK: (f32, f32, f32) = (0.91, 0.93, 0.91);

/// WinAnsi bytes for the standard PDF fonts (Spanish, Portuguese and French letters included).
fn win_ansi(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let b: Vec<u8> = match c {
            '€' => vec![0x80],
            '…' => vec![0x85],
            '‘' => vec![0x91],
            '’' => vec![0x92],
            '“' => vec![0x93],
            '”' => vec![0x94],
            '•' => vec![0x95],
            '–' => vec![0x96],
            '—' => vec![0x97],
            '→' => b"->".to_vec(),
            '↶' => b"<-".to_vec(),
            '(' | ')' | '\\' => vec![b'\\', c as u8],
            c if (c as u32) < 0x20 => vec![b' '],
            c if (c as u32) <= 0xff => vec![c as u32 as u8],
            _ => vec![b'?'],
        };
        out.extend(b);
    }
    out
}

fn approx_width(s: &str, size: f32, bold: bool) -> f32 {
    s.chars().count() as f32 * size * if bold { 0.56 } else { 0.51 }
}

fn fit(s: &str, size: f32, bold: bool, width: f32) -> String {
    if approx_width(s, size, bold) <= width {
        return s.to_string();
    }
    let keep = ((width / (size * if bold { 0.56 } else { 0.51 })) as usize).saturating_sub(1).max(1);
    let chars: Vec<char> = s.chars().collect();
    format!("…{}", chars[chars.len().saturating_sub(keep)..].iter().collect::<String>())
}

impl Pdf {
    fn new() -> Self {
        Pdf { pages: Vec::new(), cur: Vec::new(), y: PH - 40.0 }
    }

    fn raw(&mut self, s: &str) {
        self.cur.extend_from_slice(s.as_bytes());
    }

    fn text(&mut self, x: f32, y: f32, size: f32, bold: bool, col: (f32, f32, f32), s: &str) {
        self.raw(&format!("BT /{} {:.1} Tf {:.3} {:.3} {:.3} rg {:.2} {:.2} Td (", if bold { "F2" } else { "F1" }, size, col.0, col.1, col.2, x, y));
        let enc = win_ansi(s);
        self.cur.extend_from_slice(&enc);
        self.raw(") Tj ET\n");
    }

    fn text_right(&mut self, xr: f32, y: f32, size: f32, bold: bool, col: (f32, f32, f32), s: &str) {
        let w = approx_width(s, size, bold);
        self.text(xr - w, y, size, bold, col, s);
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, col: (f32, f32, f32)) {
        self.raw(&format!("{:.3} {:.3} {:.3} rg {:.2} {:.2} {:.2} {:.2} re f\n", col.0, col.1, col.2, x, y, w, h));
    }

    fn polyline(&mut self, pts: &[(f32, f32)], col: (f32, f32, f32), width: f32) {
        if pts.len() < 2 {
            return;
        }
        self.raw(&format!("{:.3} {:.3} {:.3} RG {:.2} w 1 J 1 j {:.2} {:.2} m ", col.0, col.1, col.2, width, pts[0].0, pts[0].1));
        for p in &pts[1..] {
            self.raw(&format!("{:.2} {:.2} l ", p.0, p.1));
        }
        self.raw("S\n");
    }

    fn new_page(&mut self) {
        let done = std::mem::take(&mut self.cur);
        self.pages.push(done);
        self.y = PH - 46.0;
    }

    /// Start a new page when less than `h` points are left.
    fn need(&mut self, h: f32) {
        if self.y - h < 50.0 {
            self.new_page();
        }
    }

    fn heading(&mut self, s: &str) {
        // keep a heading together with at least a few rows
        self.need(90.0);
        self.y -= 26.0;
        self.text(MX, self.y, 13.0, true, INK, s);
        self.y -= 8.0;
        self.rect(MX, self.y, PW - 2.0 * MX, 0.8, TRACK);
        self.y -= 4.0;
    }

    /// A row: label, share bar, then right-aligned columns.
    fn bar_row(&mut self, indent: f32, label: &str, frac: f64, cols: &[String]) {
        self.need(16.0);
        self.y -= 15.0;
        // label | bar | up to three right-aligned columns of 64 pt each
        let label_w = 212.0 - indent;
        self.text(MX + indent, self.y, 9.0, false, INK, &fit(label, 9.0, false, label_w));
        let bx = MX + 218.0;
        let bw = 70.0;
        self.rect(bx, self.y, bw, 6.0, TRACK);
        let fw = (bw * frac.clamp(0.0, 1.0) as f32).max(if frac > 0.0 { 1.5 } else { 0.0 });
        self.rect(bx, self.y, fw, 6.0, ACCENT);
        let mut xr = PW - MX;
        for c in cols.iter().rev() {
            self.text_right(xr, self.y, 9.0, false, WEAK, &fit(c, 9.0, false, 60.0));
            xr -= 66.0;
        }
    }

    fn finish(mut self, title: &str) -> Vec<u8> {
        if !self.cur.is_empty() || self.pages.is_empty() {
            let done = std::mem::take(&mut self.cur);
            self.pages.push(done);
        }
        let total = self.pages.len();
        // page numbers
        for (i, p) in self.pages.iter_mut().enumerate() {
            let s = format!("BT /F1 8 Tf 0.42 0.45 0.43 rg {:.2} 24 Td (ÁrbolView  {}/{}) Tj ET\n", PW - MX - 70.0, i + 1, total);
            p.extend(win_ansi_cmd(&s));
        }
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets: Vec<usize> = Vec::new();
        let page_count = self.pages.len();
        // objects: 1 catalog, 2 pages, 3 F1, 4 F2, 5 info, then (page, content) pairs
        let kids: Vec<String> = (0..page_count).map(|i| format!("{} 0 R", 6 + i * 2)).collect();
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), page_count).into_bytes(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec(),
            {
                let mut v = b"<< /Producer (ArbolView) /Title (".to_vec();
                v.extend(win_ansi(title));
                v.extend_from_slice(b") >>");
                v
            },
        ];
        for (i, content) in self.pages.iter().enumerate() {
            objs.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PW} {PH}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                    7 + i * 2
                )
                .into_bytes(),
            );
            let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            stream.extend_from_slice(content);
            stream.extend_from_slice(b"\nendstream");
            objs.push(stream);
        }
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).into_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).into_bytes());
        for o in offsets {
            out.extend(format!("{:010} 00000 n \n", o).into_bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{}\n%%EOF\n", objs.len() + 1, xref).into_bytes());
        out
    }
}

/// A content-stream command whose text part may hold non-ASCII letters.
fn win_ansi_cmd(cmd: &str) -> Vec<u8> {
    // only the text between "(" and ") Tj" needs re-encoding
    match (cmd.find('('), cmd.rfind(") Tj")) {
        (Some(a), Some(b)) if a < b => {
            let mut v = cmd[..=a].as_bytes().to_vec();
            v.extend(win_ansi(&cmd[a + 1..b]));
            v.extend_from_slice(cmd[b..].as_bytes());
            v
        }
        _ => cmd.as_bytes().to_vec(),
    }
}

pub fn pdf(r: &Report) -> Vec<u8> {
    let l = r.lang;
    let n = r.root;
    let mut p = Pdf::new();
    // header band
    p.rect(0.0, PH - 96.0, PW, 96.0, ACCENT);
    p.text(MX, PH - 46.0, 20.0, true, (1.0, 1.0, 1.0), t(l, "rep_title"));
    p.text(MX, PH - 66.0, 10.0, false, (0.93, 0.97, 0.94), &fit(&r.root_path.to_string_lossy(), 10.0, false, PW - 2.0 * MX));
    p.text(MX, PH - 82.0, 9.0, false, (0.86, 0.94, 0.89), &format!("{} {}", t(l, "rep_generated"), format_time(r.ts)));
    p.y = PH - 112.0;
    // summary cards
    let cards = [("col_size", sz(n.size)), ("col_alloc", sz(n.alloc)), ("col_files", group(n.files)), ("col_folders", group(n.folders))];
    let cw = (PW - 2.0 * MX - 3.0 * 10.0) / 4.0;
    for (i, (k, v)) in cards.iter().enumerate() {
        let x = MX + i as f32 * (cw + 10.0);
        p.rect(x, p.y - 46.0, cw, 46.0, (0.95, 0.96, 0.95));
        p.text(x + 10.0, p.y - 17.0, 8.5, false, WEAK, t(l, k));
        p.text(x + 10.0, p.y - 36.0, 14.0, true, if i == 0 { ACCENT } else { INK }, v);
    }
    p.y -= 60.0;
    if let Some((mp, ty, total, avail)) = &r.disk {
        let used = if *total > 0 { 1.0 - *avail as f64 / *total as f64 } else { 0.0 };
        p.text(MX, p.y, 9.0, false, WEAK, &format!("{} {} ({}): {} {} {}", t(l, "disk_w"), mp, ty, sz(*avail), t(l, "free_of"), sz(*total)));
        p.y -= 10.0;
        p.rect(MX, p.y, PW - 2.0 * MX, 6.0, TRACK);
        p.rect(MX, p.y, (PW - 2.0 * MX) * used as f32, 6.0, if used > 0.9 { (0.75, 0.22, 0.17) } else { ACCENT });
        p.y -= 4.0;
    }
    // history chart
    if r.history.len() > 1 {
        p.heading(t(l, "rep_history"));
        p.need(130.0);
        let (x0, y0, w, h) = (MX + 50.0, p.y - 110.0, PW - 2.0 * MX - 60.0, 96.0);
        p.rect(x0, y0, w, 0.8, TRACK);
        let hs = &r.history;
        let (t0, t1) = (hs[0].ts as f32, hs[hs.len() - 1].ts as f32);
        let max = hs.iter().map(|q| q.size).max().unwrap_or(1).max(1) as f32;
        let min = hs.iter().map(|q| q.size).min().unwrap_or(0) as f32;
        let lo = (min * 0.9).min(max - 1.0);
        let pts: Vec<(f32, f32)> = hs.iter().map(|q| (x0 + (q.ts as f32 - t0) / (t1 - t0).max(1.0) * w, y0 + (q.size as f32 - lo) / (max - lo).max(1.0) * h)).collect();
        p.polyline(&pts, ACCENT, 2.0);
        for (x, y) in &pts {
            p.rect(x - 1.8, y - 1.8, 3.6, 3.6, ACCENT);
        }
        p.text(MX, y0 + h - 4.0, 8.0, false, WEAK, &sz(max as u64));
        p.text(MX, y0, 8.0, false, WEAK, &sz(lo as u64));
        p.text(x0, y0 - 12.0, 8.0, false, WEAK, &format_time(hs[0].ts));
        p.text_right(x0 + w, y0 - 12.0, 8.0, false, WEAK, &format_time(hs[hs.len() - 1].ts));
        p.y = y0 - 18.0;
    }
    // folders
    p.heading(t(l, "rep_folders"));
    for f in &r.folders {
        p.bar_row(f.depth as f32 * 12.0, &f.name, f.pct / 100.0, &[group(f.files), sz(f.size), format_pct(f.pct)]);
    }
    // types
    p.heading(t(l, "types_title"));
    let ttotal = r.types.iter().map(|x| x.1).sum::<u64>().max(1);
    for (e, b, c) in r.types.iter().take(15) {
        p.bar_row(0.0, &ext_label(l, e), *b as f64 / ttotal as f64, &[group(*c), sz(*b)]);
    }
    // largest files
    p.heading(t(l, "largest"));
    let lmax = r.largest.first().map(|x| x.1).unwrap_or(1).max(1);
    for (path, b, m) in r.largest.iter().take(20) {
        p.bar_row(0.0, &path.to_string_lossy(), *b as f64 / lmax as f64, &[format_time(*m), sz(*b)]);
    }
    // ages
    p.heading(t(l, "age_title"));
    let atotal = r.ages.iter().map(|a| a.0).sum::<u64>().max(1);
    for (i, (b, c)) in r.ages.iter().enumerate() {
        p.bar_row(0.0, t(l, age_keys()[i]), *b as f64 / atotal as f64, &[group(*c), sz(*b)]);
    }
    p.finish(&format!("{} — {}", t(l, "rep_title"), r.root_path.display()))
}

/// Where a report goes by default: ~/arbolview-<folder>-<time>.<ext>
pub fn default_path(root: &Path, f: Format) -> PathBuf {
    let home = crate::platform::home();
    let name: String = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into())
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    home.join(format!("arbolview-{}-{}.{}", name, now(), f.ext()))
}
