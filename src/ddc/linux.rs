// Linux: shells out to ddcutil (works without root when the user can open /dev/i2c-*).
use super::{INPUT_SOURCE, Monitor, Outcome};
use crate::config::{Config, MonitorMatch, xdg_dir};
use std::path::PathBuf;
use std::process::Command;

const SLEEP_MULTIPLIER: &str = "0.3";

struct Display {
    bus: u32,
    connector: String,
    /// "MFG:model:serial" from ddcutil.
    mfg: String,
    model: String,
    serial: String,
}

pub fn detect(cfg: &Config) -> Result<Vec<Monitor>, String> {
    let displays = list_displays()?;
    let chosen: Vec<Option<u32>> = cfg.monitors.iter().map(|m| choose(&displays, &m.find).map(|d| d.bus)).collect();
    Ok(displays
        .iter()
        .map(|d| Monitor {
            id: format!("/dev/i2c-{} ({})", d.bus, d.connector),
            description: format!("{} {}, serial {}", d.mfg, d.model, d.serial),
            serial: Some(d.serial.clone()).filter(|s| !s.is_empty()),
            model: Some(d.model.clone()).filter(|s| !s.is_empty()),
            current: get_input(d.bus).ok(),
            shared: chosen.iter().position(|b| *b == Some(d.bus)),
        })
        .collect())
}

pub fn switch(m: &MonitorMatch, code: u8) -> Result<Outcome, String> {
    // The bus number can change between boots: use the cached one, re-detect when it fails.
    let (bus, current) = match cached_bus(m).and_then(|b| get_input(b).ok().map(|c| (b, c))) {
        Some(found) => found,
        None => {
            let displays = list_displays()?;
            let d = choose(&displays, m).ok_or("monitor not found")?;
            save_bus(m, d.bus);
            (d.bus, get_input(d.bus)?)
        }
    };
    if current == code {
        return Ok(Outcome::Already);
    }
    let out = ddcutil(&[
        "--bus",
        &bus.to_string(),
        "--noverify",
        "setvcp",
        &format!("{INPUT_SOURCE:x}"),
        &format!("0x{code:02x}"),
    ])?;
    if !out.status.success() {
        return Err(format!("ddcutil setvcp failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(Outcome::Switched { from: Some(current) })
}

fn ddcutil(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("ddcutil")
        .args(["--sleep-multiplier", SLEEP_MULTIPLIER])
        .args(args)
        .output()
        .map_err(|e| format!("cannot run ddcutil: {e}"))
}

fn get_input(bus: u32) -> Result<u8, String> {
    let out = ddcutil(&["--bus", &bus.to_string(), "--brief", "getvcp", &format!("{INPUT_SOURCE:x}")])?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_getvcp(&text).ok_or_else(|| format!("no input reading on bus {bus}: {}", text.trim()))
}

/// "VCP 60 SNC x0f" -> 0x0f
fn parse_getvcp(text: &str) -> Option<u8> {
    let f: Vec<&str> = text.split_whitespace().collect();
    match f.as_slice() {
        ["VCP", _, "SNC", v, ..] => u8::from_str_radix(v.trim_start_matches('x'), 16).ok(),
        _ => None,
    }
}

fn list_displays() -> Result<Vec<Display>, String> {
    let out = ddcutil(&["detect", "--brief"])?;
    Ok(parse_detect(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_detect(text: &str) -> Vec<Display> {
    let mut list = Vec::new();
    let mut cur: Option<Display> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("Display ") || line.starts_with("Invalid display") {
            list.extend(cur.take());
            if line.starts_with("Display ") {
                cur = Some(Display {
                    bus: 0,
                    connector: String::new(),
                    mfg: String::new(),
                    model: String::new(),
                    serial: String::new(),
                });
            }
        } else if let (Some(d), Some((key, val))) = (cur.as_mut(), line.split_once(':')) {
            let val = val.trim();
            match key {
                "I2C bus" => d.bus = val.trim_start_matches("/dev/i2c-").parse().unwrap_or(0),
                "DRM connector" => d.connector = val.to_string(),
                "Monitor" => {
                    let mut parts = val.splitn(3, ':');
                    d.mfg = parts.next().unwrap_or("").to_string();
                    d.model = parts.next().unwrap_or("").to_string();
                    d.serial = parts.next().unwrap_or("").to_string();
                }
                _ => {}
            }
        }
    }
    list.extend(cur);
    list
}

/// Serial match first, then model; a single display is taken when neither is configured.
fn choose<'a>(displays: &'a [Display], m: &MonitorMatch) -> Option<&'a Display> {
    let ids: Vec<(&str, &str)> = displays.iter().map(|d| (d.serial.as_str(), d.model.as_str())).collect();
    match super::match_identity(&ids, m) {
        Some(i) => Some(&displays[i]),
        None if !super::identity_configured(m) && displays.len() == 1 => displays.first(),
        None => None,
    }
}

fn bus_dir() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache").join("deskflow-companion")
}

/// One cache file per monitor, named after what finds it.
fn bus_file(m: &MonitorMatch) -> PathBuf {
    let key = m.serial.as_deref().or(m.model.as_deref()).unwrap_or("any");
    let key: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    bus_dir().join(format!("bus-{key}"))
}

/// Drops the cached buses (the shared monitors changed).
pub fn forget_bus() {
    if let Ok(dir) = std::fs::read_dir(bus_dir()) {
        for e in dir.flatten() {
            if e.file_name().to_string_lossy().starts_with("bus") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

fn cached_bus(m: &MonitorMatch) -> Option<u32> {
    std::fs::read_to_string(bus_file(m)).ok()?.trim().parse().ok()
}

fn save_bus(m: &MonitorMatch, bus: u32) {
    let _ = std::fs::create_dir_all(bus_dir());
    let _ = std::fs::write(bus_file(m), bus.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETECT: &str = "Display 1\n   I2C bus:          /dev/i2c-4\n   DRM connector:    card1-DP-2\n   \
        drm_connector_id: 837\n   Monitor:          BNQ:BenQ EX2710S:AB000000001\n\n\
        Invalid display\n   I2C bus:          /dev/i2c-7\n";

    #[test]
    fn parses_detect() {
        let d = parse_detect(DETECT);
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].bus, d[0].model.as_str(), d[0].serial.as_str()), (4, "BenQ EX2710S", "AB000000001"));
        let by_model = MonitorMatch { model: Some("ex2710".into()), ..Default::default() };
        assert!(choose(&d, &by_model).is_some());
        let wrong = MonitorMatch { serial: Some("nope".into()), ..Default::default() };
        assert!(choose(&d, &wrong).is_none());
    }

    #[test]
    fn parses_getvcp() {
        assert_eq!(parse_getvcp("VCP 60 SNC x0f\n"), Some(0x0f));
        assert_eq!(parse_getvcp("VCP 60 ERR\n"), None);
    }
}
