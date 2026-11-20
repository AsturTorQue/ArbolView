//! Disk use over time: a history log with one line per finished scan, and
//! scheduled background scans through systemd user timers.

use crate::scan::Node;
use crate::tools::data_home;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct HistPoint {
    pub ts: i64,
    pub size: u64,
    pub alloc: u64,
    pub files: u64,
    pub folders: u64,
    pub fs_total: u64,
    pub fs_free: u64,
}

fn history_file() -> PathBuf {
    data_home().join("arbolview").join("history.tsv")
}

/// Append one line: time, folder totals and the free space of its disk.
pub fn append_history(root: &Path, n: &Node, ts: i64) {
    let (fs_total, fs_free) = crate::fsinfo::statvfs(&root.to_string_lossy()).map(|s| (s.0, s.2)).unwrap_or((0, 0));
    let f = history_file();
    if let Some(d) = f.parent() {
        let _ = fs::create_dir_all(d);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = writeln!(file, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", ts, n.size, n.alloc, n.files, n.folders, fs_total, fs_free, root.display());
    }
}

/// Every recorded point for `root`, oldest first.
pub fn read_history(root: &Path) -> Vec<HistPoint> {
    let want = root.to_string_lossy();
    let mut v: Vec<HistPoint> = fs::read_to_string(history_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(8, '\t').collect();
            if f.len() < 8 || f[7] != want {
                return None;
            }
            Some(HistPoint {
                ts: f[0].parse().ok()?,
                size: f[1].parse().ok()?,
                alloc: f[2].parse().ok()?,
                files: f[3].parse().ok()?,
                folders: f[4].parse().ok()?,
                fs_total: f[5].parse().ok()?,
                fs_free: f[6].parse().ok()?,
            })
        })
        .collect();
    v.sort_by_key(|p| p.ts);
    v
}

// ------------------------------------------------------------------- schedules

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Every {
    Hourly,
    Daily,
    Weekly,
}

impl Every {
    pub fn calendar(self) -> &'static str {
        match self {
            Every::Hourly => "hourly",
            Every::Daily => "daily",
            Every::Weekly => "weekly",
        }
    }

    pub fn parse(s: &str) -> Option<Every> {
        match s.to_lowercase().as_str() {
            "hourly" | "hora" | "cada-hora" => Some(Every::Hourly),
            "daily" | "diario" | "dia" | "día" => Some(Every::Daily),
            "weekly" | "semanal" | "semana" => Some(Every::Weekly),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Every::Hourly => "mon_hourly",
            Every::Daily => "mon_daily",
            Every::Weekly => "mon_weekly",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Schedule {
    pub path: PathBuf,
    pub every: Every,
    pub unit: String,
}

fn unit_name(path: &Path) -> String {
    format!("arbolview-{}", crate::tools::root_key(path))
}

pub fn schedule_for(path: &Path) -> Option<Every> {
    list_schedules().into_iter().find(|s| s.path == path).map(|s| s.every)
}

// ---------------------------------------------------- macOS and Windows: a small registry
// of what was scheduled (launchd and the Task Scheduler hold the jobs themselves)

#[cfg(not(target_os = "linux"))]
fn registry_file() -> PathBuf {
    data_home().join("arbolview").join("schedules.tsv")
}

#[cfg(not(target_os = "linux"))]
fn registry_write(path: &Path, every: Option<Every>) {
    let f = registry_file();
    let mut lines: Vec<String> = fs::read_to_string(&f).unwrap_or_default().lines().filter(|l| l.split('\t').nth(1) != Some(&*path.to_string_lossy())).map(|s| s.to_string()).collect();
    if let Some(e) = every {
        lines.push(format!("{}\t{}", e.calendar(), path.display()));
    }
    if let Some(d) = f.parent() {
        let _ = fs::create_dir_all(d);
    }
    let _ = fs::write(&f, lines.join("\n") + "\n");
}

#[cfg(not(target_os = "linux"))]
pub fn list_schedules() -> Vec<Schedule> {
    let mut v: Vec<Schedule> = fs::read_to_string(registry_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (e, p) = l.split_once('\t')?;
            let path = PathBuf::from(p);
            Some(Schedule { unit: unit_name(&path), path, every: Every::parse(e)? })
        })
        .collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

#[cfg(target_os = "macos")]
fn agent_file(path: &Path) -> PathBuf {
    crate::platform::home().join("Library/LaunchAgents").join(format!("com.{}.plist", unit_name(path)))
}

#[cfg(target_os = "macos")]
pub fn set_schedule(path: &Path, every: Every) -> Result<(), String> {
    let exe = crate::fsinfo::self_exe().ok_or("cannot find the program")?;
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let secs = match every {
        Every::Hourly => 3600,
        Every::Daily => 86400,
        Every::Weekly => 604800,
    };
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>com.{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>scan</string><string>{}</string><string>--quiet</string></array>\n<key>StartInterval</key><integer>{secs}</integer>\n<key>LowPriorityIO</key><true/>\n<key>Nice</key><integer>19</integer>\n</dict></plist>\n",
        unit_name(path),
        esc(&exe.to_string_lossy()),
        esc(&path.to_string_lossy())
    );
    let f = agent_file(path);
    if let Some(d) = f.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let _ = std::process::Command::new("launchctl").arg("unload").arg(&f).output();
    fs::write(&f, plist).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("launchctl").args(["load", "-w"]).arg(&f).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    registry_write(path, Some(every));
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn remove_schedule(path: &Path) -> Result<(), String> {
    let f = agent_file(path);
    let _ = std::process::Command::new("launchctl").arg("unload").arg(&f).output();
    let _ = fs::remove_file(&f);
    registry_write(path, None);
    Ok(())
}

#[cfg(windows)]
fn schtasks(args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("schtasks").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(windows)]
pub fn set_schedule(path: &Path, every: Every) -> Result<(), String> {
    let exe = crate::fsinfo::self_exe().ok_or("cannot find the program")?;
    let sc = match every {
        Every::Hourly => "HOURLY",
        Every::Daily => "DAILY",
        Every::Weekly => "WEEKLY",
    };
    let tr = format!("\"{}\" scan \"{}\" --quiet", exe.display(), path.display());
    let name = format!("ArbolView\\{}", unit_name(path));
    schtasks(&["/Create", "/F", "/SC", sc, "/TN", &name, "/TR", &tr])?;
    registry_write(path, Some(every));
    Ok(())
}

#[cfg(windows)]
pub fn remove_schedule(path: &Path) -> Result<(), String> {
    let name = format!("ArbolView\\{}", unit_name(path));
    let _ = schtasks(&["/Delete", "/F", "/TN", &name]);
    registry_write(path, None);
    Ok(())
}

// ---------------------------------------------------- Linux: systemd user timers

#[cfg(target_os = "linux")]
fn units_dir() -> PathBuf {
    crate::platform::config_dir().join("systemd").join("user")
}

#[cfg(target_os = "linux")]
fn systemctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("systemctl").arg("--user").args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "linux")]
fn quote(p: &str) -> String {
    format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Install (or replace) a user timer that runs `arbolview scan <path> --quiet`.
#[cfg(target_os = "linux")]
pub fn set_schedule(path: &Path, every: Every) -> Result<(), String> {
    let exe = crate::fsinfo::self_exe().ok_or("cannot find the program")?;
    let dir = units_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = unit_name(path);
    let p = path.to_string_lossy();
    let service = format!(
        "# arbolview-path={p}\n[Unit]\nDescription=ÁrbolView disk usage snapshot of {p}\n\n[Service]\nType=oneshot\nExecStart={} scan {} --quiet\nNice=19\nIOSchedulingClass=idle\n",
        quote(&exe.to_string_lossy()),
        quote(&p)
    );
    let timer = format!(
        "# arbolview-path={p}\n# arbolview-every={}\n[Unit]\nDescription=ÁrbolView scheduled scan of {p}\n\n[Timer]\nOnCalendar={}\nPersistent=true\nRandomizedDelaySec=300\n\n[Install]\nWantedBy=timers.target\n",
        every.calendar(),
        every.calendar()
    );
    fs::write(dir.join(format!("{name}.service")), service).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{name}.timer")), timer).map_err(|e| e.to_string())?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", &format!("{name}.timer")])
}

#[cfg(target_os = "linux")]
pub fn remove_schedule(path: &Path) -> Result<(), String> {
    let name = unit_name(path);
    let _ = systemctl(&["disable", "--now", &format!("{name}.timer")]);
    let dir = units_dir();
    let _ = fs::remove_file(dir.join(format!("{name}.timer")));
    let _ = fs::remove_file(dir.join(format!("{name}.service")));
    systemctl(&["daemon-reload"])
}

#[cfg(target_os = "linux")]
pub fn list_schedules() -> Vec<Schedule> {
    let Ok(rd) = fs::read_dir(units_dir()) else { return Vec::new() };
    let mut v = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("arbolview-") && name.ends_with(".timer")) {
            continue;
        }
        let text = fs::read_to_string(e.path()).unwrap_or_default();
        let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("# arbolview-{k}=")).map(|s| s.to_string()));
        if let (Some(p), Some(ev)) = (get("path"), get("every").and_then(|s| Every::parse(&s))) {
            v.push(Schedule { path: PathBuf::from(p), every: ev, unit: name.trim_end_matches(".timer").to_string() });
        }
    }
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}
