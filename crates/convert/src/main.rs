//! deslop-convert: dev-time migrator from `third-party/*` sources to
//! committed TOML packs under `rules/builtin/**`.
//!
//! Deterministic: same third-party snapshots always produce byte-identical
//! output, which is what CI parity checks assert.

mod aatell;
mod emit;
mod merge;
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

    emit_all(&merged, &patterns)?;

    if check {
        verify_parity()?;
    }
    Ok(())
}

fn read_file(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Emit every generated pack + NOTICE.
fn emit_all(terms: &[MergedTerm], patterns: &[wsc_ts::TsPattern]) -> Result<(), String> {
    let rules = Path::new("rules/builtin");

    // modern-vocabulary: deterministic chunked files.
    let pack = rules.join("modern-vocabulary");
    std::fs::create_dir_all(&pack).map_err(|e| e.to_string())?;
    clear_generated(&pack)?;
    for (i, chunk) in terms.chunks(200).enumerate() {
        let name = format!("vocab-{}.toml", i + 1);
        let body = emit::vocab_group(
            i,
            "MODERN-VOCAB",
            "ai-vocabulary",
            Some("AI-tell vocabulary; state the idea in your own plain words"),
            chunk,
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
    clear_generated(&ppack)?;
    for pat in patterns {
        let fname = format!("{}.toml", emit::slugify(&pat.name));
        std::fs::write(ppack.join(fname), pattern_group(pat)).map_err(|e| e.to_string())?;
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

fn pattern_group(pat: &wsc_ts::TsPattern) -> String {
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
    let _ = writeln!(out, "must_match = [] # TODO seed from upstream test corpus");
    let _ = writeln!(out, "must_not_match = []");
    let _ = writeln!(out);
    let _ = writeln!(out, "[[entries]]");
    let _ = writeln!(out, "slug = \"main\"");
    // Seed (t7c7): negative-parallelism carries a named capture so golden
    // tests exercise {payload} interpolation.
    if pat.name == "negative-parallelism" {
        let class_end = "{1,80}?[,;.:—–-]";
        let seeded = pat.pattern.replacen(
            class_end,
            &format!("{class_end}(?P<payload>[^.!?\\n]{{1,160}})"),
            1,
        );
        let _ = writeln!(out, "regex = {}", toml_lit_multiline(&seeded));
        let _ = writeln!(
            out,
            "advice = {}",
            emit::toml_lit(emit::seed_pattern_advice(&pat.name).expect("pattern seed"),)
        );
    } else if pat.name == "audience-hedge" {
        // Seed (t7c7): single named capture, advice echoes it back.
        let seeded = pat
            .pattern
            .replacen("\\bwhether you", "(?P<hedge>\\bwhether you", 1);
        let seeded = seeded.replacen(
            "\\b[^.!?\\n]{1,80}?\\bor\\b",
            "[^.!?\\n]{1,80}?\\bor\\b)",
            1,
        );
        let _ = writeln!(out, "regex = {}", toml_lit_multiline(&seeded));
        let _ = writeln!(
            out,
            "advice = {}",
            emit::toml_lit(
                "\"{hedge}\" flattens the audience into a marketing segment; address THIS reader's actual situation"
            )
        );
    } else {
        let _ = writeln!(out, "regex = {}", toml_lit_multiline(&pat.pattern));
        let _ = writeln!(
            out,
            "advice = \"Rewrite the construction plainly (wsc: {})\"",
            escape_double(&pat.reason)
        );
    }
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

/// Byte-compare emitted output vs committed packs. Because emission writes
/// relative ./rules paths, parity is proven by regenerating into a clean
/// checkout in CI and asserting empty `git status --porcelain` on rules/.
fn verify_parity() -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain", "rules/"])
        .output()
        .map_err(|e| e.to_string())?;
    let status = String::from_utf8_lossy(&out.stdout);
    if status.trim().is_empty() {
        Ok(())
    } else {
        Err(format!(
            "parity check FAILED — committed rules/ differ from regenerated output:\n{status}"
        ))
    }
}
