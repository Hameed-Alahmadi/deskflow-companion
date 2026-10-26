// `dfc settings` (Windows): a small window for the config, opened from the tray. Its own process,
// so its modal loops and DDC reads never block the daemon; Save rewrites only the fields shown
// here and the daemon picks the file up by itself.
use crate::config::{self, Config, MonitorMatch, SharedMonitor, Vcp};
use crate::ddc::{self, Monitor};
use std::cell::RefCell;
use std::collections::BTreeMap;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, HBRUSH, HFONT, HGDIOBJ, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL,
    CB_RESETCONTENT, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWN, CBS_DROPDOWNLIST, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_NUMBER, ES_WANTRETURN,
    FindWindowW, GetCursorPos, GetMessageW, GetWindowTextLengthW, GetWindowTextW, HMENU, IDCANCEL, IDOK,
    IsDialogMessageW, LoadIconW, MB_ICONWARNING, MB_OK, MSG, MessageBoxW, NONCLIENTMETRICSW, PostQuitMessage,
    RegisterClassW, SPI_GETNONCLIENTMETRICS, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SendMessageW, SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_SETFONT, WNDCLASSW, WS_CAPTION,
    WS_CHILD, WS_EX_CLIENTEDGE, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

const CLASS: PCWSTR = w!("DeskflowCompanionSettings");
const ID_SHARED: i32 = 18;
const ID_ADD: i32 = 19;
const ID_REMOVE: i32 = 20;
const ID_MONITOR: i32 = 10;
const ID_CURRENT: i32 = 11;
const ID_REFRESH: i32 = 12;
const ID_INPUTS: i32 = 13;
const ID_MACHINE: i32 = 14;
const ID_AUTO: i32 = 15;
const ID_HOTKEY: i32 = 16;
const ID_WINDOW_MS: i32 = 17;

/// Client area at 96 dpi.
const WIDTH: i32 = 480;
const HEIGHT: i32 = 364;

struct Control {
    hwnd: HWND,
    /// x, y, w, h at 96 dpi.
    rect: (i32, i32, i32, i32),
}

struct State {
    hwnd: HWND,
    controls: Vec<Control>,
    font: HFONT,
    cfg: Config,
    /// Detected displays.
    monitors: Vec<Monitor>,
    /// The shared monitors being edited, and which one the controls show.
    drafts: Vec<Draft>,
    cur: usize,
}

struct Draft {
    name: String,
    find: MonitorMatch,
    /// The inputs box text.
    inputs: String,
    /// Index in cfg.monitors, None for one added here.
    orig: Option<usize>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub fn run() -> Result<(), String> {
    // SAFETY: Win32 window setup and message loop on this thread.
    unsafe {
        if let Ok(h) = FindWindowW(CLASS, PCWSTR::null()) {
            if !h.is_invalid() {
                let _ = SetForegroundWindow(h);
                return Ok(());
            }
        }
        let cfg = Config::load()?;
        let monitors = ddc::detect(&cfg).unwrap_or_default();

        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as *const u16)).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as isize as *mut _),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("Deskflow Companion settings"),
            style,
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

        let mut controls = Vec::new();
        let mut add = |class: PCWSTR, text: &str, style: u32, ex: WINDOW_EX_STYLE, id: i32, rect: (i32, i32, i32, i32)| {
            let h = CreateWindowExW(
                ex,
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(instance.into()),
                None,
            )
            .unwrap_or_default();
            controls.push(Control { hwnd: h, rect });
        };
        let none = WINDOW_EX_STYLE::default();
        let label = |t: &'static str| (w!("STATIC"), t);
        let tab = WS_TABSTOP.0;
        let (c, t) = label("Shared monitor:");
        add(c, t, 0, none, 0, (12, 17, 150, 20));
        add(w!("COMBOBOX"), "", tab | CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0, none, ID_SHARED, (170, 14, 190, 200));
        add(w!("BUTTON"), "Add", tab | BS_PUSHBUTTON as u32, none, ID_ADD, (366, 13, 48, 26));
        add(w!("BUTTON"), "Remove", tab | BS_PUSHBUTTON as u32, none, ID_REMOVE, (418, 13, 50, 26));
        let (c, t) = label("Display:");
        add(c, t, 0, none, 0, (12, 49, 150, 20));
        add(w!("COMBOBOX"), "", tab | CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0, none, ID_MONITOR, (170, 46, 298, 200));
        let (c, t) = label("Its input now:");
        add(c, t, 0, none, 0, (12, 81, 150, 20));
        add(w!("STATIC"), "", 0, none, ID_CURRENT, (170, 81, 200, 20));
        add(w!("BUTTON"), "Refresh", tab | BS_PUSHBUTTON as u32, none, ID_REFRESH, (388, 76, 80, 26));
        let (c, t) = label("Inputs (name = code):");
        add(c, t, 0, none, 0, (12, 113, 150, 20));
        let multi = (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32 | WS_VSCROLL.0;
        add(w!("EDIT"), "", tab | multi, WS_EX_CLIENTEDGE, ID_INPUTS, (170, 110, 298, 72));
        let (c, t) = label("This machine:");
        add(c, t, 0, none, 0, (12, 199, 150, 20));
        add(w!("COMBOBOX"), "", tab | CBS_DROPDOWN as u32 | WS_VSCROLL.0, none, ID_MACHINE, (170, 196, 150, 200));
        add(w!("BUTTON"), "Switch automatically (Deskflow and hotkey)", tab | BS_AUTOCHECKBOX as u32, none, ID_AUTO, (170, 228, 298, 20));
        let (c, t) = label("Hotkey:");
        add(c, t, 0, none, 0, (12, 259, 150, 20));
        add(w!("EDIT"), "", tab | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE, ID_HOTKEY, (170, 256, 150, 24));
        let (c, t) = label("Local mouse window (ms):");
        add(c, t, 0, none, 0, (12, 291, 156, 20));
        add(w!("EDIT"), "", tab | (ES_AUTOHSCROLL | ES_NUMBER) as u32, WS_EX_CLIENTEDGE, ID_WINDOW_MS, (170, 288, 80, 24));
        add(w!("BUTTON"), "Save", tab | BS_DEFPUSHBUTTON as u32, none, IDOK.0, (296, 326, 82, 28));
        add(w!("BUTTON"), "Cancel", tab | BS_PUSHBUTTON as u32, none, IDCANCEL.0, (386, 326, 82, 28));

        let drafts = cfg
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| Draft { name: m.name.clone(), find: m.find.clone(), inputs: inputs_text(m), orig: Some(i) })
            .collect();
        STATE.with_borrow_mut(|s| {
            *s = Some(State { hwnd, controls, font: HFONT::default(), cfg, monitors, drafts, cur: 0 })
        });

        // Place the window on the monitor under the cursor, at that monitor's dpi.
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
        let work = mi.rcWork;
        let _ = SetWindowPos(hwnd, None, work.left, work.top, 100, 100, SWP_NOZORDER | SWP_NOACTIVATE);
        let dpi = GetDpiForWindow(hwnd);
        let (w, h) = window_size(dpi, style);
        let x = work.left + (work.right - work.left - w) / 2;
        let y = work.top + (work.bottom - work.top - h) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER);
        STATE.with_borrow_mut(|s| {
            let s = s.as_mut().unwrap();
            layout(s, dpi);
            fill(s);
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if let Some(c) = STATE.with_borrow(|s| s.as_ref().map(|s| control(s, ID_SHARED))) {
            let _ = SetFocus(Some(c));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    Ok(())
}

fn window_size(dpi: u32, style: WINDOW_STYLE) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: scale(WIDTH, dpi), bottom: scale(HEIGHT, dpi) };
    // SAFETY: valid rect pointer.
    let _ = unsafe { AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE::default(), dpi) };
    (r.right - r.left, r.bottom - r.top)
}

fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

/// Positions the controls and sets the message font for `dpi`.
fn layout(s: &mut State, dpi: u32) {
    // SAFETY: our own child windows; the old font is deleted after the controls stopped using it.
    unsafe {
        let mut ncm = NONCLIENTMETRICSW { cbSize: size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let _ = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0).0,
            dpi,
        );
        let font = CreateFontIndirectW(&ncm.lfMessageFont);
        for c in &s.controls {
            let (x, y, w, h) = c.rect;
            let _ = SetWindowPos(c.hwnd, None, scale(x, dpi), scale(y, dpi), scale(w, dpi), scale(h, dpi), SWP_NOZORDER | SWP_NOACTIVATE);
            SendMessageW(c.hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
        if !s.font.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(s.font.0));
        }
        s.font = font;
    }
}

fn control(s: &State, id: i32) -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::GetDlgItem;
    // SAFETY: our own window.
    unsafe { GetDlgItem(Some(s.hwnd), id).unwrap_or_default() }
}

fn set_text(h: HWND, text: &str) {
    // SAFETY: valid string for the call.
    let _ = unsafe { SetWindowTextW(h, &HSTRING::from(text)) };
}

fn text(h: HWND) -> String {
    // SAFETY: the buffer is as long as the text plus the terminator.
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn send(h: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: standard control messages with by-value arguments or strings that outlive the call.
    unsafe { SendMessageW(h, msg, Some(WPARAM(w)), Some(LPARAM(l))).0 }
}

fn combo_add(h: HWND, item: &str) {
    let s = HSTRING::from(item);
    send(h, CB_ADDSTRING, 0, s.as_ptr() as isize);
}

fn inputs_text(m: &SharedMonitor) -> String {
    m.inputs.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\r\n")
}

/// Puts the config and the detected monitors into the controls.
fn fill(s: &mut State) {
    fill_shared(s);
    load_draft(s, 0);
    let machine = control(s, ID_MACHINE);
    for k in s.cfg.machines() {
        combo_add(machine, &k);
    }
    set_text(machine, &s.cfg.this_machine);
    send(control(s, ID_AUTO), BM_SETCHECK, s.cfg.auto_switch as usize, 0);
    set_text(control(s, ID_HOTKEY), &s.cfg.hotkey);
    set_text(control(s, ID_WINDOW_MS), &s.cfg.physical_input_window_ms.to_string());
}

fn fill_shared(s: &State) {
    let combo = control(s, ID_SHARED);
    send(combo, CB_RESETCONTENT, 0, 0);
    for d in &s.drafts {
        combo_add(combo, &d.name);
    }
    send(combo, CB_SETCURSEL, s.cur, 0);
    // SAFETY: our own control.
    let _ = unsafe { EnableWindow(control(s, ID_REMOVE), s.drafts.len() > 1) };
}

/// The detected display a draft stands for.
fn display_of(s: &State, d: &Draft) -> Option<usize> {
    s.monitors.iter().position(|m| d.orig.is_some() && m.shared == d.orig).or_else(|| {
        let ided = d.find.serial.is_some() || d.find.model.is_some();
        s.monitors.iter().position(|m| ided && m.serial == d.find.serial && m.model == d.find.model)
    })
}

/// Shows draft `i` in the per-monitor controls.
fn load_draft(s: &mut State, i: usize) {
    s.cur = i;
    send(control(s, ID_SHARED), CB_SETCURSEL, i, 0);
    let combo = control(s, ID_MONITOR);
    send(combo, CB_RESETCONTENT, 0, 0);
    for m in &s.monitors {
        combo_add(combo, &m.description);
    }
    let sel = display_of(s, &s.drafts[i]).map_or(-1, |d| d as isize);
    send(combo, CB_SETCURSEL, sel as usize, 0);
    set_text(control(s, ID_INPUTS), &s.drafts[i].inputs);
    show_current(s);
}

/// Keeps what was typed for the current draft.
fn store_draft(s: &mut State) {
    let text = text(control(s, ID_INPUTS));
    s.drafts[s.cur].inputs = text;
}

fn selected_monitor(s: &State) -> Option<usize> {
    let i = send(control(s, ID_MONITOR), CB_GETCURSEL, 0, 0);
    (i >= 0 && (i as usize) < s.monitors.len()).then_some(i as usize)
}

fn show_current(s: &State) {
    let text = match selected_monitor(s).map(|i| s.monitors[i].current) {
        Some(Some(c)) => {
            let inputs = parse_inputs(&text(control(s, ID_INPUTS))).unwrap_or_default();
            let name = inputs.iter().find(|(_, v)| v.0 == c).map(|(k, _)| format!(" ({k})"));
            format!("{}{}", Vcp(c), name.unwrap_or_default())
        }
        Some(None) => "unreadable".into(),
        None => "display not found".into(),
    };
    set_text(control(s, ID_CURRENT), &text);
}

/// The chosen display becomes what finds the current shared monitor.
fn display_chosen(s: &mut State) {
    let Some(m) = selected_monitor(s).map(|i| &s.monitors[i]) else { return };
    let find = MonitorMatch { serial: m.serial.clone(), model: m.model.clone(), position: None };
    let cur = s.cur;
    if s.drafts[cur].find.serial != find.serial || s.drafts[cur].find.model != find.model {
        let name = unique_name(&s.drafts, cur, &config::default_name(&find, cur));
        let d = &mut s.drafts[cur];
        d.find = find;
        if d.orig.is_none() {
            d.name = name;
            fill_shared(s);
        }
    }
    show_current(s);
}

fn unique_name(drafts: &[Draft], skip: usize, base: &str) -> String {
    (1..)
        .map(|n| if n == 1 { base.to_string() } else { format!("{base} {n}") })
        .find(|n| !drafts.iter().enumerate().any(|(i, d)| i != skip && d.name == *n))
        .unwrap()
}

fn add_draft(s: &mut State) {
    store_draft(s);
    let n = s.drafts.len();
    let name = unique_name(&s.drafts, usize::MAX, &format!("Monitor {}", n + 1));
    // The first monitor's codes as a start: the same machines, often the same ports.
    let inputs = s.drafts[0].inputs.clone();
    s.drafts.push(Draft { name, find: MonitorMatch::default(), inputs, orig: None });
    s.cur = n;
    fill_shared(s);
    load_draft(s, n);
}

fn remove_draft(s: &mut State) {
    if s.drafts.len() > 1 {
        s.drafts.remove(s.cur);
        s.cur = 0;
        fill_shared(s);
        load_draft(s, 0);
    }
}

fn parse_inputs(text: &str) -> Result<BTreeMap<String, Vcp>, String> {
    let mut inputs = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (name, code) = line
            .split_once('=')
            .or_else(|| line.split_once(':'))
            .ok_or_else(|| format!("'{line}' is not 'name = code'"))?;
        let (name, code) = (name.trim(), code.trim());
        let code = config::parse_code(code).ok_or_else(|| format!("'{code}' is not a code like 0x11"))?;
        if name.is_empty() {
            return Err(format!("'{line}' has no name"));
        }
        inputs.insert(name.into(), Vcp(code));
    }
    Ok(inputs)
}

/// Reads the controls into a config, or says what is wrong.
fn collect(s: &mut State) -> Result<Config, String> {
    store_draft(s);
    let mut c = s.cfg.clone();
    c.monitors.clear();
    for d in &s.drafts {
        if d.find.serial.is_none() && d.find.model.is_none() && d.find.position.is_none() {
            return Err(format!("{}: choose its display (only displays with EDID can be told apart)", d.name));
        }
        let inputs = parse_inputs(&d.inputs).map_err(|e| format!("{} inputs: {e}", d.name))?;
        c.monitors.push(SharedMonitor { name: d.name.clone(), find: d.find.clone(), inputs });
    }
    c.this_machine = text(control(s, ID_MACHINE)).trim().to_string();
    c.auto_switch = send(control(s, ID_AUTO), BM_GETCHECK, 0, 0) == 1;
    c.hotkey = text(control(s, ID_HOTKEY)).trim().to_string();
    let ms = text(control(s, ID_WINDOW_MS));
    c.physical_input_window_ms = ms.trim().parse().map_err(|_| format!("local mouse window: '{ms}' is not a number"))?;
    c.validate()?;
    Ok(c)
}

fn save(s: &mut State) -> bool {
    let r = collect(s).and_then(|new| {
        Config::update(|c| {
            c.monitors = new.monitors;
            c.this_machine = new.this_machine;
            c.auto_switch = new.auto_switch;
            c.hotkey = new.hotkey;
            c.physical_input_window_ms = new.physical_input_window_ms;
        })
    });
    match r {
        Ok(_) => {
            crate::log!("settings saved");
            true
        }
        Err(e) => {
            // SAFETY: valid strings; the box is modal to our window.
            unsafe {
                MessageBoxW(Some(s.hwnd), &HSTRING::from(e), w!("Deskflow Companion"), MB_OK | MB_ICONWARNING);
            }
            false
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let code = (wparam.0 >> 16) as u32;
            // Take the state out while handling: MessageBox and detect pump messages.
            let Some(mut s) = STATE.with_borrow_mut(Option::take) else { return LRESULT(0) };
            let mut close = false;
            match id {
                ID_SHARED if code == CBN_SELCHANGE => {
                    store_draft(&mut s);
                    let i = send(control(&s, ID_SHARED), CB_GETCURSEL, 0, 0);
                    if i >= 0 && (i as usize) < s.drafts.len() {
                        load_draft(&mut s, i as usize);
                    }
                }
                ID_MONITOR if code == CBN_SELCHANGE => display_chosen(&mut s),
                ID_ADD => add_draft(&mut s),
                ID_REMOVE => remove_draft(&mut s),
                ID_REFRESH => {
                    store_draft(&mut s);
                    s.monitors = ddc::detect(&s.cfg).unwrap_or_default();
                    let cur = s.cur;
                    load_draft(&mut s, cur);
                }
                x if x == IDOK.0 => close = save(&mut s),
                x if x == IDCANCEL.0 => close = true,
                _ => {}
            }
            STATE.with_borrow_mut(|slot| *slot = Some(s));
            if close {
                // SAFETY: our own window.
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xffff) as u32;
            // SAFETY: lParam points to the suggested window rect for the new dpi.
            let r = unsafe { *(lparam.0 as *const RECT) };
            STATE.with_borrow_mut(|s| s.as_mut().map(|s| layout(s, dpi)));
            // SAFETY: our own window.
            let _ = unsafe {
                SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE)
            };
            return LRESULT(0);
        }
        WM_CLOSE => {
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(0);
        }
        WM_DESTROY => {
            // SAFETY: no preconditions.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwarding the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
