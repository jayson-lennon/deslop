//! Determinism: identical input runs must produce byte-identical output
//! (spec tw6j; golden stability depends on it).

use std::process::Command;

/// Lint one doc twice; assert both human and JSON output are stable.
fn run_twice(doc: &str, format: &str) -> (String, String) {
    let run = || {
        let out = Command::new(env!("CARGO_BIN_EXE_deslop"))
            .arg(doc)
            .args(["--color", "never", "--format", format])
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .output()
            .expect("runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    (run(), run())
}

#[test]
fn human_output_is_byte_stable_across_runs() {
    // Given the busiest seed doc.
    // When linting it twice.
    let (a, b) = run_twice("tests/fixtures/docs/seed-pattern-payload.md", "human");
    // Then both runs agree byte-for-byte.
    assert_eq!(a, b);
}

#[test]
fn json_output_is_byte_stable_across_runs() {
    // Given the busiest seed doc.
    // When linting it twice.
    let (a, b) = run_twice("tests/fixtures/docs/seed-pattern-payload.md", "json");
    // Then both runs agree byte-for-byte.
    assert_eq!(a, b);
}

#[test]
fn repeated_identical_findings_keep_a_stable_order() {
    // Given a doc with several findings of mixed tiers.
    let doc = std::env::temp_dir().join("deslop-stable-order.md");
    std::fs::write(
        &doc,
        "We will leverage the tapestry. It's not merely faster; it's transformative. [cite: 2]\n",
    )
    .expect("write");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_deslop"))
            .arg(&doc)
            .args(["--color", "never", "--format", "json"])
            .output()
            .expect("runs")
    };
    // When linting twice.
    let a = String::from_utf8_lossy(&run().stdout).into_owned();
    let b = String::from_utf8_lossy(&run().stdout).into_owned();
    // Then the serialized order (path, start, tier, id) repeats exactly.
    assert_eq!(a, b);
    let _ = std::fs::remove_file(&doc);
}
