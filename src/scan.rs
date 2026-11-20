//! Background directory scanner for Linux file systems.
//!
//! Sizes follow the rules written in MECHANICS.md: the apparent size is the
//! byte length of each file, the allocated size is `st_blocks * 512`, hard
//! links are counted once, symbolic links are never followed and, by default,
//! the scan stays on the file system it started on.

use std::collections::{BinaryHeap, HashSet};
use std::cmp::Reverse;
use std::fs;
use crate::platform::facts;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Node {
    pub name: Box<str>,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub alloc: u64,
    pub files: u64,
    pub folders: u64,
    pub mtime: i64,
    pub uid: u32,
    pub gid: u32,
    /// st_mode: file type and permission bits.
    pub mode: u32,
    pub denied: bool,
    pub other_fs: bool,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new_dir(name: &str) -> Self {
        Node {
            name: name.into(),
            is_dir: true,
            is_link: false,
            size: 0,
            alloc: 0,
            files: 0,
            folders: 0,
            mtime: 0,
            uid: 0,
            gid: 0,
            mode: 0o040755,
            denied: false,
            other_fs: false,
            children: Vec::new(),
        }
    }

    pub fn add_child_totals(&mut self, c: &Node) {
        self.size += c.size;
        self.alloc += c.alloc;
        if c.is_dir {
            self.files += c.files;
            self.folders += c.folders + 1;
        } else {
            self.files += 1;
        }
        self.mtime = self.mtime.max(c.mtime);
    }

    pub fn sub_child_totals(&mut self, c: &Node) {
        self.size = self.size.saturating_sub(c.size);
        self.alloc = self.alloc.saturating_sub(c.alloc);
        if c.is_dir {
            self.files = self.files.saturating_sub(c.files);
            self.folders = self.folders.saturating_sub(c.folders + 1);
        } else {
            self.files = self.files.saturating_sub(1);
        }
    }

    /// Remove the entry at `comps` (path below this node), fixing every total on the way.
    pub fn take(&mut self, comps: &[String]) -> Option<Node> {
        let first = comps.first()?;
        let idx = self.children.iter().position(|c| *c.name == **first)?;
        let removed = if comps.len() == 1 { self.children.remove(idx) } else { self.children[idx].take(&comps[1..])? };
        self.sub_child_totals(&removed);
        Some(removed)
    }

    /// Put `node` back into the folder at `parent` (path below this node).
    pub fn put(&mut self, parent: &[String], node: Node) -> bool {
        if let Some(first) = parent.first() {
            let Some(idx) = self.children.iter().position(|c| *c.name == **first) else { return false };
            let totals = node.clone_totals();
            if !self.children[idx].put(&parent[1..], node) {
                return false;
            }
            self.add_child_totals(&totals);
            true
        } else {
            self.add_child_totals(&node);
            self.children.push(node);
            self.children.sort_by(|a, b| b.size.cmp(&a.size));
            true
        }
    }

    fn clone_totals(&self) -> Node {
        let mut n = Node::new_dir("");
        n.is_dir = self.is_dir;
        n.size = self.size;
        n.alloc = self.alloc;
        n.files = self.files;
        n.folders = self.folders;
        n.mtime = self.mtime;
        n
    }

    pub fn recompute(&mut self) {
        let (mut s, mut a, mut f, mut d, mut m) = (0, 0, 0, 0, self.mtime);
        for c in &self.children {
            s += c.size;
            a += c.alloc;
            m = m.max(c.mtime);
            if c.is_dir {
                f += c.files;
                d += c.folders + 1;
            } else {
                f += 1;
            }
        }
        self.size = s;
        self.alloc = a;
        self.files = f;
        self.folders = d;
        self.mtime = m;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScanOptions {
    pub one_filesystem: bool,
    pub hardlinks_once: bool,
    pub skip_virtual: bool,
    /// Leave symbolic links out entirely. Otherwise a link counts with its
    /// own tiny size; links are never followed either way.
    pub skip_symlinks: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { one_filesystem: true, hardlinks_once: true, skip_virtual: true, skip_symlinks: false }
    }
}

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub folders: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub current: Mutex<String>,
    pub cancel: AtomicBool,
}

pub struct BigFile {
    pub path: PathBuf,
    pub size: u64,
    pub alloc: u64,
    pub mtime: i64,
}

pub enum ScanMsg {
    /// One finished child of the scanned root (sent as soon as it is done).
    Child(Node),
    /// The root itself (its own metadata), sent first.
    Root { mtime: i64, uid: u32, gid: u32, mode: u32 },
    Done { top_files: Vec<BigFile>, cancelled: bool },
    Failed(String),
}

/// Pseudo file systems that hold no real disk data.
#[cfg(target_os = "linux")]
pub const VIRTUAL_PATHS: &[&str] = &["/proc", "/sys", "/dev", "/run"];
#[cfg(target_os = "macos")]
pub const VIRTUAL_PATHS: &[&str] = &["/dev", "/System/Volumes/VM", "/private/var/vm"];
#[cfg(windows)]
pub const VIRTUAL_PATHS: &[&str] = &[];

struct Ctx<'a> {
    opts: ScanOptions,
    root_dev: Option<u64>,
    seen: HashSet<(u64, u64)>,
    top: BinaryHeap<Reverse<(u64, PathBuf, u64, i64)>>,
    prog: &'a Progress,
}

const TOP_FILES: usize = 100;

pub fn spawn(root: PathBuf, opts: ScanOptions, prog: Arc<Progress>, tx: Sender<ScanMsg>, ctx: egui::Context) {
    std::thread::Builder::new()
        .name("scanner".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            run(&root, opts, &prog, &tx, &ctx);
            ctx.request_repaint();
        })
        .expect("scanner thread");
}

fn run(root: &Path, opts: ScanOptions, prog: &Progress, tx: &Sender<ScanMsg>, ui: &egui::Context) {
    let meta = match fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(e) => {
            let _ = tx.send(ScanMsg::Failed(format!("{}: {}", root.display(), e)));
            return;
        }
    };
    let f = facts(&meta);
    let _ = tx.send(ScanMsg::Root { mtime: f.mtime, uid: f.uid, gid: f.gid, mode: f.mode });
    let mut ctx = Ctx { opts, root_dev: f.dev, seen: HashSet::new(), top: BinaryHeap::new(), prog };
    match fs::read_dir(root) {
        Ok(rd) => {
            for ent in rd.flatten() {
                if prog.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let p = ent.path();
                if let Some(n) = scan_entry(&p, &mut ctx) {
                    let _ = tx.send(ScanMsg::Child(n));
                    ui.request_repaint();
                }
            }
        }
        Err(e) => {
            prog.errors.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(ScanMsg::Failed(format!("{}: {}", root.display(), e)));
        }
    }
    let mut top: Vec<BigFile> = ctx
        .top
        .into_iter()
        .map(|Reverse((size, path, alloc, mtime))| BigFile { path, size, alloc, mtime })
        .collect();
    top.sort_by(|a, b| b.size.cmp(&a.size));
    let _ = tx.send(ScanMsg::Done { top_files: top, cancelled: prog.cancel.load(Ordering::Relaxed) });
}

fn scan_entry(path: &Path, ctx: &mut Ctx) -> Option<Node> {
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => {
            ctx.prog.errors.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    let ft = meta.file_type();
    let f = facts(&meta);
    if ft.is_dir() {
        let mut node = Node::new_dir(&name);
        node.mtime = f.mtime;
        node.uid = f.uid;
        node.gid = f.gid;
        node.mode = f.mode;
        // The directory entry itself occupies blocks too.
        let own_alloc = f.alloc;
        ctx.prog.folders.fetch_add(1, Ordering::Relaxed);
        if ctx.opts.skip_virtual && VIRTUAL_PATHS.iter().any(|v| Path::new(v) == path) {
            node.other_fs = true;
            return Some(node);
        }
        if ctx.opts.one_filesystem && f.dev.is_some() && f.dev != ctx.root_dev {
            node.other_fs = true;
            return Some(node);
        }
        if let Ok(mut cur) = ctx.prog.current.lock() {
            cur.clear();
            cur.push_str(&path.to_string_lossy());
        }
        match fs::read_dir(path) {
            Ok(rd) => {
                for ent in rd.flatten() {
                    if ctx.prog.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Some(c) = scan_entry(&ent.path(), ctx) {
                        node.add_child_totals(&c);
                        node.children.push(c);
                    }
                }
            }
            Err(_) => {
                node.denied = true;
                ctx.prog.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        node.alloc += own_alloc;
        node.children.shrink_to_fit();
        node.children.sort_by(|a, b| b.size.cmp(&a.size));
        Some(node)
    } else {
        if ft.is_symlink() && ctx.opts.skip_symlinks {
            return None;
        }
        let mut size = meta.len();
        let mut alloc = f.alloc;
        if ctx.opts.hardlinks_once && f.nlink > 1 && !ft.is_symlink() {
            if let Some(id) = f.id {
                if !ctx.seen.insert(id) {
                    size = 0;
                    alloc = 0;
                }
            }
        }
        ctx.prog.files.fetch_add(1, Ordering::Relaxed);
        ctx.prog.bytes.fetch_add(size, Ordering::Relaxed);
        if size > 0 && !ft.is_symlink() {
            let min = ctx.top.peek().map(|r| r.0 .0).unwrap_or(0);
            if ctx.top.len() < TOP_FILES || size > min {
                ctx.top.push(Reverse((size, path.to_path_buf(), alloc, f.mtime)));
                if ctx.top.len() > TOP_FILES {
                    ctx.top.pop();
                }
            }
        }
        Some(Node {
            name: name.into(),
            is_dir: false,
            is_link: ft.is_symlink(),
            size,
            alloc,
            files: 0,
            folders: 0,
            mtime: f.mtime,
            uid: f.uid,
            gid: f.gid,
            mode: f.mode,
            denied: false,
            other_fs: false,
            children: Vec::new(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Alloc,
    Files,
    Folders,
    Percent,
    Modified,
    Owner,
    Perms,
}

pub fn sort_tree(n: &mut Node, key: SortKey, asc: bool, owners: &dyn Fn(u32) -> String) {
    n.children.sort_by(|a, b| {
        let o = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size | SortKey::Percent => a.size.cmp(&b.size),
            SortKey::Alloc => a.alloc.cmp(&b.alloc),
            SortKey::Files => a.files.cmp(&b.files),
            SortKey::Folders => a.folders.cmp(&b.folders),
            SortKey::Modified => a.mtime.cmp(&b.mtime),
            SortKey::Owner => owners(a.uid).cmp(&owners(b.uid)),
            SortKey::Perms => (a.mode & 0o7777).cmp(&(b.mode & 0o7777)),
        };
        if asc { o } else { o.reverse() }
    });
    for c in &mut n.children {
        if c.is_dir {
            sort_tree(c, key, asc, owners);
        }
    }
}

/// Size, file count per extension below a node.
pub fn extensions(n: &Node, out: &mut std::collections::HashMap<String, (u64, u64)>) {
    for c in &n.children {
        if c.is_dir {
            extensions(c, out);
        } else {
            let ext = match c.name.rfind('.') {
                Some(i) if i > 0 && i + 1 < c.name.len() => c.name[i + 1..].to_lowercase(),
                _ => String::new(),
            };
            let e = out.entry(ext).or_insert((0, 0));
            e.0 += c.size;
            e.1 += 1;
        }
    }
}

/// Upper age limits (seconds) of the age buckets; the last bucket is open.
pub const AGE_LIMITS: [i64; 5] = [30 * 86400, 182 * 86400, 365 * 86400, 2 * 365 * 86400, 5 * 365 * 86400];

/// Size and file count per age bucket (by last modification) below a node.
pub fn ages(n: &Node, now: i64, out: &mut [(u64, u64); 6]) {
    for c in &n.children {
        if c.is_dir {
            ages(c, now, out);
        } else {
            let age = now - c.mtime;
            let b = AGE_LIMITS.iter().position(|l| age < *l).unwrap_or(5);
            out[b].0 += c.size;
            out[b].1 += 1;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffKind {
    OnlyA,
    OnlyB,
    Changed,
}

#[derive(Clone, Debug)]
pub struct DiffRow {
    pub rel: String,
    pub kind: DiffKind,
    pub is_dir: bool,
    pub size_a: u64,
    pub size_b: u64,
    pub mode_a: u32,
    pub mode_b: u32,
}

pub const MAX_DIFF_ROWS: usize = 20000;

/// Differences between two scanned trees, matched by name. A folder present on
/// one side only is one row (its insides are not listed). Two files differ when
/// their size, or their permission bits, are not the same. Folders whose
/// contents match fully produce no row.
pub fn diff(a: &Node, b: &Node, rel: &str, out: &mut Vec<DiffRow>) {
    use std::collections::HashMap;
    let rel_of = |name: &str| if rel.is_empty() { name.to_string() } else { format!("{}/{}", rel, name) };
    let bmap: HashMap<&str, &Node> = b.children.iter().map(|c| (&*c.name, c)).collect();
    let amap: HashMap<&str, &Node> = a.children.iter().map(|c| (&*c.name, c)).collect();
    for ca in &a.children {
        if out.len() >= MAX_DIFF_ROWS {
            return;
        }
        match bmap.get(&*ca.name) {
            None => out.push(DiffRow { rel: rel_of(&ca.name), kind: DiffKind::OnlyA, is_dir: ca.is_dir, size_a: ca.size, size_b: 0, mode_a: ca.mode, mode_b: 0 }),
            Some(cb) => {
                if ca.is_dir && cb.is_dir {
                    diff(ca, cb, &rel_of(&ca.name), out);
                } else if ca.is_dir != cb.is_dir || ca.size != cb.size || (ca.mode & 0o7777) != (cb.mode & 0o7777) {
                    out.push(DiffRow { rel: rel_of(&ca.name), kind: DiffKind::Changed, is_dir: ca.is_dir, size_a: ca.size, size_b: cb.size, mode_a: ca.mode, mode_b: cb.mode });
                }
            }
        }
    }
    for cb in &b.children {
        if out.len() >= MAX_DIFF_ROWS {
            return;
        }
        if !amap.contains_key(&*cb.name) {
            out.push(DiffRow { rel: rel_of(&cb.name), kind: DiffKind::OnlyB, is_dir: cb.is_dir, size_a: 0, size_b: cb.size, mode_a: 0, mode_b: cb.mode });
        }
    }
}

/// Scans one folder to the end on the caller's thread (used by the comparer's
/// worker thread), returning the whole tree.
pub fn scan_blocking(root: &Path, opts: ScanOptions, prog: &Progress) -> Option<Node> {
    scan_full(root, opts, prog).map(|(n, _)| n)
}

/// Like `scan_blocking`, also returning the largest files (for reports).
pub fn scan_full(root: &Path, opts: ScanOptions, prog: &Progress) -> Option<(Node, Vec<BigFile>)> {
    let meta = fs::symlink_metadata(root).ok()?;
    let mut ctx = Ctx { opts, root_dev: facts(&meta).dev, seen: HashSet::new(), top: BinaryHeap::new(), prog };
    let mut n = scan_entry(root, &mut ctx)?;
    n.name = root.to_string_lossy().into();
    let mut top: Vec<BigFile> = ctx.top.into_iter().map(|Reverse((size, path, alloc, mtime))| BigFile { path, size, alloc, mtime }).collect();
    top.sort_by(|a, b| b.size.cmp(&a.size));
    Some((n, top))
}
