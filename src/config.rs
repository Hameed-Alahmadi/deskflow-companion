// Config file: JSON, every field optional (missing fields take the defaults below).
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "RawConfig")]
pub struct Config {
    /// Name of this machine in the monitors' `inputs`; `dfc enter` switches to it.
    pub this_machine: String,
    /// Switch when Deskflow brings the cursor here (Linux `dfc enter`, Windows daemon and hotkey).
    /// Per machine; `dfc switch` and the tray's "Switch to" work either way.
    pub auto_switch: bool,
    /// The monitors connected to more than one machine. A monitor without an input for this
    /// machine is left alone here.
    pub monitors: Vec<SharedMonitor>,
    /// Windows daemon hotkey: centres the cursor on the first shared monitor and switches it.
    pub hotkey: String,
    /// Windows daemon: a real-device mouse event this recent means the local mouse moved the cursor.
    pub physical_input_window_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedMonitor {
    /// Label in menus and the log.
    pub name: String,
    #[serde(flatten)]
    pub find: MonitorMatch,
    /// VCP 0x60 input code per machine name.
    pub inputs: BTreeMap<String, Vcp>,
}

/// How to find a shared monitor: serial first, then model; Windows uses position only for displays
/// without EDID.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// "left" (left of the primary), "right" or "primary".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

/// The file as written: also takes the 0.1/0.2 single-monitor form (`monitor` + `inputs`).
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    this_machine: Option<String>,
    auto_switch: Option<bool>,
    monitors: Option<Vec<SharedMonitor>>,
    monitor: Option<MonitorMatch>,
    inputs: Option<BTreeMap<String, Vcp>>,
    hotkey: Option<String>,
    physical_input_window_ms: Option<u64>,
}

impl From<RawConfig> for Config {
    fn from(r: RawConfig) -> Self {
        let d = Config::default();
        let monitors = match (r.monitors, r.monitor, r.inputs) {
            (Some(m), _, _) => m,
            (None, None, None) => d.monitors,
            (None, find, inputs) => {
                let old = d.monitors.into_iter().next().unwrap();
                let find = find.unwrap_or(old.find);
                vec![SharedMonitor { name: default_name(&find, 0), find, inputs: inputs.unwrap_or(old.inputs) }]
            }
        };
        let monitors = monitors
            .into_iter()
            .enumerate()
            .map(|(i, mut m)| {
                if m.name.trim().is_empty() {
                    m.name = default_name(&m.find, i);
                }
                m
            })
            .collect();
        Config {
            this_machine: r.this_machine.unwrap_or(d.this_machine),
            auto_switch: r.auto_switch.unwrap_or(d.auto_switch),
            monitors,
            hotkey: r.hotkey.unwrap_or(d.hotkey),
            physical_input_window_ms: r.physical_input_window_ms.unwrap_or(d.physical_input_window_ms),
        }
    }
}

/// The model, or "Monitor N".
pub fn default_name(m: &MonitorMatch, index: usize) -> String {
    m.model.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| format!("Monitor {}", index + 1))
}

impl Default for Config {
    fn default() -> Self {
        let mut inputs = BTreeMap::new();
        inputs.insert("pc".into(), Vcp(0x0f));
        inputs.insert("laptop".into(), Vcp(0x11));
        Config {
            this_machine: if cfg!(windows) { "laptop" } else { "pc" }.into(),
            auto_switch: true,
            // No identity: the only DDC/CI display (Linux) or the only non-primary one (Windows).
            // `dfc detect` and the settings fill in serial and model.
            monitors: vec![SharedMonitor { name: "Monitor 1".into(), find: MonitorMatch::default(), inputs }],
            hotkey: "Ctrl+Alt+F13".into(),
            physical_input_window_ms: 300,
        }
    }
}

impl Config {
    /// Loads the config file, or the defaults when it does not exist.
    pub fn load() -> Result<Config, String> {
        let path = path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Checks what `load` can't: the machine name, the monitor names and (Windows) the hotkey.
    pub fn validate(&self) -> Result<(), String> {
        if self.monitors.is_empty() {
            return Err("no shared monitors".into());
        }
        if !self.monitors.iter().any(|m| m.inputs.contains_key(&self.this_machine)) {
            return Err(format!("this_machine '{}' is in no monitor's inputs", self.this_machine));
        }
        for (i, m) in self.monitors.iter().enumerate() {
            if self.monitors[..i].iter().any(|o| o.name == m.name) {
                return Err(format!("two shared monitors are called '{}'", m.name));
            }
        }
        #[cfg(windows)]
        crate::hotkey::parse(&self.hotkey)?;
        Ok(())
    }

    /// Every machine name of every monitor, sorted.
    pub fn machines(&self) -> Vec<String> {
        let mut v: Vec<String> = self.monitors.iter().flat_map(|m| m.inputs.keys().cloned()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The monitors that `target` (machine name, or with `monitor` given also a literal code) applies
    /// to, with the code for each.
    pub fn targets(&self, target: &str, monitor: Option<&str>) -> Result<Vec<(&SharedMonitor, u8)>, String> {
        let chosen: Vec<&SharedMonitor> = match monitor {
            Some(name) => vec![self.monitor(name)?],
            None => self.monitors.iter().collect(),
        };
        let found: Vec<(&SharedMonitor, u8)> =
            chosen.iter().filter_map(|m| m.inputs.get(target).map(|v| (*m, v.0))).collect();
        if !found.is_empty() {
            return Ok(found);
        }
        match (parse_code(target), monitor) {
            (Some(code), Some(_)) => Ok(vec![(chosen[0], code)]),
            (Some(code), None) if self.monitors.len() == 1 => Ok(vec![(&self.monitors[0], code)]),
            (Some(_), None) => Err("a code needs the monitor too: dfc switch 0x11 <monitor>".into()),
            (None, _) => Err(format!(
                "unknown machine '{target}' (known: {}; or give a code like 0x11)",
                self.machines().join(", ")
            )),
        }
    }

    #[cfg_attr(windows, allow(dead_code))]
    /// Adds a shared monitor found by `find`, with the first monitor's input codes as a starting
    /// point (they often differ: check them). Returns its name.
    pub fn add_monitor(&mut self, find: MonitorMatch) -> String {
        let base = default_name(&find, self.monitors.len());
        let name = (1..)
            .map(|n| if n == 1 { base.clone() } else { format!("{base} {n}") })
            .find(|n| !self.monitors.iter().any(|m| m.name == *n))
            .unwrap();
        let inputs = self.monitors.first().map(|m| m.inputs.clone()).unwrap_or_default();
        self.monitors.push(SharedMonitor { name: name.clone(), find, inputs });
        name
    }

    pub fn monitor(&self, name: &str) -> Result<&SharedMonitor, String> {
        self.monitors.iter().find(|m| m.name.eq_ignore_ascii_case(name)).ok_or_else(|| {
            let names: Vec<&str> = self.monitors.iter().map(|m| m.name.as_str()).collect();
            format!("unknown monitor '{name}' (shared monitors: {})", names.join(", "))
        })
    }

    /// Writes the config (temp file, then rename, so readers never see half a file).
    pub fn save(&self) -> Result<(), String> {
        let path = path();
        let err = |e: std::io::Error| format!("{}: {e}", path.display());
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(err)?;
        // An antivirus or indexer can hold the file for a moment.
        let mut tries = 0;
        loop {
            match std::fs::rename(&tmp, &path) {
                Ok(()) => return Ok(()),
                Err(_) if tries < 3 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => return Err(err(e)),
            }
        }
    }

    /// Re-reads the file, applies `f` and saves, so concurrent writers only change their own fields.
    pub fn update(f: impl FnOnce(&mut Config)) -> Result<Config, String> {
        let mut c = Config::load()?;
        f(&mut c);
        c.save()?;
        Ok(c)
    }
}

/// Size and modification time, to notice when someone else rewrote the file.
pub fn stamp() -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(path()).ok()?;
    Some((m.modified().ok()?, m.len()))
}

pub fn path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default())
            .join("DeskflowCompanion")
            .join("config.json")
    }
    #[cfg(not(windows))]
    {
        xdg_dir("XDG_CONFIG_HOME", ".config").join("deskflow-companion").join("config.json")
    }
}

#[cfg(not(windows))]
pub fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback),
    }
}

/// "0x11", "x11" or decimal "17".
pub fn parse_code(s: &str) -> Option<u8> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).or_else(|| s.strip_prefix('x')) {
        Some(hex) => u8::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

/// A VCP value, written as "0x0f" in the file; plain numbers are accepted too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vcp(pub u8);

impl fmt::Display for Vcp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "0x{:02x}", self.0)
    }
}

impl Serialize for Vcp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Vcp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(u8),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(n) => Ok(Vcp(n)),
            Raw::Str(s) => parse_code(&s)
                .map(Vcp)
                .ok_or_else(|| serde::de::Error::custom(format!("bad input code '{s}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_single_monitor_file() {
        let c: Config = serde_json::from_str(
            r#"{"this_machine": "pc", "monitor": {"model": "EX2710S"}, "inputs": {"pc": "0x0f", "mac": 18}}"#,
        )
        .unwrap();
        assert_eq!(c.monitors.len(), 1);
        assert_eq!(c.monitors[0].name, "EX2710S");
        assert_eq!(c.monitors[0].find.serial, None);
        assert_eq!(c.monitors[0].inputs["mac"], Vcp(0x12));
        assert_eq!(c.physical_input_window_ms, 300);
        assert!(c.auto_switch);
        // Written back in the new form, and read the same.
        let again: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(again.monitors, c.monitors);
    }

    #[test]
    fn several_monitors() {
        let c: Config = serde_json::from_str(
            r#"{"monitors": [
                {"model": "EX2710S", "inputs": {"pc": "0x0f", "laptop": "0x11"}},
                {"serial": "123", "inputs": {"pc": "0x12"}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(c.monitors[1].name, "Monitor 2");
        assert_eq!(c.machines(), ["laptop", "pc"]);
        let t = c.targets("pc", None).unwrap();
        assert_eq!(t.iter().map(|(m, code)| (m.name.as_str(), *code)).collect::<Vec<_>>(), [("EX2710S", 0x0f), ("Monitor 2", 0x12)]);
        assert_eq!(c.targets("laptop", None).unwrap().len(), 1);
        assert_eq!(c.targets("0x11", Some("monitor 2")).unwrap()[0].1, 0x11);
        assert!(c.targets("0x11", None).is_err());
        assert!(c.targets("tv", None).is_err());
        assert!(c.validate().is_ok());
        let mut dup = c.clone();
        dup.monitors[1].name = "EX2710S".into();
        assert!(dup.validate().is_err());
    }

    #[test]
    fn codes() {
        let c = Config::default();
        assert_eq!(c.targets("laptop", None).unwrap()[0].1, 0x11);
        assert_eq!(c.targets("0x0F", None).unwrap()[0].1, 0x0f);
        assert_eq!(c.targets("17", None).unwrap()[0].1, 17);
        assert!(c.targets("tv", None).is_err());
    }
}
