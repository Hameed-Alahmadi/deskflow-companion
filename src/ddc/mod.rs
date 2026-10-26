// DDC/CI access: find the shared monitor, read and set its input source (VCP 0x60).
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{detect, forget_bus, switch};
#[cfg(windows)]
pub use self::windows::{detect, find, switch, switch_hmonitor};

pub const INPUT_SOURCE: u8 = 0x60;

/// Index of the display the config names by identity: serial first, then model (case-insensitive
/// substring, so a replaced unit of the same model still matches). `ids` is (serial, model).
fn match_identity(ids: &[(&str, &str)], m: &crate::config::MonitorMatch) -> Option<usize> {
    if let Some(sn) = m.serial.as_deref().filter(|s| !s.is_empty()) {
        if let Some(i) = ids.iter().position(|(s, _)| *s == sn) {
            return Some(i);
        }
    }
    let model = m.model.as_deref().filter(|s| !s.is_empty())?.to_lowercase();
    ids.iter().position(|(_, md)| !md.is_empty() && md.to_lowercase().contains(&model))
}

/// Whether the config names the monitor by serial or model at all.
fn identity_configured(m: &crate::config::MonitorMatch) -> bool {
    m.serial.as_deref().is_some_and(|s| !s.is_empty()) || m.model.as_deref().is_some_and(|s| !s.is_empty())
}

/// One display as seen by `dfc detect`.
pub struct Monitor {
    /// Linux: i2c bus; Windows: display device name.
    pub id: String,
    pub description: String,
    /// EDID identity, for writing the config's monitor match.
    pub serial: Option<String>,
    pub model: Option<String>,
    pub current: Option<u8>,
    /// Index of the shared monitor (config `monitors`) this display is.
    pub shared: Option<usize>,
}

pub enum Outcome {
    /// Already on the target input: nothing sent (re-setting it blanks the screen).
    Already,
    Switched { from: Option<u8> },
}
