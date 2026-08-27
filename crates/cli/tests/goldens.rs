//! Golden snapshot tests pinned to the six seed rules (spec tm3m/AC5).
//! Captured with --color=never; output must be byte-identical.
//!
//! Regenerating on an intentional renderer change: update the .golden.txt
//! files and re-present them for user approval before proceeding to bulk
//! advice work (spec gate tivs).

use std::process::Command;

const DOCS: [&str; 6] = [
    "seed-vocab-replacement",
    "seed-vocab-report-only",
    "seed-artifact",
    "seed-pattern-single-capture",
    "seed-pattern-payload",
    "seed-metric-em-dash",
];

fn golden_path(doc: &str, ext: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/goldens")
        .join(format!("{doc}.{ext}.golden.txt"))
}

fn run_format(doc: &str, format: &str) -> String {
    // Relative doc path from the repo root keeps outputs machine-stable.
    let doc_rel = format!("tests/fixtures/docs/{doc}.md");
    let out = Command::new(env!("CARGO_BIN_EXE_deslop"))
        .arg(&doc_rel)
        .args(["--color", "never", "--format", format])
        .env_remove("NO_COLOR")
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .expect("runs");
    assert_ne!(out.status.code(), Some(2), "load failure");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !text.contains(env!("CARGO_MANIFEST_DIR")),
        "absolute paths leak into output"
    );
    text
}

#[test]
fn human_goldens_match() {
    for doc in DOCS {
        let actual = run_format(doc, "human");
        let expected_path = golden_path(doc, "human");
        if !expected_path.exists() {
            std::fs::write(&expected_path, &actual).expect("write golden");
            panic!("golden created for {doc}; rerun to verify");
        }
        let expected = std::fs::read_to_string(&expected_path).expect("read golden");
        assert_eq!(
            actual, expected,
            "{doc} human render drifted; regenerate goldens only with user approval"
        );
    }
}

#[test]
fn json_goldens_match() {
    for doc in DOCS {
        let actual = run_format(doc, "json");
        let expected_path = golden_path(doc, "json");
        if !expected_path.exists() {
            std::fs::write(&expected_path, &actual).expect("write golden");
            panic!("golden created for {doc}; rerun to verify");
        }
        let expected = std::fs::read_to_string(&expected_path).expect("read golden");
        assert_eq!(
            actual, expected,
            "{doc} json render drifted; regenerate goldens only with user approval"
        );
    }
}
