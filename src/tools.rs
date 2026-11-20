//! File management tools: the trash (freedesktop.org Trash spec), duplicate
//! finder, quick clean-up, scan history snapshots and advanced filters.

use crate::scan::Node;
use std::collections::HashMap;
use std::fs;
use std::hash::Hasher;
use std::io::{Read, Write};
use crate::platform::facts;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

fn home() -> PathBuf {
    crate::platform::home()
}

pub fn data_home() -> PathBuf {
    crate::platform::data_dir()
}

pub fn which(cmd: &str) -> bool {
    let names: Vec<String> = if cfg!(windows) { vec![format!("{cmd}.exe"), cmd.to_string()] } else { vec![cmd.to_string()] };
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| names.iter().any(|n| d.join(n).is_file())))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn local_iso_time() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[cfg(target_os = "linux")]
fn percent_encode(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::new();
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{:02X}", b));
        }
    }
    s
}

// ------------------------------------------------------------------- trash

/// What is needed to undo one "move to trash".
#[derive(Clone, Debug)]
pub struct TrashRecord {
    pub original: PathBuf,
    pub trashed: PathBuf,
    pub info: PathBuf,
}

/// The trash folder for `path`: the home trash when it is on the same file
/// system, otherwise `$topdir/.Trash-$uid` at the top of that file system.
#[cfg(target_os = "linux")]
fn trash_dir_for(path: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dev = fs::symlink_metadata(path).map_err(|e| e.to_string())?.dev();
    let ht = data_home().join("Trash");
    let _ = fs::create_dir_all(ht.join("files"));
    let _ = fs::create_dir_all(ht.join("info"));
    if fs::metadata(&ht).map(|m| m.dev() == dev).unwrap_or(false) {
        return Ok(ht);
    }
    let mut top = path.parent().map(|p| p.to_path_buf()).ok_or("no parent")?;
    while let Some(up) = top.parent() {
        match fs::metadata(up) {
            Ok(m) if m.dev() == dev => top = up.to_path_buf(),
            _ => break,
        }
    }
    let uid = unsafe { libc::getuid() };
    let t = top.join(format!(".Trash-{uid}"));
    for sub in ["files", "info"] {
        fs::create_dir_all(t.join(sub)).map_err(|e| e.to_string())?;
    }
    let _ = fs::set_permissions(&t, fs::Permissions::from_mode(0o700));
    Ok(t)
}

/// Windows and macOS: the system's own Recycle Bin / Trash.
#[cfg(not(target_os = "linux"))]
pub fn trash(path: &Path) -> Result<TrashRecord, String> {
    trash::delete(path).map_err(|e| e.to_string())?;
    Ok(TrashRecord { original: path.to_path_buf(), trashed: PathBuf::new(), info: PathBuf::new() })
}

#[cfg(windows)]
pub fn restore(r: &TrashRecord) -> Result<(), String> {
    if r.original.exists() {
        return Err(format!("{} already exists", r.original.display()));
    }
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    let newest = items.into_iter().filter(|i| i.original_path() == r.original).max_by_key(|i| i.time_deleted);
    match newest {
        Some(i) => trash::os_limited::restore_all([i]).map_err(|e| e.to_string()),
        None => Err(format!("{} is no longer in the Recycle Bin", r.original.display())),
    }
}

#[cfg(target_os = "macos")]
pub fn restore(r: &TrashRecord) -> Result<(), String> {
    Err(format!("macOS: {} → Finder ▸ Trash ▸ Put Back", r.original.display()))
}

#[cfg(target_os = "linux")]
pub fn trash(path: &Path) -> Result<TrashRecord, String> {
    let dir = trash_dir_for(path)?;
    let base = path.file_name().map(|s| s.to_string_lossy().into_owned()).ok_or("no name")?;
    for n in 1..10000 {
        let name = if n == 1 { base.clone() } else { format!("{base}.{n}") };
        let info = dir.join("info").join(format!("{name}.trashinfo"));
        let target = dir.join("files").join(&name);
        if target.exists() {
            continue;
        }
        // create_new reserves the name atomically, as the spec asks
        let mut f = match fs::OpenOptions::new().write(true).create_new(true).open(&info) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        let _ = write!(f, "[Trash Info]\nPath={}\nDeletionDate={}\n", percent_encode(path), local_iso_time());
        drop(f);
        return match fs::rename(path, &target) {
            Ok(_) => Ok(TrashRecord { original: path.to_path_buf(), trashed: target, info }),
            Err(e) => {
                let _ = fs::remove_file(&info);
                Err(e.to_string())
            }
        };
    }
    Err("trash is full".into())
}

#[cfg(target_os = "linux")]
pub fn restore(r: &TrashRecord) -> Result<(), String> {
    if r.original.exists() {
        return Err(format!("{} already exists", r.original.display()));
    }
    fs::rename(&r.trashed, &r.original).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&r.info);
    Ok(())
}

// ------------------------------------------------------------------- duplicates

#[derive(Clone, Debug)]
pub struct DupGroup {
    pub size: u64,
    pub paths: Vec<PathBuf>,
}

impl DupGroup {
    pub fn wasted(&self) -> u64 {
        self.size * (self.paths.len().saturating_sub(1)) as u64
    }
}

#[derive(Default)]
pub struct DupProgress {
    pub checked: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

/// Every regular file of the tree at least `min` bytes big.
pub fn collect_files(n: &Node, path: &Path, min: u64, out: &mut Vec<(PathBuf, u64)>) {
    for c in &n.children {
        if c.is_dir {
            collect_files(c, &path.join(&*c.name), min, out);
        } else if !c.is_link && c.size >= min && c.size > 0 {
            out.push((path.join(&*c.name), c.size));
        }
    }
}

fn hash_file(p: &Path, limit: Option<u64>) -> Option<u64> {
    let mut f = fs::File::open(p).ok()?;
    let mut h = std::hash::DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut left = limit.unwrap_or(u64::MAX);
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = f.read(&mut buf[..want]).ok()?;
        if n == 0 {
            break;
        }
        h.write(&buf[..n]);
        left -= n as u64;
    }
    Some(h.finish())
}

/// Same size, then same first 64 KiB, then same whole content. Hard links to
/// one file (same device and inode) count once.
pub fn find_duplicates(files: Vec<(PathBuf, u64)>, prog: &DupProgress) -> Vec<DupGroup> {
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for (p, s) in files {
        by_size.entry(s).or_default().push(p);
    }
    let candidates: Vec<(u64, Vec<PathBuf>)> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    prog.total.store(candidates.iter().map(|(_, v)| v.len() as u64).sum(), Ordering::Relaxed);
    let mut groups = Vec::new();
    for (size, paths) in candidates {
        if prog.cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut seen = std::collections::HashSet::new();
        let mut quick: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        for p in paths {
            prog.checked.fetch_add(1, Ordering::Relaxed);
            let Ok(m) = fs::metadata(&p) else { continue };
            if let Some(id) = facts(&m).id {
                if !seen.insert(id) {
                    continue;
                }
            }
            if let Some(h) = hash_file(&p, Some(64 * 1024)) {
                quick.entry(h).or_default().push(p);
            }
        }
        for (_, ps) in quick.into_iter().filter(|(_, v)| v.len() > 1) {
            let mut full: HashMap<u64, Vec<PathBuf>> = HashMap::new();
            for p in ps {
                if prog.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let h = if size <= 64 * 1024 { Some(0) } else { hash_file(&p, None) };
                if let Some(h) = h {
                    full.entry(h).or_default().push(p);
                }
            }
            for (_, mut v) in full.into_iter().filter(|(_, v)| v.len() > 1) {
                v.sort();
                groups.push(DupGroup { size, paths: v });
            }
        }
    }
    groups.sort_by(|a, b| b.wasted().cmp(&a.wasted()));
    groups.truncate(1000);
    groups
}

// ------------------------------------------------------------------- clean-up

#[derive(Clone, Debug)]
pub enum CleanAction {
    /// Delete everything inside these folders (the folders themselves stay).
    DeleteContents(Vec<PathBuf>),
    /// A system command that needs root (run through pkexec unless root).
    Root(Vec<&'static str>),
    /// A command the person may run as themselves.
    User(Vec<&'static str>),
    /// Windows: empty the Recycle Bin.
    EmptyRecycleBin,
}

fn dev_caches(h: &Path) -> Option<CleanItem> {
    let mut dirs: Vec<PathBuf> = [".npm/_cacache", ".cargo/registry/cache", ".gradle/caches", ".local/share/pnpm/store", ".bun/install/cache"]
        .iter()
        .map(|d| h.join(d))
        .collect();
    if let Some(local) = dirs::data_local_dir() {
        if cfg!(windows) {
            dirs.push(local.join("npm-cache").join("_cacache"));
            dirs.push(local.join("pip").join("cache"));
        }
    }
    dirs.retain(|p| p.is_dir());
    if dirs.is_empty() {
        return None;
    }
    let detail = dirs.iter().map(|p| p.strip_prefix(h).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| p.display().to_string())).collect::<Vec<_>>().join(", ");
    Some(CleanItem { key: "cl_dev", detail, measure: dirs.clone(), size: None, action: CleanAction::DeleteContents(dirs), selected: false })
}

#[cfg(windows)]
pub fn cleanup_items() -> Vec<CleanItem> {
    let h = home();
    let mut v = Vec::new();
    let temp = std::env::temp_dir();
    if temp.is_dir() {
        v.push(CleanItem { key: "cl_temp", detail: temp.display().to_string(), measure: vec![temp.clone()], size: None, action: CleanAction::DeleteContents(vec![temp]), selected: false });
    }
    v.push(CleanItem { key: "cl_trash", detail: "Recycle Bin".into(), measure: Vec::new(), size: None, action: CleanAction::EmptyRecycleBin, selected: true });
    v.extend(dev_caches(&h));
    v
}

#[cfg(target_os = "macos")]
pub fn cleanup_items() -> Vec<CleanItem> {
    let h = home();
    let mut v = Vec::new();
    let caches = h.join("Library/Caches");
    if caches.is_dir() {
        v.push(CleanItem { key: "cl_cache", detail: "~/Library/Caches".into(), measure: vec![caches.clone()], size: None, action: CleanAction::DeleteContents(vec![caches]), selected: false });
    }
    let trash = h.join(".Trash");
    if trash.is_dir() {
        v.push(CleanItem { key: "cl_trash", detail: "~/.Trash".into(), measure: vec![trash.clone()], size: None, action: CleanAction::DeleteContents(vec![trash]), selected: true });
    }
    v.extend(dev_caches(&h));
    if which("brew") {
        v.push(CleanItem { key: "cl_pkg", detail: "brew cleanup".into(), measure: Vec::new(), size: None, action: CleanAction::User(vec!["brew", "cleanup"]), selected: false });
    }
    v
}

#[derive(Clone, Debug)]
pub struct CleanItem {
    pub key: &'static str,
    pub detail: String,
    pub measure: Vec<PathBuf>,
    pub size: Option<u64>,
    pub action: CleanAction,
    pub selected: bool,
}

/// Disk use of a folder (allocated blocks), not following links or mounts.
pub fn du(p: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(p) else { return 0 };
    let first = facts(&m);
    let dev = first.dev;
    let mut total = first.alloc;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            let f = facts(&m);
            if f.dev != dev || m.file_type().is_symlink() {
                continue;
            }
            total += f.alloc;
            if m.is_dir() {
                stack.push(e.path());
            }
        }
    }
    total
}

#[cfg(target_os = "linux")]
pub fn cleanup_items() -> Vec<CleanItem> {
    let h = home();
    let mut v = Vec::new();
    let cache = h.join(".cache");
    if cache.is_dir() {
        v.push(CleanItem { key: "cl_cache", detail: "~/.cache".into(), measure: vec![cache.clone()], size: None, action: CleanAction::DeleteContents(vec![cache]), selected: false });
    }
    let trash = data_home().join("Trash");
    if trash.is_dir() {
        let dirs = vec![trash.join("files"), trash.join("info")];
        v.push(CleanItem { key: "cl_trash", detail: "~/.local/share/Trash".into(), measure: vec![trash.join("files")], size: None, action: CleanAction::DeleteContents(dirs), selected: true });
    }
    v.extend(dev_caches(&h));
    if Path::new("/var/log/journal").is_dir() && which("journalctl") {
        v.push(CleanItem {
            key: "cl_journal",
            detail: "journalctl --vacuum-time=2weeks".into(),
            measure: vec!["/var/log/journal".into()],
            size: None,
            action: CleanAction::Root(vec!["journalctl", "--vacuum-time=2weeks"]),
            selected: false,
        });
    }
    let pkg: Option<(&str, Vec<&'static str>)> = if Path::new("/var/cache/dnf").is_dir() && which("dnf") {
        Some(("/var/cache/dnf", vec!["dnf", "clean", "packages"]))
    } else if Path::new("/var/cache/apt/archives").is_dir() && which("apt-get") {
        Some(("/var/cache/apt/archives", vec!["apt-get", "clean"]))
    } else if Path::new("/var/cache/pacman/pkg").is_dir() && which("pacman") {
        Some(("/var/cache/pacman/pkg", vec!["pacman", "-Sc", "--noconfirm"]))
    } else if Path::new("/var/cache/zypp").is_dir() && which("zypper") {
        Some(("/var/cache/zypp", vec!["zypper", "clean", "--all"]))
    } else if which("rpm-ostree") {
        Some(("/var/cache/rpm-ostree", vec!["rpm-ostree", "cleanup", "-m"]))
    } else {
        None
    };
    if let Some((dir, cmd)) = pkg {
        v.push(CleanItem { key: "cl_pkg", detail: cmd.join(" "), measure: vec![dir.into()], size: None, action: CleanAction::Root(cmd), selected: false });
    }
    if which("flatpak") {
        v.push(CleanItem {
            key: "cl_flatpak",
            detail: "flatpak uninstall --unused".into(),
            measure: Vec::new(),
            size: None,
            action: CleanAction::User(vec!["flatpak", "uninstall", "--unused", "-y", "--noninteractive"]),
            selected: false,
        });
    }
    v
}

#[cfg(windows)]
fn empty_recycle_bin() -> bool {
    match trash::os_limited::list() {
        Ok(items) => items.is_empty() || trash::os_limited::purge_all(items).is_ok(),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
fn empty_recycle_bin() -> bool {
    false
}

fn delete_contents(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    let mut ok = true;
    for e in rd.flatten() {
        let p = e.path();
        let r = match e.file_type() {
            Ok(t) if t.is_dir() => fs::remove_dir_all(&p),
            _ => fs::remove_file(&p),
        };
        ok &= r.is_ok();
    }
    ok
}

/// Run the selected clean-up items; returns (bytes freed by measured items, failures).
pub fn run_cleanup(items: &[CleanItem]) -> (u64, Vec<&'static str>) {
    let mut freed = 0;
    let mut failed = Vec::new();
    let is_root = crate::fsinfo::is_root_user();
    let mut root_cmds: Vec<(usize, Vec<&'static str>)> = Vec::new();
    for (i, it) in items.iter().enumerate().filter(|(_, it)| it.selected) {
        let before: u64 = it.measure.iter().map(|p| du(p)).sum();
        let ok = match &it.action {
            CleanAction::DeleteContents(dirs) => dirs.iter().map(|d| delete_contents(d)).fold(true, |a, b| a & b),
            CleanAction::User(cmd) => std::process::Command::new(cmd[0]).args(&cmd[1..]).status().map(|s| s.success()).unwrap_or(false),
            CleanAction::EmptyRecycleBin => empty_recycle_bin(),
            CleanAction::Root(cmd) => {
                if is_root {
                    std::process::Command::new(cmd[0]).args(&cmd[1..]).status().map(|s| s.success()).unwrap_or(false)
                } else {
                    root_cmds.push((i, cmd.clone()));
                    continue;
                }
            }
        };
        let after: u64 = it.measure.iter().map(|p| du(p)).sum();
        freed += before.saturating_sub(after);
        if !ok {
            failed.push(it.key);
        }
    }
    if !root_cmds.is_empty() {
        // one password prompt for every root item: the commands are fixed strings
        let script = root_cmds.iter().map(|(_, c)| c.join(" ")).collect::<Vec<_>>().join("; ");
        let before: u64 = root_cmds.iter().flat_map(|(i, _)| items[*i].measure.iter()).map(|p| du(p)).sum();
        let ok = std::process::Command::new("pkexec").args(["sh", "-c", &script]).status().map(|s| s.success()).unwrap_or(false);
        let after: u64 = root_cmds.iter().flat_map(|(i, _)| items[*i].measure.iter()).map(|p| du(p)).sum();
        freed += before.saturating_sub(after);
        if !ok {
            failed.extend(root_cmds.iter().map(|(i, _)| items[*i].key));
        }
    }
    (freed, failed)
}

// ------------------------------------------------------------------- snapshots

fn snapshot_dir() -> PathBuf {
    data_home().join("arbolview").join("snapshots")
}

pub fn root_key(root: &Path) -> String {
    let mut h = std::hash::DefaultHasher::new();
    h.write(root.to_string_lossy().as_bytes());
    format!("{:016x}", h.finish())
}

const KEEP_SNAPSHOTS: usize = 12;

/// Save every folder's size and file count; keeps the newest 12 per folder.
pub fn save_snapshot(root: &Path, n: &Node, ts: i64) -> Option<PathBuf> {
    fn walk(n: &Node, rel: &str, out: &mut String) {
        out.push_str(&format!("{}\t{}\t{}\n", n.size, n.files, rel));
        for c in &n.children {
            if c.is_dir {
                let r = if rel.is_empty() { c.name.to_string() } else { format!("{}/{}", rel, c.name) };
                walk(c, &r, out);
            }
        }
    }
    let dir = snapshot_dir();
    fs::create_dir_all(&dir).ok()?;
    let mut s = format!("#arbolview-snapshot\t1\t{}\t{}\n", ts, root.display());
    walk(n, "", &mut s);
    let file = dir.join(format!("{}-{}.tsv", root_key(root), ts));
    fs::write(&file, s).ok()?;
    let list = list_snapshots(root);
    for (_, old) in list.iter().skip(KEEP_SNAPSHOTS) {
        let _ = fs::remove_file(old);
    }
    Some(file)
}

/// (time, file) of the saved scans of `root`, newest first.
pub fn list_snapshots(root: &Path) -> Vec<(i64, PathBuf)> {
    let prefix = format!("{}-", root_key(root));
    let mut v: Vec<(i64, PathBuf)> = fs::read_dir(snapshot_dir())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let ts = name.strip_prefix(&prefix)?.strip_suffix(".tsv")?.parse().ok()?;
                    Some((ts, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

pub fn load_snapshot(file: &Path) -> HashMap<String, u64> {
    let mut m = HashMap::new();
    let Ok(text) = fs::read_to_string(file) else { return m };
    for line in text.lines().skip(1) {
        let mut it = line.splitn(3, '\t');
        if let (Some(s), Some(_), Some(rel)) = (it.next(), it.next(), it.next()) {
            if let Ok(s) = s.parse() {
                m.insert(rel.to_string(), s);
            }
        }
    }
    m
}

#[derive(Clone, Debug)]
pub struct ChangeRow {
    pub rel: String,
    pub depth: usize,
    pub old: Option<u64>,
    pub new: Option<u64>,
}

impl ChangeRow {
    pub fn delta(&self) -> i128 {
        self.new.unwrap_or(0) as i128 - self.old.unwrap_or(0) as i128
    }
}

/// Folders whose size changed since the snapshot, up to `max_depth` levels.
pub fn changes(old: &HashMap<String, u64>, root: &Node, max_depth: usize) -> Vec<ChangeRow> {
    fn walk(n: &Node, rel: &str, depth: usize, max: usize, old: &HashMap<String, u64>, seen: &mut std::collections::HashSet<String>, out: &mut Vec<ChangeRow>) {
        seen.insert(rel.to_string());
        let before = old.get(rel).copied();
        if depth > 0 && before != Some(n.size) {
            out.push(ChangeRow { rel: rel.to_string(), depth, old: before, new: Some(n.size) });
        }
        if depth >= max {
            return;
        }
        for c in &n.children {
            if c.is_dir {
                let r = if rel.is_empty() { c.name.to_string() } else { format!("{}/{}", rel, c.name) };
                walk(c, &r, depth + 1, max, old, seen, out);
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    walk(root, "", 0, max_depth, old, &mut seen, &mut out);
    for (rel, s) in old {
        let depth = if rel.is_empty() { 0 } else { rel.matches('/').count() + 1 };
        if depth > 0 && depth <= max_depth && !seen.contains(rel) {
            // gone, unless an ancestor is gone too (then the ancestor is the row)
            let parent_gone = rel.rsplit_once('/').map(|(p, _)| !seen.contains(p)).unwrap_or(false);
            if !parent_gone {
                out.push(ChangeRow { rel: rel.clone(), depth, old: Some(*s), new: None });
            }
        }
    }
    out.sort_by(|a, b| b.delta().unsigned_abs().cmp(&a.delta().unsigned_abs()));
    out.truncate(3000);
    out
}

// ------------------------------------------------------------------- filters

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Video,
    Image,
    Audio,
    Document,
    Archive,
    DiskImage,
}

impl Kind {
    pub const ALL: [Kind; 6] = [Kind::Video, Kind::Image, Kind::Audio, Kind::Document, Kind::Archive, Kind::DiskImage];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Video => "t_video",
            Kind::Image => "t_image",
            Kind::Audio => "t_audio",
            Kind::Document => "t_doc",
            Kind::Archive => "t_archive",
            Kind::DiskImage => "t_diskimg",
        }
    }

    fn exts(self) -> &'static [&'static str] {
        match self {
            Kind::Video => &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "3gp", "vob"],
            Kind::Image => &["jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "svg", "cr2", "nef", "arw", "dng", "psd", "xcf", "avif", "kra"],
            Kind::Audio => &["mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "aac", "wma", "aiff", "ape", "mka"],
            Kind::Document => &["pdf", "doc", "docx", "odt", "ods", "odp", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "md", "epub", "csv"],
            Kind::Archive => &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "lz4", "deb", "rpm", "jar", "apk"],
            Kind::DiskImage => &["iso", "img", "qcow2", "vdi", "vmdk", "vhd", "vhdx", "appimage", "raw"],
        }
    }

    pub fn matches(self, name: &str) -> bool {
        match name.rfind('.') {
            Some(i) if i + 1 < name.len() => {
                let e = name[i + 1..].to_lowercase();
                self.exts().contains(&e.as_str())
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Filters {
    pub min_size: u64,
    pub older_years: u32,
    pub kind: Option<Kind>,
}

impl Filters {
    pub fn active(&self) -> bool {
        self.min_size > 0 || self.older_years > 0 || self.kind.is_some()
    }

    /// Whether a file passes (folders never do when a filter is on).
    pub fn pass(&self, n: &Node, now: i64) -> bool {
        if n.is_dir {
            return !self.active();
        }
        if n.size < self.min_size {
            return false;
        }
        if self.older_years > 0 && now - n.mtime < self.older_years as i64 * 365 * 86400 {
            return false;
        }
        if let Some(k) = self.kind {
            if !k.matches(&n.name) {
                return false;
            }
        }
        true
    }

    pub fn stamp(&self) -> u64 {
        self.min_size ^ ((self.older_years as u64) << 56) ^ (self.kind.map(|k| k as u64 + 1).unwrap_or(0) << 60)
    }
}

pub type Shared<T> = Arc<std::sync::Mutex<T>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{scan_blocking, Progress, ScanOptions};

    // tests that set XDG_DATA_HOME must not run at the same time
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn sandbox(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("arbolview-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    // the freedesktop trash is Linux's; Windows and macOS use the system's own
    #[cfg(target_os = "linux")]
    #[test]
    fn trash_and_restore() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = sandbox("trash");
        std::env::set_var("XDG_DATA_HOME", d.join("data"));
        let f = d.join("work").join("a.txt");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, b"hola").unwrap();
        let rec = trash(&f).unwrap();
        assert!(!f.exists());
        assert!(rec.trashed.exists());
        let info = fs::read_to_string(&rec.info).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        restore(&rec).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"hola");
        assert!(!rec.info.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn duplicates_found() {
        let d = sandbox("dups");
        let big = vec![7u8; 200_000];
        fs::write(d.join("a.bin"), &big).unwrap();
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("sub/b.bin"), &big).unwrap();
        let mut other = big.clone();
        other[150_000] = 1; // same size and same start, different content
        fs::write(d.join("c.bin"), &other).unwrap();
        // a hard link is the same file, not a duplicate (only Unix tells them apart here)
        #[cfg(unix)]
        fs::hard_link(d.join("a.bin"), d.join("a-link.bin")).unwrap();
        let mut opts = ScanOptions::default();
        opts.hardlinks_once = false;
        let n = scan_blocking(&d, opts, &Progress::default()).unwrap();
        let mut files = Vec::new();
        collect_files(&n, &d, 1, &mut files);
        let g = find_duplicates(files, &DupProgress::default());
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(g[0].paths.len(), 2); // a (or its hard link) and sub/b; c differs
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn snapshot_changes() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = sandbox("snap");
        std::env::set_var("XDG_DATA_HOME", d.join("data"));
        let root = d.join("root");
        fs::create_dir_all(root.join("grows")).unwrap();
        fs::create_dir_all(root.join("goes")).unwrap();
        fs::write(root.join("grows/x"), vec![0u8; 1000]).unwrap();
        fs::write(root.join("goes/y"), vec![0u8; 500]).unwrap();
        let n1 = scan_blocking(&root, ScanOptions::default(), &Progress::default()).unwrap();
        save_snapshot(&root, &n1, 100).unwrap();
        fs::write(root.join("grows/z"), vec![0u8; 3000]).unwrap();
        fs::remove_dir_all(root.join("goes")).unwrap();
        fs::create_dir_all(root.join("new")).unwrap();
        let n2 = scan_blocking(&root, ScanOptions::default(), &Progress::default()).unwrap();
        let list = list_snapshots(&root);
        assert_eq!(list.len(), 1);
        let old = load_snapshot(&list[0].1);
        let rows = changes(&old, &n2, 2);
        let find = |r: &str| rows.iter().find(|c| c.rel == r).cloned();
        assert_eq!(find("grows").unwrap().delta(), 3000);
        assert!(find("goes").unwrap().new.is_none());
        assert!(find("new").unwrap().old.is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn filter_kinds() {
        assert!(Kind::Video.matches("peli.MKV"));
        assert!(!Kind::Video.matches("notas.txt"));
        assert!(Kind::DiskImage.matches("fedora.iso"));
    }
}
