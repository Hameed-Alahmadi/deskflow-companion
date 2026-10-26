// Windows tray icon of the daemon: icon, tooltip and the right-click menu. Every call here may
// send messages to Explorer (and TrackPopupMenu runs a modal loop), so callers must not hold the
// daemon's state borrow.
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{LIM_SMALL, LoadIconMetric};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, HICON, HMENU, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW,
    SetForegroundWindow, SetMenuDefaultItem, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_APP,
    WM_NULL,
};
use windows::core::{HSTRING, PCWSTR};

/// Callback message for tray events.
pub const WM_TRAY: u32 = WM_APP + 1;
const ID: u32 = 1;

pub struct Icons {
    active: HICON,
    paused: HICON,
}

impl Icons {
    /// Icons 1 and 2 from assets/dfc.rc, at the tray's size.
    pub fn load() -> Icons {
        // SAFETY: resource ids from our own module; a failed load leaves a null icon (blank tray slot).
        unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let load = |id: usize| LoadIconMetric(Some(inst.into()), PCWSTR(id as *const u16), LIM_SMALL).unwrap_or_default();
            Icons { active: load(1), paused: load(2) }
        }
    }
}

/// What the tray shows.
pub struct Look {
    pub paused: bool,
    pub tip: String,
}

fn data(hwnd: HWND, icons: &Icons, look: &Look) -> NOTIFYICONDATAW {
    let mut d = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: if look.paused { icons.paused } else { icons.active },
        ..Default::default()
    };
    let tip: Vec<u16> = look.tip.encode_utf16().take(d.szTip.len() - 1).collect();
    d.szTip[..tip.len()].copy_from_slice(&tip);
    d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    d
}

/// Adds the icon; fails while Explorer's tray is not up yet (early at logon).
pub fn add(hwnd: HWND, icons: &Icons, look: &Look) -> bool {
    let d = data(hwnd, icons, look);
    // SAFETY: d is a fully initialised NOTIFYICONDATAW.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &d); // left over after Explorer restarted
        Shell_NotifyIconW(NIM_ADD, &d).as_bool() && Shell_NotifyIconW(NIM_SETVERSION, &d).as_bool()
    }
}

pub fn modify(hwnd: HWND, icons: &Icons, look: &Look) -> bool {
    // SAFETY: as in add.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data(hwnd, icons, look)).as_bool() }
}

pub fn delete(hwnd: HWND) {
    let d = NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: ID, ..Default::default() };
    // SAFETY: as in add.
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
}

/// Menu entries; the numbers are the ids `menu` returns (non-zero).
pub enum Item {
    Command(String, usize),
    Check(String, bool, usize),
    /// Shown bold, like a double click target.
    Default(String, usize),
    Sub(String, Vec<Item>),
    Separator,
}

fn build(m: HMENU, items: &[Item]) {
    // SAFETY: m is a live menu; strings outlive each call.
    unsafe {
        for item in items {
            let (flags, text, id) = match item {
                Item::Command(t, id) | Item::Default(t, id) => (MF_STRING, HSTRING::from(t.as_str()), *id),
                Item::Check(t, on, id) => (if *on { MF_STRING | MF_CHECKED } else { MF_STRING }, HSTRING::from(t.as_str()), *id),
                Item::Sub(t, sub) => {
                    let Ok(s) = CreatePopupMenu() else { continue };
                    build(s, sub);
                    (MF_STRING | MF_POPUP, HSTRING::from(t.as_str()), s.0 as usize)
                }
                Item::Separator => (MF_SEPARATOR, HSTRING::new(), 0),
            };
            let _ = AppendMenuW(m, flags, id, if text.is_empty() { PCWSTR::null() } else { PCWSTR(text.as_ptr()) });
            if let Item::Default(_, id) = item {
                let _ = SetMenuDefaultItem(m, *id as u32, 0);
            }
        }
    }
}

/// Shows the menu at (x, y) and returns the id of the chosen item.
pub fn menu(hwnd: HWND, x: i32, y: i32, items: &[Item]) -> Option<usize> {
    // SAFETY: the menu is created, shown and destroyed here (submenus with it); hwnd is the daemon window.
    unsafe {
        let m = CreatePopupMenu().ok()?;
        build(m, items);
        // Without the foreground window the menu does not close when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(m, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY).0, x, y, hwnd, None).0;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        (cmd > 0).then_some(cmd as usize)
    }
}
