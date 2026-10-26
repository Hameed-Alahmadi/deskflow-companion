// Append-only log with local timestamps. Logging never fails the caller.
use std::io::Write;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
            .join("DeskflowCompanion")
            .join("dfc.log")
    }
    #[cfg(not(windows))]
    {
        crate::config::xdg_dir("XDG_STATE_HOME", ".local/state").join("deskflow-companion.log")
    }
}

pub fn log(msg: &str) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {msg}", now());
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log::log(&format!($($arg)*)) };
}

/// Timestamp usable in file names: 2026-10-26T145531.
pub fn stamp() -> String {
    let t = now();
    t[..t.find('.').unwrap_or(t.len())].replace(':', "")
}

#[cfg(target_os = "linux")]
fn now() -> String {
    // SAFETY: clock_gettime and localtime_r with valid pointers.
    unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&ts.tv_sec, &mut tm);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
            ts.tv_nsec / 1_000_000
        )
    }
}

#[cfg(windows)]
fn now() -> String {
    // SAFETY: GetLocalTime only fills the returned struct.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}
