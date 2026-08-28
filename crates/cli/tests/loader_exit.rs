//! Negative loader suite at CLI level: bad packs abort with exit 2 and a
//! pointed diagnostic naming file (and line when known).

use assert_cmd::Command;

fn deslop() -> Command {
    Command::cargo_bin("deslop").expect("binary builds")
}

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn cfg_for(tmp: &std::path::Path, pack: &str) -> String {
    let cfg_path = tmp.join("cfg.toml");
    std::fs::write(
        &cfg_path,
        format!("[packs]\nbuiltin = [\"{pack}\"]\nextra_paths = []\n"),
    )
    .expect("cfg");
    cfg_path.to_string_lossy().into_owned()
}

fn cfg_for_two(tmp: &std::path::Path, pack_a: &str, pack_b: &str) -> String {
    let cfg_path = tmp.join("cfg2.toml");
    std::fs::write(
        &cfg_path,
        format!("[packs]\nbuiltin = [\"{pack_a}\", \"{pack_b}\"]\nextra_paths = []\n"),
    )
    .expect("cfg");
    cfg_path.to_string_lossy().into_owned()
}

const OK_RULE: &str = r#"
[[group]]
id-base = "OK-VOCAB"
kind = "vocab"
tier = 2
category = "c"

[group.fixtures]
must_match = ["delve deep"]

[[group.entries]]
slug = "delve"
terms = ["delve"]
"#;

#[test]
fn uncomplilable_regex_exits_two_naming_file() {
    // Given a pack with a pattern entry whose regex cannot compile.
    let tmp = tempfile::tempdir().expect("tempdir");
    let broken = r#"
[[group]]
id-base = "BAD-REGEX"
kind = "pattern"
tier = 2
category = "c"

[group.fixtures]
must_match = ["x"]

[[group.entries]]
slug = "open"
regex = '([unclosed'
"#;
    write(tmp.path(), "rules/pack.toml", broken);
    let cfg = cfg_for(tmp.path(), "pack");

    // When linting any existing path.
    let output = deslop()
        .arg("--config")
        .arg(&cfg)
        .arg(".")
        .current_dir(tmp.path())
        .output()
        .expect("runs");

    // Then exit is 2 and the diagnostic names the offending file.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("pack.toml"), "{stderr}");
}

#[test]
fn fixture_failure_blocks_lint_run_with_file_named() {
    // Given a rule whose own must_match can never hit, plus a healthy rule.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/ok.toml", OK_RULE);
    write(
        tmp.path(),
        "rules/lying.toml",
        r#"
[[group]]
id-base = "LYING-RULE"
kind = "vocab"
tier = 2
category = "c"

[group.fixtures]
must_match = ["nothing relevant here"]

[[group.entries]]
slug = "delve"
terms = ["delve"]
"#,
    );
    let cfg = cfg_for_two(tmp.path(), "ok", "lying");

    // When linting.
    let output = deslop()
        .arg("--config")
        .arg(&cfg)
        .arg(".")
        .current_dir(tmp.path())
        .output()
        .expect("runs");

    // Then exit 2 names the lying rule's file specifically.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("lying.toml"), "{stderr}");
}

#[test]
fn duplicate_group_ids_across_packs_exit_two() {
    // Given the same id-base in two DIFFERENT packs.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack-a.toml", OK_RULE);
    write(tmp.path(), "rules/pack-b.toml", OK_RULE);
    let cfg = cfg_for_two(tmp.path(), "pack-a", "pack-b");

    // When linting.
    let output = deslop()
        .arg("--config")
        .arg(&cfg)
        .arg(".")
        .current_dir(tmp.path())
        .output()
        .expect("runs");

    // Then exit 2 flags the collision.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already defined in"), "{stderr}");
}

#[test]
fn duplicate_entry_ids_within_pack_exit_two() {
    // Given one group with two entries claiming the same slug.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        tmp.path(),
        "rules/pack.toml",
        r#"
[[group]]
id-base = "OK-VOCAB"
kind = "vocab"
tier = 2
category = "c"

[group.fixtures]
must_match = ["delve deep"]

[[group.entries]]
slug = "delve"
terms = ["delve"]

[[group.entries]]
slug = "delve"
terms = ["delved"]
"#,
    );
    let cfg = cfg_for(tmp.path(), "pack");

    // When linting.
    let output = deslop()
        .arg("--config")
        .arg(&cfg)
        .arg(".")
        .current_dir(tmp.path())
        .output()
        .expect("runs");

    // Then exit 2 flags the duplicated composite entry id.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("duplicate entry id"), "{stderr}");
}

#[test]
fn unknown_stat_and_missing_threshold_together() {
    // Given two metric rules: one unknown stat, one missing threshold_gt.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        tmp.path(),
        "rules/badstat.toml",
        r#"
[[group]]
id-base = "M-BAD-STAT"
kind = "metric"
tier = 3
category = "density"
stat = "vibes_per_paragraph"
threshold-gt = 1.0
window = "paragraph"
terms = ["delve"]

[group.fixtures]
must_match = []
"#,
    );
    write(
        tmp.path(),
        "rules/nothresh.toml",
        r#"
[[group]]
id-base = "M-NO-THRESH"
kind = "metric"
tier = 3
category = "density"
stat = "em_dash_rate"
window = "document"

[group.fixtures]
must_match = []
"#,
    );
    let cfg = cfg_for_two(tmp.path(), "badstat", "nothresh");

    // When linting.
    let output = deslop()
        .arg("--config")
        .arg(&cfg)
        .arg(".")
        .current_dir(tmp.path())
        .output()
        .expect("runs");

    // Then BOTH errors accumulate into one exit-2 run.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown stat"), "{stderr}");
    assert!(stderr.contains("requires `threshold_gt`"), "{stderr}");
}
