//! `--width` truncation contract for human output.
//!
//! Explicit widths cut every rendered source line to the terminal budget
//! with the caret still on the flagged word; piped runs (no flag) and
//! `--width 0` stay untruncated and byte-identical to the historical
//! output.

mod common;

use std::process::Command;
use unicode_width::UnicodeWidthStr;

/// Long-line fixture: 170 chars, `leverage` mid-line (byte 42).
const DOC: &str = "long-line.md";

/// Lint the long-line fixture hermetically with the given extra flags.
fn lint(extra: &[&str]) -> (i32, String) {
    let dir = tempfile::tempdir().expect("tmpdir");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/docs/{DOC}")),
        dir.path().join(DOC),
    )
    .expect("seed doc");
    let hermetic = common::HermeticRules::provision();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deslop"));
    hermetic.apply(&mut cmd);
    for flag in extra {
        cmd.arg(flag);
    }
    let out = cmd
        .arg(DOC)
        .args(["--color", "never"])
        .current_dir(dir.path())
        .output()
        .expect("runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// Cell length of a rendered source line; `None` for non-source gutter
/// lines (caret marks, label tails, notes), which carry no line number.
fn excerpt_cell_len(line: &str) -> Option<usize> {
    let (prefix, text) = line.split_once(" │ ")?;
    prefix.trim().parse::<usize>().ok().map(|_| text.width())
}

#[test]
fn explicit_width_truncates_every_source_line() {
    // Given the long-line doc linted with an explicit 60-cell width.

    // When rendering human output.
    let (code, stdout) = lint(&["--width", "60"]);

    // Then the run still reports the finding (exit 1), the flagged word
    // survives with its caret, and every SOURCE excerpt line fits the
    // budget in display cells: width 60 − 1 line digit − 3 gutter − 4
    // padding = 52 cells. Note lines (` = help:`) are a deliberate
    // non-goal and stay full width.
    assert_eq!(code, 1);
    assert!(stdout.contains('…'), "expected a cut marker: {stdout}");
    assert!(
        stdout.contains("leverage"),
        "flagged word visible: {stdout}"
    );
    assert!(stdout.contains('^'), "caret present: {stdout}");
    for line in stdout.lines().filter_map(excerpt_cell_len) {
        assert!(line <= 52, "excerpt too wide ({line} cells)");
    }
}

#[test]
fn piped_output_without_width_is_untruncated() {
    // Given the long-line doc linted without any width flag (piped run).

    // When rendering human output.
    let (code, stdout) = lint(&[]);

    // Then the full line is printed verbatim with its finding.
    assert_eq!(code, 1);
    assert!(!stdout.contains('…'), "no cut marker: {stdout}");
    assert!(
        stdout.contains("without further debate"),
        "full line: {stdout}"
    );
}

#[test]
fn width_zero_matches_default_piped_output_byte_for_byte() {
    // Given the same doc linted twice.

    // When once with --width 0 and once with no flag.
    let (zero_code, zero_out) = lint(&["--width", "0"]);
    let (default_code, default_out) = lint(&[]);

    // Then both runs report identically, byte for byte.
    assert_eq!(zero_code, default_code);
    assert_eq!(zero_out, default_out);
}
