//! Lenient JSON loading for resource-pack files.
//!
//! Bedrock pack JSON often contains `//` and `/* */` comments and trailing
//! commas, which `serde_json` rejects. [`parse_lenient`] strips those first.

use serde_json::Value;

/// Remove comments and trailing commas (outside of strings).
pub fn strip_json(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            out.push(c as char);
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                out.push('"');
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b',' => {
                // Drop the comma if the next non-whitespace char closes a container.
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] as char).is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                    i += 1;
                } else {
                    out.push(',');
                    i += 1;
                }
            }
            _ => {
                // Copy a whole UTF-8 sequence.
                let len = utf8_len(c);
                out.push_str(
                    std::str::from_utf8(&bytes[i..(i + len).min(bytes.len())]).unwrap_or(""),
                );
                i += len;
            }
        }
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Parse JSON that may contain comments / trailing commas.
pub fn parse_lenient(src: &str) -> Result<Value, serde_json::Error> {
    let src = src.trim_start_matches('\u{feff}');
    match serde_json::from_str(src) {
        Ok(v) => Ok(v),
        Err(_) => serde_json::from_str(&strip_json(src)),
    }
}

/// Read and leniently parse a JSON file.
pub fn load_lenient(path: &std::path::Path) -> Result<Value, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_lenient(&s).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_trailing_commas() {
        let v = parse_lenient("// header\n{ \"a\": \"x//y\", /* c */ \"b\": [1,2,], }").unwrap();
        assert_eq!(v["a"], "x//y");
        assert_eq!(v["b"][1], 2);
    }
}
