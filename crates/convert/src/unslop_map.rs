//! unslop converter: reads the byte-map switch out of main.zig.
//!
//! We parse the E2-80-XX switch arms rather than hardcoding them, so a
//! re-run against an updated snapshot picks up new substitutions.

/// One substitution rule: E2-80-b2 byte triple → ASCII replacement.
#[derive(Debug, Clone)]
pub struct ByteRule {
    pub prefix: [u8; 2],
    pub last: u8,
    pub replacement: String,
}

/// Extract inner switch arms like `0x9C, 0x9D => ... '"'` from main.zig.
/// The enclosing match pins `prefix = (0xE2, 0x80)`; each arm lists one or
/// more final bytes mapping to a quoted replacement string.
///
/// # Errors
///
/// Fails if the file cannot be read or no byte-map is found.
pub fn read(path: &std::path::Path) -> Result<Vec<ByteRule>, String> {
    const DEFAULT_PREFIX: [u8; 2] = [0xE2, 0x80];
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rules = Vec::new();
    for line in raw.lines() {
        let line = line.trim_start();
        if !(line.starts_with("0x") && line.contains("=>")) {
            continue;
        }
        let (lhs, rhs) = line.split_once("=>").unwrap_or(("", ""));
        let hexes: Vec<&str> = lhs
            .split(',')
            .map(str::trim)
            .filter(|t| t.starts_with("0x"))
            .collect();
        if hexes.is_empty() {
            continue;
        }
        let lasts: Option<Vec<u8>> = hexes
            .iter()
            .map(|h| u8::from_str_radix(&h[2..], 16).ok())
            .collect();
        let bytes = match lasts {
            Some(b) if !b.is_empty() => b,
            _ => continue,
        };
        let rep = match replacement_of(rhs) {
            Some(r) => r,
            None => continue,
        };
        for last in bytes {
            rules.push(ByteRule {
                prefix: DEFAULT_PREFIX,
                last,
                replacement: rep.clone(),
            });
        }
    }
    if rules.is_empty() {
        return Err(format!("{}: no substitution arms found", path.display()));
    }
    Ok(rules)
}

/// Replacement text for a switch arm. Two shapes occur:
/// `append(allocator, '"')` (zig char literal) and
/// `appendSlice(allocator, "...")` (multi-char string).
fn replacement_of(arm_body: &str) -> Option<String> {
    if arm_body.contains("appendSlice") {
        let q = arm_body.find('"')?;
        let rest = &arm_body[q + 1..];
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    // Char literal: first ' ... ' pair after "append(".
    let start = arm_body.find('\'')?;
    let rest = &arm_body[start + 1..];
    let end = rest.find('\'')?;
    let ch = &rest[..end];
    match ch {
        "\\\"" => Some("\"".into()),
        "\\\'" => Some("'".into()),
        one_char => Some(one_char.to_string()),
    }
}

/// Convenience: BTreeMap keyed by (prefix, final byte) for deterministic
/// iteration and unambiguous reconstruction of the transform.
pub type ByteIndex = std::collections::BTreeMap<([u8; 2], u8), String>;

#[must_use]
pub fn index(rules: &[ByteRule]) -> ByteIndex {
    rules
        .iter()
        .map(|r| ((r.prefix, r.last), r.replacement.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::read;
    #[test]
    fn extracts_all_five_arms() {
        let rules = read(std::path::Path::new(
            "../../third-party/unslop/src/main.zig",
        ))
        .expect("reads zig source");
        // Lines 311..314: double-quote pair, single-quote pair, dash pair,
        // ellipsis => 8 ByteRule rows (one per final byte).
        assert_eq!(rules.len(), 7, "{rules:?}");
    }
}
