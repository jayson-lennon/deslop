//! JSON serializer contract: valid JSON, frozen field order, char-safe
//! columns (spec tuhu).

mod common;

use std::process::Command;

fn run_json(doc: &str) -> (String, i32) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("doc.md");
    std::fs::write(&path, doc).expect("write");
    let bin = env!("CARGO_BIN_EXE_deslop");
    let hermetic = common::HermeticRules::provision();
    let mut cmd = Command::new(bin);
    hermetic.apply(&mut cmd);
    let out = cmd
        .args([path.to_str().expect("utf8"), "--format", "json"])
        .output()
        .expect("runs");
    // Exit 1 = findings reported (expected on dirty docs); 2 = failure.
    assert_ne!(
        out.status.code(),
        Some(2),
        "lint failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn json_output_is_valid_and_fields_ordered() {
    // Given a document with one vocab hit and one artifact.
    let doc = "We must leverage the contentReference[oaicite:3]{index=3} synergy.\n";

    // When rendering as JSON.
    let (stdout, _exit) = run_json(doc);

    // Then every line parses as an object…
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid array");
    assert!(!parsed.is_empty());
    // …and the RAW bytes show field order frozen with rule_id first
    // (re-serializing through a Value map would re-alphabetize).
    for line in stdout.lines().skip(1) {
        let line = line.trim_end_matches([',']);
        if !line.starts_with("  {") {
            continue;
        }
        let first_key = line.split('"').nth(1).expect("first key");
        assert_eq!(first_key, "rule_id", "field order frozen at rule_id");
    }
}

#[test]
fn json_columns_are_char_based_not_byte() {
    // Given a line whose hit sits AFTER a multi-byte char (em dash).
    let doc = "Run it — then leverage this.\n";

    // When rendering.
    let (stdout, _) = run_json(doc);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid");

    // Then the leverage hit's col equals its 1-based CHAR column on line 1:
    // chars: R(1)u2n3 4i5t6 7—8 9t10h11e12n13 14l15... => col 15.
    let hit = parsed
        .iter()
        .find(|v| v["excerpt"] == "leverage")
        .expect("leverage finding present");
    assert_eq!(hit["span"]["line"], 1);
    assert_eq!(hit["span"]["col"], 15, "char-derived column");
}
