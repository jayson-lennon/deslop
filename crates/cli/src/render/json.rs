//! JSON serializer with frozen field order (spec: render/json.rs).
//!
//! Hand-built to guarantee byte-stable ordering for golden snapshots;
//! `serde_json` object maps would not.

use std::io::Write;

use super::FiledFinding;

/// Escape a string per RFC 8259 with the minimal control set.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn quote(s: &str) -> String {
    format!("\"{}\"", json_escape(s))
}

fn col_of(src: &str, offset: usize) -> usize {
    super::line_col(src, offset).1
}

/// `KindTag` name as serialized in JSON.
fn kind_name(kind: deslop_core::finding::KindTag) -> &'static str {
    use deslop_core::finding::KindTag as K;
    match kind {
        K::Vocab => "vocab",
        K::Pattern => "pattern",
        K::LiteralBan => "literal-ban",
        K::Metric => "metric",
        K::Plugin => "plugin",
    }
}

/// Render all findings as a JSON array. One finding = one compact object,
/// wrapped in an array; trailing newline included.
///
/// # Errors
///
/// Fails when writing fails.
pub fn render_json(filed: &[FiledFinding<'_>], out: &mut dyn Write) -> std::io::Result<()> {
    writeln!(out, "[")?;
    let last_idx = filed.len().saturating_sub(1);
    for (i, f) in filed.iter().enumerate() {
        let tier = f.finding.tier as u8;
        let comma = if i == last_idx { "" } else { "," };
        // Frozen field order: rule_id, kind, tier, category, path, span
        // (byte + char-derived line/col), excerpt, message, advice, url.
        write!(
            out,
            "  {{\"rule_id\":{},\"kind\":{},\"tier\":{tier},\"category\":{},",
            quote(&f.finding.entry_id),
            quote(kind_name(f.finding.kind)),
            quote(&f.finding.category),
        )?;
        write!(
            out,
            "\"path\":{},\"span\":{{\"start\":{},\"end\":{},\"line\":{},\"col\":{}}},",
            quote(f.path),
            f.finding.span.start,
            f.finding.span.end,
            line_of(f.src, f.finding.span.start),
            col_of(f.src, f.finding.span.start),
        )?;
        write!(
            out,
            "\"excerpt\":{},\"message\":{},",
            quote(&f.finding.excerpt),
            quote(&f.finding.message),
        )?;
        write!(
            out,
            "\"replacement\":{},\"advice\":{}",
            match &f.finding.replacement {
                Some(r) => quote(r),
                None => "null".to_owned(),
            },
            match &f.finding.advice {
                Some(a) => quote(a),
                None => "null".to_owned(),
            }
        )?;
        if let Some((text, href)) = &f.finding.url {
            write!(
                out,
                ",\"url\":{{\"text\":{},\"href\":{}}}",
                quote(text),
                quote(href)
            )?;
        }
        // New field goes LAST (frozen-order contract covers only older
        // fields); omitted entirely when there is no context line.
        if let Some(context) = &f.finding.context {
            write!(out, ",\"context\":{}", quote(context))?;
        }
        writeln!(out, "}}{comma}")?;
    }
    writeln!(out, "]")
}

fn line_of(src: &str, offset: usize) -> usize {
    super::line_col(src, offset).0
}
