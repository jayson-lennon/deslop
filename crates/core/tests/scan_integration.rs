//! Black-box tests over the PUBLIC scan API (spec cases T1, T3, T4, T11, T13).

use deslop_core::config::Config;
use deslop_core::rule::loader;
use deslop_core::scanner::{LintSettings, scan};

fn load_with(toml_files: &[(&str, &str)]) -> deslop_core::rule::RuleSet {
    let tmp = tempfile::tempdir().expect("tmp");
    for (name, content) in toml_files {
        let p = tmp.path().join("rules").join(name);
        let parent = p.parent().expect("parent");
        std::fs::create_dir_all(parent).expect("nested");
        std::fs::write(p, content).expect("write rule");
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
    let loaded = loader::load(&cfg, camino::Utf8Path::from_path(tmp.path()).expect("utf8"));
    assert!(
        loaded.errors.is_empty(),
        "fixture rules must load cleanly: {:?}",
        loaded.errors
    );
    loaded.rule_set
}

const LITERAL_RULE: &str = r#"
[[group]]
id-base = "TEST-LIT"
kind = "literal-ban"
tier = 1
category = "artifact"

[group.fixtures]
must_match = ["see contentReference[oaicite:4]{index=4} here"]

[[group.entries]]
slug = "oaicite"
terms = ['contentReference[oaicite:{N}]{{index={N}}}']
"#;

const VOCAB_RULE: &str = r#"
[[group]]
id-base = "TEST-VOCAB"
kind = "vocab"
tier = 2
category = "cliche"

[group.fixtures]
must_match = ["a testament to her vision"]

[[group.entries]]
slug = "testament"
terms = ["testament to"]
advice = 'state the claim directly instead'
"#;

const STEM_RULE: &str = r#"
[[group]]
id-base = "TEST-STEM"
kind = "vocab"
tier = 2
category = "cliche"

[group.fixtures]
must_match = ["we delve deeper", "he delved deeper"]

[[group.entries]]
slug = "delve"
terms = ["delve"]
stems = true
"#;

#[test]
fn t1_chatgpt_artifact_flagged_at_tier_one() {
    // Given a doc containing a pasted ChatGPT citation artifact.
    let rules = load_with(&[("lit.toml", LITERAL_RULE)]);
    let src = "See this thing contentReference[oaicite:16]{index=16} ok?";

    // When scanning.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then exactly one Tier-1 finding with an exact excerpt.
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.tier, deslop_core::finding::Tier::Artifact);
    assert_eq!(f.excerpt, src[f.span.start..f.span.end]);
}

#[test]
fn t3_wikipedia_placeholder_in_ref_html_flagged() {
    // Given a raw <ref> citation paste with an unfilled URL placeholder.
    let rules = load_with(&[("lit.toml", LITERAL_RULE)]);
    let src = "<ref>{{cite web |url=URL |title=x}}</ref>";
    let rules2 = load_with(&[(
        "ph.toml",
        r#"
[[group]]
id-base = "TEST-PH"
kind = "literal-ban"
tier = 1
category = "placeholder"

[group.fixtures]
must_match = ["|url=URL "]

[[group.entries]]
slug = "url-placeholder"
terms = ["|url=URL ", "|url=PASTE_"]
"#,
    )]);

    // When scanning both rulesets.
    let a = scan(src, &rules, &LintSettings::default());
    let b = scan(src, &rules2, &LintSettings::default());

    // Then the ref-paste placeholder fires (raw HTML is visible text).
    assert!(!b.is_empty(), "placeholder missed in html paste");
    // And unrelated artifacts don't fire on clean-ish text.
    assert!(a.is_empty());
}

#[test]
fn t4_vocab_tell_with_interpolated_advice() {
    // Given the seeded vocab tell.
    let rules = load_with(&[("v.toml", VOCAB_RULE)]);
    let src = "Her archive is a testament to her vision.";

    // When scanning.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then one Tier-2 hit whose excerpt equals the matched phrase and whose
    // advice survived interpolation.
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f.tier, deslop_core::finding::Tier::Tell);
    assert_eq!(f.excerpt, "testament to");
    assert_eq!(
        f.advice.as_deref(),
        Some("state the claim directly instead")
    );
}

#[test]
fn t11_code_fence_and_inline_code_stay_silent() {
    // Given banned terms locked inside code constructs.
    let rules = load_with(&[
        ("v.toml", VOCAB_RULE),
        ("s.toml", STEM_RULE),
        ("lit.toml", LITERAL_RULE),
    ]);
    let src = "```\ndelve\ncontentReference[oaicite:9]{index=9}\n```\nrun `testament to` now\n";

    // When scanning.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then nothing fires from either construct.
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn t13_inflected_term_hits_without_duplicates() {
    // Given stems expansion on `delve`.
    let rules = load_with(&[("s.toml", STEM_RULE)]);

    // When scanning a doc using both base and inflected forms.
    let src = "we delve deeper; earlier he delved deeper";
    let findings = scan(src, &rules, &LintSettings::default());

    // Then each form fires exactly once (two hits total).
    assert_eq!(findings.len(), 2, "{findings:?}");
    let excerpts: Vec<&str> = findings.iter().map(|f| f.excerpt.as_str()).collect();
    assert!(excerpts.contains(&"delve"));
    assert!(excerpts.contains(&"delved"));
}

#[test]
fn excerpts_always_equal_source_slices() {
    // Given mixed rules firing on a messy doc.
    let rules = load_with(&[
        ("v.toml", VOCAB_RULE),
        ("s.toml", STEM_RULE),
        ("lit.toml", LITERAL_RULE),
    ]);
    let src = "delve — a testament to her vision — contentReference[oaicite:2]{index=2}";

    // When scanning.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then EVERY excerpt byte-equals its span slice.
    for f in &findings {
        assert_eq!(f.excerpt, &src[f.span.start..f.span.end], "{}", f.entry_id);
    }
}

#[test]
fn deterministic_sort_order_across_runs() {
    // Given any doc and ruleset.
    let rules = load_with(&[
        ("v.toml", VOCAB_RULE),
        ("s.toml", STEM_RULE),
        ("lit.toml", LITERAL_RULE),
    ]);
    let src = "delve testament to contentReference[oaicite:1]{index=1} delve";

    // When scanning twice.
    let a = scan(src, &rules, &LintSettings::default());
    let b = scan(src, &rules, &LintSettings::default());

    // Then orderings are identical and sorted by offset.
    assert_eq!(a, b);
    let offsets: Vec<usize> = a.iter().map(|f| f.span.start).collect();
    let mut sorted = offsets.clone();
    sorted.sort();
    assert_eq!(offsets, sorted);
}

const PATTERN_RULE: &str = r#"
[[group]]
id-base = "TEST-PAT"
kind = "pattern"
tier = 2
category = "construction"

[group.fixtures]
must_match = ["stands as a testament to the effort"]

[[group.entries]]
slug = "main"
regex = 'stands as an?'
"#;

#[test]
fn same_term_in_two_groups_reports_one_finding_from_highest_tier() {
    // Given the same term in a tier-2 group and a tier-3 group (one file,
    // two groups — dedup keeps the stricter tier 2).
    let shadow = VOCAB_RULE
        .replace("TEST-VOCAB", "TEST-VOCAB-2")
        .replace("tier = 2", "tier = 3");
    let rules = load_with(&[("v.toml", &format!("{VOCAB_RULE}\n{shadow}"))]);
    let src = "a testament to her vision";

    // When scanning.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then exactly one finding, owned by the tier-2 group.
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].entry_id, "TEST-VOCAB#testament");
    assert_eq!(findings[0].tier, deslop_core::finding::Tier::Tell);
}

#[test]
fn identical_regex_string_in_two_groups_fans_out_to_both_owners() {
    // Given two groups sharing the exact same regex string.
    let shadow = PATTERN_RULE.replace("TEST-PAT", "TEST-PAT-2");
    let rules = load_with(&[("p.toml", &format!("{PATTERN_RULE}\n{shadow}"))]);
    let src = "it stands as a testament to the effort";

    // When scanning.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then ONE compiled regex produced a finding for EACH owner
    // (`#` sorts before `-`, so TEST-PAT#main comes first).
    let mut ids: Vec<&str> = findings.iter().map(|f| f.entry_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["TEST-PAT#main", "TEST-PAT-2#main"]);
}

#[test]
fn distinct_regex_strings_are_not_merged() {
    // Given two groups with DIFFERENT regex strings.
    let other = PATTERN_RULE
        .replace("stands as an?", "serves as a")
        .replace("TEST-PAT", "TEST-PAT-2")
        .replace("stands as a testament to the effort", "serves as a beacon of hope");
    let rules = load_with(&[("p.toml", &format!("{PATTERN_RULE}\n{other}"))]);
    let src = "it stands as a testament; it serves as a beacon";

    // When scanning.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then each string matched on its own: two findings, one per group.
    assert_eq!(findings.len(), 2, "{findings:?}");
    let mut ids: Vec<&str> = findings.iter().map(|f| f.entry_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["TEST-PAT#main", "TEST-PAT-2#main"]);
}

#[test]
fn metric_conflict_keeps_stricter_threshold_and_warns() {
    // Given two metric groups with the same (stat, window, terms) but
    // different thresholds.
    let metric = |gid: &str, th: &str| {
        format!(
            r#"
[[group]]
id-base = "{gid}"
kind = "metric"
tier = 3
category = "signals"
stat = "term_cluster_max"
threshold-gt = {th}
window = "paragraph"
terms = ["delve", "garner"]
"#
        )
    };
    let rules = load_with(&[("m.toml", &metric("METRIC-A", "1"))]);
    let rules_both = load_with(&[(
        "m.toml",
        &format!("{}\n{}", metric("METRIC-A", "1"), metric("METRIC-B", "5")),
    )]);
    let src = "we delve and garner; we delve and garner; we delve and garner";

    // When scanning the single-metric ruleset.
    let findings = scan(src, &rules, &LintSettings::default());

    // Then threshold 1 fires on 2 distinct terms per paragraph.
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].entry_id, "METRIC-A");

    // And when both exist, the STRICTER threshold survives.
    let findings2 = scan(src, &rules_both, &LintSettings::default());
    assert_eq!(findings2.len(), 1, "{findings2:?}");
    assert_eq!(findings2[0].entry_id, "METRIC-A");
}
