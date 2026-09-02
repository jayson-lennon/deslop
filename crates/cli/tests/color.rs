//! Color plumbing: `--color always` emits ANSI codes, `never` emits none,
//! and `auto` respects `NO_COLOR` even when piped output would suppress it.

mod common;

use std::process::Command;

fn lint(doc: &str, color: &str, env: &[(&str, &str)]) -> String {
    let hermetic = common::HermeticRules::provision();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deslop"));
    hermetic.apply(&mut cmd);
    hermetic.pin_seed_config(&mut cmd);
    cmd.arg(doc)
        .args(["--color", color, "--format", "human"])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    cmd.env_remove("NO_COLOR");
    for (k, v) in env {
        cmd.env(k, v);
    }
    String::from_utf8_lossy(&cmd.output().expect("runs").stdout).into_owned()
}

const DOC: &str = "tests/fixtures/docs/seed-vocab-replacement.md";

#[test]
fn always_emits_ansi_codes() {
    // Given a piped (non-tty) run with --color always.
    // When linting.
    let out = lint(DOC, "always", &[]);
    // Then ANSI escape sequences are present.
    assert!(out.contains('\x1b'), "expected ANSI codes");
}

#[test]
fn never_emits_no_ansi_codes_even_forced() {
    // Given --color never.
    // When linting.
    let out = lint(DOC, "never", &[]);
    // Then the output is plain text.
    assert!(!out.contains('\x1b'), "expected no ANSI codes");
}

#[test]
fn auto_suppresses_color_when_no_color_is_set() {
    // Given NO_COLOR set with --color auto on a piped stream.
    // When linting.
    let out = lint(DOC, "auto", &[("NO_COLOR", "1")]);
    // Then no ANSI codes appear.
    assert!(!out.contains('\x1b'));
}

#[test]
fn auto_stays_plain_on_piped_output_without_no_color() {
    // Given piped output (not a tty) and no NO_COLOR.
    // When linting with auto.
    let out = lint(DOC, "auto", &[]);
    // Then color stays off (pipes never get ANSI).
    assert!(!out.contains('\x1b'));
}
