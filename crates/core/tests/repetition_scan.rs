//! Repetition scan integration: three variants through the public `scan`
//! surface with hermetic packs and the `FakeEmbedder` seam.

use deslop_core::embedder::FakeEmbedder;
use deslop_core::scanner::{LintSettings, scan_with_plugins};

fn pack(toml: &str) -> deslop_core::rule::RuleSet {
    let tmp = tempfile::tempdir().expect("tmp");
    let p = tmp.path().join("rules").join("rep.toml");
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(&p, toml).expect("write");
    let cfg = deslop_core::config::Config {
        packs: deslop_core::config::Packs {
            builtin: vec!["rep".into()],
            extra_paths: vec![],
        },
        ..deslop_core::config::Config::default()
    };
    let loaded = deslop_core::rule::loader::load(
        &cfg,
        camino::Utf8Path::from_path(tmp.path()).expect("utf8"),
        None,
    );
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    loaded.rule_set
}

fn near_verbatim_pack() -> deslop_core::rule::RuleSet {
    pack(
        r#"
[[group]]
id-base = "REP-NV"
kind = "repetition"
tier = 2
category = "repetition"
variant = "near-verbatim"
threshold = 0.5
message = "Near-verbatim repetition across {count} sentences"
advice = "Cut or merge the repeats; say the new thing once."

[group.fixtures]
must_match = []
"#,
    )
}

#[test]
fn near_verbatim_flags_paraphrased_repeat() {
    // Given a near-verbatim pack and a doc repeating one sentence twice.
    let rules = near_verbatim_pack();
    let src = "First intro line.\n\n\
        The committee approved the plan on Tuesday morning.\n\
        Some other detail here that stands alone.\n\
        The committee approved the plan on Tuesday morning today.\n";

    // When scanning with no embedder.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then one repetition finding reports both members.
    let reps: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.entry_id == "REP-NV")
        .collect();
    assert_eq!(reps.len(), 1, "{:?}", out.findings);
    let ctx = reps[0].context.as_deref().expect("context");
    assert!(ctx.contains("line 3:"), "{ctx}");
    assert!(ctx.contains("line 5:"), "{ctx}");
    // And the message carries the member count.
    assert_eq!(
        reps[0].message,
        "Near-verbatim repetition across 2 sentences"
    );
}

#[test]
fn near_verbatim_ignores_unrelated_sentences() {
    // Given a doc whose sentences share no shingles.
    let rules = near_verbatim_pack();
    let src = "The cat sat on the warm mat.\n\
        Ancient Romans built aqueducts for water.\n";

    // When scanning.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then nothing fires.
    assert!(
        !out.findings.iter().any(|f| f.entry_id == "REP-NV"),
        "{:?}",
        out.findings
    );
}

#[test]
fn near_verbatim_spans_work_across_blank_line() {
    // Given the repeated sentence pair split by a blank line.
    let rules = near_verbatim_pack();
    let src = "The committee approved the plan on Tuesday morning.\n\n\
        The committee approved the plan on Tuesday morning.\n";

    // When scanning.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then the cross-paragraph pair still clusters into one finding.
    let reps: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.entry_id == "REP-NV")
        .collect();
    assert_eq!(reps.len(), 1, "{:?}", out.findings);
}

#[test]
fn short_one_liners_never_fire_near_verbatim() {
    // Given a doc of five-word-or-fewer stub lines that would otherwise match.
    let rules = near_verbatim_pack();
    let src = "Nope.\n\nBeautiful system. Fantastic.\n\nNope.\n";

    // When scanning.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then nothing fires: units below five words are excluded.
    assert!(
        !out.findings.iter().any(|f| f.entry_id == "REP-NV"),
        "{:?}",
        out.findings
    );
}

fn propositional_pack() -> deslop_core::rule::RuleSet {
    pack(
        r#"
[[group]]
id-base = "REP-PROP"
kind = "repetition"
tier = 2
category = "repetition"
variant = "propositional"
threshold = 0.85
message = "Repeated proposition across {count} sentences"

[group.fixtures]
must_match = []
"#,
    )
}

#[test]
fn propositional_flags_similar_embedding_clusters() {
    // Given a fake embedder giving three near-identical vectors to a family.
    let rules = propositional_pack();
    // The sentences paraphrase one another (similar meaning, different
    // words) so they pass the propositional bar without tripping the
    // near-verbatim suppression rule.
    let src = "Alpha wrote a sweeping history of the canal project.\n\
        Beta covered the canal project in a very long book.\n\
        Gamma described the canal project at great length.\n\
        Wholly different content entirely about bread prices this winter.\n";
    let embedder = FakeEmbedder::new(|s: &str| {
        if s.starts_with("Wholly") {
            vec![0.0, 1.0]
        } else {
            vec![1.0, 0.0]
        }
    });

    // When scanning with that embedder.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], Some(&embedder));

    // Then one finding lists the three family members.
    let reps: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.entry_id == "REP-PROP")
        .collect();
    assert_eq!(reps.len(), 1, "{:?}", out.findings);
    let ctx = reps[0].context.as_deref().expect("context");
    assert_eq!(ctx.lines().count(), 4); // header + 3 members
}

#[test]
fn propositional_suppresses_when_all_members_near_verbatim() {
    // Given two sentences that are BOTH near-verbatim AND embedding-identical.
    let rules = pack(
        r#"
[[group]]
id-base = "REP-NV"
kind = "repetition"
tier = 2
category = "repetition"
variant = "near-verbatim"
threshold = 0.5

[group.fixtures]
must_match = []

[[group]]
id-base = "REP-PROP"
kind = "repetition"
tier = 2
category = "repetition"
variant = "propositional"
threshold = 0.85

[group.fixtures]
must_match = []
"#,
    );
    let src = "The committee approved the plan on Tuesday morning.\n\
        The committee approved the plan, on Tuesday morning.\n";
    let embedder = FakeEmbedder::new(|_| vec![1.0, 0.0]);

    // When scanning with both packs enabled.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], Some(&embedder));
    let nv: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.entry_id == "REP-NV")
        .collect();
    let prop: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.entry_id == "REP-PROP")
        .collect();

    // Then the near-verbatim lint reports it and propositional stays quiet.
    assert_eq!(nv.len(), 1, "nv: {:?}", out.findings);
    assert_eq!(prop.len(), 0, "prop: {:?}", out.findings);
}

#[test]
fn propositional_skips_with_warning_when_embedder_missing() {
    // Given only the propositional pack and no embedder.
    let rules = propositional_pack();
    let src = "Alpha sentence one.\nBeta sentence two.\n";

    // When scanning.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then no repetition findings, and one skip warning names the model.
    assert!(
        !out.findings.iter().any(|f| f.entry_id == "REP-PROP"),
        "{:?}",
        out.findings
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("all-MiniLM-L6-v2")),
        "{:?}",
        out.warnings
    );
}

fn content_family_pack() -> deslop_core::rule::RuleSet {
    pack(
        r#"
[[group]]
id-base = "REP-FAM"
kind = "repetition"
tier = 3
category = "repetition"
variant = "content-family"
threshold = 0.35
min-members = 3

[group.fixtures]
must_match = []
"#,
    )
}

#[test]
fn content_family_clusters_three_diffuse_paragraphs() {
    // Given four paragraphs circling one narrow idea plus one outlier.
    // Below the eight-paragraph bar the ubiquitous filter is inert, so the
    // shared "canal" still links the family.
    let rules = content_family_pack();
    let src = "The canal excavation crews demanded endless volcanic rock.\n\n\
        The canal excavation crews nearly surrendered twice.\n\n\
        The canal excavation crews faced the hardest challenge.\n\n\
        The canal excavation crews called it the greatest dig.\n\n\
        Unrelated: the price of bread rose sharply that winter.\n";

    // When scanning.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then one family finding covers the three canal paragraphs.
    let reps: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.entry_id == "REP-FAM")
        .collect();
    assert_eq!(reps.len(), 1, "{:?}", out.findings);
    let ctx = reps[0].context.as_deref().expect("context");
    assert!(ctx.contains("line 1:"), "{ctx}");
    assert!(ctx.contains("line 3:"), "{ctx}");
    assert!(ctx.contains("line 5:"), "{ctx}");
    assert!(ctx.contains("line 7:"), "{ctx}");
    assert!(!ctx.contains("line 9:"), "{ctx}");
}

#[test]
fn repetition_allow_override_silences_group() {
    // Given the near-verbatim pack allowed via [lints].
    let rules = near_verbatim_pack();
    let mut settings = LintSettings::default();
    settings
        .levels
        .insert("REP-NV".to_string(), deslop_core::config::LintLevel::Allow);
    let src = "The committee approved the plan on Tuesday morning.\n\n\
        The committee approved the plan on Tuesday morning.\n";

    // When scanning with the override.
    let out = scan_with_plugins(src, &rules, &settings, &[], None);

    // Then the finding is silenced.
    assert!(
        !out.findings.iter().any(|f| f.entry_id == "REP-NV"),
        "{:?}",
        out.findings
    );
}

#[test]
fn repetition_output_is_byte_identical_across_runs() {
    // Given a firing document.
    let rules = near_verbatim_pack();
    let src = "The committee approved the plan on Tuesday morning.\n\n\
        The committee approved the plan on Tuesday morning.\n";

    // When scanning twice.
    let a = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);
    let b = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then both runs produce identical findings (determinism).
    assert_eq!(a.findings, b.findings);
}

fn nv_pack_with(max_distance: &str) -> deslop_core::rule::RuleSet {
    pack(&format!(
        r#"
[[group]]
id-base = "REP-NV"
kind = "repetition"
tier = 2
category = "repetition"
variant = "near-verbatim"
threshold = 0.5
{max_distance}

[group.fixtures]
must_match = []
"#
    ))
}

fn filler(n_words: usize) -> String {
    (0..n_words)
        .map(|i| format!("w{i} "))
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn close_twins_still_fire_within_default_distance() {
    // Given twin sentences adjacent to each other.
    let rules = near_verbatim_pack();
    let src = "The committee approved the plan on Tuesday morning.\n\n\
        The committee approved the plan on Tuesday morning today.\n";

    // When scanning.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then the pair fires (distance well under the 200-token default).
    assert_eq!(
        out.findings
            .iter()
            .filter(|f| f.entry_id == "REP-NV")
            .count(),
        1,
        "{:?}",
        out.findings
    );
}

#[test]
fn far_twins_go_silent_beyond_max_distance() {
    // Given twin sentences separated by more than 200 tokens of filler.
    let src = format!(
        "The committee approved the plan on Tuesday morning.\n\n{}\n\n\
        The committee approved the plan on Tuesday morning today.\n",
        filler(250)
    );
    let rules = near_verbatim_pack();

    // When scanning.
    let out = scan_with_plugins(&src, &rules, &LintSettings::default(), &[], None);

    // Then no finding: the pair is beyond the default cap.
    assert!(
        !out.findings.iter().any(|f| f.entry_id == "REP-NV"),
        "{:?}",
        out.findings
    );
}

#[test]
fn exactly_at_max_distance_still_fires() {
    // Given twin sentences separated by filler so the gap equals the cap.
    // Twin 1 is 8 tokens; twin 2 starts at token index 18 (8 twin words
    // + 10 filler words — blank lines carry no tokens), so a cap of 18
    // lands exactly on the inclusive boundary.
    let src = format!(
        "The committee approved the plan on Tuesday morning.\n\n{}\n\n\
        The committee approved the plan on Tuesday morning today.\n",
        filler(10)
    );
    let rules = nv_pack_with("max-distance = 18");

    // When scanning.
    let out = scan_with_plugins(&src, &rules, &LintSettings::default(), &[], None);

    // Then the pair still fires: the boundary is inclusive.
    let gap = deslop_core::scanner::repetition::tokens_between(
        &deslop_core::scanner::repetition::token_positions(&src),
        0,
        src.find("The committee approved the plan on Tuesday morning today.")
            .expect("twin 2"),
    );
    eprintln!("DBG actual gap = {gap}");
    assert_eq!(gap, 18);
    assert_eq!(
        out.findings
            .iter()
            .filter(|f| f.entry_id == "REP-NV")
            .count(),
        1,
        "{:?}",
        out.findings
    );
}

#[test]
fn explicit_max_distance_tightens_the_cap() {
    // Given a cap of 5 and twins separated by ~15 filler tokens.
    let src = format!(
        "The committee approved the plan on Tuesday morning.\n\n{}\n\n\
        The committee approved the plan on Tuesday morning today.\n",
        filler(15)
    );
    let rules = nv_pack_with("max-distance = 5");

    // When scanning.
    let out = scan_with_plugins(&src, &rules, &LintSettings::default(), &[], None);

    // Then the pair is beyond even its similarity: silent.
    assert!(
        !out.findings.iter().any(|f| f.entry_id == "REP-NV"),
        "{:?}",
        out.findings
    );
}

#[test]
fn far_member_cannot_bridge_a_chain() {
    // Given A~B close (near-verbatim pair) and C similar to B but far away.
    let src = format!(
        "The committee approved the plan on Tuesday morning.\n\
        The committee approved the plan on Tuesday morning today.\n\n{}\n\n\
        The committee approved the plan on Tuesday morning again.\n",
        filler(250)
    );
    let rules = nv_pack_with("max-distance = 5");

    // When scanning.
    let out = scan_with_plugins(&src, &rules, &LintSettings::default(), &[], None);

    // Then any component that forms has only its close members, never all 3.
    for f in out.findings.iter().filter(|f| f.entry_id == "REP-NV") {
        let ctx = f.context.as_deref().expect("context");
        assert_eq!(ctx.lines().count(), 3, "{ctx}"); // header + 2 members
    }
}

#[test]
fn content_family_ignores_max_distance() {
    // Given the far-flung canal family (members hundreds of tokens apart)
    // under a tiny cap on the content-family group.
    let src = "The canal excavation crews demanded endless volcanic rock.\n\n\
        The canal excavation crews nearly surrendered twice.\n\n\
        The canal excavation crews faced the hardest challenge.\n\n\
        The canal excavation crews called it the greatest dig.\n\n\
        Unrelated: the price of bread rose sharply that winter.\n";
    let rules = pack(
        r#"
[[group]]
id-base = "REP-FAM"
kind = "repetition"
tier = 3
category = "repetition"
variant = "content-family"
threshold = 0.5
min-members = 3
max-distance = 2

[group.fixtures]
must_match = []
"#,
    );

    // When scanning.
    let out = scan_with_plugins(src, &rules, &LintSettings::default(), &[], None);

    // Then the family still fires: content-family is distance-blind.
    let fams = out
        .findings
        .iter()
        .filter(|f| f.entry_id == "REP-FAM")
        .count();
    assert_eq!(fams, 1, "{:?}", out.findings);
}
