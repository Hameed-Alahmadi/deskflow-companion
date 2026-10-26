// Hotkey strings like "Ctrl+Alt+F13" -> RegisterHotKey modifiers and virtual-key code.
#![cfg_attr(not(windows), allow(dead_code))]

const MOD_ALT: u32 = 0x1;
const MOD_CONTROL: u32 = 0x2;
const MOD_SHIFT: u32 = 0x4;
const MOD_WIN: u32 = 0x8;
const MOD_NOREPEAT: u32 = 0x4000;

pub fn parse(s: &str) -> Result<(u32, u32), String> {
    let mut mods = MOD_NOREPEAT;
    let mut vk = None;
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" => mods |= MOD_WIN,
            k => {
                let code = key(k).ok_or_else(|| format!("hotkey '{s}': unknown key '{part}'"))?;
                if vk.replace(code).is_some() {
                    return Err(format!("hotkey '{s}': more than one key"));
                }
            }
        }
    }
    vk.map(|vk| (mods, vk)).ok_or_else(|| format!("hotkey '{s}': no key"))
}

/// F1-F24, A-Z, 0-9.
fn key(k: &str) -> Option<u32> {
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then_some(0x6f + n); // VK_F1 = 0x70
    }
    match k.as_bytes() {
        [c] if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase() as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn hotkeys() {
        assert_eq!(parse("Ctrl+Alt+F13"), Ok((0x4003, 0x7c)));
        assert_eq!(parse("win + shift + d"), Ok((0x400c, 'D' as u32)));
        assert!(parse("Ctrl+Alt").is_err());
        assert!(parse("Ctrl+F25").is_err());
        assert!(parse("A+B").is_err());
    }
}
