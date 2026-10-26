// EDID base block: manufacturer, model name and serial (the same strings ddcutil prints).
#![cfg_attr(not(windows), allow(dead_code))]

pub struct Edid {
    /// Three-letter PNP id, e.g. "BNQ".
    pub mfg: String,
    pub model: Option<String>,
    pub serial: Option<String>,
}

pub fn parse(b: &[u8]) -> Option<Edid> {
    if b.len() < 128 || b[..8] != [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0] {
        return None;
    }
    let id = u16::from_be_bytes([b[8], b[9]]);
    let mfg = [10, 5, 0].iter().map(|s| (b'A' - 1 + ((id >> s) & 0x1f) as u8) as char).collect();
    let mut edid = Edid { mfg, model: None, serial: None };
    // Four 18-byte descriptors; display descriptors start with 00 00 00 <tag> 00.
    for d in b[54..126].chunks(18) {
        if d[..3] != [0, 0, 0] {
            continue;
        }
        let text: String = d[5..].iter().take_while(|&&c| c != b'\n').map(|&c| c as char).collect();
        let text = text.trim().to_string();
        match d[3] {
            0xff if !text.is_empty() => edid.serial = Some(text),
            0xfc if !text.is_empty() => edid.model = Some(text),
            _ => {}
        }
    }
    if edid.serial.is_none() {
        let n = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
        if n != 0 {
            edid.serial = Some(n.to_string());
        }
    }
    Some(edid)
}

#[cfg(test)]
mod tests {
    #[test]
    fn benq() {
        let e = super::parse(include_bytes!("../tests/fixtures/benq-ex2710s.edid")).unwrap();
        assert_eq!(e.mfg, "BNQ");
        assert_eq!(e.model.as_deref(), Some("BenQ EX2710S"));
        assert_eq!(e.serial.as_deref(), Some("AB000000001"));
        assert!(super::parse(&[0; 128]).is_none());
    }
}
