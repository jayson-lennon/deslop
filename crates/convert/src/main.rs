//! deslop-convert: dev-time migrator from `third-party/*` sources to
//! committed TOML packs under `rules/builtin/**`.
//!
//! Deterministic: same third-party snapshots always produce byte-identical
//! output, which is what CI parity checks assert.

mod aatell;
mod emit;
pub mod merge;
mod notice_gen;
mod slop_json;
mod stopslop;
mod unslop_map;
#[doc(hidden)]
pub mod wsc_ts;

use std::path::Path;

use merge::MergedTerm;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let check = args.iter().any(|a| a == "--check");
    if let Err(e) = run(check) {
        eprintln!("deslop-convert: {e}");
        std::process::exit(1);
    }
}

const MIN_VOCAB_RAW: usize = 480;
const MIN_VOCAB_UNIQUE: usize = 400;
const MIN_WSC_PATTERNS: usize = 12;
const MIN_ARTIFACT_TERMS: usize = 40;
const MIN_METRIC_RULES: usize = 8;

fn run(check: bool) -> Result<(), String> {
    let root = Path::new("third-party");
    let mut raw = Vec::new();

    let slop_terms =
        slop_json::read(&root.join("anti-ai-slop/skill/scripts/anti_ai_slop/patterns.json"))?;
    println!("anti-ai-slop: {}", slop_terms.len());
    raw.extend(slop_terms);

    let wsc_src = read_file(&root.join("wsc/src/core/words.ts"))?;
    let wsc_words = wsc_ts::vocabulary(&wsc_src);
    let wsc_phrases = wsc_ts::phrases(&wsc_src);
    println!(
        "wsc: {} words + {} phrases",
        wsc_words.len(),
        wsc_phrases.len()
    );
    raw.extend(wsc_words);
    raw.extend(wsc_phrases);

    let aatell_terms = aatell::read(&root.join("anti-ai-tell/data/vocabulary.json"))?;
    println!("anti-ai-tell: {}", aatell_terms.len());
    raw.extend(aatell_terms);

    let stopslop_terms = stopslop::read(&root.join("stop-slop/references/phrases.md"))?;
    println!("stop-slop: {}", stopslop_terms.len());
    raw.extend(stopslop_terms);

    let raw_count = raw.len();
    let merged = merge::merge_vocab(raw);
    if merged.len() < MIN_VOCAB_UNIQUE || raw_count < MIN_VOCAB_RAW {
        return Err(format!(
            "minimum-count assertion failed: {raw_count} raw (min {MIN_VOCAB_RAW}), {} unique (min {MIN_VOCAB_UNIQUE})",
            merged.len()
        ));
    }
    println!("vocab: {raw_count} raw -> {} unique", merged.len());

    let patterns = wsc_ts::patterns(&wsc_src);
    println!("wsc patterns: {}", patterns.len());
    if patterns.len() < MIN_WSC_PATTERNS {
        return Err(format!(
            "minimum-count assertion failed: {} wsc patterns (min {MIN_WSC_PATTERNS})",
            patterns.len()
        ));
    }

    assert_authored_pack_counts()?;

    // unslop byte-map: verified readable, used later for normalization
    // parity tests (phase 5 goldens).
    let unslop_rules = unslop_map::read(&root.join("unslop/src/main.zig"))?;
    if unslop_rules.len() < 4 {
        return Err("unslop byte-map extraction came up short".into());
    }
    let byte_index = unslop_map::index(&unslop_rules);
    println!("unslop substitution rules: {}", byte_index.len());

    if check {
        // Parity: regenerate over a TEMP COPY of the packs (never the
        // working tree) and byte-compare against the committed files.
        let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
        let regen = tmp.path().join("regen");
        copy_dir(Path::new("rules/builtin"), &regen)?;
        emit_all_into(&regen, &merged, &patterns)?;
        let mut diff = diff_trees(Path::new("rules/builtin"), &regen)?;
        diff.sort();
        if !diff.is_empty() {
            return Err(format!(
                "parity check FAILED — committed packs differ from regenerated output:\n{}",
                diff.join("\n")
            ));
        }
        println!("parity check OK");
        return Ok(());
    }
    emit_all_into(Path::new("rules/builtin"), &merged, &patterns)?;
    Ok(())
}

fn read_file(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Emit every generated pack + NOTICE under `rules`.
fn emit_all_into(
    rules: &Path,
    terms: &[MergedTerm],
    patterns: &[wsc_ts::TsPattern],
) -> Result<(), String> {
    // modern-vocabulary: severity-ranked groups first (hard-ban /
    // strong-flag / watch: the user's [lints] control surface), then the
    // unclassified remainder chunked alphabetically as vocab-N.
    let pack = rules.join("modern-vocabulary");
    std::fs::create_dir_all(&pack).map_err(|e| e.to_string())?;
    // Hand-authored advice lives only in the current files; harvest for
    // every candidate name BEFORE clear_generated wipes them.
    let vocab_names: Vec<String> = ["hard-ban.toml", "strong-flag.toml", "watch.toml"]
        .iter()
        .map(|s| s.to_string())
        .chain((0..20).map(|i| format!("vocab-{}.toml", i + 1)))
        .collect();
    let saved_advice: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, String>,
    > = vocab_names
        .iter()
        .map(|n| (n.clone(), emit::harvest_advice(&pack, n)))
        .collect();
    let saved_of = |n: &str| saved_advice.get(n).cloned().unwrap_or_default();
    clear_generated(&pack)?;
    for rank in (1u8..=3).rev() {
        let picked: Vec<crate::merge::MergedTerm> = terms
            .iter()
            .filter(|t| t.severity == rank)
            .cloned()
            .collect();
        if picked.is_empty() {
            continue;
        }
        let (fname, id_base, advice) = match rank {
            3 => (
                "hard-ban.toml",
                "MODERN-VOCAB-HARD-BAN",
                Some("Highest measured AI ratios; no legitimate use offsets the signal"),
            ),
            2 => (
                "strong-flag.toml",
                "MODERN-VOCAB-STRONG-FLAG",
                Some("Strong AI tell; one use is worth flagging"),
            ),
            _ => (
                "watch.toml",
                "MODERN-VOCAB-WATCH",
                Some("Common word with measured AI excess; fine alone, a cluster is a tell"),
            ),
        };
        let saved = saved_of(fname);
        let body = emit::vocab_group(0, id_base, "ai-vocabulary", advice, &picked, &saved);
        std::fs::write(pack.join(fname), body).map_err(|e| e.to_string())?;
    }
    let rest: Vec<crate::merge::MergedTerm> =
        terms.iter().filter(|t| t.severity == 0).cloned().collect();
    for (i, chunk) in rest.chunks(200).enumerate() {
        let name = format!("vocab-{}.toml", i + 1);
        let saved = saved_of(&name);
        let body = emit::vocab_group(
            i,
            "MODERN-VOCAB",
            "ai-vocabulary",
            Some("AI-tell vocabulary; state the idea in your own plain words"),
            chunk,
            &saved,
        );
        std::fs::write(pack.join(&name), body).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        pack.join("NOTICE.toml"),
        notice_gen::render(&["anti-ai-slop", "wsc", "anti-ai-tell", "stop-slop"]),
    )
    .map_err(|e| e.to_string())?;

    // prose-constructions: one pattern file per wsc rule.
    let ppack = rules.join("prose-constructions");
    std::fs::create_dir_all(&ppack).map_err(|e| e.to_string())?;
    // Hand-curated [fixtures] must survive clear_generated: harvest first.
    let saved_fx: Vec<(Option<String>, Option<String>)> = patterns
        .iter()
        .map(|pat| {
            let fname = format!("{}.toml", emit::slugify(&pat.name));
            (
                existing_fixtures(&ppack, &fname),
                existing_regex(&ppack, &fname),
            )
        })
        .collect();
    clear_generated(&ppack)?;
    for (pat, (fixtures, regex)) in patterns.iter().zip(saved_fx) {
        let fname = format!("{}.toml", emit::slugify(&pat.name));
        std::fs::write(
            ppack.join(fname),
            pattern_group(pat, fixtures.as_deref(), regex.as_deref()),
        )
        .map_err(|e| e.to_string())?;
    }
    std::fs::write(ppack.join("NOTICE.toml"), notice_gen::render(&["wsc"]))
        .map_err(|e| e.to_string())?;

    Ok(())
}

fn clear_generated(pack: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(pack).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|x| x == "toml") {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Extract the `regex` body of the first `[[entries]]` in an existing
/// pattern file. Shipped patterns are hand-tuned forks of upstream (engine
/// fixes, capture insertion), so once authored they are the authority.
fn existing_regex(pack: &Path, fname: &str) -> Option<String> {
    let text = std::fs::read_to_string(pack.join(fname)).ok()?;
    let marker = "regex = '''";
    let start = text.find(marker)?;
    let body_start = start + marker.len() + 1;
    let rest = &text[body_start..];
    let end = rest.find("\n'''")?;
    Some(rest[..end].to_string())
}

/// Extract the hand-curated `[fixtures]` table (up to the next header).
fn existing_fixtures(pack: &Path, fname: &str) -> Option<String> {
    let text = std::fs::read_to_string(pack.join(fname)).ok()?;
    let start = text.find("[fixtures]")?;
    let rest = &text[start..];
    let end = rest["[fixtures]".len()..]
        .find("\n[")
        .map(|e| e + "[fixtures]".len())
        .unwrap_or(rest.len());
    Some(rest[..end].trim_end().to_string())
}

fn pattern_group(pat: &wsc_ts::TsPattern, fixtures: Option<&str>, regex: Option<&str>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "# Generated by crates/convert — do not edit by hand.");
    let _ = writeln!(
        out,
        "id-base = \"PROSE-PAT-{}\"",
        emit::slugify(&pat.name).to_uppercase()
    );
    let _ = writeln!(out, "kind = \"pattern\"");
    let _ = writeln!(out, "tier = 2");
    let _ = writeln!(out, "category = \"{}\"", pat.name);
    let _ = writeln!(out);
    let _ = writeln!(out, "[fixtures]");
    match fixtures {
        Some(fx) => {
            let _ = writeln!(out, "{}", fx.trim_start_matches("[fixtures]").trim_start());
        }
        None => {
            let _ = writeln!(out, "must_match = [] # TODO seed from upstream test corpus");
            let _ = writeln!(out, "must_not_match = []");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "[[entries]]");
    let _ = writeln!(out, "slug = \"main\"");

    // Regex source, in priority order: shipped fork > seeded transform >
    // raw upstream transcription.
    if let Some(rx) = regex {
        let _ = writeln!(out, "regex = {}", toml_lit_multiline(rx));
    } else {
        let seeded = match pat.name.as_str() {
            "negative-parallelism" => {
                let class_end = "{1,80}?[,;.:—–-]";
                Some(pat.pattern.replacen(
                    class_end,
                    &format!("{class_end}(?P<payload>[^.!?\\n]{{1,160}})"),
                    1,
                ))
            }
            "audience-hedge" => {
                let opened = pat
                    .pattern
                    .replacen("\\bwhether you", "(?P<hedge>\\bwhether you", 1);
                Some(opened.replacen(
                    "\\b[^.!?\\n]{1,80}?\\bor\\b",
                    "[^.!?\\n]{1,80}?\\bor\\b)",
                    1,
                ))
            }
            _ => None,
        };
        match seeded {
            Some(sx) => {
                let _ = writeln!(out, "regex = {}", toml_lit_multiline(&sx));
            }
            None => {
                let _ = writeln!(out, "regex = {}", toml_lit_multiline(&pat.pattern));
            }
        }
    }

    // Advice source: seeded advice for the two capture-seeded patterns,
    // generic wsc advice otherwise. (Hand-authored advice on shipped
    // forks would need the same preserve-as-raw treatment if it ever
    // diverges from this generic wording.)
    let advice_line = match pat.name.as_str() {
        "negative-parallelism" => {
            emit::toml_lit(emit::seed_pattern_advice(&pat.name).expect("pattern seed"))
        }
        "audience-hedge" => emit::toml_lit(
            "\"{hedge}\" flattens the audience into a marketing segment; address THIS reader's actual situation",
        ),
        _ => format!(
            "\"Rewrite the construction plainly (wsc: {})\"",
            escape_double(&pat.reason)
        ),
    };
    let _ = writeln!(out, "advice = {advice_line}");
    out
}

fn escape_double(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn toml_lit_multiline(s: &str) -> String {
    format!("'''\n{}\n'''", s.replace("'''", "'\\u0027\\u0027\\u0027'"))
}

/// Authored-pack minimums (AC4): artifact terms >= 40, metric rules >= 8.
fn assert_authored_pack_counts() -> Result<(), String> {
    let artifact_terms: usize = std::fs::read_dir(Path::new("rules/builtin/artifacts"))
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter(|e| e.file_name() != "NOTICE.toml")
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .map(|t| {
            let mut in_terms = false;
            t.lines()
                .filter(|l| {
                    let t = l.trim();
                    if t == "terms = [" {
                        in_terms = true;
                        return false;
                    }
                    if in_terms && (t == "]" || t == "],") {
                        in_terms = false;
                        return false;
                    }
                    in_terms && t.len() >= 3 && (t.starts_with('\'') || t.starts_with('"')) && {
                        let b = t.as_bytes();
                        b[b.len() - 1] == b','
                            && (b[b.len() - 2] == b'\'' || b[b.len() - 2] == b'"')
                    }
                })
                .count()
        })
        .sum();
    if artifact_terms < MIN_ARTIFACT_TERMS {
        return Err(format!(
            "minimum-count assertion failed: {artifact_terms} artifact terms (min {MIN_ARTIFACT_TERMS})"
        ));
    }
    let metric_rules = std::fs::read_dir(Path::new("rules/builtin/document-signals"))
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension().is_some_and(|x| x == "toml") && e.file_name() != "NOTICE.toml"
        })
        .count();
    if metric_rules < MIN_METRIC_RULES {
        return Err(format!(
            "minimum-count assertion failed: {metric_rules} metric rules (min {MIN_METRIC_RULES})"
        ));
    }
    Ok(())
}

/// Recursively copy a pack tree.
fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path.file_name().ok_or("bad file name")?.to_os_string();
        if path.is_dir() {
            copy_dir(&path, &dst.join(name))?;
        } else {
            std::fs::copy(&path, dst.join(name)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Names of files (recursively) whose contents differ, plus files missing
/// on either side.
/// Packs the converter owns (generated). Hand-authored packs are not
/// regenerated and thus out of parity scope.
const GENERATED_PACKS: [&str; 2] = ["modern-vocabulary", "prose-constructions"];

fn diff_trees(a: &Path, b: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for pack in GENERATED_PACKS {
        diff_walk(&a.join(pack), &a.join(pack), &b.join(pack), &mut out)?;
    }
    Ok(out)
}

fn diff_walk(root: &Path, a: &Path, b: &Path, out: &mut Vec<String>) -> Result<(), String> {
    for entry in std::fs::read_dir(a).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_path_buf();
        if path.is_dir() {
            diff_walk(root, &path, &b.join(&rel), out)?;
        } else {
            let other = b.join(&rel);
            let same = match (std::fs::read(&path), std::fs::read(&other)) {
                (Ok(x), Ok(y)) => x == y,
                _ => false,
            };
            if !same {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    Ok(())
}
