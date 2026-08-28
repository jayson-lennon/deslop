//! Message/advice template grammar.
//!
//! Templates interpolate with `{name}` placeholders, optionally carrying a
//! format spec after `:` (Rust-`format!`-style):
//!
//! - vocab: `{match}` = the matched form
//! - pattern: any NAMED capture from the regex, e.g. `{payload}`
//! - metric: `{value}` and `{per_words}`
//!
//! Format specs form a tiny closed set — `{value:.0%}` renders 0.44 as
//! `44%`. Unknown names AND unknown specs refuse the pack at load time
//! rather than leaking raw `{...}` into findings.

/// One `{name:spec}` reference extracted from a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    /// Variable name before the `:` (or the whole inner text).
    pub name: String,
    /// Format spec after the `:`, if present.
    pub spec: Option<String>,
}

/// Parse `name` plus optional `:spec` out of a placeholder body.
fn split_spec(body: &str) -> Placeholder {
    match body.split_once(':') {
        Some((name, spec)) => Placeholder {
            name: name.to_owned(),
            spec: Some(spec.to_owned()),
        },
        None => Placeholder {
            name: body.to_owned(),
            spec: None,
        },
    }
}

/// Extract referenced placeholders from a template.
pub fn placeholders(template: &str) -> Vec<String> {
    extract(template).into_iter().map(|p| p.name).collect()
}

/// Extract placeholders WITH their format specs.
fn extract(template: &str) -> Vec<Placeholder> {
    segments(template)
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Placeholder(placeholder) => Some(placeholder),
            Segment::Text(_) => None,
        })
        .collect()
}

/// One piece of a scanned template: literal text or a placeholder reference.
#[derive(Debug, PartialEq, Eq)]
enum Segment<'t> {
    /// Verbatim text — literal chars, escape pairs, or non-reference braces.
    Text(&'t str),
    /// A `{name:spec}` reference with a well-formed name.
    Placeholder(Placeholder),
}

/// Walk a template once, splitting it into literal and placeholder segments.
///
/// All slicing happens at char boundaries: the cursor comes from
/// `char_indices()` and placeholder extents come from `find('}')`, so
/// multibyte text can never be cut mid-char.
fn segments(template: &str) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment<'_>> = Vec::new();
    let mut run_start = 0; // start of the pending literal run
    let mut chars = template.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '{' && c != '}' {
            continue; // still part of the pending literal run
        }
        if run_start < i {
            out.push(Segment::Text(&template[run_start..i]));
        }
        if chars.peek().is_some_and(|&(_, n)| n == c) {
            // Doubled brace: escape pair rendering as a single literal brace.
            chars.next();
            out.push(Segment::Text(&template[i..i + 1]));
            run_start = i + 2;
        } else if c == '}' {
            // Lone closing brace is literal.
            out.push(Segment::Text(&template[i..i + 1]));
            run_start = i + 1;
        } else if let Some(close) = template[i + 1..].find('}') {
            let placeholder = split_spec(&template[i + 1..i + 1 + close]);
            if !placeholder.name.is_empty()
                && placeholder
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                out.push(Segment::Placeholder(placeholder));
            } else {
                // Not a reference — keep the raw braces visible.
                out.push(Segment::Text(&template[i..i + close + 2]));
            }
            chars.nth(close); // consume the body chars plus the closing brace
            run_start = i + close + 2;
        } else {
            // Unterminated placeholder: literal.
            out.push(Segment::Text(&template[i..i + 1]));
            run_start = i + 1;
        }
    }
    if run_start < template.len() {
        out.push(Segment::Text(&template[run_start..]));
    }
    out
}

/// Apply a format spec to an already-rendered scalar value.
///
/// Closed set: `None` passes through; `.0%`..`.3%` render a fraction as a
/// percentage; `.0`..`.2` control decimal places. Anything else is a pack
/// authoring error (rejected by [`validate_spec`]).
fn apply_spec(raw: &str, spec: Option<&str>) -> String {
    let Some(spec) = spec else {
        return raw.to_string();
    };
    match spec {
        ".0%" | ".1%" | ".2%" => {
            let places = spec.len() - 3; // digits after the dot
            match raw.parse::<f64>() {
                Ok(v) => format!("{:.*}%", places, v * 100.0),
                Err(_) => raw.to_string(),
            }
        }
        ".0" | ".1" | ".2" => match raw.parse::<f64>() {
            Ok(v) => format!("{:.*}", &spec[1..].parse::<usize>().unwrap_or(1), v),
            Err(_) => raw.to_string(),
        },
        _ => raw.to_string(),
    }
}

/// Specs the grammar accepts; unknown specs are load errors.
fn validate_spec(spec: Option<&str>) -> Result<(), String> {
    match spec {
        None | Some(".0%") | Some(".1%") | Some(".2%") | Some(".0") | Some(".1") | Some(".2") => {
            Ok(())
        }
        Some(other) => Err(format!(
            "unknown format spec `:{other}` (allowed: .0 .1 .2 .0% .1% .2%)"
        )),
    }
}

/// Validate a template against the names available in its context.
///
/// # Errors
///
/// Returns a message listing every unknown placeholder or format spec.
pub fn validate(template: &str, allowed: &[&str]) -> Result<(), String> {
    let found = extract(template);
    let unknown: Vec<&Placeholder> = found
        .iter()
        .filter(|p| !allowed.contains(&p.name.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown placeholder(s) {} - allowed: {}",
            unknown
                .iter()
                .map(|p| format!("{{{}}}", p.name))
                .collect::<Vec<_>>()
                .join(", "),
            allowed
                .iter()
                .map(|s| format!("{{{s}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for p in &found {
        validate_spec(p.spec.as_deref()).map_err(|e| format!("placeholder {{{}}}: {e}", p.name))?;
    }
    Ok(())
}

/// Render a template now that all values exist; unmatched placeholders render
/// as-is rather than vanishing (misconfiguration should be visible).
pub fn render(template: &str, values: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    for segment in segments(template) {
        match segment {
            Segment::Text(text) => out.push_str(text),
            Segment::Placeholder(p) => match values(&p.name) {
                Some(v) => out.push_str(&apply_spec(&v, p.spec.as_deref())),
                None => out.push_str(&format!("{{{}}}", p.name)),
            },
        }
    }
    out
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

    #[test]
    fn percent_spec_renders_fraction_as_percentage() {
        // Given a fraction value and a .0% spec.
        let lookup = |name: &str| (name == "value").then(|| "0.44".to_string());

        // When rendering.
        let out = render("{value:.0%} of quotes are curly", &lookup);

        // Then the fraction renders as a whole percentage.
        assert_eq!(out, "44% of quotes are curly");
    }

    #[test]
    fn decimal_spec_controls_precision() {
        // Given a long float and a .2 spec.
        let lookup = |name: &str| (name == "value").then(|| "3.14159".to_string());

        // When rendering.
        let out = render("cv is {value:.2}", &lookup);

        // Then only two decimals survive.
        assert_eq!(out, "cv is 3.14");
    }

    #[test]
    fn bare_value_keeps_scanner_formatting() {
        // Given a value already formatted by the scanner and no spec.
        let lookup = |name: &str| (name == "value").then(|| "5.0".to_string());

        // When rendering.
        let out = render("{value} distinct words", &lookup);

        // Then it passes through untouched.
        assert_eq!(out, "5.0 distinct words");
    }

    #[test]
    fn validate_rejects_unknown_format_spec() {
        // Given a spec outside the closed set.
        let result = validate("{value:+.3}", &["value"]);

        // Then validation names the spec.
        assert!(result.unwrap_err().contains(":+.3"));
    }

    #[test]
    fn validate_accepts_known_specs() {
        // Given every spec in the closed set.
        let template = "{value:.0%} {value:.1%} {value:.2%} {value:.0} {value:.1} {value:.2}";

        // When validating.
        let result = validate(template, &["value"]);

        // Then all pass.
        assert!(result.is_ok());
    }

    #[test]
    fn render_keeps_multibyte_text_around_placeholders() {
        // Given a template with CJK text and a curly quote around a placeholder.
        let lookup = |name: &str| (name == "match").then(|| "delve".to_string());

        // When rendering.
        let out = render("「{match}」水平线 — “{match}”", &lookup);

        // Then multibyte neighbors survive byte-for-byte around both hits.
        assert_eq!(out, "「delve」水平线 — “delve”");
    }

    #[test]
    fn placeholders_extract_after_multibyte_escape_pairs() {
        // Given an escape pair preceded by multibyte text and emoji.
        let names = placeholders("“引用”{{index}} 😀 {match}");

        // Then the escape pair stays a pair and the real ref is still found.
        assert_eq!(names, vec!["match"]);
    }
}
