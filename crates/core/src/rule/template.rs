//! Message/advice template grammar.
//!
//! Templates interpolate with `{name}` placeholders:
//!
//! - vocab: `{match}` = the matched form
//! - pattern: any NAMED capture from the regex, e.g. `{payload}`
//! - metric: `{value}` and `{per_words}`
//!
//! Literal braces are doubled: `{{` renders as `{`, `}}` as `}`.
//!
//! The validator runs at load time so a typo'd placeholder refuses the pack
//! instead of emitting malformed findings.

/// Extract referenced placeholder names from a template.
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes.get(i + 1) == Some(&b'{') => i += 2,
            b'}' if bytes.get(i + 1) == Some(&b'}') => i += 2,
            b'{' => {
                if let Some(close) = template[i + 1..].find('}') {
                    let name = &template[i + 1..i + 1 + close];
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        out.push(name.to_owned());
                    }
                    i += close + 2;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Validate a template against the names available in its context.
///
/// # Errors
///
/// Returns a message listing every unknown placeholder.
pub fn validate(template: &str, allowed: &[&str]) -> Result<(), String> {
    let names = placeholders(template);
    let unknown: Vec<&String> = names
        .iter()
        .filter(|n| !allowed.contains(&n.as_str()))
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unknown placeholder(s) {} — allowed: {}",
            unknown
                .iter()
                .map(|s| format!("{{{s}}}"))
                .collect::<Vec<_>>()
                .join(", "),
            allowed
                .iter()
                .map(|s| format!("{{{s}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Render a template now that all values exist; unmatched placeholders render
/// as-is rather than vanishing (misconfiguration should be visible).
pub fn render(template: &str, values: &dyn Fn(&str) -> Option<String>) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                out.push('{');
                i += 2;
            }
            b'}' if bytes.get(i + 1) == Some(&b'}') => {
                out.push('}');
                i += 2;
            }
            b'{' => {
                if let Some(close) = template[i + 1..].find('}') {
                    let name = &template[i + 1..i + 1 + close];
                    match values(name) {
                        Some(v) => out.push_str(&v),
                        None => out.push_str(&format!("{{{name}}}")),
                    }
                    i += close + 2;
                } else {
                    out.push('{');
                    i += 1;
                }
            }
            b => {
                // Advance by the full UTF-8 char width.
                let width = utf8_width(b);
                out.push_str(&template[i..i + width.min(template.len() - i)]);
                i += width;
            }
        }
    }
    out
}

fn utf8_width(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_extracts_named_references() {
        // Given a template mixing known placeholder kinds.
        let names = placeholders("roughly \"{payload}\" over {match} and {value}");

        // Then all three names surface.
        assert_eq!(names, vec!["payload", "match", "value"]);
    }

    #[test]
    fn doubled_braces_are_not_placeholders() {
        // Given literal-brace escapes.
        let names = placeholders("cite like {{index}} stays");

        // Then nothing is extracted ({{ is an escape, not a ref).
        assert!(names.is_empty());
    }

    #[test]
    fn validate_rejects_unknown_capture_names() {
        // Given a template naming a capture the regex does not define.
        let result = validate("echo {nonexistent}", &["payload"]);

        // Then validation fails naming the offender.
        assert!(result.unwrap_err().contains("{nonexistent}"));
    }

    #[test]
    fn render_substitutes_and_keeps_literal_braces() {
        // Given values for one name and doubled braces elsewhere.
        let out = render("say {match} with {{index=1}} kept", &|name| {
            (name == "match").then(|| "delve".to_string())
        });

        // Then substitution happens and braces survive as literals.
        assert_eq!(out, "say delve with {index=1} kept");
    }
}
