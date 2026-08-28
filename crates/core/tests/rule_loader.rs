//! Loader integration: temp pack files -> Loaded { rule_set, errors }.
//!
//! Flat-pack world: a builtin stem `p` resolves to `<root>/rules/p.toml`,
//! one file per pack, any number of `[[group]]` tables inside.

use deslop_core::config::Config;
use deslop_core::rule::loader::load;

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

const GOOD_VOCAB: &str = r#"
[[group]]
id-base = "MODERN-VOCAB"
kind = "vocab"
tier = 2
category = "delve-era"
message = "AI-register vocabulary"
enabled = true

[group.fixtures]
must_match = ["we must delve deeper"]

[[group.entries]]
slug = "SLUG-X"
terms = ["delve"]
"#;

const BAD_TOML: &str = r#"
[[group]]
id-base = "BROKEN
kind = "vocab"
"#;

const NO_SLUG: &str = r#"
[[group]]
id-base = "NO-SLUG"
kind = "vocab"
tier = 2
category = "c"

[group.fixtures]
must_match = ["x delve y"]

[[group.entries]]
terms = ["delve"]
"#;

fn cfg_for(packs: &[&str]) -> Config {
    Config {
        packs: deslop_core::config::Packs {
            builtin: packs.iter().map(|s| (*s).to_owned()).collect(),
            extra_paths: vec![],
        },
        ..Config::default()
    }
}

#[test]
fn loads_good_pack_with_zero_errors() {
    // Given a pack file with one well-formed group.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack.toml", GOOD_VOCAB);

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then no errors and one group lands.
    assert!(loaded.errors.is_empty(), "errors: {:?}", loaded.errors);
    assert_eq!(loaded.rule_set.groups.len(), 1);
}

#[test]
fn missing_pack_file_is_recorded_naming_the_stem() {
    // Given a configured pack whose file does not exist.
    let tmp = tempfile::tempdir().expect("tempdir");

    // When loading.
    let loaded = load(
        &cfg_for(&["missing"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then an error names the expected flat path.
    assert!(
        loaded
            .errors
            .iter()
            .any(|e| e.path.ends_with("rules/missing.toml")),
        "{:?}",
        loaded.errors
    );
}

#[test]
fn bad_toml_yields_error_with_line_number() {
    // Given a pack file with broken TOML.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack.toml", BAD_TOML);

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then exactly one error naming the file and a line.
    assert_eq!(loaded.errors.len(), 1);
    let err = &loaded.errors[0];
    assert!(err.path.ends_with("pack.toml"), "{}", err.path);
    assert!(err.line.is_some());
    assert!(err.message.contains("invalid rule TOML"));
}

#[test]
fn errors_accumulate_across_packs() {
    // Given two separate bad pack files.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack-a.toml", NO_SLUG);
    write(tmp.path(), "rules/pack-b.toml", BAD_TOML);

    // When loading.
    let loaded = load(
        &cfg_for(&["pack-a", "pack-b"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then errors from BOTH files accumulate (never first-fail).
    let files: Vec<String> = loaded.errors.iter().map(|e| e.path.clone()).collect();
    assert!(files.iter().any(|f| f.contains("pack-a.toml")), "{files:?}");
    assert!(files.iter().any(|f| f.contains("pack-b.toml")), "{files:?}");
}

#[test]
fn multi_group_file_loads_every_group() {
    // Given one pack file with two groups of DISTINCT terms (no dedup).
    let second = GOOD_VOCAB
        .replace("MODERN-VOCAB", "CLUSTER-X")
        .replace("delve", "garner");
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        tmp.path(),
        "rules/pack.toml",
        &format!("{GOOD_VOCAB}\n{second}"),
    );

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then both groups are active.
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(loaded.rule_set.groups.len(), 2);
}

#[test]
fn duplicate_term_across_groups_dedups_to_highest_tier_owner() {
    // Given the same term in a tier-2 group and a tier-3 group.
    let shadow = GOOD_VOCAB
        .replace("MODERN-VOCAB", "CLUSTER-X")
        .replace("tier = 2", "tier = 3");
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        tmp.path(),
        "rules/pack.toml",
        &format!("{GOOD_VOCAB}\n{shadow}"),
    );

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then the higher tier keeps the term and the emptied group is gone,
    // with a dedup line naming winner and loser.
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(loaded.rule_set.groups.len(), 1);
    assert_eq!(loaded.rule_set.groups[0].id_base, "MODERN-VOCAB");
    assert!(
        loaded
            .dedup_warnings
            .iter()
            .any(|w| w.contains("MODERN-VOCAB") && w.contains("CLUSTER-X")),
        "{:?}",
        loaded.dedup_warnings
    );
}

#[test]
fn duplicate_id_bases_are_flagged_across_files_and_within_one_file() {
    // Given the same id-base in two pack files.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack-a.toml", GOOD_VOCAB);
    write(tmp.path(), "rules/pack-b.toml", GOOD_VOCAB);

    // When loading.
    let loaded = load(
        &cfg_for(&["pack-a", "pack-b"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then a collision error appears.
    assert!(
        loaded
            .errors
            .iter()
            .any(|e| e.message.contains("already defined in")),
        "{:?}",
        loaded.errors
    );
}

#[test]
fn duplicate_entry_slugs_across_groups_are_legal_within_a_file() {
    // Given two groups in one file whose entries share a slug.
    let other = GOOD_VOCAB.replace("MODERN-VOCAB", "OTHER-GROUP");
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        tmp.path(),
        "rules/pack.toml",
        &format!("{GOOD_VOCAB}\n{other}"),
    );

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then no duplicate-id error appears (ids differ by group prefix).
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
}

#[test]
fn extra_path_pack_file_loads() {
    // Given an extra pack referenced by path (distinct terms, no dedup).
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        tmp.path(),
        "rules/aatell.toml",
        &GOOD_VOCAB.replace("delve", "garner"),
    );
    write(
        tmp.path(),
        "team/custom.toml",
        &GOOD_VOCAB
            .replace("MODERN-VOCAB", "TEAM")
            .replace("delve", "reckon"),
    );

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["aatell".into()],
            extra_paths: vec![camino::Utf8PathBuf::from("team/custom.toml")],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then both builtin and extra packs contribute groups.
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(loaded.rule_set.groups.len(), 2);
}

#[test]
fn rule_failing_own_fixture_is_refused() {
    // Given a rule whose must_match sample cannot hit.
    let broken = r#"
[[group]]
id-base = "BROKEN-FIXTURE"
kind = "pattern"
tier = 2
category = "c"

[group.fixtures]
must_match = ["no keyword present here at all"]

[[group.entries]]
slug = "delve"
regex = 'delve'
"#;
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack.toml", broken);

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then a fixture failure is recorded naming the entry.
    assert!(
        loaded
            .errors
            .iter()
            .any(|e| e.message.contains("fixture failure") && e.message.contains("delve")),
        "{:?}",
        loaded.errors
    );
}

#[test]
fn rule_passing_fixtures_reports_no_error() {
    // Given the same pattern rule with a hitting positive and clean negative.
    let good = r#"
[[group]]
id-base = "GOOD-PATTERN"
kind = "pattern"
tier = 2
category = "c"

[group.fixtures]
must_match = ["we must delve deeper"]
must_not_match = ["studying the study of studies"]

[[group.entries]]
slug = "delve"
regex = 'delve'
"#;
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack.toml", good);

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then no fixture failures exist.
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
}

#[test]
fn advice_with_unknown_placeholder_is_refused() {
    // Given a vocab entry whose advice references a bogus placeholder.
    let bad = r#"
[[group]]
id-base = "BAD-TEMPLATE"
kind = "vocab"
tier = 2
category = "c"

[group.fixtures]
must_match = ["delve into it"]

[[group.entries]]
slug = "delve"
terms = ["delve"]
advice = 'replace {bogus} please'
"#;
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "rules/pack.toml", bad);

    // When loading.
    let loaded = load(
        &cfg_for(&["pack"]),
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
    );

    // Then the template error is recorded naming the field.
    assert!(
        loaded
            .errors
            .iter()
            .any(|e| e.message.contains("advice template") && e.message.contains("{bogus}")),
        "{:?}",
        loaded.errors
    );
}
