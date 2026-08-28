//! `deslop fix`: mechanical rewrites from vocab entries carrying a
//! replacement. Dry-run by default; `--write` applies.

use std::collections::BTreeMap;
use std::io::Write;

use deslop_core::config::Config;
use deslop_core::finding::{Finding, KindTag, Span};
use deslop_core::scanner;

/// Result tallies for one run.
#[derive(Debug, Default)]
pub struct FixSummary {
    /// Rewrites applied to disk (or that would apply in dry-run).
    pub applied: usize,
    /// Replacement-bearing hits skipped because they overlap another
    /// finding's span of any kind.
    pub skipped_overlaps: usize,
    /// Findings that remain unfixable after the pass (report-only hits).
    pub unfixable: usize,
    pub files_changed: BTreeMap<String, usize>,
}

/// Context for one fix invocation.
pub struct FixCmd<'a> {
    pub cfg: &'a Config,
    /// `--rules-dir` override: the directory containing pack TOMLs.
    pub rules_dir: Option<camino::Utf8PathBuf>,
    pub write: bool,
    pub color_override: Option<deslop_core::config::ColorChoice>,
    pub format_override: Option<deslop_core::config::FormatName>,
    /// Loaded `[plugins]` modules; empty when none configured. Plugins are
    /// report-only: their findings never participate in fixes.
    pub plugins: Vec<Box<dyn deslop_core::plugin::LintPlugin>>,
}

impl FixCmd<'_> {
    /// Execute; returns process exit code.
    ///
    /// # Errors
    ///
    /// Fails when pack loading fails or a document cannot be read/written.
    pub fn run(&mut self) -> Result<i32, error_stack::Report<super::CmdError>> {
        let loaded = crate::cmd::rules_cmd::load_for_lint(self.cfg, self.rules_dir.clone());
        if !loaded.errors.is_empty() {
            let stderr = std::io::stderr();
            let mut lock = stderr.lock();
            let _ = crate::render::human::render_load_errors(&loaded.errors, &mut lock);
            return Err(super::fail("rule packs failed to load"));
        }

        let paths: Vec<camino::Utf8PathBuf> = vec![".".into()];
        let corpus =
            match crate::cmd::lint_cmd::Corpus::gather(&paths, self.cfg.scan.respect_gitignore) {
                Ok(c) => c,
                Err(e) => return Err(super::fail(format!("cannot read input: {e}"))),
            };

        let settings = scanner::LintSettings {
            max_tier: None,
            levels: self.cfg.lint.clone(),
        };
        let grand_total = {
            let mut total = FixSummary::default();
            for doc in &corpus.docs {
                // Fix remains plugin-free by design: findings from plugins
                // would only pollute overlap accounting, so no plugin pass
                // is appended here.
                let outcome = scanner::scan_with_plugins(
                    &doc.src,
                    &loaded.rule_set,
                    &settings,
                    &self.plugins,
                );
                let findings = outcome.findings;
                for warning in &outcome.warnings {
                    eprintln!("{warning}");
                }
                let summary = fix_document(&findings);
                if summary.applied == 0 {
                    total.unfixable += summary.unfixable;
                    continue;
                }
                if self.write {
                    let rewritten = rewrite(&doc.src, &findings);
                    if let Err(e) = std::fs::write(&doc.path, rewritten) {
                        return Err(super::fail(format!("cannot write {}: {e}", doc.path)));
                    }
                }
                total.applied += summary.applied;
                total.skipped_overlaps += summary.skipped_overlaps;
                total.unfixable += summary.unfixable;
                *total.files_changed.entry(doc.path.to_string()).or_default() += summary.applied;
            }
            total
        };

        report(&grand_total, self.write);

        // Exit contract matches lint: any remaining artifact/tell (i.e. the
        // skipped overlaps or report-only hits at tiers 1-2) => 1.
        let dirty = grand_total.skipped_overlaps > 0 || grand_total.unfixable > 0;
        Ok(if dirty {
            crate::ExitCode::FindingsReported as i32
        } else {
            crate::ExitCode::Clean as i32
        })
    }
}

/// One loop per function: classify candidates vs everything else.
fn fix_document(findings: &[Finding]) -> FixSummary {
    let mut summary = FixSummary::default();
    for finding in findings {
        match findable(finding, findings) {
            Fixability::Fixable => summary.applied += 1,
            Fixability::Overlapped => summary.skipped_overlaps += 1,
            Fixability::NotFixable => {
                // Only artifact/tell severities keep the run "dirty";
                // hints never affect exit per spec.
                if finding.tier != deslop_core::finding::Tier::Density {
                    summary.unfixable += 1;
                }
            }
        }
    }
    summary
}

enum Fixability {
    Fixable,
    Overlapped,
    NotFixable,
}

fn findable(finding: &Finding, findings: &[Finding]) -> Fixability {
    let fixable = finding.kind == KindTag::Vocab && finding.replacement.is_some();
    if !fixable {
        return Fixability::NotFixable;
    }
    let contested = findings
        .iter()
        .any(|other| !std::ptr::eq(other, finding) && spans_overlap(&finding.span, &other.span));
    if contested {
        Fixability::Overlapped
    } else {
        Fixability::Fixable
    }
}

fn spans_overlap(a: &Span, b: &Span) -> bool {
    a.start < b.end && b.start < a.end
}

/// Right-to-left splice of non-overlapping candidate replacements.
fn rewrite(src: &str, findings: &[Finding]) -> String {
    let mut editable: Vec<&Finding> = findings
        .iter()
        .filter(|f| matches!(findable(f, findings), Fixability::Fixable))
        .collect();
    editable.sort_by_key(|f| std::cmp::Reverse(f.span.start));

    let mut out = src.to_owned();
    for finding in editable {
        let Some(replacement) = &finding.replacement else {
            continue;
        };
        // Deletion rewrites: also eat one following space so
        // "crucial and it is important to note that we" doesn't collapse
        // into "and  we" with a double blank. Space is ASCII, so advancing
        // by its UTF-8 length stays on a char boundary.
        if replacement.is_empty() {
            let mut end = finding.span.end;
            if out[end..].starts_with(' ') {
                end += ' '.len_utf8();
            }
            out.replace_range(finding.span.start..end, "");
        } else {
            out.replace_range(finding.span.start..finding.span.end, replacement);
        }
    }
    out
}

/// Print the human-readable summary.
fn report(summary: &FixSummary, wrote: bool) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let verb = if wrote { "fixed" } else { "would fix" };
    let _ = writeln!(
        lock,
        "{verb}: {} finding(s); skipped (overlapping other findings): {}; remaining unfixable: {}",
        summary.applied, summary.skipped_overlaps, summary.unfixable
    );
    for (path, count) in &summary.files_changed {
        let _ = writeln!(lock, "  {path}: {count}");
    }
}
