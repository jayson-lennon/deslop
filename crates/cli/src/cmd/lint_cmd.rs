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

/// Width auto-detection: the flag wins; else the TTY width probe; else 0
/// (untruncated). The probe is injected so the resolution stays testable.
fn resolve_width(flag: Option<usize>, tty_width: Option<usize>) -> usize {
    match flag {
        Some(width) => width,
        None => tty_width.unwrap_or(0),
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

/// One lint invocation: overrides and document paths.
pub struct ScanRun<'a> {
    pub cfg: &'a Config,
    pub paths: Vec<camino::Utf8PathBuf>,
    pub format_override: Option<FormatName>,
    pub color_override: Option<ColorChoice>,
    /// Explicit human-output width; `None` auto-detects the TTY, `Some(0)`
    /// disables truncation.
    pub width_override: Option<usize>,
    /// Loaded `[plugins]` modules; empty when none configured.
    pub plugins: Vec<Box<dyn deslop_core::plugin::LintPlugin>>,
    /// Embedding-model compute backend; `None` = CPU default.
    pub gpu: Option<deslop_core::embedder::GpuBackend>,
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

        // The embedder is built at most once per run, and only when an
        // enabled repetition group actually needs the model. Missing model
        // files already failed the pack at load; a build failure here (bad
        // files, OOM) degrades to the scan-time skip warning.
        let embedder = repetition_embedder(
            &loaded.rule_set,
            &settings,
            self.gpu.unwrap_or(deslop_core::embedder::GpuBackend::Cpu),
        );
        let embedder_ref = embedder
            .as_ref()
            .map(|e| e as &dyn deslop_core::embedder::Embedder);

        // Findings are owned per-doc; collect (finding, doc-index) then
        // borrow views at render time so lifetimes stay simple.
        let mut all: Vec<(Finding, usize)> = Vec::new();
        for (idx, doc) in corpus.docs.iter().enumerate() {
            let outcome = scanner::scan_with_plugins(
                &doc.src,
                &loaded.rule_set,
                &settings,
                &self.plugins,
                embedder_ref,
            );
            for warning in &outcome.warnings {
                eprintln!("{warning}");
            }
            for finding in outcome.findings {
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
        let tty_width = {
            use std::io::IsTerminal;
            std::io::stdout()
                .is_terminal()
                .then(|| terminal_size::terminal_size().map(|(w, _)| usize::from(w.0)))
                .flatten()
        };
        let width = resolve_width(self.width_override, tty_width);
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        if let Err(e) = crate::render::render(format, color, width, &filed, &mut lock) {
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

/// Build the shared model embedder when any enabled repetition group needs
/// one. Returns `None` when no model-dependent group is active.
fn repetition_embedder(
    rules: &deslop_core::rule::RuleSet,
    settings: &deslop_core::scanner::LintSettings,
    backend: deslop_core::embedder::GpuBackend,
) -> Option<deslop_core::embedder::CandleEmbedder> {
    let needs_model = rules.groups.iter().any(|g| {
        if !g.enabled {
            return false;
        }
        if settings.level_for(&g.id_base, &g.id_base) == Some(deslop_core::config::LintLevel::Allow)
        {
            return false;
        }
        matches!(
            g.repetition.as_ref().map(|s| s.variant),
            Some(deslop_core::rule::RepetitionVariant::Propositional)
        )
    });
    if !needs_model {
        return None;
    }
    let dir: Result<camino::Utf8PathBuf, String> = match std::env::var("DESLOP_MODELS_DIR") {
        Ok(v) => Ok(camino::Utf8PathBuf::from(v)),
        Err(_) => crate::cmd::plugin_cmd::resolve_data_dir()
            .map(|d| d.join("deslop").join("models"))
            .ok_or_else(|| "deslop: no user data directory available".to_string()),
    };
    let Ok(models_root) = dir else {
        return None;
    };
    match deslop_core::embedder::CandleEmbedder::from_dir(
        &models_root.join("all-MiniLM-L6-v2").into_std_path_buf(),
        backend,
    ) {
        Ok(e) => Some(e),
        Err(e) => {
            eprintln!("deslop: embedding model failed to load: {e}");
            None
        }
    }
}
