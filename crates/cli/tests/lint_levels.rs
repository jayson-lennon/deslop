//! `[lints]` control surface: clippy-style levels with precedence
//! slug > group > tier default. Unknown levels are config errors (exit 2);
//! unknown lint ids are tolerated (renames happen).

mod common;

use std::process::Command;

/// Run deslop over the seed doc with the given config text. `tag` keeps
/// temp dirs unique so parallel tests never share state.
fn lint_with_config(tag: &str, cfg: &str) -> (i32, String) {
    let dir = std::env::temp_dir().join(format!("deslop-lints-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join(".deslop.toml"), cfg).expect("write cfg");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/docs/seed-vocab-replacement.md"),
        dir.join("doc.md"),
    )
    .expect("seed doc");
    let hermetic = common::HermeticRules::provision();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deslop"));
    hermetic.apply(&mut cmd);
    let out = cmd
        .arg("doc.md")
        .args(["--color", "never"])
        .current_dir(&dir)
        .output()
        .expect("runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn group_allow_suppresses_all_its_entries() {
    // Given a group-level allow for the only group firing in the doc.
    let cfg = "[lints]\nAATELL = \"allow\"\n";

    // When linting a doc that triggers `leverage`.
    let (code, out) = lint_with_config("ga", cfg);

    // Then no strong-flag findings remain and the run is clean.
    assert_eq!(code, 0, "exit 0 with the only group allowed");
    assert!(!out.contains("STRONG-FLAG"));
}

#[test]
fn slug_allow_beats_group_warn() {
    // Given a group escalated to error with one entry allowed.
    let cfg = "[lints]\nAATELL = \"error\"\n\"AATELL#leverage\" = \"allow\"\n";

    // When linting the doc.
    let (code, out) = lint_with_config("sg", cfg);

    // Then the slug allow suppresses the only hit, so exit is clean.
    assert_eq!(code, 0, "slug allow overrides group error");
    assert!(!out.contains("leverage-fix-14"));
}

#[test]
fn group_error_demotes_to_failing_severity() {
    // Given the group escalated to error with no slug override.
    let cfg = "[lints]\nAATELL = \"error\"\n";

    // When linting the doc.
    let (code, out) = lint_with_config("ge", cfg);

    // Then the finding renders as error and fails the run.
    assert_eq!(code, 1);
    assert!(out.contains("error[AATELL#leverage]"));
}

#[test]
fn demotion_to_note_keeps_exit_clean() {
    // Given the group demoted to note.
    let cfg = "[lints]\nAATELL = \"note\"\n";

    // When linting the doc.
    let (code, out) = lint_with_config("dn", cfg);

    // Then the finding is a note and hints never fail the run.
    assert_eq!(code, 0, "notes never affect exit");
    assert!(out.contains("note[AATELL#leverage]"));
}

#[test]
fn unknown_level_is_config_error_exit_2() {
    // Given a misspelled level.
    let cfg = "[lints]\nFOO = \"silent\"\n";

    // When linting.
    let (code, _out) = lint_with_config("ul", cfg);

    // Then the config is rejected (typo, not a rename).
    assert_eq!(code, 2);
}

#[test]
fn unknown_lint_id_is_tolerated() {
    // Given an id for a rule that does not exist (renamed upstream).
    let cfg = "[lints]\nNOT-A-REAL-GROUP = \"allow\"\n";

    // When linting the doc.
    let (code, out) = lint_with_config("ui", cfg);

    // Then the run proceeds with default levels.
    assert_eq!(code, 1, "finding still fires");
    assert!(out.contains("warning[AATELL#leverage]"));
}

#[test]
fn rules_listing_shows_effective_levels() {
    // Given a group allow.
    let cfg = "[lints]\nAATELL = \"allow\"\n";
    let dir = std::env::temp_dir().join("deslop-lints-test-rules");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join(".deslop.toml"), cfg).expect("write cfg");

    // When listing rules (against hermetic packs).
    let hermetic = common::HermeticRules::provision();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deslop"));
    hermetic.apply(&mut cmd);
    let out = cmd.arg("rules").current_dir(&dir).output().expect("runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();

    // Then the listing shows allow for that group's entries and warn
    // elsewhere.
    assert!(text.contains("allow"));
    assert!(text.contains("warn"));
}
