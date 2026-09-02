//! Repetition dedup: one owner per variant; the strictest threshold wins.

use deslop_core::config::Config;
use deslop_core::rule::loader::load;

fn load_with(toml_files: &[(&str, &str)]) -> (deslop_core::rule::RuleSet, Vec<String>) {
    let tmp = tempfile::tempdir().expect("tmp");
    for (name, content) in toml_files {
        let p = tmp.path().join("rules").join(name);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, content).expect("write");
    }
    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: toml_files
                .iter()
                .map(|(n, _)| n.trim_end_matches(".toml").to_owned())
                .collect(),
            extra_paths: vec![],
        },
        ..Config::default()
    };
    let mut loaded = load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"), None);
    let warnings = std::mem::take(&mut loaded.dedup_warnings);
    (loaded.rule_set, warnings)
}

fn repetition(gid: &str, variant: &str, threshold: &str) -> String {
    format!(
        r#"
[[group]]
id-base = "{gid}"
kind = "repetition"
tier = 2
category = "repetition"
variant = "{variant}"
threshold = {threshold}

[group.fixtures]
must_match = []
"#
    )
}

#[test]
fn higher_threshold_repetition_group_wins_dedup() {
    // Given two propositional groups with thresholds 0.7 and 0.9.
    let (rules, warnings) = load_with(&[(
        "r.toml",
        &format!(
            "{}\n{}",
            repetition("REP-LOW", "propositional", "0.7"),
            repetition("REP-HIGH", "propositional", "0.9")
        ),
    )]);

    // Then only the stricter (higher) group survives.
    let ids: Vec<&str> = rules.groups.iter().map(|g| g.id_base.as_str()).collect();
    assert_eq!(ids, ["REP-HIGH"]);
    // And the drop is announced naming the loser.
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("propositional") && w.contains("REP-LOW")),
        "{warnings:?}"
    );
}

#[test]
fn equal_threshold_repetition_keeps_config_order() {
    // Given two propositional groups with the SAME threshold.
    let (rules, _) = load_with(&[(
        "r.toml",
        &format!(
            "{}\n{}",
            repetition("REP-FIRST", "propositional", "0.8"),
            repetition("REP-SECOND", "propositional", "0.8")
        ),
    )]);

    // Then the first-configured group keeps the variant.
    let ids: Vec<&str> = rules.groups.iter().map(|g| g.id_base.as_str()).collect();
    assert_eq!(ids, ["REP-FIRST"]);
}

#[test]
fn distinct_repetition_variants_do_not_collide() {
    // Given near-verbatim and propositional groups (different variants).
    let (rules, warnings) = load_with(&[(
        "r.toml",
        &format!(
            "{}\n{}",
            repetition("REP-NV", "near-verbatim", "0.6"),
            repetition("REP-PROP", "propositional", "0.8")
        ),
    )]);

    // Then both survive: they are different detectors, not duplicates.
    let mut ids: Vec<&str> = rules.groups.iter().map(|g| g.id_base.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["REP-NV", "REP-PROP"]);
    assert!(warnings.is_empty(), "{warnings:?}");
}
