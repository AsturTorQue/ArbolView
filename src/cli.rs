//! Command line: scans, reports, history and schedules without opening a window.

use crate::fsinfo::{format_size, format_time, group, Unit};
use crate::i18n::Lang;
use crate::monitor::{self, Every};
use crate::report::{self, Format};
use crate::scan::{self, Progress, ScanOptions};
use std::io::Write;
use std::path::PathBuf;

pub const COMMANDS: &[&str] = &["scan", "history", "schedule", "help", "--help", "-h", "--version", "-V"];

fn help(lang: Lang) -> String {
    let es = lang == Lang::Es;
    let h = if es {
        r#"ÁrbolView — uso de disco para Linux

USO
  arbolview                          abre la ventana
  arbolview <carpeta>                abre la ventana escaneando esa carpeta
  arbolview scan <carpeta> [opciones]
  arbolview history <carpeta> [--format text|csv|json]
  arbolview schedule add <carpeta> --every hourly|daily|weekly
  arbolview schedule remove <carpeta>
  arbolview schedule list

OPCIONES DE scan
  -f, --format FMT      text (por defecto), csv, json, html o pdf
  -o, --output ARCHIVO  dónde escribir ("-" = salida estándar). Sin -o, text/csv/json
                        van a la salida estándar y html/pdf a ~/arbolview-<carpeta>-<hora>.<ext>
  -d, --depth N         niveles de carpetas en el informe (2)
      --rows N          filas máximas de carpetas (200)
      --all-filesystems entrar también en otros discos montados dentro
      --skip-links      excluir los enlaces simbólicos
      --no-history      no guardar este escaneo en el historial
      --warn-free PCT   salir con código 3 si el disco tiene menos de PCT % libre
  -q, --quiet           no escribir nada (para tareas programadas)
      --lang es|en|pt|fr

CÓDIGOS DE SALIDA
  0 bien · 1 error · 2 uso incorrecto · 3 aviso de espacio (--warn-free)

EJEMPLOS
  arbolview scan ~ --format pdf -o ~/informe.pdf
  arbolview scan / --format json --depth 3 | jq .top_folders
  arbolview scan /srv --warn-free 10 --quiet || echo "¡Disco casi lleno!"
  arbolview schedule add ~ --every daily
"#
    } else {
        r#"ÁrbolView — disk usage for Linux

USAGE
  arbolview                          open the window
  arbolview <folder>                 open the window scanning that folder
  arbolview scan <folder> [options]
  arbolview history <folder> [--format text|csv|json]
  arbolview schedule add <folder> --every hourly|daily|weekly
  arbolview schedule remove <folder>
  arbolview schedule list

scan OPTIONS
  -f, --format FMT      text (default), csv, json, html or pdf
  -o, --output FILE     where to write ("-" = standard output). Without -o, text/csv/json
                        go to standard output and html/pdf to ~/arbolview-<folder>-<time>.<ext>
  -d, --depth N         folder levels in the report (2)
      --rows N          most folder rows (200)
      --all-filesystems also enter other disks mounted inside
      --skip-links      leave symbolic links out
      --no-history      don't save this scan in the history
      --warn-free PCT   exit with code 3 when the disk has less than PCT % free
  -q, --quiet           print nothing (for scheduled jobs)
      --lang es|en|pt|fr

EXIT CODES
  0 ok · 1 error · 2 bad usage · 3 space warning (--warn-free)

EXAMPLES
  arbolview scan ~ --format pdf -o ~/report.pdf
  arbolview scan / --format json --depth 3 | jq .top_folders
  arbolview scan /srv --warn-free 10 --quiet || echo "Disk almost full!"
  arbolview schedule add ~ --every daily
"#
    };
    h.to_string()
}

fn usage_error(msg: &str) -> i32 {
    eprintln!("arbolview: {msg}\n(arbolview --help)");
    2
}

fn expand(p: &str) -> PathBuf {
    let p = if let Some(rest) = p.strip_prefix("~") {
        format!("{}{}", crate::platform::home().display(), rest)
    } else {
        p.to_string()
    };
    let pb = PathBuf::from(p);
    crate::platform::canonical(&pb)
}

/// Run a command; returns the process exit code.
pub fn run(args: Vec<String>) -> i32 {
    let mut lang = Lang::from_env();
    if let Some(i) = args.iter().position(|a| a == "--lang") {
        if let Some(l) = args.get(i + 1).and_then(|v| Lang::from_code(v)) {
            lang = l;
        }
    }
    match args.first().map(|s| s.as_str()) {
        Some("--version") | Some("-V") => {
            println!("ÁrbolView {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{}", help(lang));
            0
        }
        Some("scan") => scan_cmd(&args[1..], lang),
        Some("history") => history_cmd(&args[1..], lang),
        Some("schedule") => schedule_cmd(&args[1..]),
        Some(other) => usage_error(&format!("unknown command {other}")),
    }
}

fn scan_cmd(args: &[String], lang: Lang) -> i32 {
    let mut path: Option<PathBuf> = None;
    let mut format = Format::Text;
    let mut output: Option<String> = None;
    let mut depth = 2usize;
    let mut rows = 200usize;
    let mut opts = ScanOptions::default();
    let mut history = true;
    let mut quiet = false;
    let mut warn_free: Option<f64> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-f" | "--format" => match it.next().and_then(|v| Format::parse(v)) {
                Some(f) => format = f,
                None => return usage_error("--format needs text, csv, json, html or pdf"),
            },
            "-o" | "--output" => output = it.next().cloned(),
            "-d" | "--depth" => match it.next().and_then(|v| v.parse().ok()) {
                Some(d) => depth = d,
                None => return usage_error("--depth needs a number"),
            },
            "--rows" => match it.next().and_then(|v| v.parse().ok()) {
                Some(r) => rows = r,
                None => return usage_error("--rows needs a number"),
            },
            "--all-filesystems" => opts.one_filesystem = false,
            "--skip-links" => opts.skip_symlinks = true,
            "--no-history" => history = false,
            "-q" | "--quiet" => quiet = true,
            "--warn-free" => match it.next().and_then(|v| v.trim_end_matches('%').parse().ok()) {
                Some(p) => warn_free = Some(p),
                None => return usage_error("--warn-free needs a percentage"),
            },
            "--lang" => {
                it.next();
            }
            s if s.starts_with('-') => return usage_error(&format!("unknown option {s}")),
            s => path = Some(expand(s)),
        }
    }
    let Some(path) = path else { return usage_error("scan needs a folder") };
    if !path.is_dir() {
        eprintln!("arbolview: {}: not a folder", path.display());
        return 1;
    }
    let prog = Progress::default();
    let Some((node, top)) = scan::scan_full(&path, opts, &prog) else {
        eprintln!("arbolview: {}: cannot read", path.display());
        return 1;
    };
    let ts = report::now();
    if history {
        crate::tools::save_snapshot(&path, &node, ts);
        monitor::append_history(&path, &node, ts);
    }
    let hist = monitor::read_history(&path);
    let rep = report::build(lang, &path, &node, &top, depth, rows, hist);
    let mut code = 0;
    if let Some(pct) = warn_free {
        if let Some((_, _, total, avail)) = &rep.disk {
            if *total > 0 && (*avail as f64 / *total as f64 * 100.0) < pct {
                code = 3;
                if !quiet {
                    eprintln!("arbolview: {} {} / {} ({:.1} % < {pct} %)", path.display(), format_size(*avail, Unit::Auto), format_size(*total, Unit::Auto), *avail as f64 / *total as f64 * 100.0);
                }
            }
        }
    }
    if quiet && output.is_none() {
        return code;
    }
    let bytes = report::render(&rep, format);
    let to_stdout = match &output {
        Some(o) => o == "-",
        None => matches!(format, Format::Text | Format::Csv | Format::Json),
    };
    if to_stdout {
        let _ = std::io::stdout().write_all(&bytes);
    } else {
        let file = output.map(|o| expand_out(&o)).unwrap_or_else(|| report::default_path(&path, format));
        if let Err(e) = std::fs::write(&file, &bytes) {
            eprintln!("arbolview: {}: {e}", file.display());
            return 1;
        }
        if !quiet {
            println!("{}", file.display());
        }
    }
    let _ = prog.errors.load(std::sync::atomic::Ordering::Relaxed);
    code
}

fn expand_out(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~") {
        PathBuf::from(format!("{}{}", crate::platform::home().display(), rest))
    } else {
        PathBuf::from(p)
    }
}

fn history_cmd(args: &[String], lang: Lang) -> i32 {
    let mut path = None;
    let mut format = Format::Text;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-f" | "--format" => match it.next().and_then(|v| Format::parse(v)) {
                Some(f) => format = f,
                None => return usage_error("--format needs text, csv or json"),
            },
            "--lang" => {
                it.next();
            }
            s if s.starts_with('-') => return usage_error(&format!("unknown option {s}")),
            s => path = Some(expand(s)),
        }
    }
    let Some(path) = path else { return usage_error("history needs a folder") };
    let h = monitor::read_history(&path);
    let mut out = String::new();
    match format {
        Format::Csv => {
            out.push_str("time,size,allocated,files,folders,disk_total,disk_free\n");
            for p in &h {
                out.push_str(&format!("{},{},{},{},{},{},{}\n", p.ts, p.size, p.alloc, p.files, p.folders, p.fs_total, p.fs_free));
            }
        }
        Format::Json => {
            let v: Vec<String> = h
                .iter()
                .map(|p| format!("{{\"time\":{},\"size\":{},\"allocated\":{},\"files\":{},\"folders\":{},\"disk_total\":{},\"disk_free\":{}}}", p.ts, p.size, p.alloc, p.files, p.folders, p.fs_total, p.fs_free))
                .collect();
            out.push_str(&format!("[{}]\n", v.join(",")));
        }
        _ => {
            if h.is_empty() {
                out.push_str(&format!("{}\n", crate::i18n::t(lang, "hist_none")));
            }
            let mut prev: Option<u64> = None;
            for p in &h {
                let d = prev.map(|q| {
                    let d = p.size as i128 - q as i128;
                    format!("  ({}{})", if d >= 0 { "+" } else { "−" }, format_size(d.unsigned_abs() as u64, Unit::Auto))
                });
                out.push_str(&format!("{}  {:>12}  {:>10} {}{}\n", format_time(p.ts), format_size(p.size, Unit::Auto), group(p.files), crate::i18n::t(lang, "files_w"), d.unwrap_or_default()));
                prev = Some(p.size);
            }
        }
    }
    print!("{out}");
    0
}

fn schedule_cmd(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("list") | None => {
            let v = monitor::list_schedules();
            if v.is_empty() {
                println!("-");
            }
            for s in v {
                println!("{:<8} {}  ({}.timer)", s.every.calendar(), s.path.display(), s.unit);
            }
            0
        }
        Some("add") => {
            let mut path = None;
            let mut every = None;
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--every" => every = it.next().and_then(|v| Every::parse(v)),
                    s if s.starts_with('-') => return usage_error(&format!("unknown option {s}")),
                    s => path = Some(expand(s)),
                }
            }
            let (Some(path), Some(every)) = (path, every) else { return usage_error("schedule add <folder> --every hourly|daily|weekly") };
            match monitor::set_schedule(&path, every) {
                Ok(()) => {
                    println!("{} {}", every.calendar(), path.display());
                    0
                }
                Err(e) => {
                    eprintln!("arbolview: {e}");
                    1
                }
            }
        }
        Some("remove") => {
            let Some(path) = args.get(1).map(|s| expand(s)) else { return usage_error("schedule remove <folder>") };
            match monitor::remove_schedule(&path) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("arbolview: {e}");
                    1
                }
            }
        }
        Some(other) => usage_error(&format!("unknown schedule action {other}")),
    }
}
