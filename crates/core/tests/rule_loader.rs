//! Loader integration: temp pack trees -> Loaded { rule_set, errors }.

use deslop_core::config::Config;
use deslop_core::rule::loader::load;

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

const GOOD_VOCAB: &str = r#"
id-base = "MODERN-VOCAB"
kind = "vocab"
tier = 2
category = "delve-era"
message = "AI-register vocabulary"
enabled = true

[fixtures]
must_match = ["we must delve deeper"]

[[entries]]
slug = "delve"
terms = ["delve"]
"#;

const BAD_TOML: &str = r#"
id-base = "BROKEN
kind = "vocab"
"#;

const NO_SLUG: &str = r#"
id-base = "NO-SLUG"
kind = "vocab"
tier = 2
category = "c"

[fixtures]
must_match = ["x delve y"]

[[entries]]
terms = ["delve"]
"#;

#[test]
fn loads_good_pack_with_zero_errors() {
    // Given a pack with one well-formed rule file.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "builtin/pack/a.toml", GOOD_VOCAB);

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["pack".into()],
            extra_paths: vec![],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then no errors and one group lands.
    assert!(loaded.errors.is_empty(), "errors: {:?}", loaded.errors);
    assert_eq!(loaded.rule_set.groups.len(), 1);
}

#[test]
fn bad_toml_yields_error_with_line_number() {
    // Given a file with broken TOML.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "builtin/pack/bad.toml", BAD_TOML);

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["pack".into()],
            extra_paths: vec![],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then exactly one error naming the file and a line.
    assert_eq!(loaded.errors.len(), 1);
    let err = &loaded.errors[0];
    assert!(err.path.ends_with("bad.toml"), "{}", err.path);
    assert!(err.line.is_some());
    assert!(err.message.contains("invalid rule TOML"));
}

#[test]
fn missing_slug_and_missing_stat_accumulate_together() {
    // Given two separate bad files.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "builtin/pack/noslug.toml", NO_SLUG);
    write(tmp.path(), "builtin/pack/bad.toml", BAD_TOML);

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["pack".into()],
            extra_paths: vec![],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then errors from BOTH files accumulate (never first-fail).
    let files: Vec<String> = loaded.errors.iter().map(|e| e.path.clone()).collect();
    assert!(files.iter().any(|f| f.contains("noslug.toml")), "{files:?}");
    assert!(files.iter().any(|f| f.contains("bad.toml")), "{files:?}");
}

#[test]
fn duplicate_group_ids_across_files_are_flagged() {
    // Given two files sharing an id-base.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "builtin/pack/one.toml", GOOD_VOCAB);
    write(tmp.path(), "builtin/pack/two.toml", GOOD_VOCAB);

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["pack".into()],
            extra_paths: vec![],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then a duplicate-id error appears pointing at the second file.
    assert!(
        loaded
            .errors
            .iter()
            .any(|e| e.message.contains("duplicate group id-base") && e.path.ends_with("two.toml")),
        "errors: {:?}",
        loaded.errors
    );
}

const CONVERTED: &str = r#"
id-base = "SRC-VOCAB"
kind = "vocab"
tier = 2
category = "c"

[origin]
repo = "https://github.com/example/src"
commit = "aaaa1111bbbb2222cccc3333dddd4444eeee5555"

[fixtures]
must_match = ["delve now"]

[[entries]]
slug = "delve"
terms = ["delve"]
"#;

const NOTICE_OK: &str = r#"
license = "MIT"
[[origin]]
repo = "https://github.com/example/src"
commit = "aaaa1111bbbb2222cccc3333dddd4444eeee5555"
"#;

#[test]
fn converted_rule_with_matching_notice_loads() {
    // Given a converted rule and a NOTICE covering its origin.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "builtin/pack/rule.toml", CONVERTED);
    write(tmp.path(), "builtin/pack/NOTICE.toml", NOTICE_OK);

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["pack".into()],
            extra_paths: vec![],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then attribution checks pass silently.
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
}

#[test]
fn origin_missing_from_notice_is_refused() {
    // Given a rule whose commit is absent from the NOTICE.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "builtin/pack/rule.toml", CONVERTED);
    write(
        tmp.path(),
        "builtin/pack/NOTICE.toml",
        "license = \"MIT\"\n[[origin]]\nrepo = \"https://github.com/other\"\ncommit = \"ffff\"\n",
    );

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["pack".into()],
            extra_paths: vec![],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then an error says the origin is not listed.
    assert!(
        loaded
            .errors
            .iter()
            .any(|e| e.message.contains("not listed in pack NOTICE")),
        "{:?}",
        loaded.errors
    );
}

#[test]
fn converted_rule_without_notice_file_is_refused() {
    // Given a converted rule with NO NOTICE.toml beside it.
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "builtin/pack/rule.toml", CONVERTED);

    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["pack".into()],
            extra_paths: vec![],
        },
        ..Config::default()
    };

    // When loading.
    let loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));

    // Then the error names the missing NOTICE.
    assert!(
        loaded
            .errors
            .iter()
            .any(|e| e.message.contains("no NOTICE.toml")),
        "{:?}",
        loaded.errors
    );
}
