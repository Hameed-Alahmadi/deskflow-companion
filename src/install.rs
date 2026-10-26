// install / uninstall: puts dfc in a fixed place, points Deskflow's enter/exitCommand at it and,
// on Windows, starts the daemon at logon. Re-running install updates; the previous binary is kept
// next to the new one (run it with `install` to roll back).
use crate::config::{self, Config};
use crate::{ini, log};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Opts {
    pub dry_run: bool,
    /// Edit Deskflow.conf without stopping and starting Deskflow.
    pub no_restart: bool,
}

const CORE: &str = "core";
#[cfg(windows)]
const TASK: &str = "DeskflowCompanion";
#[cfg(target_os = "linux")]
const DESKFLOW_UNIT: &str = "app-org.deskflow.deskflow@autostart.service";

/// Prints each step; runs it unless this is a dry run.
struct Steps<'a>(&'a Opts);

impl Steps<'_> {
    fn run(&self, what: &str, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        println!("{}{what}", if self.0.dry_run { "would " } else { "" });
        if self.0.dry_run { Ok(()) } else { f().map_err(|e| format!("{what}: {e}")) }
    }
}

pub fn install(o: &Opts) -> Result<(), String> {
    let steps = Steps(o);
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let (target, prev) = binary_paths();

    #[cfg(windows)]
    if task_exists(TASK) {
        steps.run(&format!("stop the daemon (task {TASK})"), || {
            stop_daemon();
            Ok(())
        })?;
    }
    #[cfg(target_os = "linux")]
    let tray_was_running = {
        let mut was = false;
        steps.run("stop the tray if it runs", || crate::linuxtray::stop_running().map(|r| was = r))?;
        was
    };
    if !same_file(&me, &target) {
        let keep = if target.exists() { format!(" (previous kept as {})", prev.display()) } else { String::new() };
        steps.run(&format!("copy {} to {}{keep}", me.display(), target.display()), || {
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            if target.exists() {
                let _ = fs::remove_file(&prev);
                // Renaming works even while the old binary runs.
                fs::rename(&target, &prev).map_err(|e| e.to_string())?;
            }
            fs::copy(&me, &target).map(drop).map_err(|e| e.to_string())
        })?;
    }
    let cfg_path = config::path();
    if !cfg_path.exists() {
        steps.run(&format!("write the default config to {}", cfg_path.display()), || {
            fs::create_dir_all(cfg_path.parent().unwrap()).map_err(|e| e.to_string())?;
            let text = serde_json::to_string_pretty(&Config::default()).map_err(|e| e.to_string())?;
            fs::write(&cfg_path, text + "\n").map_err(|e| e.to_string())
        })?;
    }
    edit_deskflow(&steps, &hooks(&target), true)?;
    #[cfg(target_os = "linux")]
    {
        let desktop = autostart_path();
        steps.run(&format!("start the tray at login ({})", desktop.display()), || {
            fs::create_dir_all(desktop.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::write(&desktop, autostart_entry(&target)).map_err(|e| e.to_string())
        })?;
        steps.run("start the tray", || match crate::linuxtray::start(&target)? {
            true => Ok(()),
            false => {
                println!("  no desktop session here (ssh?): it starts at the next login{}", if tray_was_running { "; the old one was stopped" } else { "" });
                Ok(())
            }
        })?;
    }
    #[cfg(windows)]
    {
        steps.run(&format!("register the logon task {TASK} and start it"), || match create_task(&target) {
            // A task registered from an elevated prompt can't be replaced from a normal one. Its
            // command is the same fixed path, so keep it and just start it.
            Err(e) if task_exists(TASK) => {
                println!("  could not re-register ({e}); keeping the existing task");
                run(hidden("schtasks").args(["/run", "/tn", TASK]))
            }
            r => r,
        })?;
    }
    if !o.dry_run {
        log!("installed {} to {}", env!("CARGO_PKG_VERSION"), target.display());
    }
    Ok(())
}

pub fn uninstall(o: &Opts) -> Result<(), String> {
    let steps = Steps(o);
    #[cfg(windows)]
    if task_exists(TASK) {
        steps.run(&format!("stop and remove the logon task {TASK}"), || {
            stop_daemon();
            run(hidden("schtasks").args(["/delete", "/tn", TASK, "/f"]))
        })?;
    }
    #[cfg(target_os = "linux")]
    {
        steps.run("stop the tray if it runs", || crate::linuxtray::stop_running().map(drop))?;
        let desktop = autostart_path();
        if desktop.exists() {
            steps.run(&format!("remove {}", desktop.display()), || fs::remove_file(&desktop).map_err(|e| e.to_string()))?;
        }
    }
    let saved = saved_path();
    let original: Option<Vec<(String, Option<String>)>> =
        fs::read_to_string(&saved).ok().and_then(|t| serde_json::from_str(&t).ok());
    match original {
        Some(values) => {
            edit_deskflow(&steps, &values, false)?;
            if saved.exists() {
                steps.run(&format!("remove {}", saved.display()), || fs::remove_file(&saved).map_err(|e| e.to_string()))?;
            }
        }
        // No saved lines (installed elsewhere?): just turn the dfc hooks off.
        None => {
            let off = vec![("enableEnterCommand".into(), Some("false".into())), ("enableExitCommand".into(), Some("false".into()))];
            let text = fs::read_to_string(deskflow_conf()).unwrap_or_default();
            if ini::get(&text, CORE, "enterCommand").is_some_and(|c| c.contains("dfc")) {
                edit_deskflow(&steps, &off, false)?;
            }
        }
    }
    let (target, _) = binary_paths();
    println!("left in place: {}, {}, {}", target.display(), config::path().display(), log::path().display());
    if !o.dry_run {
        log!("uninstalled");
    }
    Ok(())
}

/// The enter/exitCommand values for the installed binary (raw, as written in Deskflow.conf).
fn hooks(exe: &Path) -> Vec<(String, Option<String>)> {
    let p = exe.display().to_string();
    let p = if p.contains(' ') { format!("\"{p}\"") } else { p };
    let cmd = |arg: &str| Some(ini::escape(&format!("{p} {arg}")));
    let mut v = vec![("enableEnterCommand".to_string(), Some("true".to_string())), ("enterCommand".into(), cmd("enter"))];
    if cfg!(windows) {
        v.push(("enableExitCommand".into(), Some("true".into())));
        v.push(("exitCommand".into(), cmd("leave")));
    } else {
        // Linux has nothing to do on leave; the other machine switches when the cursor arrives.
        v.push(("enableExitCommand".into(), Some("false".into())));
    }
    v
}

/// Applies `values` to Deskflow.conf [core]: Deskflow stopped first (it rewrites the file),
/// file backed up, Deskflow started again. With `save`, the lines being replaced are kept for
/// uninstall (only the first time, so updates keep the true original).
fn edit_deskflow(steps: &Steps, values: &[(String, Option<String>)], save: bool) -> Result<(), String> {
    let path = deskflow_conf();
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e} (start Deskflow once first)", path.display()))?;
    let refs: Vec<(&str, Option<&str>)> = values.iter().map(|(k, v)| (k.as_str(), v.as_deref())).collect();
    if ini::set(&text, CORE, &refs) == text {
        println!("Deskflow.conf is already set up");
        return Ok(());
    }
    for (k, v) in &refs {
        let old = ini::get(&text, CORE, k);
        if old.as_deref() != *v {
            println!("  {k}: {} -> {}", old.as_deref().unwrap_or("(none)"), v.unwrap_or("(removed)"));
        }
    }
    let restart = !steps.0.no_restart;
    let what = format!(
        "{}back up and edit {}{}",
        if restart { "stop Deskflow, " } else { "" },
        path.display(),
        if restart { ", start Deskflow" } else { "" }
    );
    steps.run(&what, || {
        let was_running = if restart { deskflow_stop()? } else { false };
        let result = (|| {
            // Read again: Deskflow may have written it while stopping.
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let saved = saved_path();
            if save && !saved.exists() {
                let original: Vec<(&str, Option<String>)> =
                    refs.iter().map(|(k, _)| (*k, ini::get(&text, CORE, k))).collect();
                fs::create_dir_all(saved.parent().unwrap()).map_err(|e| e.to_string())?;
                fs::write(&saved, serde_json::to_string_pretty(&original).unwrap()).map_err(|e| e.to_string())?;
            }
            // Never overwrite an earlier backup (two runs can fall in the same second).
            let stamp = log::stamp();
            let backup = (0..)
                .map(|n| match n {
                    0 => path.with_file_name(format!("Deskflow.conf.{stamp}.bak")),
                    n => path.with_file_name(format!("Deskflow.conf.{stamp}-{n}.bak")),
                })
                .find(|p| !p.exists())
                .unwrap();
            fs::copy(&path, &backup).map_err(|e| format!("backup: {e}"))?;
            println!("  backup: {}", backup.display());
            fs::write(&path, ini::set(&text, CORE, &refs)).map_err(|e| e.to_string())
        })();
        // Start Deskflow again even when the edit failed.
        if was_running {
            deskflow_start()?;
        }
        result
    })
}

fn binary_paths() -> (PathBuf, PathBuf) {
    #[cfg(windows)]
    let (dir, name, prev) = (
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("DeskflowCompanion"),
        "dfc.exe",
        "dfc.prev.exe",
    );
    #[cfg(not(windows))]
    let (dir, name, prev) = (PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/bin"), "dfc", "dfc.prev");
    (dir.join(name), dir.join(prev))
}

/// The Deskflow.conf lines install replaced, kept for uninstall.
fn saved_path() -> PathBuf {
    config::path().with_file_name("deskflow-original.json")
}

fn deskflow_conf() -> PathBuf {
    #[cfg(windows)]
    let dir = PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default());
    #[cfg(not(windows))]
    let dir = config::xdg_dir("XDG_CONFIG_HOME", ".config");
    dir.join("Deskflow").join("Deskflow.conf")
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// A command that never opens a console window (dfc itself has none on Windows).
fn hidden(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn run(c: &mut Command) -> Result<(), String> {
    let out = c.output().map_err(|e| format!("{:?}: {e}", c.get_program()))?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if msg.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { msg };
        Err(format!("{:?} failed: {msg}", c.get_program()))
    }
}

/// Stops the Deskflow GUI (and with it the core); true when it was running.
#[cfg(target_os = "linux")]
fn deskflow_stop() -> Result<bool, String> {
    if hidden("systemctl").args(["--user", "is-active", "--quiet", DESKFLOW_UNIT]).status().is_ok_and(|s| s.success()) {
        run(hidden("systemctl").args(["--user", "stop", DESKFLOW_UNIT]))?;
        return Ok(true);
    }
    if hidden("pgrep").args(["-x", "deskflow"]).status().is_ok_and(|s| s.success()) {
        return Err("Deskflow runs outside its autostart unit: close it and run again".into());
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn deskflow_start() -> Result<(), String> {
    run(hidden("systemctl").args(["--user", "start", DESKFLOW_UNIT]))
}

#[cfg(windows)]
fn deskflow_stop() -> Result<bool, String> {
    let out = hidden("tasklist").args(["/fi", "imagename eq deskflow.exe", "/fo", "csv", "/nh"]).output();
    if !out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("deskflow.exe")) {
        return Ok(false);
    }
    // Forced, so the GUI cannot write its settings back over the edit.
    run(hidden("taskkill").args(["/f", "/im", "deskflow.exe"]))?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    Ok(true)
}

#[cfg(windows)]
fn deskflow_start() -> Result<(), String> {
    if task_exists("DeskflowGUI") {
        return run(hidden("schtasks").args(["/run", "/tn", "DeskflowGUI"]));
    }
    let exe = PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_default()).join("Deskflow").join("deskflow.exe");
    hidden(&exe.display().to_string()).spawn().map(drop).map_err(|e| format!("{}: {e}", exe.display()))
}

#[cfg(target_os = "linux")]
fn autostart_path() -> PathBuf {
    config::xdg_dir("XDG_CONFIG_HOME", ".config").join("autostart").join("deskflow-companion.desktop")
}

#[cfg(target_os = "linux")]
fn autostart_entry(exe: &Path) -> String {
    // Desktop Entry Exec quoting: double quotes, with \ " ` $ escaped.
    let p = exe.display().to_string();
    let p = if p.contains([' ', '"', '\\', '`', '$']) {
        format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$"))
    } else {
        p
    };
    format!(
        "[Desktop Entry]\nType=Application\nName=Deskflow Companion\nComment=Tray icon: switch the shared monitor, settings\nExec={p} tray\nIcon=video-display\nTerminal=false\nX-KDE-autostart-phase=2\n"
    )
}

/// Closes the daemon (so its tray icon goes away), then ends the task for good measure. Over ssh
/// (session 0) the window can't be seen and only /end works.
#[cfg(all(test, target_os = "linux"))]
#[test]
fn autostart_exec_quoting() {
    assert!(autostart_entry(Path::new("/home/a/.local/bin/dfc")).contains("\nExec=/home/a/.local/bin/dfc tray\n"));
    assert!(autostart_entry(Path::new("/home/a b/dfc")).contains("\nExec=\"/home/a b/dfc\" tray\n"));
    assert!(autostart_entry(Path::new("/x/$y")).contains("Exec=\"/x/\\$y\" tray"));
}

#[cfg(windows)]
fn stop_daemon() {
    if crate::winwatch::close_running() {
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !crate::winwatch::close_running() {
                break;
            }
        }
    }
    let _ = hidden("schtasks").args(["/end", "/tn", TASK]).output();
}

#[cfg(windows)]
fn task_exists(name: &str) -> bool {
    hidden("schtasks").args(["/query", "/tn", name]).output().is_ok_and(|o| o.status.success())
}

/// Logon task for `dfc daemon` in the user's session: no battery stop, no time limit
/// (schtasks /sc onlogon would set both).
#[cfg(windows)]
fn create_task(exe: &Path) -> Result<(), String> {
    // whoami rather than %USERDOMAIN%, which is WORKGROUP in some sessions (e.g. over ssh).
    let out = hidden("whoami").output().map_err(|e| format!("whoami: {e}"))?;
    let user = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if user.is_empty() {
        return Err("whoami gave no user name".into());
    }
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Deskflow Companion: switches the shared monitor to this machine</Description></RegistrationInfo>
  <Triggers><LogonTrigger><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>daemon</Arguments></Exec></Actions>
</Task>
"#,
        esc(&exe.display().to_string()),
        user = esc(&user)
    );
    let file = exe.with_file_name("task.xml");
    let bytes: Vec<u8> = std::iter::once(0xfeffu16).chain(xml.encode_utf16()).flat_map(u16::to_le_bytes).collect();
    fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let file_arg = file.display().to_string();
    let created = run(hidden("schtasks").args(["/create", "/tn", TASK, "/xml", &file_arg, "/f"]));
    let _ = fs::remove_file(&file);
    created?;
    run(hidden("schtasks").args(["/run", "/tn", TASK]))
}
