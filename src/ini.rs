// Minimal edits to Qt's INI format (Deskflow.conf) that keep every other line as it is.
// Values are handled raw (as written in the file); `escape` turns a plain string into Qt's form.

/// Raw value of `key` in `[section]`.
pub fn get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == format!("[{section}]");
        } else if in_section {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// Sets (Some) or removes (None) raw values in `[section]`; new keys go after the section's last
/// line, a missing section is appended. Line endings follow the file.
pub fn set(text: &str, section: &str, values: &[(&str, Option<&str>)]) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let header = format!("[{section}]");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let start = lines.iter().position(|l| l.trim() == header);
    let start = match start {
        Some(i) => i,
        None => {
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(header);
            lines.len() - 1
        }
    };
    for (key, value) in values {
        let end = (start + 1..lines.len()).find(|&i| lines[i].trim().starts_with('[')).unwrap_or(lines.len());
        let found = (start + 1..end).find(|&i| lines[i].split_once('=').is_some_and(|(k, _)| k.trim() == *key));
        match (found, value) {
            (Some(i), Some(v)) => lines[i] = format!("{key}={v}"),
            (Some(i), None) => {
                lines.remove(i);
            }
            (None, Some(v)) => {
                let mut at = end;
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.insert(at, format!("{key}={v}"));
            }
            (None, None) => {}
        }
    }
    let mut out = lines.join(nl);
    out.push_str(nl);
    out
}

/// Qt's string escaping: backslash and quote escaped, quoted when it holds list/comment characters
/// or leading/trailing spaces.
pub fn escape(v: &str) -> String {
    let e = v.replace('\\', "\\\\").replace('"', "\\\"");
    if v.contains([',', ';', '=', '#']) || v.starts_with(' ') || v.ends_with(' ') {
        format!("\"{e}\"")
    } else {
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: &str = include_str!("../tests/fixtures/deskflow-windows.conf");
    const LINUX: &str = include_str!("../tests/fixtures/deskflow-linux.conf");

    #[test]
    fn windows_conf() {
        let cmd = escape(r"C:\Users\User\AppData\Local\DeskflowCompanion\dfc.exe enter");
        assert_eq!(cmd, r"C:\\Users\\User\\AppData\\Local\\DeskflowCompanion\\dfc.exe enter");
        let old = get(WIN, "core", "enterCommand").unwrap();
        assert!(old.ends_with("flag.exe on"));
        let new = set(WIN, "core", &[("enterCommand", Some(&cmd)), ("enableExitCommand", Some("true"))]);
        assert_eq!(get(&new, "core", "enterCommand").as_deref(), Some(cmd.as_str()));
        assert_eq!(new.lines().count(), WIN.lines().count());
        // Restoring the old raw value gives back the original file.
        assert_eq!(set(&new, "core", &[("enterCommand", Some(&old))]), WIN);
    }

    #[test]
    fn add_and_remove() {
        let new = set(LINUX, "core", &[("newKey", Some("1"))]);
        assert_eq!(get(&new, "core", "newKey").as_deref(), Some("1"));
        // Inserted inside [core], before the blank line that ends it.
        let i = new.lines().position(|l| l == "newKey=1").unwrap();
        assert_eq!(new.lines().nth(i - 1), Some("lastVersion=1.27.0.0"));
        assert_eq!(set(&new, "core", &[("newKey", None)]), LINUX);
        let added = set("[gui]\nx=1\n", "core", &[("a", Some("b"))]);
        assert_eq!(added, "[gui]\nx=1\n\n[core]\na=b\n");
    }

    #[test]
    fn escaping() {
        assert_eq!(escape("a, b"), "\"a, b\"");
        assert_eq!(escape(r#"say "hi""#), r#"say \"hi\""#);
    }
}
