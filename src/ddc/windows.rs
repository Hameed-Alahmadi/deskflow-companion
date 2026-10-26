// Windows: dxva2 monitor configuration API. The shared monitor is found by its EDID serial or
// model (read from the registry), or by its position relative to the primary as a fallback.
use super::{INPUT_SOURCE, Monitor, Outcome};
use crate::config::{Config, MonitorMatch};
use crate::edid::{self, Edid};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR, SetVCPFeature,
};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

/// Extra input reads, 250 ms apart, before switching blind.
const READ_RETRIES: u32 = 6;

struct Display {
    hmon: HMONITOR,
    device: String,
    rect: RECT,
    primary: bool,
    edid: Option<Edid>,
}

pub fn detect(cfg: &Config) -> Result<Vec<Monitor>, String> {
    let displays = list_displays();
    let chosen: Vec<Option<HMONITOR>> = cfg.monitors.iter().map(|m| choose(&displays, &m.find).map(|d| d.hmon)).collect();
    Ok(displays
        .iter()
        .map(|d| {
            let r = d.rect;
            let (desc, current) = with_physical(d.hmon, |h, desc| (desc.to_string(), get_input(h)))
                .unwrap_or_else(|e| (e, None));
            let desc = match &d.edid {
                Some(e) => format!(
                    "{} {}, serial {}",
                    e.mfg,
                    e.model.as_deref().unwrap_or("?"),
                    e.serial.as_deref().unwrap_or("?")
                ),
                None => format!("{desc} (no EDID)"),
            };
            Monitor {
                id: d.device.clone(),
                description: format!(
                    "{desc}, {}x{} at ({}, {}){}",
                    r.right - r.left,
                    r.bottom - r.top,
                    r.left,
                    r.top,
                    if d.primary { ", primary" } else { "" }
                ),
                serial: d.edid.as_ref().and_then(|e| e.serial.clone()),
                model: d.edid.as_ref().and_then(|e| e.model.clone()),
                current,
                shared: chosen.iter().position(|h| *h == Some(d.hmon)),
            }
        })
        .collect())
}

pub fn switch(m: &MonitorMatch, code: u8) -> Result<Outcome, String> {
    let (hmon, _) = find(m).ok_or("monitor not found")?;
    switch_hmonitor(hmon, code)
}

/// The display `m` finds and its rectangle (virtual-screen pixels).
pub fn find(m: &MonitorMatch) -> Option<(HMONITOR, RECT)> {
    choose(&list_displays(), m).map(|d| (d.hmon, d.rect))
}

pub fn switch_hmonitor(hmon: HMONITOR, code: u8) -> Result<Outcome, String> {
    with_physical(hmon, |h, _| {
        // Right after an input change the monitor does not answer for a while; setting blind
        // would re-set an input it may already be on (a visible blank), so wait a bit first.
        let mut current = get_input(h);
        for _ in 0..READ_RETRIES {
            if current.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            current = get_input(h);
        }
        if current == Some(code) {
            return Ok(Outcome::Already);
        }
        // SAFETY: h is a valid physical monitor handle for the duration of the closure.
        if unsafe { SetVCPFeature(h, INPUT_SOURCE, code as u32) } == 0 {
            return Err(format!("SetVCPFeature failed: {}", windows::core::Error::from_thread()));
        }
        Ok(Outcome::Switched { from: current })
    })?
}

fn get_input(h: windows::Win32::Foundation::HANDLE) -> Option<u8> {
    let mut cur = 0u32;
    let mut max = 0u32;
    // SAFETY: valid handle and out pointers.
    let ok = unsafe { GetVCPFeatureAndVCPFeatureReply(h, INPUT_SOURCE, None, &mut cur, Some(&mut max)) };
    (ok != 0).then_some((cur & 0xff) as u8)
}

/// Runs `f` on the first physical monitor behind `hmon`, then releases the handles.
fn with_physical<T>(
    hmon: HMONITOR,
    f: impl FnOnce(windows::Win32::Foundation::HANDLE, &str) -> T,
) -> Result<T, String> {
    let mut n = 0u32;
    // SAFETY: hmon comes from EnumDisplayMonitors / MonitorFromPoint; arrays are sized by n.
    unsafe {
        GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut n).map_err(|e| format!("no DDC/CI ({e})"))?;
        if n == 0 {
            return Err("no physical monitor".into());
        }
        let mut arr = vec![PHYSICAL_MONITOR::default(); n as usize];
        GetPhysicalMonitorsFromHMONITOR(hmon, &mut arr).map_err(|e| format!("no DDC/CI ({e})"))?;
        // PHYSICAL_MONITOR is packed: copy the fields out instead of borrowing them.
        let (h, raw_desc) = (arr[0].hPhysicalMonitor, arr[0].szPhysicalMonitorDescription);
        let desc = String::from_utf16_lossy(&raw_desc);
        let r = f(h, desc.trim_end_matches('\0'));
        let _ = DestroyPhysicalMonitors(&arr);
        Ok(r)
    }
}

fn list_displays() -> Vec<Display> {
    unsafe extern "system" fn cb(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: data is the &mut Vec passed below; it outlives the enumeration.
        let list = unsafe { &mut *(data.0 as *mut Vec<Display>) };
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(hmon, &mut mi.monitorInfo) }.as_bool() {
            list.push(Display {
                hmon,
                device: wstr(&mi.szDevice),
                rect: mi.monitorInfo.rcMonitor,
                primary: mi.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
                edid: read_edid(&mi.szDevice),
            });
        }
        true.into()
    }
    let mut list: Vec<Display> = Vec::new();
    // SAFETY: the callback only touches `list`.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

/// EDID identity first; position ("left" = left of the primary, "right",
/// "primary") only among displays whose EDID could not be read, so an unplugged shared monitor
/// never makes another one switch. Without any match rule, the only non-primary display is taken.
fn choose<'a>(displays: &'a [Display], m: &MonitorMatch) -> Option<&'a Display> {
    let ids: Vec<(&str, &str)> = displays
        .iter()
        .map(|d| match &d.edid {
            Some(e) => (e.serial.as_deref().unwrap_or(""), e.model.as_deref().unwrap_or("")),
            None => ("", ""),
        })
        .collect();
    if let Some(i) = super::match_identity(&ids, m) {
        return Some(&displays[i]);
    }
    let by_identity = super::identity_configured(m);
    let mut candidates = displays.iter().filter(|d| !by_identity || d.edid.is_none());
    match m.position.as_deref() {
        Some("left") => candidates.find(|d| d.rect.left < 0),
        Some("right") => candidates.find(|d| !d.primary && d.rect.left > 0),
        Some("primary") => candidates.find(|d| d.primary),
        _ if by_identity => None,
        _ => {
            let mut others = candidates.filter(|d| !d.primary);
            match (others.next(), others.next()) {
                (Some(d), None) => Some(d),
                _ => None,
            }
        }
    }
}

/// EDID of the active monitor on a display device ("\\.\DISPLAY6"). The device interface name
/// "\\?\DISPLAY#BNQ7FA4#5&2b7b5f1&0&UID4353#{guid}" names its registry key under Enum\DISPLAY.
fn read_edid(device: &[u16; 32]) -> Option<Edid> {
    let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    let mut i = 0;
    loop {
        // SAFETY: device is NUL-terminated; dd.cb is set.
        if !unsafe { EnumDisplayDevicesW(PCWSTR(device.as_ptr()), i, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME) }.as_bool() {
            return None;
        }
        if dd.StateFlags.contains(DISPLAY_DEVICE_ACTIVE) {
            break;
        }
        i += 1;
    }
    let id = wstr(&dd.DeviceID);
    let parts: Vec<&str> = id.split('#').collect();
    if parts.len() < 3 {
        return None;
    }
    let key = HSTRING::from(format!(
        "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{}\\{}\\Device Parameters",
        parts[1], parts[2]
    ));
    let mut buf = [0u8; 512];
    let mut size = buf.len() as u32;
    // SAFETY: buf and size describe a writable buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return None;
    }
    edid::parse(&buf[..size as usize])
}

fn wstr(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}
