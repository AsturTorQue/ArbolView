//! System information: mounted disks, user names, dates and number formatting.

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Mount {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    pub options: String,
    pub total: u64,
    pub free: u64,
    pub avail: u64,
    pub inodes: u64,
    pub inodes_free: u64,
    pub block_size: u64,
}

impl Mount {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { self.used() as f32 / self.total as f32 }
    }
    pub fn read_only(&self) -> bool {
        self.options.split(',').any(|o| o == "ro")
    }
}

/// File system types that never hold user data on a disk.
const PSEUDO: &[&str] = &[
    "proc", "sysfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "securityfs", "pstore", "bpf",
    "debugfs", "tracefs", "configfs", "fusectl", "mqueue", "hugetlbfs", "autofs", "binfmt_misc",
    "efivarfs", "rpc_pipefs", "nsfs", "selinuxfs", "ramfs", "fuse.portal", "fuse.gvfsd-fuse",
];

fn unescape(s: &str) -> String {
    // /proc/mounts escapes spaces and tabs as octal (\040, \011).
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() + 0 && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// (total, free, available, inodes, free inodes, block size) of the disk holding `path`.
pub fn statvfs(path: &str) -> Option<(u64, u64, u64, u64, u64, u64)> {
    crate::platform::disk_space(path)
}

/// Mounted disks on macOS and Windows.
#[cfg(not(target_os = "linux"))]
pub fn mounts(include_pseudo: bool) -> Vec<Mount> {
    const MAC_SKIP: &[&str] = &["devfs", "autofs", "nullfs"];
    let mut out: Vec<Mount> = Vec::new();
    for (dev, mp, ty, opts) in crate::platform::raw_mounts() {
        if !include_pseudo && MAC_SKIP.contains(&ty.as_str()) {
            continue;
        }
        // macOS: the sealed system volume and its helper volumes repeat the data disk
        if !include_pseudo && mp.starts_with("/System/Volumes/") && mp != "/System/Volumes/Data" {
            continue;
        }
        let Some((total, free, avail, inodes, inodes_free, bs)) = statvfs(&mp) else { continue };
        if total == 0 && !include_pseudo {
            continue;
        }
        out.push(Mount { device: dev, mount_point: mp, fs_type: ty, options: opts, total, free, avail, inodes, inodes_free, block_size: bs });
    }
    out
}

#[cfg(target_os = "linux")]
pub fn mounts(include_pseudo: bool) -> Vec<Mount> {
    let text = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut out: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(dev), Some(mp), Some(ty), Some(opts)) = (it.next(), it.next(), it.next(), it.next()) else {
            continue;
        };
        let pseudo = PSEUDO.contains(&ty) || ty.starts_with("cgroup");
        if pseudo && !include_pseudo {
            continue;
        }
        let mp = unescape(mp);
        // AppImage runtimes mount themselves as read-only ".mount_*" folders
        if !include_pseudo && mp.rsplit('/').next().is_some_and(|l| l.starts_with(".mount_")) {
            continue;
        }
        if !include_pseudo && (ty == "tmpfs" || ty == "overlay" || ty == "squashfs") && mp != "/tmp" {
            continue;
        }
        let Some((total, free, avail, inodes, inodes_free, bs)) = statvfs(&mp) else { continue };
        if total == 0 && !include_pseudo {
            continue;
        }
        // Bind mounts of the same device show once (first mount point wins).
        if !include_pseudo && dev.starts_with('/') && out.iter().any(|m| m.device == dev && m.total == total) {
            continue;
        }
        out.push(Mount {
            device: unescape(dev),
            mount_point: mp,
            fs_type: ty.to_string(),
            options: opts.to_string(),
            total,
            free,
            avail,
            inodes,
            inodes_free,
            block_size: bs,
        });
    }
    out
}

/// User names by id. On Windows files show no owner here, so this stays empty.
pub fn users() -> HashMap<u32, String> {
    let mut m = HashMap::new();
    if cfg!(windows) {
        return m;
    }
    // macOS keeps most accounts outside /etc/passwd: add the current user directly
    if let Some((uid, name)) = crate::platform::current_user() {
        m.insert(uid, name);
    }
    if let Ok(t) = std::fs::read_to_string("/etc/passwd") {
        for l in t.lines() {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() > 2 {
                if let Ok(uid) = f[2].parse() {
                    m.entry(uid).or_insert_with(|| f[0].to_string());
                }
            }
        }
    }
    m
}

pub fn groups() -> HashMap<u32, String> {
    let mut m = HashMap::new();
    if let Ok(t) = std::fs::read_to_string("/etc/group") {
        for l in t.lines() {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() > 2 {
                if let Ok(gid) = f[2].parse() {
                    m.insert(gid, f[0].to_string());
                }
            }
        }
    }
    m
}

/// `ls -l` style permissions, e.g. `drwxr-xr-x`, with setuid/setgid/sticky.
pub fn perm_string(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o020000 => 'c',
        0o060000 => 'b',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    };
    let mut s = String::with_capacity(10);
    s.push(kind);
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    let exec = |x: u32, special: u32, on: char, off: char| match (mode & x != 0, mode & special != 0) {
        (true, true) => on,
        (false, true) => off,
        (true, false) => 'x',
        (false, false) => '-',
    };
    s.push(bit(0o400, 'r'));
    s.push(bit(0o200, 'w'));
    s.push(exec(0o100, 0o4000, 's', 'S'));
    s.push(bit(0o040, 'r'));
    s.push(bit(0o020, 'w'));
    s.push(exec(0o010, 0o2000, 's', 'S'));
    s.push(bit(0o004, 'r'));
    s.push(bit(0o002, 'w'));
    s.push(exec(0o001, 0o1000, 't', 'T'));
    s
}

/// The program to start again (desktop entry, timers, root window): the
/// AppImage file itself when running from one, since its inside is mounted
/// at a different temporary folder every run.
pub fn self_exe() -> Option<std::path::PathBuf> {
    if let Some(a) = std::env::var_os("APPIMAGE") {
        let p = std::path::PathBuf::from(a);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::current_exe().ok()
}

pub fn is_root_user() -> bool {
    crate::platform::is_admin()
}

/// Local date and time, dd/mm/yyyy hh:mm.
pub fn format_time(t: i64) -> String {
    use chrono::TimeZone;
    if t <= 0 {
        return String::new();
    }
    match chrono::Local.timestamp_opt(t, 0).single() {
        Some(d) => d.format("%d/%m/%Y %H:%M").to_string(),
        None => String::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Auto,
    Bytes,
    KB,
    MB,
    GB,
}

/// Group digits with dots and use a decimal comma (Spanish style).
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    out
}

fn dec(v: f64, places: usize) -> String {
    let s = format!("{:.*}", places, v);
    let (int, frac) = match s.split_once('.') {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (s.clone(), None),
    };
    let int = group(int.parse::<u64>().unwrap_or(0));
    match frac {
        Some(f) => format!("{},{}", int, f),
        None => int,
    }
}

pub fn format_size(b: u64, unit: Unit) -> String {
    const K: f64 = 1024.0;
    let v = b as f64;
    match unit {
        Unit::Bytes => format!("{} Bytes", group(b)),
        Unit::KB => format!("{} KB", dec(v / K, 1)),
        Unit::MB => format!("{} MB", dec(v / K / K, 1)),
        Unit::GB => format!("{} GB", dec(v / K / K / K, 2)),
        Unit::Auto => {
            if v < K {
                format!("{} Bytes", b)
            } else if v < K * K {
                format!("{} KB", dec(v / K, 1))
            } else if v < K * K * K {
                format!("{} MB", dec(v / K / K, 1))
            } else if v < K * K * K * K {
                format!("{} GB", dec(v / K / K / K, 1))
            } else {
                format!("{} TB", dec(v / K / K / K / K, 1))
            }
        }
    }
}

pub fn format_pct(p: f64) -> String {
    format!("{} %", dec(p, 1))
}
