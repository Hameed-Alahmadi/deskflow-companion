// `dfc tray` (Linux): StatusNotifierItem tray icon (KDE) with the settings as menus. Started at
// login from ~/.config/autostart; one instance per user, held by a lock on the pid file, which
// install/uninstall use to stop it (the pid alone can't be trusted: install renames the binary).
use crate::config::{self, Config};
use crate::ddc::{self, Monitor};
use crate::log;
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

const CONFIG_CHECK: Duration = Duration::from_secs(2);

enum Cmd {
    Detect,
}

struct Tray {
    cfg: Config,
    /// Why the file on disk is not the config shown.
    cfg_error: Option<String>,
    /// None while detecting.
    monitors: Option<Vec<Monitor>>,
    tx: Sender<Cmd>,
}

pub fn run() -> Result<(), String> {
    let _lock = lock()?;
    let (cfg, cfg_error) = match load() {
        Ok(c) => (c, None),
        Err(e) => {
            log!("tray: config: {e}; showing the defaults");
            (Config::default(), Some(e))
        }
    };
    let (tx, rx) = mpsc::channel();
    let handle = Tray { cfg, cfg_error, monitors: None, tx: tx.clone() }
        // Plasma may not be up yet at login: wait for its watcher instead of failing.
        .assume_sni_available(true)
        .spawn()
        .map_err(|e| format!("tray: {e}"))?;
    log!("tray started");
    let _ = tx.send(Cmd::Detect);
    let mut stamp = config::stamp();
    loop {
        match rx.recv_timeout(CONFIG_CHECK) {
            Ok(Cmd::Detect) => {
                handle.update(|t| t.monitors = None);
                let cfg = handle.update(|t| t.cfg.clone()).unwrap_or_default();
                let monitors = ddc::detect(&cfg).unwrap_or_else(|e| {
                    log!("tray: detect: {e}");
                    Vec::new()
                });
                handle.update(|t| t.monitors = Some(monitors));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if config::stamp() != stamp {
            stamp = config::stamp();
            handle.update(Tray::reload);
        }
        if handle.is_closed() {
            return Ok(());
        }
    }
}

fn load() -> Result<Config, String> {
    Config::load().and_then(|c| c.validate().map(|()| c))
}

impl Tray {
    fn reload(&mut self) {
        match load() {
            Ok(c) => {
                let monitor_changed = c.monitors.iter().map(|m| &m.find).ne(self.cfg.monitors.iter().map(|m| &m.find));
                self.cfg = c;
                self.cfg_error = None;
                if monitor_changed {
                    let _ = self.tx.send(Cmd::Detect);
                }
            }
            Err(e) => {
                if self.cfg_error.as_ref() != Some(&e) {
                    log!("tray: config: {e}");
                    self.cfg_error = Some(e);
                }
            }
        }
    }

    /// Writes one change to the file (re-read first, so other writers' fields survive).
    fn change(&mut self, f: impl FnOnce(&mut Config)) {
        match Config::update(f) {
            Ok(c) => self.cfg = c,
            Err(e) => {
                log!("tray: {e}");
                notify(&e);
            }
        }
    }
}

impl ksni::Tray for Tray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "deskflow-companion".into()
    }

    fn title(&self) -> String {
        "Deskflow Companion".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        if self.cfg.auto_switch {
            vec![icon(24, include_bytes!("../assets/linux/active-24.rgba")), icon(48, include_bytes!("../assets/linux/active-48.rgba"))]
        } else {
            vec![icon(24, include_bytes!("../assets/linux/paused-24.rgba")), icon(48, include_bytes!("../assets/linux/paused-48.rgba"))]
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let mut lines = vec![format!("This machine: {}", self.cfg.this_machine)];
        if !self.cfg.auto_switch {
            lines.push("Automatic switching off".into());
        }
        if self.cfg_error.is_some() {
            lines.push("Config error, see the log".into());
        }
        ToolTip { title: "Deskflow Companion".into(), description: lines.join("\n"), ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let machines = self.cfg.machines();
        let mut items: Vec<MenuItem<Self>> = machines.iter().map(|name| self.switch_item(name, None)).collect();
        if self.cfg.monitors.len() > 1 {
            for m in &self.cfg.monitors {
                items.push(
                    SubMenu {
                        label: m.name.clone(),
                        submenu: m.inputs.keys().map(|name| self.switch_item(name, Some(&m.name))).collect(),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }
        items.push(MenuItem::Separator);
        items.push(
            CheckmarkItem {
                label: "Switch automatically".into(),
                checked: self.cfg.auto_switch,
                activate: Box::new(|t: &mut Self| {
                    let on = !t.cfg.auto_switch;
                    t.change(|c| c.auto_switch = on);
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(SubMenu { label: "Shared monitors".into(), submenu: self.monitor_menu(), ..Default::default() }.into());
        let names = machines.clone();
        items.push(
            SubMenu {
                label: "This machine".into(),
                submenu: vec![
                    RadioGroup {
                        selected: names.iter().position(|n| *n == self.cfg.this_machine).unwrap_or(usize::MAX),
                        select: Box::new(move |t: &mut Self, i| {
                            let name = names[i].clone();
                            t.change(|c| c.this_machine = name);
                        }),
                        options: machines.iter().map(|k| RadioItem { label: k.clone(), ..Default::default() }).collect(),
                        ..Default::default()
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
        );
        items.extend([
            StandardItem {
                label: "Edit config file...".into(),
                activate: Box::new(|_: &mut Self| {
                    if !config::path().exists() {
                        let _ = Config::default().save();
                    }
                    open(&config::path());
                }),
                ..Default::default()
            }
            .into(),
            StandardItem { label: "Open log".into(), activate: Box::new(|_: &mut Self| open(&crate::log::path())), ..Default::default() }.into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|_: &mut Self| {
                    log!("tray: quit");
                    std::process::exit(0)
                }),
                ..Default::default()
            }
            .into(),
        ]);
        items
    }
}

impl Tray {
    /// "Switch to <machine>", on every monitor that has it or only on `monitor`.
    fn switch_item(&self, name: &str, monitor: Option<&str>) -> MenuItem<Self> {
        let label = if *name == self.cfg.this_machine { format!("Switch to {name} (this machine)") } else { format!("Switch to {name}") };
        let mut args = vec!["switch".to_string(), name.to_string()];
        args.extend(monitor.map(String::from));
        StandardItem {
            label,
            activate: Box::new(move |_: &mut Self| spawn_self(&args.iter().map(String::as_str).collect::<Vec<_>>())),
            ..Default::default()
        }
        .into()
    }

    /// The detected displays, ticked when shared; ticking one adds it, unticking removes it.
    fn monitor_menu(&self) -> Vec<MenuItem<Self>> {
        let detect_again =
            StandardItem { label: "Detect again".into(), activate: Box::new(|t: &mut Self| drop(t.tx.send(Cmd::Detect))), ..Default::default() };
        let Some(monitors) = &self.monitors else {
            return vec![StandardItem { label: "Detecting...".into(), enabled: false, ..Default::default() }.into()];
        };
        let mut items: Vec<MenuItem<Self>> = Vec::new();
        if monitors.is_empty() {
            items.push(StandardItem { label: "No DDC/CI monitors found".into(), enabled: false, ..Default::default() }.into());
        }
        for m in monitors {
            let input = m.current.map_or("input unreadable".into(), |c| format!("input {}", config::Vcp(c)));
            let label = match m.shared.map(|i| &self.cfg.monitors[i]) {
                Some(s) => format!("{} [{}] ({input})", m.description, s.name),
                None => format!("{} ({input})", m.description),
            };
            let (shared, find) =
                (m.shared, config::MonitorMatch { serial: m.serial.clone(), model: m.model.clone(), position: None });
            items.push(
                CheckmarkItem {
                    label,
                    checked: shared.is_some(),
                    activate: Box::new(move |t: &mut Self| t.toggle_monitor(shared, find.clone())),
                    ..Default::default()
                }
                .into(),
            );
        }
        for (i, s) in self.cfg.monitors.iter().enumerate() {
            if !monitors.iter().any(|m| m.shared == Some(i)) {
                items.push(StandardItem { label: format!("{} (not found)", s.name), enabled: false, ..Default::default() }.into());
            }
        }
        items.push(MenuItem::Separator);
        items.push(detect_again.into());
        items
    }

    fn toggle_monitor(&mut self, shared: Option<usize>, find: config::MonitorMatch) {
        match shared {
            Some(_) if self.cfg.monitors.len() == 1 => notify("The last shared monitor can't be removed."),
            Some(i) => {
                let name = self.cfg.monitors[i].name.clone();
                self.change(|c| c.monitors.retain(|m| m.name != name));
            }
            None if find.serial.is_none() && find.model.is_none() => notify("This monitor has no EDID, so dfc can't tell it apart."),
            None => {
                let mut added = String::new();
                self.change(|c| added = c.add_monitor(find));
                if !added.is_empty() {
                    notify(&format!("Added {added} with the input codes of the first monitor: check them with Edit config file."));
                }
            }
        }
        ddc::forget_bus();
        let _ = self.tx.send(Cmd::Detect);
    }
}

/// RGBA bytes (from scripts/icons.sh) to the ARGB the protocol wants.
fn icon(size: i32, rgba: &[u8]) -> Icon {
    let data = rgba.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    Icon { width: size, height: size, data }
}

/// Runs dfc again (switch), so the menu never waits on ddcutil.
fn spawn_self(args: &[&str]) {
    let r = std::env::current_exe().and_then(|exe| Command::new(exe).args(args).stdin(Stdio::null()).spawn());
    match r {
        Ok(mut child) => {
            // Reap it, or it stays a zombie until the tray exits.
            std::thread::spawn(move || child.wait());
        }
        Err(e) => log!("tray: dfc {}: {e}", args.join(" ")),
    }
}

fn open(path: &Path) {
    if let Err(e) = Command::new("xdg-open").arg(path).stdin(Stdio::null()).spawn().map(|mut c| std::thread::spawn(move || c.wait())) {
        log!("tray: xdg-open {}: {e}", path.display());
    }
}

fn notify(msg: &str) {
    let _ = Command::new("notify-send").args(["-a", "Deskflow Companion", "Deskflow Companion", msg]).spawn();
}

fn pid_file() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("deskflow-companion-tray.pid")
}

fn try_lock(f: &File) -> bool {
    // SAFETY: flock on an open descriptor.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Takes the single-instance lock for the life of the process and records the pid.
fn lock() -> Result<File, String> {
    let path = pid_file();
    let mut f = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !try_lock(&f) {
        return Err("tray already running".into());
    }
    f.set_len(0).map_err(|e| e.to_string())?;
    write!(f, "{}", std::process::id()).map_err(|e| e.to_string())?;
    Ok(f)
}

/// Stops a running tray; true when one was running.
pub fn stop_running() -> Result<bool, String> {
    let Ok(mut f) = OpenOptions::new().read(true).write(true).open(pid_file()) else { return Ok(false) };
    if try_lock(&f) {
        return Ok(false);
    }
    let mut pid = String::new();
    let _ = f.read_to_string(&mut pid);
    let pid: i32 = pid.trim().parse().map_err(|_| format!("tray lock held but no pid in {}", pid_file().display()))?;
    // SAFETY: plain signal; the lock proves this pid is the tray.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if try_lock(&f) {
            return Ok(true);
        }
    }
    Err(format!("tray (pid {pid}) did not stop"))
}

/// Starts `exe tray` detached from the terminal; false when there is no desktop session (ssh).
pub fn start(exe: &Path) -> Result<bool, String> {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() || std::env::var_os("WAYLAND_DISPLAY").or(std::env::var_os("DISPLAY")).is_none() {
        return Ok(false);
    }
    let mut c = Command::new(exe);
    c.arg("tray").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe; own session, so closing the terminal doesn't kill it.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map(|_| true).map_err(|e| format!("{}: {e}", exe.display()))
}
