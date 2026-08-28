//! rustc-style rendering via codespan-reporting.
//!
//! Mapping (spec: render/human.rs): Tier1->Error, Tier2->Warning, Tier3->Note;
//! entry id as diagnostic code; message as primary label; advice as a
//! secondary `help:` label on the same span; url as a trailing note.
//!
//! The same funnel serves loader diagnostics ([`render_load_errors`]): those
//! point into the offending TOML file instead of a document.

use std::collections::BTreeMap;
use std::io::Write;

use codespan_reporting::diagnostic::{Diagnostic, Label, LabelStyle, Severity};
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term::emit;

use super::FiledFinding;
use deslop_core::config::ColorChoice;
use deslop_core::finding::Tier;

/// Tier -> codespan severity.
fn severity(tier: Tier) -> Severity {
    match tier {
        Tier::Artifact => Severity::Error,
        Tier::Tell => Severity::Warning,
        Tier::Density => Severity::Note,
    }
}

/// Build the diagnostic for one finding against a registered `file_id`.
///
/// A finding whose span is empty at byte 0 is DOCUMENT-level (whole-doc
/// metrics): rendered as a note with no span label, since pointing a caret
/// at whatever text opens the file reads as "this lint is about line 1".
pub fn diagnostic(f: &deslop_core::finding::Finding, file_id: usize) -> Diagnostic<usize> {
    if f.span.start == 0 && f.span.end == 0 {
        let mut d = Diagnostic::new(severity(f.tier))
            .with_code(f.entry_id.clone())
            .with_message(f.message.clone());
        if let Some(advice) = &f.advice {
            d = d.with_notes(vec![format!("help: {advice}")]);
        }
        if let Some((text, href)) = &f.url {
            d = d.with_notes(vec![format!("see: {text} - {href}")]);
        }
        return d;
    }
    let mut labels = vec![Label {
        style: LabelStyle::Primary,
        file_id,
        range: f.span.start..f.span.end,
        message: f.message.clone(),
    }];
    if let Some(advice) = &f.advice {
        labels.push(Label {
            style: LabelStyle::Secondary,
            file_id,
            range: f.span.start..f.span.end,
            message: format!("help: {advice}"),
        });
    }
    let notes = f
        .url
        .as_ref()
        .map(|(text, href)| vec![format!("see: {text} - {href}")])
        .unwrap_or_default();

    Diagnostic {
        severity: severity(f.tier),
        code: Some(f.entry_id.clone()),
        message: String::new(),
        labels,
        notes,
    }
}

/// Render findings to `out`, styling per `color`.
///
/// # Errors
///
/// Fails when writing fails.
pub fn render_human(
    filed: &[FiledFinding<'_>],
    color: ColorChoice,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    // The files DB owns its strings so lifetimes never leak into callers.
    let mut files = SimpleFiles::new();
    // Distinct path -> registered id (dedupe repeated documents).
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();

    let config = codespan_reporting::term::Config::default();
    let mut buffer = match color {
        ColorChoice::Always => codespan_reporting::term::termcolor::Buffer::ansi(),
        ColorChoice::Auto | ColorChoice::Never => {
            codespan_reporting::term::termcolor::Buffer::no_color()
        }
    };

    for finding in filed {
        let file_id = match ids.get(finding.path) {
            Some(id) => *id,
            None => {
                let id = files.add(finding.path.to_owned(), finding.src.to_owned());
                ids.insert(finding.path, id);
                id
            }
        };
        let d = diagnostic(finding.finding, file_id);
        emit(&mut buffer, &config, &files, &d).map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    out.write_all(buffer.as_slice())
}

/// Render rule-load errors against their TOML sources with the same
/// renderer. Missing files render with a path-only note instead of a span.
///
/// # Errors
///
/// Fails when writing fails.
pub fn render_load_errors(
    errors: &[deslop_core::rule::loader::LoadError],
    out: &mut dyn Write,
) -> std::io::Result<()> {
    let mut sources: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for path in errors.iter().map(|e| &e.path) {
        if !sources.contains_key(path.as_str()) {
            let src = std::fs::read_to_string(path).unwrap_or_default();
            sources.insert(path.clone(), src);
        }
    }

    let mut files = SimpleFiles::new();
    // path -> (file_id, per-line byte starts). One registration per path.
    let mut registered: BTreeMap<String, (usize, Vec<usize>)> = BTreeMap::new();
    for (path, src) in &sources {
        let id = files.add(path.as_str(), src.as_str());
        registered.insert(path.clone(), (id, line_starts(src)));
    }

    let mut diagnostics = Vec::new();
    for err in errors {
        let Some((file_id, starts)) = registered.get(&err.path) else {
            continue;
        };
        let (file_id, starts) = (*file_id, starts.clone());

        let label = err.line.and_then(|line| {
            let idx = line.saturating_sub(1);
            let start = *starts.get(idx)?;
            let end = starts.get(idx + 1).copied().unwrap_or(start);
            Some(start..end.saturating_sub(1).max(start))
        });
        // The file name must survive rendering even with no known line:
        // codespan prints the `-- path` header only when labels exist.
        let mut d = Diagnostic::error().with_message(format!("{}: {}", err.path, err.message));
        match label {
            Some(range) => {
                d = d.with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id,
                    range,
                    message: "while validating this rule".to_owned(),
                }]);
            }
            None => {
                d = d.with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id,
                    range: 0..0,
                    message: "while validating this rule".to_owned(),
                }]);
            }
        }
        diagnostics.push(d);
    }

    let config = codespan_reporting::term::Config::default();
    let mut buffer = codespan_reporting::term::termcolor::Buffer::no_color();
    for d in &diagnostics {
        emit(&mut buffer, &config, &files, d).map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    out.write_all(buffer.as_slice())
}

/// Byte offset where each 1-based line begins (starts[0] == 0).
fn line_starts(src: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            src.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_indexes_every_line() {
        // Given a three-line source.
        let src = "a\nbb\nccc\n";

        // When computing line starts.
        let starts = line_starts(src);

        // Then lines begin at their byte offsets, including the empty tail.
        assert_eq!(starts, vec![0, 2, 5, 9]);
    }
}
