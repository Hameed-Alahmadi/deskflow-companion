// Windows daemon: switches the shared monitor to this
// machine when the cursor enters it from Deskflow, or when the hotkey is pressed (sent by the
// Linux Deskflow layout); the hotkey also centres the cursor on it. Entering the monitor with
// this machine's own mouse does not switch.
//
// Source detection: `dfc enter` / `dfc leave` (Deskflow enter/exitCommand) set and reset a named
// event while Deskflow holds the cursor; the local mouse also arrives as injected input, so Raw
// Input alone can't tell them apart. Raw Input still catches a real device moving at the same time.
//
// The daemon also owns the tray icon (wintray.rs) and reloads the config when it changes on disk
// (settings window, tray toggle, hand edits). A bad config never stops it: it keeps the last good
// one (defaults at start) and says so in the tooltip.
use crate::config::{self, Config, Vcp};
use crate::wintray::{self, Item, Look};
use crate::ddc::{self, Outcome};
use crate::{hotkey, log};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, ResetEvent, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_HEADER, RIDEV_INPUTSINK, RIM_TYPEMOUSE,
    RegisterRawInputDevices,
};
use windows::Win32::UI::Shell::{NIN_SELECT, NINF_KEY, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_SHOWNORMAL, SetCursorPos, SetTimer,
    WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_INPUT, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const EVENT_NAME: PCWSTR = w!("Local\\DeskflowCompanion.DeskflowActive");
const MUTEX_NAME: PCWSTR = w!("Local\\DeskflowCompanion.Daemon");
const HOTKEY_ID: i32 = 1;
const POLL_MS: u32 = 50;
const MUTEX_WAIT_MS: u32 = 5000;
/// When the hotkey is taken (e.g. by an exiting old instance), retry this often.
const HOTKEY_RETRY_MS: u32 = 2000;
/// Same for the tray icon, which can't be added before Explorer's tray is up.
const TRAY_RETRY_MS: u32 = 2000;
const CONFIG_CHECK_MS: u32 = 1000;
/// Window class of the daemon; install/uninstall close it with WM_CLOSE.
pub const CLASS: PCWSTR = w!("DeskflowCompanionDaemon");

/// "TaskbarCreated": Explorer restarted, the tray icon has to be added again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct State {
    cfg: Config,
    hwnd: HWND,
    hotkey: (u32, u32),
    hotkey_ok: bool,
    hotkey_last_try: u32,
    event: HANDLE,
    last_physical: u32,
    last_injected: u32,
    last_hmon: HMONITOR,
    /// The shared monitor (index in cfg.monitors) the cursor is on.
    on_shared: Option<usize>,
    cfg_stamp: Option<(std::time::SystemTime, u64)>,
    cfg_checked: u32,
    /// Why the file on disk is not the config in use.
    cfg_error: Option<String>,
    tray_ok: bool,
    tray_last_try: u32,
    /// The tray look changed and needs NIM_MODIFY.
    tray_dirty: bool,
}

impl State {
    fn look(&self) -> Look {
        let mut tip = format!("Deskflow Companion: {}", self.cfg.this_machine);
        if !self.cfg.auto_switch {
            tip += ", automatic switching off";
        }
        if self.cfg_error.is_some() {
            tip += "\nconfig error, see the log";
        }
        Look { paused: !self.cfg.auto_switch, tip }
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// `dfc enter` / `dfc leave`.
pub fn set_deskflow_active(active: bool) -> Result<(), String> {
    // SAFETY: plain handle calls; the handle is closed before returning.
    unsafe {
        let ev = OpenEventW(EVENT_MODIFY_STATE, false, EVENT_NAME)
            .map_err(|e| format!("daemon not running ({e})"))?;
        let r = if active { SetEvent(ev) } else { ResetEvent(ev) };
        let _ = CloseHandle(ev);
        r.map_err(|e| e.to_string())
    }
}

pub fn run() -> Result<(), String> {
    let (cfg, cfg_error) = match load() {
        Ok(c) => (c, None),
        Err(e) => {
            log!("config: {e}; using the defaults until it is fixed");
            (Config::default(), Some(e))
        }
    };
    let hotkey = hotkey::parse(&cfg.hotkey)?;
    // SAFETY: Win32 setup on this thread; the window procedure only runs inside GetMessageW below.
    unsafe {
        // Owned for the life of the process. An instance that is still exiting (install ends the
        // task, then starts it again) releases it within moments, so wait for that first.
        let mutex = CreateMutexW(None, false, MUTEX_NAME).map_err(|e| e.to_string())?;
        let w = WaitForSingleObject(mutex, MUTEX_WAIT_MS);
        if w != WAIT_OBJECT_0 && w != WAIT_ABANDONED {
            return Err("daemon already running".into());
        }
        let event = CreateEventW(None, true, false, EVENT_NAME).map_err(|e| e.to_string())?;
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: CLASS, ..Default::default() };
        if RegisterClassW(&wc) == 0 {
            return Err(format!("RegisterClassW: {}", windows::core::Error::from_thread()));
        }
        // Hidden top-level window, never shown: a message-only one gets no TaskbarCreated.
        // Receives raw input, the hotkey, the poll timer and the tray callbacks.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS,
            w!("Deskflow Companion"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW: {e}"))?;

        let dev = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
        let raw = RegisterRawInputDevices(&[dev], size_of::<RAWINPUTDEVICE>() as u32);
        let hk = register_hotkey(hwnd, hotkey);
        log!(
            "daemon started: this machine {} ({}), auto switch {}, hotkey {} {}, raw input {}",
            cfg.this_machine,
            describe(&cfg),
            if cfg.auto_switch { "on" } else { "off" },
            cfg.hotkey,
            hk.as_ref().map_or_else(|e| format!("not registered ({e}), retrying"), |_| "ok".into()),
            raw.map_or_else(|e| format!("FAILED ({e})"), |_| "ok".into())
        );

        let now = GetTickCount();
        let long_ago = now.wrapping_sub(100_000);
        STATE.with_borrow_mut(|s| {
            *s = Some(State {
                cfg,
                hwnd,
                hotkey,
                hotkey_ok: hk.is_ok(),
                hotkey_last_try: now,
                event,
                last_physical: long_ago,
                last_injected: long_ago,
                last_hmon: HMONITOR::default(),
                on_shared: None,
                cfg_stamp: config::stamp(),
                cfg_checked: now,
                cfg_error,
                tray_ok: false,
                tray_last_try: long_ago,
                tray_dirty: false,
            })
        });
        let icons = wintray::Icons::load();
        ICONS.with(|i| i.set(icons).ok());
        tray_sync(hwnd);
        SetTimer(Some(hwnd), 1, POLL_MS, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
    log!("daemon stopped");
    Ok(())
}

/// "EX2710S 0x11" for each shared monitor this machine has an input on.
fn describe(cfg: &Config) -> String {
    let v: Vec<String> = mine(cfg).map(|(_, m, code)| format!("{} {}", m.name, Vcp(code))).collect();
    v.join(", ")
}

/// The shared monitors this machine has an input on: (index, monitor, code).
fn mine(cfg: &Config) -> impl Iterator<Item = (usize, &config::SharedMonitor, u8)> {
    cfg.monitors.iter().enumerate().filter_map(|(i, m)| m.inputs.get(&cfg.this_machine).map(|v| (i, m, v.0)))
}

/// The config file, checked as far as the daemon needs it.
fn load() -> Result<Config, String> {
    Config::load().and_then(|c| c.validate().map(|()| c))
}

/// Asks a running daemon to quit (removing its tray icon); false when none runs.
pub fn close_running() -> bool {
    // SAFETY: plain window lookup and a posted message.
    unsafe {
        match FindWindowW(CLASS, PCWSTR::null()) {
            Ok(h) if !h.is_invalid() => PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok(),
            _ => false,
        }
    }
}

thread_local! {
    static ICONS: std::cell::OnceCell<wintray::Icons> = const { std::cell::OnceCell::new() };
}

/// Runs `f` on the state. None while it is borrowed already: a handler made a call that let
/// another message in (anything that sends to another thread can). That message is dropped.
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut s) => s.as_mut().map(f),
        Err(_) => {
            log!("re-entered while busy, message dropped");
            None
        }
    })
}

/// Adds the tray icon if it is missing (and due a retry), or updates it if its look changed.
/// Called without the state borrowed: Shell_NotifyIconW waits on Explorer.
fn tray_sync(hwnd: HWND) {
    // SAFETY: no preconditions.
    let now = unsafe { GetTickCount() };
    let Some((ok, due, dirty, look)) = with(|s| {
        let due = now.wrapping_sub(s.tray_last_try) >= TRAY_RETRY_MS;
        let r = (s.tray_ok, due, s.tray_dirty, s.look());
        if !s.tray_ok && due {
            s.tray_last_try = now;
        }
        s.tray_dirty = false;
        r
    }) else {
        return;
    };
    ICONS.with(|icons| {
        let Some(icons) = icons.get() else { return };
        if !ok && due {
            if wintray::add(hwnd, icons, &look) {
                log!("tray icon added");
                with(|s| s.tray_ok = true);
            }
        } else if ok && dirty && !wintray::modify(hwnd, icons, &look) {
            with(|s| s.tray_ok = false);
        }
    });
}

fn tray_menu(hwnd: HWND, x: i32, y: i32) {
    let Some(cfg) = with(|s| s.cfg.clone()) else { return };
    // Menu ids: 1.. for the fixed items, 100.. for switch commands (index into `switches`).
    const AUTO: usize = 1;
    const SETTINGS: usize = 2;
    const LOG: usize = 3;
    const EXIT: usize = 4;
    let mut switches: Vec<Vec<String>> = Vec::new();
    let mut switch_item = |name: &str, monitor: Option<&str>| {
        let label = if name == cfg.this_machine { format!("Switch to {name} (this machine)") } else { format!("Switch to {name}") };
        let mut args = vec!["switch".to_string(), name.to_string()];
        args.extend(monitor.map(String::from));
        switches.push(args);
        Item::Command(label, 99 + switches.len())
    };
    let mut items: Vec<Item> = cfg.machines().iter().map(|n| switch_item(n, None)).collect();
    if cfg.monitors.len() > 1 {
        for m in &cfg.monitors {
            let sub = m.inputs.keys().map(|n| switch_item(n, Some(&m.name))).collect();
            items.push(Item::Sub(m.name.clone(), sub));
        }
    }
    items.extend([
        Item::Separator,
        Item::Check("Switch automatically".into(), cfg.auto_switch, AUTO),
        Item::Default("Settings...".into(), SETTINGS),
        Item::Command("Open log".into(), LOG),
        Item::Separator,
        Item::Command("Exit".into(), EXIT),
    ]);
    let Some(id) = wintray::menu(hwnd, x, y, &items) else { return };
    match id {
        AUTO => match Config::update(|c| c.auto_switch = !cfg.auto_switch) {
            Ok(_) => {
                with(reload);
                tray_sync(hwnd);
            }
            Err(e) => log!("tray: {e}"),
        },
        SETTINGS => spawn_self(&["settings"]),
        LOG => {
            let path = windows::core::HSTRING::from(crate::log::path().as_os_str());
            // SAFETY: valid strings for the duration of the call.
            unsafe { ShellExecuteW(Some(hwnd), w!("open"), &path, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
        }
        EXIT => {
            log!("exit from the tray");
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        id if id >= 100 => {
            if let Some(args) = switches.get(id - 100) {
                spawn_self(&args.iter().map(String::as_str).collect::<Vec<_>>());
            }
        }
        _ => {}
    }
}

/// Runs dfc again (switching, settings) so the daemon never blocks on it.
fn spawn_self(args: &[&str]) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let r = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).args(args).creation_flags(CREATE_NO_WINDOW).spawn());
    if let Err(e) = r {
        log!("tray: dfc {}: {e}", args.join(" "));
    }
}

/// Takes the config file again if it changed. Bad files are logged once and leave the old one in use.
fn reload(s: &mut State) {
    s.cfg_stamp = config::stamp();
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            if s.cfg_error.as_ref() != Some(&e) {
                log!("config: {e}; keeping the previous one");
                s.cfg_error = Some(e);
                s.tray_dirty = true;
            }
            return;
        }
    };
    if cfg.hotkey != s.cfg.hotkey {
        // SAFETY: our window and hotkey id.
        let _ = unsafe { UnregisterHotKey(Some(s.hwnd), HOTKEY_ID) };
        s.hotkey = hotkey::parse(&cfg.hotkey).expect("validated");
        s.hotkey_ok = register_hotkey(s.hwnd, s.hotkey).is_ok();
        // SAFETY: no preconditions.
        s.hotkey_last_try = unsafe { GetTickCount() };
    }
    if cfg.monitors.iter().map(|m| &m.find).ne(s.cfg.monitors.iter().map(|m| &m.find)) {
        s.last_hmon = HMONITOR::default();
        s.on_shared = None;
    }
    log!(
        "config reloaded: this machine {} ({}), auto switch {}, hotkey {}{}",
        cfg.this_machine,
        describe(&cfg),
        if cfg.auto_switch { "on" } else { "off" },
        cfg.hotkey,
        if s.hotkey_ok { "" } else { " (not registered, retrying)" }
    );
    s.cfg = cfg;
    s.cfg_error = None;
    s.tray_dirty = true;
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            if let Some(physical) = raw_mouse_source(HRAWINPUT(lparam.0 as _)) {
                // SAFETY: no preconditions.
                let now = unsafe { GetTickCount() };
                with(|s| if physical { s.last_physical = now } else { s.last_injected = now });
            }
        }
        WM_TIMER => {
            with(poll);
            tray_sync(hwnd);
        }
        WM_HOTKEY => {
            with(on_hotkey);
        }
        wintray::WM_TRAY => {
            // NOTIFYICON_VERSION_4: the event in the low word of lParam, the anchor point in wParam.
            match (lparam.0 & 0xffff) as u32 {
                WM_CONTEXTMENU => tray_menu(hwnd, wparam.0 as i16 as i32, (wparam.0 >> 16) as i16 as i32),
                n if n == NIN_SELECT || n == NIN_SELECT | NINF_KEY => spawn_self(&["settings"]),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            wintray::delete(hwnd);
            // SAFETY: no preconditions.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with(|s| {
                s.tray_ok = false;
                s.tray_last_try = s.tray_last_try.wrapping_sub(TRAY_RETRY_MS);
            });
            tray_sync(hwnd);
        }
        _ => {}
    }
    // SAFETY: forwarding the original arguments (also required after WM_INPUT for cleanup).
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Some(true) for a real mouse, Some(false) for injected input, None for anything else.
fn raw_mouse_source(hraw: HRAWINPUT) -> Option<bool> {
    let mut hdr = RAWINPUTHEADER::default();
    let mut size = size_of::<RAWINPUTHEADER>() as u32;
    // SAFETY: the buffer is a RAWINPUTHEADER of the size given.
    let r = unsafe {
        GetRawInputData(hraw, RID_HEADER, Some(&mut hdr as *mut _ as *mut _), &mut size, size)
    };
    (r != u32::MAX && hdr.dwType == RIM_TYPEMOUSE.0).then(|| !hdr.hDevice.0.is_null())
}

fn register_hotkey(hwnd: HWND, (mods, vk): (u32, u32)) -> windows::core::Result<()> {
    // SAFETY: hwnd is our message-only window.
    unsafe { RegisterHotKey(Some(hwnd), HOTKEY_ID, HOT_KEY_MODIFIERS(mods), vk) }
}

fn poll(s: &mut State) {
    // SAFETY: no preconditions.
    let now = unsafe { GetTickCount() };
    if now.wrapping_sub(s.cfg_checked) >= CONFIG_CHECK_MS {
        s.cfg_checked = now;
        if config::stamp() != s.cfg_stamp {
            reload(s);
        }
    }
    if !s.hotkey_ok {
        if now.wrapping_sub(s.hotkey_last_try) >= HOTKEY_RETRY_MS {
            s.hotkey_last_try = now;
            s.hotkey_ok = register_hotkey(s.hwnd, s.hotkey).is_ok();
            if s.hotkey_ok {
                log!("hotkey {} registered", s.cfg.hotkey);
            }
        }
    }
    let mut p = POINT::default();
    // SAFETY: valid out pointer.
    if unsafe { GetCursorPos(&mut p) }.is_err() {
        return;
    }
    // SAFETY: no preconditions.
    let hmon = unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) };
    if hmon == s.last_hmon {
        return;
    }
    s.last_hmon = hmon;
    let on = mine(&s.cfg).find(|(_, m, _)| ddc::find(&m.find).is_some_and(|(h, _)| h == hmon)).map(|(i, _, _)| i);
    if let Some(i) = on.filter(|i| s.on_shared != Some(*i)) {
        entered(s, hmon, i);
    }
    s.on_shared = on;
}

fn entered(s: &mut State, hmon: HMONITOR, index: usize) {
    // SAFETY: no preconditions; the event handle lives as long as the process.
    let (now, event_set) = unsafe { (GetTickCount(), WaitForSingleObject(s.event, 0) == WAIT_OBJECT_0) };
    let physical_ago = now.wrapping_sub(s.last_physical);
    let injected_ago = now.wrapping_sub(s.last_injected);
    let src = format!(
        "(deskflow {}, physical {physical_ago} ms ago, injected {injected_ago} ms ago)",
        if event_set { "active" } else { "inactive" }
    );
    if event_set && (physical_ago as u64) >= s.cfg.physical_input_window_ms {
        if s.cfg.auto_switch {
            switch(s, hmon, index, &format!("entered from Deskflow {src}"));
        } else {
            log!("entered from Deskflow, automatic switching off {src}");
        }
    } else {
        log!("entered with the local mouse, no switch {src}");
    }
}

/// Takes the first shared monitor this machine has an input on.
fn on_hotkey(s: &mut State) {
    let Some((index, (hmon, r))) = mine(&s.cfg).next().and_then(|(i, m, _)| ddc::find(&m.find).map(|f| (i, f))) else {
        log!("hotkey: shared monitor not found");
        return;
    };
    // SAFETY: no preconditions.
    let _ = unsafe { SetCursorPos((r.left + r.right) / 2, (r.top + r.bottom) / 2) };
    s.last_hmon = hmon;
    s.on_shared = Some(index);
    if s.cfg.auto_switch {
        switch(s, hmon, index, "hotkey");
    } else {
        log!("hotkey: automatic switching off, cursor moved only");
    }
}

fn switch(s: &State, hmon: HMONITOR, index: usize, why: &str) {
    let m = &s.cfg.monitors[index];
    let to = Vcp(m.inputs[&s.cfg.this_machine].0);
    let why = if s.cfg.monitors.len() > 1 { format!("{why} [{}]", m.name) } else { why.to_string() };
    match ddc::switch_hmonitor(hmon, to.0) {
        Ok(Outcome::Already) => log!("{why}: already on {to}"),
        Ok(Outcome::Switched { from }) => {
            let from = from.map_or("unknown".to_string(), |c| Vcp(c).to_string());
            log!("{why}: {from} -> {to}");
        }
        Err(e) => log!("{why}: switch to {to} failed: {e}"),
    }
}
