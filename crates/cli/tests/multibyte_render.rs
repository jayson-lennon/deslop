//! Multibyte documents must render, not panic.
//!
//! Metric findings can anchor on multibyte characters (e.g. the first curly
//! quote for curly-double-ratio), and human-format rendering derives a line
//! number from an end-minus-one byte offset. When that offset landed inside
//! a multibyte character, `line_col` panicked mid-render on real corpora.
//! The renderer now floor-clamps to char boundaries; these tests pin the
//! behavior through the binary, hermetically.

mod common;

use std::process::Command;

/// A document whose curly-quote metric finding anchors on `“` (3 bytes).
/// Prose is padded so sentence windows and the paragraph scan see real text
/// without needing any other trigger.
const CURLY_DOC: &str = "curly-quotes.md";

/// 40 repetitions clears both floors the metric applies: 80 curly doubles
/// (floor: 20) and 280 words (floor: 250). Every quote is the 3-byte `“` or
/// `”`, so the finding's anchor is multibyte.
fn curly_text() -> String {
    "He said, \u{201C}the phrase holds\u{201D} and left. ".repeat(40)
}

/// Hermetic CLI run over a doc containing the given text, with flags.
fn lint_doc(name: &str, text: &str, extra: &[&str]) -> (i32, String, String) {
    let dir = tempfile::tempdir().expect("tmpdir");
    std::fs::write(dir.path().join(name), text).expect("seed doc");
    let hermetic = common::HermeticRules::provision();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deslop"));
    hermetic.apply(&mut cmd);
    for flag in extra {
        cmd.arg(flag);
    }
    let out = cmd
        .arg(name)
        .args(["--color", "never"])
        .current_dir(dir.path())
        .output()
        .expect("runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn human_width_render_survives_curly_quote_metric_anchor() {
    // Given a doc whose curly-double-ratio finding anchors on a 3-byte
    // curly quote, rendered with an explicit width (the truncating path).
    // When linting in human format.
    let (code, stdout, stderr) = lint_doc(CURLY_DOC, &curly_text(), &["--width", "80"]);

    // Then the finding renders (a tier-3 note: reported, exit stays 0)
    // and nothing panicked on the multibyte anchor.
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("CURLY-DOUBLE-RATIO"), "{stdout}");
    assert!(!stderr.contains("panic"), "{stderr}");
}

#[test]
fn untruncated_human_render_survives_curly_quote_metric_anchor() {
    // Given the same multibyte-anchor document.
    // When linting with width disabled (the historical path).
    let (code, stdout, stderr) = lint_doc(CURLY_DOC, &curly_text(), &[]);

    // Then it renders end to end without a panic.
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("CURLY-DOUBLE-RATIO"), "{stdout}");
    assert!(!stderr.contains("panic"), "{stderr}");
}

#[test]
fn github_and_json_render_survive_curly_quote_metric_anchor() {
    // Given the same multibyte-anchor document.
    // When linting with the machine formats.
    let (gh_code, gh_out, gh_err) = lint_doc(CURLY_DOC, &curly_text(), &["--format", "github"]);
    let (json_code, json_out, json_err) = lint_doc(CURLY_DOC, &curly_text(), &["--format", "json"]);

    // Then both complete with the metric reported; JSON stays parseable.
    assert_eq!(gh_code, 0, "{gh_err}");
    assert!(gh_out.contains("CURLY-DOUBLE-RATIO"), "{gh_out}");
    assert_eq!(json_code, 0, "{json_err}");
    let parsed: serde_json::Value = serde_json::from_str(&json_out).expect("valid json");
    let tiers: Vec<&serde_json::Value> = parsed
        .as_array()
        .expect("array")
        .iter()
        .filter(|f| {
            f["rule_id"]
                .as_str()
                .is_some_and(|id| id.contains("CURLY-DOUBLE-RATIO"))
        })
        .collect();
    assert!(!tiers.is_empty(), "{json_out}");
}
