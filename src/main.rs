// dfc: Deskflow Companion. Switches a shared monitor's input to the machine Deskflow is on.
#![windows_subsystem = "windows"]

mod config;
mod ddc;
mod edid;
mod hotkey;
mod ini;
mod install;
#[cfg(target_os = "linux")]
mod linuxtray;
mod log;
#[cfg(windows)]
mod winsettings;
#[cfg(windows)]
mod wintray;
#[cfg(windows)]
mod winwatch;

use config::{Config, Vcp};
use ddc::Outcome;
use std::process::ExitCode;

const USAGE: &str = "usage: dfc <command>
  detect                 list monitors, their current input and which one the config selects
  switch <machine|0xNN> [monitor]
                         switch the shared monitors that have that machine, or just one
                         (each is skipped when already on that input)
  enter / leave          Deskflow enter/exitCommand hooks
  daemon                 Windows: watch the cursor and the hotkey, switch to this machine; tray icon
  tray                   Linux: tray icon (started at login by install)
  settings               Windows: settings window (also from the tray)
  install [--dry-run] [--no-restart]
                         copy dfc to its fixed place, set Deskflow's enter/exitCommand and
                         (Windows) the logon task; run again to update, dfc.prev install to roll back
  uninstall [--dry-run] [--no-restart]
                         remove the logon task, restore Deskflow's previous commands
  --version";

fn main() -> ExitCode {
    #[cfg(windows)]
    platform::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["detect"] => Config::load().and_then(|c| detect(&c)),
        ["switch", target] => Config::load().and_then(|c| switch(&c, target, None)),
        ["switch", target, monitor] => Config::load().and_then(|c| switch(&c, target, Some(monitor))),
        ["enter"] => enter(),
        ["leave"] => leave(),
        ["daemon"] => daemon(),
        ["tray"] => tray(),
        ["settings"] => settings(),
        ["install", flags @ ..] => install_opts(flags).and_then(|o| install::install(&o)),
        ["uninstall", flags @ ..] => install_opts(flags).and_then(|o| install::uninstall(&o)),
        ["--version" | "version"] => {
            println!("dfc {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dfc: {e}");
            log!("{}: {e}", args.join(" "));
            ExitCode::FAILURE
        }
    }
}

fn detect(cfg: &Config) -> Result<(), String> {
    let monitors = ddc::detect(cfg)?;
    if monitors.is_empty() {
        println!("no monitors found");
    }
    for m in &monitors {
        let shared = m.shared.map(|i| &cfg.monitors[i]);
        let current = m.current.map_or("unreadable".to_string(), |c| {
            let name = shared.and_then(|s| s.inputs.iter().find(|(_, v)| v.0 == c)).map(|(k, _)| format!(" ({k})"));
            format!("{}{}", Vcp(c), name.unwrap_or_default())
        });
        match shared {
            Some(s) => println!("* {} [{}]", m.id, s.name),
            None => println!("  {}", m.id),
        }
        println!("    {}", m.description);
        println!("    input: {current}");
    }
    for (i, s) in cfg.monitors.iter().enumerate() {
        if !monitors.iter().any(|m| m.shared == Some(i)) {
            println!("! shared monitor {} not found", s.name);
        }
    }
    let path = config::path();
    let state = if path.exists() { "" } else { " (not present, using defaults)" };
    println!("\n* = shared monitor per config: {}{state}", path.display());
    println!("log: {}", log::path().display());
    Ok(())
}

/// Switches every shared monitor that has an input for `target`, or only `monitor`.
fn switch(cfg: &Config, target: &str, monitor: Option<&str>) -> Result<(), String> {
    let several = cfg.monitors.len() > 1;
    let mut failed = Vec::new();
    for (m, code) in cfg.targets(target, monitor)? {
        let label = if several { format!("{target} on {}", m.name) } else { target.to_string() };
        match ddc::switch(&m.find, code) {
            Ok(Outcome::Already) => log!("switch {label}: already on {}, skipped", Vcp(code)),
            Ok(Outcome::Switched { from }) => {
                let from = from.map_or("unknown".to_string(), |c| Vcp(c).to_string());
                log!("switch {label}: {from} -> {}", Vcp(code));
            }
            Err(e) if several => failed.push(format!("{}: {e}", m.name)),
            Err(e) => failed.push(e),
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
}

/// Linux: Deskflow brought the cursor here, so show this machine.
/// Windows: tell the daemon that Deskflow holds the cursor.
fn enter() -> Result<(), String> {
    #[cfg(windows)]
    return winwatch::set_deskflow_active(true);
    #[cfg(not(windows))]
    {
        let cfg = Config::load()?;
        if !cfg.auto_switch {
            log!("enter: automatic switching off, not switching");
            return Ok(());
        }
        switch(&cfg, &cfg.this_machine, None)
    }
}

fn leave() -> Result<(), String> {
    #[cfg(windows)]
    return winwatch::set_deskflow_active(false);
    #[cfg(not(windows))]
    Ok(())
}

fn install_opts(flags: &[&str]) -> Result<install::Opts, String> {
    let mut o = install::Opts { dry_run: false, no_restart: false };
    for f in flags {
        match *f {
            "--dry-run" => o.dry_run = true,
            "--no-restart" => o.no_restart = true,
            _ => return Err(format!("unknown option {f}\n{USAGE}")),
        }
    }
    Ok(o)
}

fn daemon() -> Result<(), String> {
    #[cfg(windows)]
    return winwatch::run();
    #[cfg(not(windows))]
    Err("the daemon is Windows only".into())
}

fn tray() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return linuxtray::run();
    #[cfg(not(target_os = "linux"))]
    Err("on Windows the tray icon is part of the daemon".into())
}

fn settings() -> Result<(), String> {
    #[cfg(windows)]
    return winsettings::run();
    #[cfg(not(windows))]
    Err("on Linux the settings are in the tray menu, or edit the config file".into())
}

#[cfg(windows)]
mod platform {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

    pub fn init() {
        // SAFETY: plain process-wide setup calls.
        unsafe {
            // Real pixel coordinates for monitor rects and the cursor.
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // A windows-subsystem binary has no console: borrow the terminal's one so `dfc detect`
            // prints there. Redirected output (dfc detect > x.txt) already has handles.
            let redirected = GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
            if !redirected {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }
}
