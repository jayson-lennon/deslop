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
use codespan_reporting::term::termcolor::{Buffer, Color, ColorSpec, WriteColor};

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
    diagnostic_in_range(f, file_id, f.span.start..f.span.end)
}

/// Like [`diagnostic`], but the labels point at `range` instead of the
/// finding's own span — the truncating path passes the span remapped into
/// the truncated copy's coordinates.
fn diagnostic_in_range(
    f: &deslop_core::finding::Finding,
    file_id: usize,
    range: std::ops::Range<usize>,
) -> Diagnostic<usize> {
    if f.span.start == 0 && f.span.end == 0 {
        let mut d = Diagnostic::new(severity(f.tier))
            .with_code(f.entry_id.clone())
            .with_message(f.message.clone());
        if let Some(advice) = &f.advice {
            d = d.with_notes(vec![format!("help: {advice}")]);
        }
        if let Some(context) = &f.context {
            d = d.with_notes(vec![context.clone()]);
        }
        if let Some((text, href)) = &f.url {
            d = d.with_notes(vec![format!("see: {text} - {href}")]);
        }
        return d;
    }
    let mut labels = vec![Label {
        style: LabelStyle::Primary,
        file_id,
        range: range.clone(),
        message: f.message.clone(),
    }];
    if let Some(advice) = &f.advice {
        labels.push(Label {
            style: LabelStyle::Secondary,
            file_id,
            range: range.clone(),
            message: format!("help: {advice}"),
        });
    }
    let mut notes: Vec<String> = Vec::new();
    if let Some(context) = &f.context {
        notes.push(context.clone());
    }
    if let Some((text, href)) = &f.url {
        notes.push(format!("see: {text} - {href}"));
    }

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
/// `width` bounds source-line excerpts in cells; `0` disables truncation
/// (byte-identical to the historical output). Only stdout-TTY runs and
/// explicit `--width` runs pass a nonzero value.
///
/// # Errors
///
/// Fails when writing fails.
pub fn render_human(
    filed: &[FiledFinding<'_>],
    color: ColorChoice,
    width: usize,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    match width {
        0 => render_human_untruncated(filed, color, out),
        _ => render_human_truncated(filed, color, width, out),
    }
}

/// The historical renderer: one file registration per path, spans against
/// the raw source. `--width 0` and every piped run land here.
///
/// # Errors
///
/// Fails when writing fails.
fn render_human_untruncated(
    filed: &[FiledFinding<'_>],
    color: ColorChoice,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    // The files DB owns its strings so lifetimes never leak into callers.
    let mut files = SimpleFiles::new();
    // Distinct path -> registered id (dedupe repeated documents).
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();

    let config = codespan_reporting::term::Config::default();
    let mut buffer = buffer_for(color);

    for finding in filed {
        let file_id = match ids.get(finding.path) {
            Some(id) => *id,
            None => {
                let id = files.add(finding.path.to_owned(), finding.src.to_owned());
                ids.insert(finding.path, id);
                id
            }
        };
        // Window-spanned (anchorless) findings bypass codespan's diagnostic
        // emitter: a snippet cannot be emitted without a mark, and a caret
        // across a whole window reads as noise. Hand-rolled gutter block
        // instead, styled through the same color-aware buffer.
        if finding.finding.anchorless {
            render_anchorless(
                finding.finding,
                finding.path,
                finding.src,
                &mut buffer,
                None,
            )?;
            continue;
        }
        let d = diagnostic(finding.finding, file_id);
        emit(&mut buffer, &config, &files, &d).map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    out.write_all(buffer.as_slice())
}

/// The truncating renderer: per finding, the source is rebuilt through
/// [`super::truncate::truncate_source`] with a window anchored on the finding's
/// span, the span bounds are remapped into the copy's coordinates, and the
/// copy is registered as its own file. Codespan's mark columns are label
/// ranges minus line ranges, so text and ranges MUST move together.
///
/// # Errors
///
/// Fails when writing fails.
fn render_human_truncated(
    filed: &[FiledFinding<'_>],
    color: ColorChoice,
    width: usize,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    let mut files = SimpleFiles::new();
    let config = codespan_reporting::term::Config::default();
    let mut buffer = buffer_for(color);

    for finding in filed {
        if finding.finding.anchorless {
            let budget = anchorless_budget(width, finding.src, finding.finding);
            render_anchorless(
                finding.finding,
                finding.path,
                finding.src,
                &mut buffer,
                Some(budget),
            )?;
            continue;
        }
        // Document-level findings carry no span label; there is nothing to
        // anchor a window on, so they render against the raw source.
        if finding.finding.span.start == 0 && finding.finding.span.end == 0 {
            let file_id = files.add(finding.path.to_owned(), finding.src.to_owned());
            let d = diagnostic(finding.finding, file_id);
            emit(&mut buffer, &config, &files, &d)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            continue;
        }
        let truncated = truncate_finding_src(finding.finding, finding.src, width);
        let file_id = files.add(finding.path.to_owned(), truncated.copy);
        let d = diagnostic_in_range(finding.finding, file_id, truncated.start..truncated.end);
        emit(&mut buffer, &config, &files, &d).map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    out.write_all(buffer.as_slice())
}

/// Buffer styled per color choice, shared by both renderers.
fn buffer_for(color: ColorChoice) -> Buffer {
    match color {
        ColorChoice::Always => Buffer::ansi(),
        ColorChoice::Auto | ColorChoice::Never => Buffer::no_color(),
    }
}

/// Build the truncated source and remapped span for one spanned finding.
///
/// The budget subtracts the outer gutter (`line number + ` │ ``) plus two
/// cells per multi-line label column (codespan's `╭ ` inner gutter, one
/// column when the span crosses a line break and a second when an advice
/// label sits beside it).
fn truncate_finding_src(
    f: &deslop_core::finding::Finding,
    src: &str,
    width: usize,
) -> super::truncate::Truncated {
    let multiline = src[f.span.start..f.span.end].contains('\n');
    let advice_labels = usize::from(f.advice.is_some());
    let multi_labels = usize::from(multiline) * (1 + advice_labels);
    let start_line = super::line_col(src, f.span.start).0;
    let end_line = super::line_col(src, f.span.end.saturating_sub(1).max(f.span.start)).0;
    let line_digits = start_line.to_string().len().max(end_line.to_string().len());
    let budget = super::truncate::budget_spanned(width, line_digits, multi_labels);
    super::truncate::truncate_source(src, f.span.start, f.span.end, budget)
}

/// Cell budget for anchorless excerpt lines: gutter is number + ` │ ` (+ the
/// anchorless block pads to the widest printed line number).
fn anchorless_budget(width: usize, src: &str, f: &deslop_core::finding::Finding) -> usize {
    let (line, _) = super::line_col(src, f.span.start);
    let shown_lines = src[f.span.start..f.span.end].lines().count().min(2);
    let digits = (line + shown_lines.saturating_sub(1)).to_string().len();
    super::truncate::budget_anchorless(width, digits)
}

/// Severity header style: bold intense severity color, as codespan.
fn header_style(fg: Color) -> ColorSpec {
    let mut style = ColorSpec::new();
    style.set_bold(true).set_intense(true).set_fg(Some(fg));
    style
}

/// Gutter/line-number/note-bullet style: plain blue, as codespan.
fn blue_style() -> ColorSpec {
    let mut style = ColorSpec::new();
    style.set_fg(Some(Color::Blue));
    style
}

/// Message-tail style on the severity header line: plain bold.
fn bold_style() -> ColorSpec {
    let mut style = ColorSpec::new();
    style.set_bold(true);
    style
}

/// Render one window-spanned (anchorless) finding: the same shapes codespan
/// uses (severity header, `┌─ path:line:col`, `│` gutter, numbered source
/// lines, ` = ` notes) but NO caret/underline marks - the span covers a
/// whole window and a mark that wide is noise. Styled with the same palette
/// codespan uses (bold intense severity header, blue gutter/line numbers/
/// note bullets); the no-color buffer simply drops the escapes.
///
/// # Errors
///
/// Fails when writing fails.
fn render_anchorless(
    f: &deslop_core::finding::Finding,
    path: &str,
    src: &str,
    out: &mut Buffer,
    line_budget: Option<usize>,
) -> std::io::Result<()> {
    let header = match f.tier {
        Tier::Artifact => header_style(Color::Red),
        Tier::Tell => header_style(Color::Yellow),
        Tier::Density => header_style(Color::Green),
    };
    let gutter = blue_style();
    let (line, col) = super::line_col(src, f.span.start);
    // Gutter width fits the last line number actually printed. Never derive
    // it from span.end: subtracting one from a byte offset that sits at a
    // multibyte char boundary panics, and a document-window span capped to
    // two printed lines wants the SHOWN width anyway.
    let shown_lines = src[f.span.start..f.span.end].lines().count().min(2);
    let width = (line + shown_lines.saturating_sub(1)).to_string().len();
    // codespan aligns every gutter glyph (`┌─`, `│`, `=`) one space past the
    // line-number width, so bars line up with the `{:width$} │ ` source
    // lines regardless of how wide the numbers are.
    let pad = " ".repeat(width + 1);

    // Header: bold intense severity color on `severity[CODE]`, bold on the
    // `: message` tail - the codespan header shape.
    out.set_color(&header)?;
    write!(out, "{}[{}]", sev_name(f.tier), f.entry_id)?;
    out.set_color(&bold_style())?;
    writeln!(out, ": {}", f.message)?;
    out.reset()?;

    // Gutter: blue `┌─` then plain `path:line:col`; blue `│` continuations.
    out.set_color(&gutter)?;
    write!(out, "{pad}┌─ ")?;
    out.reset()?;
    writeln!(out, "{path}:{line}:{col}")?;
    gutter_line(out, &pad)?;
    // Numbered source lines; a window spanning more than two lines prints
    // its first two, then an ellipsis continuation (never the whole doc).
    // Under a line budget each printed line is head-truncated to the cells
    // that remain after the gutter.
    let total_lines = src[f.span.start..f.span.end].lines().count();
    let mut printed = 0usize;
    for (i, text) in src[f.span.start..f.span.end].lines().enumerate() {
        if i >= 2 {
            break;
        }
        out.set_color(&gutter)?;
        write!(out, "{:>width$} │ ", line + i)?;
        out.reset()?;
        match line_budget {
            Some(budget) => writeln!(out, "{}", super::truncate::truncate_line_head(text, budget))?,
            None => writeln!(out, "{text}")?,
        }
        printed += 1;
    }
    if printed < total_lines {
        out.set_color(&gutter)?;
        writeln!(out, "{pad}│ …")?;
        out.reset()?;
    }
    gutter_line(out, &pad)?;
    write_anchorless_notes(f, out, &pad)
}

/// Severity name as printed in the header (`error`/`warning`/`note`).
fn sev_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Artifact => "error",
        Tier::Tell => "warning",
        Tier::Density => "note",
    }
}

/// One blue `│` gutter line (no source text), aligned to the number width.
fn gutter_line(out: &mut Buffer, pad: &str) -> std::io::Result<()> {
    out.set_color(&blue_style())?;
    write!(out, "{pad}│")?;
    out.reset()?;
    writeln!(out)
}

/// One blue-bulleted ` = ` note line at the shared gutter column.
fn note_line(text: &str, out: &mut Buffer, pad: &str) -> std::io::Result<()> {
    out.set_color(&blue_style())?;
    write!(out, "{pad}=")?;
    out.reset()?;
    writeln!(out, " {text}")
}

/// Trailing ` = ` notes for an anchorless finding: help, the context list
/// (one note per line, so `Clustered terms:` renders its indented terms
/// under the header), then the reference link.
///
/// # Errors
///
/// Fails when writing fails.
fn write_anchorless_notes(
    f: &deslop_core::finding::Finding,
    out: &mut Buffer,
    pad: &str,
) -> std::io::Result<()> {
    if let Some(advice) = &f.advice {
        note_line(&format!("help: {advice}"), out, pad)?;
    }
    if let Some(context) = &f.context {
        for note in context.split('\n') {
            note_line(note, out, pad)?;
        }
    }
    if let Some((text, href)) = &f.url {
        note_line(&format!("see: {text} - {href}"), out, pad)?;
    }
    writeln!(out)
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
    use deslop_core::finding::{Finding, KindTag, Span};

    #[test]
    fn line_starts_indexes_every_line() {
        // Given a three-line source.
        let src = "a\nbb\nccc\n";

        // When computing line starts.
        let starts = line_starts(src);

        // Then lines begin at their byte offsets, including the empty tail.
        assert_eq!(starts, vec![0, 2, 5, 9]);
    }

    fn sample_finding(span: Span, src: &str) -> Finding {
        Finding {
            entry_id: "CLUSTER".to_owned(),
            kind: KindTag::Metric,
            tier: Tier::Density,
            category: "vocabulary-density".to_owned(),
            message: "3 distinct watch-list words cluster".to_owned(),
            advice: Some("vary the vocabulary".to_owned()),
            span,
            excerpt: src[span.start..span.end].to_owned(),
            url: Some(("Wiki".to_owned(), "https://example.com/wiki".to_owned())),
            context: Some("Clustered terms:\n  also\n  adept".to_owned()),
            replacement: None,
            anchorless: false,
        }
    }

    fn render_one(f: &Finding, src: &str) -> String {
        render_one_width(f, src, 0)
    }

    fn render_one_width(f: &Finding, src: &str, width: usize) -> String {
        let filed = vec![FiledFinding {
            path: "doc.md",
            src,
            finding: f,
        }];
        let mut out = Vec::new();
        render_human(&filed, ColorChoice::Never, width, &mut out).expect("render");
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn anchorless_finding_renders_gutter_block_without_carets() {
        // Given a paragraph-window finding flagged anchorless.
        let src = "First line.\n\nWe felt also aptly adept here.\n";
        let span = Span::new(13, src.len() - 1);
        let mut f = sample_finding(span, src);
        f.anchorless = true;

        // When rendering.
        let text = render_one(&f, src);

        // Then the gutter block names line and column with the source text,
        // and NO caret or underline mark is drawn anywhere.
        assert!(text.contains("┌─ doc.md:3:1"), "{text}");
        assert!(
            text.contains("3 │ We felt also aptly adept here."),
            "{text}"
        );
        assert!(!text.contains('^'), "{text}");
    }

    #[test]
    fn anchorless_notes_render_help_context_list_and_url() {
        // Given an anchorless finding with advice, a context list, and a url.
        let src = "also adept aims align across\n";
        let mut f = sample_finding(Span::new(0, src.len() - 1), src);
        f.anchorless = true;

        // When rendering.
        let text = render_one(&f, src);

        // Then notes follow the codespan ` = ` style: help first, one note
        // per context line (terms indented under the header), then the link.
        assert!(text.contains("  = help: vary the vocabulary"), "{text}");
        assert!(text.contains("  = Clustered terms:"), "{text}");
        assert!(text.contains("  =   also"), "{text}");
        assert!(text.contains("  =   adept"), "{text}");
        assert!(
            text.contains("  = see: Wiki - https://example.com/wiki"),
            "{text}"
        );
    }

    #[test]
    fn spanned_finding_still_renders_carets() {
        // Given a normal word-spanned finding (anchorless false).
        let src = "one delve two\n";
        let f = sample_finding(Span::new(4, 9), src);

        // When rendering.
        let text = render_one(&f, src);

        // Then codespan draws the caret underline as before.
        assert!(text.contains('^'), "{text}");
        assert!(text.contains("┌─ doc.md:1:5"), "{text}");
    }

    /// Slice `text` at display-cell column `col` (cell-aware, char-safe —
    /// byte indexing would misalign on multibyte markers like `…`).
    fn slice_at_cell(text: &str, col: usize) -> &str {
        let mut cells = 0;
        for (i, ch) in text.char_indices() {
            if cells == col {
                return &text[i..];
            }
            cells += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        }
        ""
    }

    #[test]
    fn truncating_width_cuts_source_line_and_marks_the_word() {
        // Given a spanned finding mid-way along a long line, rendered under
        // a narrow explicit width.
        let src = format!("{} delve tail\n", "x".repeat(30));
        let start = src.find("delve").expect("word");
        let f = sample_finding(Span::new(start, start + 5), src.as_str());

        // When rendering at width 24.
        let text = render_one_width(&f, &src, 24);

        // Then the flagged word keeps its caret: measured in display cells
        // (the `…` marker is one cell but three bytes), the caret column
        // sits exactly on `delve` of the truncated source line.
        let mut lines = text
            .lines()
            .filter(|l| l.contains("delve") || l.contains('^'));
        let source = lines.next().expect("source line");
        let marks = lines.next().expect("caret line");
        assert!(source.contains('…'), "{source}");
        let content = &source[source.find("│ ").expect("gutter") + "│ ".len()..];
        let marks_tail = &marks[marks.find("│ ").expect("gutter") + "│ ".len()..];
        let caret_cells = marks_tail.find('^').expect("caret");
        assert!(
            slice_at_cell(content, caret_cells).starts_with("delve"),
            "{source} / {marks}"
        );
    }

    #[test]
    fn anchorless_truncating_width_keeps_notes_after_truncated_excerpt() {
        // Given an anchorless finding whose window is one long line.
        let src = format!("{} also adept here.\n", "x".repeat(120));
        let mut f = sample_finding(Span::new(0, src.len() - 1), src.as_str());
        f.anchorless = true;

        // When rendering at width 40.
        let text = render_one_width(&f, &src, 40);

        // Then the excerpt line is cut but the notes survive intact after
        // the gutter block.
        let excerpt = text.lines().find(|l| l.contains(" │ ")).expect("excerpt");
        assert!(excerpt.contains('…'), "{excerpt}");
        // And the note block still carries the help text.
        assert!(text.contains("= help: vary the vocabulary"), "{text}");
        assert!(text.contains("= Clustered terms:"), "{text}");
        assert!(
            text.contains("= see: Wiki - https://example.com/wiki"),
            "{text}"
        );
    }

    #[test]
    fn width_zero_matches_truncating_run_byte_for_byte_on_short_lines() {
        // Given a spanned finding whose source line fits any sane width.
        let src = "one delve two\n";
        let f = sample_finding(Span::new(4, 9), src);

        // When rendering at width 0 and at a width far above the line.
        let legacy = render_one_width(&f, src, 0);
        let wide = render_one_width(&f, src, 400);

        // Then both produce identical bytes — truncation is invisible when
        // nothing is cut.
        assert_eq!(legacy, wide);
    }

    #[test]
    fn long_line_truncates_only_under_explicit_width() {
        // Given a spanned finding on a 200-cell line.
        let src = format!("{} delve\n", "x".repeat(200));
        let start = src.find("delve").expect("word");
        let f = sample_finding(Span::new(start, start + 5), src.as_str());

        // When rendering untruncated.
        let legacy = render_one_width(&f, &src, 0);

        // Then the full line and its mark render.
        assert!(legacy.contains(&"x".repeat(200)), "{legacy}");
        assert!(legacy.contains('^'), "{legacy}");

        // When rendering at width 40.
        let truncated = render_one_width(&f, &src, 40);

        // Then the line is cut and the flagged word survives with its mark.
        assert!(!truncated.contains(&"x".repeat(200)), "{truncated}");
        assert!(truncated.contains('…'), "{truncated}");
        assert!(truncated.contains("delve"), "{truncated}");
        assert!(truncated.contains('^'), "{truncated}");
    }

    #[test]
    fn document_level_zero_span_keeps_message_only_path() {
        // Given a (0,0) document-level finding.
        let src = "just words\n";
        let f = sample_finding(Span::new(0, 0), src);
        let mut f = f;
        f.message = "doc-level signal".to_owned();

        // When rendering.
        let text = render_one(&f, src);

        // Then codespan emits the bare note header - no gutter, no carets.
        assert!(text.contains("note[CLUSTER]: doc-level signal"), "{text}");
        assert!(!text.contains('│'), "{text}");
        assert!(!text.contains('^'), "{text}");
    }

    #[test]
    fn anchorless_multiline_window_numbers_each_line() {
        // Given a two-line window starting on line 4 (byte 9).
        let src = "l1\nl2\nl3\nwindow line a\nwindow line b\n";
        let span = Span::new(9, src.len() - 1);
        let mut f = sample_finding(span, src);
        f.anchorless = true;

        // When rendering.
        let text = render_one(&f, src);

        // Then each source line carries its own number in the gutter.
        assert!(text.contains("4 │ window line a"), "{text}");
        assert!(text.contains("5 │ window line b"), "{text}");
        assert!(!text.contains('^'), "{text}");
    }

    #[test]
    fn anchorless_three_line_window_caps_with_ellipsis() {
        // Given a window spanning four lines (document-window scale).
        let src = "a1\nb2\nc3\nd4\n";
        let mut f = sample_finding(Span::new(0, src.len() - 1), src);
        f.anchorless = true;

        // When rendering.
        let text = render_one(&f, src);

        // Then only the first two lines print, followed by an ellipsis
        // continuation line - never the whole document.
        assert!(text.contains("1 │ a1"), "{text}");
        assert!(text.contains("2 │ b2"), "{text}");
        assert!(text.contains("  │ …"), "{text}");
        assert!(!text.contains("c3"), "{text}");
        assert!(!text.contains("d4"), "{text}");
    }

    #[test]
    fn anchorless_span_ending_on_multibyte_char_renders() {
        // Given a window whose end byte sits at a multibyte char boundary.
        let src = "检查 crucial robust 检查\n";
        let end = src.find('\n').expect("newline");
        let mut f = sample_finding(Span::new(0, end), src);
        f.anchorless = true;

        // When rendering.
        let text = render_one(&f, src);

        // Then the block renders without panicking on the boundary.
        assert!(text.contains("1 │ 检查 crucial robust 检查"), "{text}");
        assert!(!text.contains('^'), "{text}");
    }
}
