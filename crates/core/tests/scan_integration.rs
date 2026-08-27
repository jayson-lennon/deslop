//! Black-box tests over the PUBLIC scan API (spec cases T1, T3, T4, T11, T13).

use deslop_core::config::Config;
use deslop_core::rule::loader;
use deslop_core::scanner::{LintSettings, scan};

fn load_with(toml_files: &[(&str, &str)]) -> deslop_core::rule::RuleSet {
    let tmp = tempfile::tempdir().expect("tmp");
    let pack = tmp.path().join("rules/builtin/t");
    std::fs::create_dir_all(&pack).expect("pack dir");
    for (name, content) in toml_files {
        let p = pack.join(name);
        let parent = p.parent().expect("parent");
        std::fs::create_dir_all(parent).expect("nested");
        std::fs::write(p, content).expect("write rule");
    }
    let cfg = Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["t".into()],
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
id-base = "TEST-LIT"
kind = "literal-ban"
tier = 1
category = "artifact"

[fixtures]
must_match = ["see contentReference[oaicite:4]{index=4} here"]

[[entries]]
slug = "oaicite"
terms = ['contentReference[oaicite:{N}]{{index={N}}}']
"#;

const VOCAB_RULE: &str = r#"
id-base = "TEST-VOCAB"
kind = "vocab"
tier = 2
category = "cliche"

[fixtures]
must_match = ["a testament to her vision"]

[[entries]]
slug = "testament"
terms = ["testament to"]
advice = 'state the claim directly instead'
"#;

const STEM_RULE: &str = r#"
id-base = "TEST-STEM"
kind = "vocab"
tier = 2
category = "cliche"

[fixtures]
must_match = ["we delve deeper", "he delved deeper"]

[[entries]]
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
id-base = "TEST-PH"
kind = "literal-ban"
tier = 1
category = "placeholder"

[fixtures]
must_match = ["|url=URL "]

[[entries]]
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
