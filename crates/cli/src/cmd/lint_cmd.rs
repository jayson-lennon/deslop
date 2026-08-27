//! Lint pipeline: collect documents, scan, render, decide exit code.

use std::io::IsTerminal;
use std::io::Write;

use deslop_core::config::{ColorChoice, Config, FormatName};
use deslop_core::finding::Finding;
use deslop_core::rule::loader;
use deslop_core::scanner;

use crate::render::FiledFinding;

/// Color auto-detection: color only when stdout is a TTY, NO_COLOR unset.
fn resolve_color(color: ColorChoice) -> ColorChoice {
    match color {
        ColorChoice::Auto => {
            if std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal() {
                ColorChoice::Never
            } else {
                ColorChoice::Always
            }
        }
        other => other,
    }
}

/// Documents gathered for one run, sorted by path for determinism.
pub struct Corpus {
    pub docs: Vec<deslop_core::doc::Doc>,
}

impl Corpus {
    /// # Errors
    ///
    /// Fails when a path cannot be read as UTF-8 text.
    pub fn gather(
        paths: &[camino::Utf8PathBuf],
        respect_gitignore: bool,
    ) -> Result<Corpus, std::io::Error> {
        let mut files = Vec::new();
        collect_files(paths, respect_gitignore, &mut files)?;
        let mut docs = Vec::with_capacity(files.len());
        for file in files {
            let src = std::fs::read_to_string(&file)?;
            docs.push(deslop_core::doc::Doc {
                path: file.clone(),
                src,
            });
        }
        Ok(Corpus { docs })
    }
}

fn collect_files(
    paths: &[camino::Utf8PathBuf],
    _respect_gitignore: bool,
    out: &mut Vec<camino::Utf8PathBuf>,
) -> std::io::Result<()> {
    // One loop per function: directory walk delegated to helper below.
    let mut expanded: Vec<camino::Utf8PathBuf> = Vec::new();
    for p in paths {
        expanded.extend(expand_path(p)?);
    }
    out.extend(expanded);
    out.sort_unstable();
    out.dedup();
    Ok(())
}

fn expand_path(p: &camino::Utf8Path) -> Result<Vec<camino::Utf8PathBuf>, std::io::Error> {
    let meta =
        std::fs::metadata(p).map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", p)))?;
    if meta.is_file() {
        return Ok(vec![p.to_owned()]);
    }
    walkdir::WalkDir::new(p)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| is_lintable(entry.path()))
        .map(|entry| {
            camino::Utf8PathBuf::try_from(entry.into_path())
                .map_err(|e| std::io::Error::other(e.into_io_error()))
        })
        .collect()
}

const LINTABLE_EXTENSIONS: [&str; 7] = ["md", "markdown", "txt", "rst", "adoc", "html", "htm"];

fn is_lintable(path: &std::path::Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => LINTABLE_EXTENSIONS.contains(&ext),
        None => false,
    }
}

/// Files ignored for document scanning (build/lock metadata).
pub struct ScanRun<'a> {
    pub cfg: &'a Config,
    pub paths: Vec<camino::Utf8PathBuf>,
    pub format_override: Option<FormatName>,
    pub color_override: Option<ColorChoice>,
}

impl ScanRun<'_> {
    /// Execute the lint; returns the process exit code.
    pub fn run(&self, loaded: loader::Loaded) -> i32 {
        // Tier filter comes from config ([scan].tiers); lint levels from
        // [lints]. No CLI lint control by design.
        let max_tier = {
            let tiers = &self.cfg.scan.tiers;
            let highest = tiers.iter().copied().max().unwrap_or(3);
            (highest != 3).then_some(highest)
        };
        let settings = scanner::LintSettings {
            max_tier,
            levels: self.cfg.lint.clone(),
        };

        let corpus = match Corpus::gather(&self.paths, self.cfg.scan.respect_gitignore) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("deslop: cannot read input: {e}");
                return crate::ExitCode::LoadFailure as i32;
            }
        };

        // Findings are owned per-doc; collect (finding, doc-index) then
        // borrow views at render time so lifetimes stay simple.
        let mut all: Vec<(Finding, usize)> = Vec::new();
        for (idx, doc) in corpus.docs.iter().enumerate() {
            for finding in scanner::scan(&doc.src, &loaded.rule_set, &settings) {
                all.push((finding, idx));
            }
        }
        let filed: Vec<FiledFinding<'_>> = all
            .iter()
            .map(|(f, idx)| FiledFinding {
                path: corpus.docs[*idx].path.as_str(),
                src: corpus.docs[*idx].src.as_str(),
                finding: f,
            })
            .collect();

        let format = self.format_override.unwrap_or(self.cfg.output.format);
        let color = resolve_color(self.color_override.unwrap_or(self.cfg.output.color));
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        if let Err(e) = crate::render::render(format, color, &filed, &mut lock) {
            eprintln!("deslop: rendering failed: {e}");
            return crate::ExitCode::LoadFailure as i32;
        }
        let _ = lock.flush();

        // Exit contract: any artifact/tell => 1; hints alone stay clean.
        let blocking = filed
            .iter()
            .any(|f| f.finding.tier != deslop_core::finding::Tier::Density);
        if blocking {
            crate::ExitCode::FindingsReported as i32
        } else {
            crate::ExitCode::Clean as i32
        }
    }
}
