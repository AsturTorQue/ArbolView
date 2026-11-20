//! Everything that differs between Linux, macOS and Windows: file facts,
//! user folders, opening things, administrator rights and disks.

use std::fs::Metadata;
use std::path::{Path, PathBuf};

/// What the scanner needs from a file's metadata, on every system.
#[derive(Clone, Copy, Debug)]
pub struct Facts {
    /// Device the entry lives on; `None` where the system doesn't say (Windows).
    pub dev: Option<u64>,
    /// (device, inode) to recognise hard links; `None` where unavailable.
    pub id: Option<(u64, u64)>,
    pub alloc: u64,
    pub uid: u32,
    pub gid: u32,
    /// Unix-style mode: type and permission bits (synthesised on Windows).
    pub mode: u32,
    pub nlink: u64,
    pub mtime: i64,
}

#[cfg(unix)]
pub fn facts(m: &Metadata) -> Facts {
    use std::os::unix::fs::MetadataExt;
    Facts {
        dev: Some(m.dev()),
        id: Some((m.dev(), m.ino())),
        alloc: m.blocks() * 512,
        uid: m.uid(),
        gid: m.gid(),
        mode: m.mode(),
        nlink: m.nlink(),
        mtime: m.mtime(),
    }
}

#[cfg(windows)]
pub fn facts(m: &Metadata) -> Facts {
    use std::os::windows::fs::MetadataExt;
    const READONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    let attrs = m.file_attributes();
    let ft = m.file_type();
    let kind = if ft.is_symlink() { 0o120000 } else if ft.is_dir() { 0o040000 } else { 0o100000 };
    let mut perm = if attrs & READONLY != 0 { 0o555 } else { 0o755 };
    if !ft.is_dir() {
        perm &= !0o111;
    }
    // hidden and system files lose "others" read, so they stand out in the column
    if attrs & (HIDDEN | SYSTEM) != 0 {
        perm &= !0o007;
    }
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // NTFS allocates in 4 KiB clusters by default
    let alloc = if ft.is_dir() { 0 } else { m.len().div_ceil(4096) * 4096 };
    Facts { dev: None, id: None, alloc, uid: 0, gid: 0, mode: kind | perm, nlink: 1, mtime }
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }))
}

/// Per-user data folder: ~/.local/share, ~/Library/Application Support or %APPDATA%.
pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(x);
        }
    }
    dirs::data_dir().unwrap_or_else(|| home().join(".local/share"))
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home().join(".config"))
}

/// The top of the whole system: `/`, or the Windows system drive.
pub fn system_root() -> PathBuf {
    if cfg!(windows) {
        let d = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        PathBuf::from(format!("{d}\\"))
    } else {
        PathBuf::from("/")
    }
}

pub fn documents() -> Option<PathBuf> {
    dirs::document_dir().filter(|p| p.is_dir())
}

pub fn downloads() -> Option<PathBuf> {
    dirs::download_dir().filter(|p| p.is_dir())
}

/// Open a folder or file with the system's default program.
pub fn open_path(p: &Path) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

pub fn open_terminal(dir: &Path) {
    #[cfg(target_os = "linux")]
    for term in ["x-terminal-emulator", "gnome-terminal", "konsole", "xfce4-terminal", "xterm"] {
        if std::process::Command::new(term).current_dir(dir).spawn().is_ok() {
            return;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").args(["-a", "Terminal"]).arg(dir).spawn();
    }
    #[cfg(windows)]
    {
        if std::process::Command::new("wt").arg("-d").arg(dir).spawn().is_err() {
            let _ = std::process::Command::new("cmd").args(["/c", "start", "cmd", "/k"]).current_dir(dir).spawn();
        }
    }
}

/// Running with administrator rights (root on Linux and macOS).
pub fn is_admin() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(windows)]
    {
        unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
    }
}

/// Whether this system can open a second window with administrator rights.
pub fn can_elevate() -> bool {
    if cfg!(target_os = "linux") {
        crate::tools::which("pkexec")
    } else {
        cfg!(windows)
    }
}

/// Start `exe args…` with administrator rights; the system asks for the password.
pub fn elevate(exe: &Path, args: &[String], extra_env: &[(String, String)]) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("pkexec");
        cmd.arg("env");
        for v in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XAUTHORITY", "XDG_SESSION_TYPE", "LANG", "LANGUAGE"] {
            if let Ok(val) = std::env::var(v) {
                cmd.arg(format!("{v}={val}"));
            }
        }
        for (k, v) in extra_env {
            cmd.arg(format!("{k}={v}"));
        }
        cmd.arg(exe).args(args);
        cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        // PowerShell's RunAs shows the UAC prompt; settings travel as arguments
        let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
        let mut all: Vec<String> = args.to_vec();
        for (k, v) in extra_env {
            all.push(format!("--{}={}", k.to_lowercase().replace('_', "-"), v));
        }
        let list = all.iter().map(|a| q(a)).collect::<Vec<_>>().join(",");
        let ps = if list.is_empty() {
            format!("Start-Process -Verb RunAs -FilePath {}", q(&exe.to_string_lossy()))
        } else {
            format!("Start-Process -Verb RunAs -FilePath {} -ArgumentList {}", q(&exe.to_string_lossy()), list)
        };
        std::process::Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps]).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (exe, args, extra_env);
        Err("not available on macOS".into())
    }
}

/// Tell the person something went wrong when there's no console to read it:
/// a message box on Windows, standard error elsewhere.
pub fn error_dialog(title: &str, text: &str) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let t: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        let m: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONERROR);
    }
    #[cfg(not(windows))]
    eprintln!("{title}: {text}");
}

/// Where a crash report is written: <data folder>/arbolview/crash.log.
pub fn crash_log() -> PathBuf {
    data_dir().join("arbolview").join("crash.log")
}

/// Windows GUI programs have no console; a command typed in one should still print there.
pub fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

/// Free space of the disk holding `path`: (total, free, available, inodes, free inodes, block size).
#[cfg(unix)]
pub fn disk_space(path: &str) -> Option<(u64, u64, u64, u64, u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let f = st.f_frsize as u64;
    Some((st.f_blocks as u64 * f, st.f_bfree as u64 * f, st.f_bavail as u64 * f, st.f_files as u64, st.f_ffree as u64, f))
}

#[cfg(windows)]
pub fn disk_space(path: &str) -> Option<(u64, u64, u64, u64, u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().chain(Some(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
    if ok == 0 {
        return None;
    }
    Some((total, free, avail, 0, 0, 4096))
}

/// Mounted disks as (device, mount point, type, options) for the non-Linux systems.
#[cfg(target_os = "macos")]
pub fn raw_mounts() -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    unsafe {
        let n = libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT);
        if n <= 0 {
            return out;
        }
        let mut buf: Vec<libc::statfs> = vec![std::mem::zeroed(); n as usize];
        let got = libc::getfsstat(buf.as_mut_ptr(), (buf.len() * std::mem::size_of::<libc::statfs>()) as libc::c_int, libc::MNT_NOWAIT);
        for s in buf.iter().take(got.max(0) as usize) {
            let cs = |a: &[libc::c_char]| std::ffi::CStr::from_ptr(a.as_ptr()).to_string_lossy().into_owned();
            let ro = if s.f_flags & libc::MNT_RDONLY as u32 != 0 { "ro" } else { "rw" };
            out.push((cs(&s.f_mntfromname), cs(&s.f_mntonname), cs(&s.f_fstypename), ro.to_string()));
        }
    }
    out
}

#[cfg(windows)]
pub fn raw_mounts() -> Vec<(String, String, String, String)> {
    use windows_sys::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeInformationW};
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        let mut name = [0u16; 261];
        let mut fsname = [0u16; 261];
        let (mut serial, mut maxlen, mut flags) = (0u32, 0u32, 0u32);
        let ok = unsafe {
            GetVolumeInformationW(wide.as_ptr(), name.as_mut_ptr(), name.len() as u32, &mut serial, &mut maxlen, &mut flags, fsname.as_mut_ptr(), fsname.len() as u32)
        };
        if ok == 0 {
            continue; // empty card reader, disconnected network drive...
        }
        let s = |a: &[u16]| String::from_utf16_lossy(&a[..a.iter().position(|c| *c == 0).unwrap_or(a.len())]);
        let label = s(&name);
        let ro = if flags & 0x0008_0000 != 0 { "ro" } else { "rw" }; // FILE_READ_ONLY_VOLUME
        out.push((if label.is_empty() { root.clone() } else { label }, root, s(&fsname), ro.to_string()));
    }
    out
}

/// Name of a user / group id (empty on Windows, where files show no owner here).
#[cfg(unix)]
pub fn current_user() -> Option<(u32, String)> {
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return None;
        }
        Some((uid, std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()))
    }
}

#[cfg(windows)]
pub fn current_user() -> Option<(u32, String)> {
    std::env::var("USERNAME").ok().map(|u| (0, u))
}
