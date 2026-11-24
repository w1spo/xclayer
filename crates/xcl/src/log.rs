//! Process-wide logger. The file handle is opened once at startup (before any file
//! hooks are installed) so logging never re-enters the VFS.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

impl Level {
    pub fn parse(s: &str) -> Level {
        match s.to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" | "warning" => Level::Warn,
            "debug" => Level::Debug,
            "trace" => Level::Trace,
            _ => Level::Info,
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

static FILE: Mutex<Option<File>> = Mutex::new(None);
// Kept separate from the ordinary log: no raw game output, tokens, paths or dumps.
static DETAIL: Mutex<Option<(File, usize)>> = Mutex::new(None);
const DETAIL_LIMIT: usize = 8 * 1024 * 1024;

pub fn init_detail(path: &Path) -> std::io::Result<()> {
    let file = File::create(path)?;
    if let Ok(mut g) = DETAIL.lock() { *g = Some((file, 0)); }
    detail(format_args!("schema=1; units=MiB; snapshots best-effort; counters cumulative unless delta"));
    Ok(())
}

/// Never wait for the detail writer, in particular from an exception handler.
/// No log hooks are invoked while the detail handle is held.
pub fn detail(args: std::fmt::Arguments<'_>) {
    let Ok(mut g) = DETAIL.try_lock() else { return };
    let Some((f, bytes)) = g.as_mut() else { return };
    if *bytes >= DETAIL_LIMIT { return; }
    let line = format!("[{:.3}] {args}\n", uptime());
    if line.len() > DETAIL_LIMIT - *bytes {
        let _ = f.write_all(b"detail limit reached; further samples omitted\n");
        *bytes = DETAIL_LIMIT;
        return;
    }
    match f.write_all(line.as_bytes()) {
        Ok(()) => *bytes += line.len(),
        Err(_) => *bytes = DETAIL_LIMIT,
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static ECHO: AtomicBool = AtomicBool::new(true);
static START: OnceLock<Instant> = OnceLock::new();

/// Seconds since the log started (the host's start) – the times log lines carry.
pub fn uptime() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

pub fn init(path: Option<&Path>, level: Level, echo_stderr: bool) {
    START.get_or_init(Instant::now);
    LEVEL.store(level as u8, Ordering::Relaxed);
    ECHO.store(echo_stderr, Ordering::Relaxed);
    if let Some(p) = path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(f) = File::create(p) {
            if let Ok(mut g) = FILE.lock() {
                *g = Some(f);
            }
        }
    }
}

pub fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

pub fn write(level: Level, args: std::fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let line = format!("[{t:9.3}][{tid:5}][{}] {args}\n", level.tag());
    if let Ok(mut g) = FILE.try_lock().or_else(|_| FILE.lock()) {
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(line.as_bytes());
            if level <= Level::Warn {
                let _ = f.flush();
            }
        }
    }
    if ECHO.load(Ordering::Relaxed) && level <= Level::Info {
        let _ = std::io::stderr().write_all(format!("xcl: {args}\n").as_bytes());
    }
}

pub fn flush() {
    if let Ok(mut g) = FILE.lock() {
        if let Some(f) = g.as_mut() {
            let _ = f.flush();
        }
    }
}

#[macro_export]
macro_rules! xlog_error { ($($a:tt)*) => { $crate::log::write($crate::log::Level::Error, format_args!($($a)*)) } }
#[macro_export]
macro_rules! xlog_warn { ($($a:tt)*) => { $crate::log::write($crate::log::Level::Warn, format_args!($($a)*)) } }
#[macro_export]
macro_rules! xlog_info { ($($a:tt)*) => { $crate::log::write($crate::log::Level::Info, format_args!($($a)*)) } }
#[macro_export]
macro_rules! xlog_debug { ($($a:tt)*) => { if $crate::log::enabled($crate::log::Level::Debug) { $crate::log::write($crate::log::Level::Debug, format_args!($($a)*)) } } }
#[macro_export]
macro_rules! xlog_trace { ($($a:tt)*) => { if $crate::log::enabled($crate::log::Level::Trace) { $crate::log::write($crate::log::Level::Trace, format_args!($($a)*)) } } }
