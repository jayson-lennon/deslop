//! deslop CLI: argument surface, dispatch, exit codes.

use std::io::Write;

use clap::{Parser, Subcommand};

pub mod cmd;
pub mod render;

/// A linter for AI-writing tells: dynamic TOML rules, three severity tiers.
#[derive(Debug, Parser)]
#[command(name = "deslop", version, about, propagate_version = true)]
struct Cli {
    /// Path(s) to lint; files or directories. Defaults to `.`.
    paths: Vec<camino::Utf8PathBuf>,

    /// Explicit config file (disables walk-up discovery).
    #[arg(long)]
    config: Option<camino::Utf8PathBuf>,

    /// Output format override.
    #[arg(long, value_enum, default_value_t = ArgFormat::Human)]
    format: ArgFormat,

    /// Color mode override.
    #[arg(long, value_enum, default_value_t = ArgColor::Auto)]
    color: ArgColor,

    /// Directory containing rule pack TOMLs (aatell.toml, slop.toml, ...).
    /// Overrides the usual resolution (~/.config/deslop/rules, ./rules, ...).
    #[arg(long, value_name = "DIR")]
    rules_dir: Option<camino::Utf8PathBuf>,

    /// Additional rule file(s) to load on top of the resolved packs.
    /// Repeatable; id-bases and ids must not collide with existing groups.
    #[arg(long = "rule-file", value_name = "FILE")]
    rule_files: Vec<camino::Utf8PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ArgFormat {
    Human,
    Json,
    Github,
}

impl From<ArgFormat> for deslop_core::config::FormatName {
    fn from(value: ArgFormat) -> Self {
        match value {
            ArgFormat::Human => deslop_core::config::FormatName::Human,
            ArgFormat::Json => deslop_core::config::FormatName::Json,
            ArgFormat::Github => deslop_core::config::FormatName::Github,
        }
    }
}

impl From<ArgColor> for deslop_core::config::ColorChoice {
    fn from(value: ArgColor) -> Self {
        match value {
            ArgColor::Auto => deslop_core::config::ColorChoice::Auto,
            ArgColor::Always => deslop_core::config::ColorChoice::Always,
            ArgColor::Never => deslop_core::config::ColorChoice::Never,
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ArgColor {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Mechanically rewrite vocabulary hits that carry a replacement.
    Fix {
        /// Apply edits (default is a dry-run summary).
        #[arg(long)]
        write: bool,
        /// Output format for the summary.
        #[arg(long, value_enum, default_value_t = ArgFormat::Human)]
        format: ArgFormat,
        /// Color control for the summary.
        #[arg(long, value_enum, default_value_t = ArgColor::Auto)]
        color: ArgColor,
    },
    /// List the effective merged ruleset.
    Rules {
        /// Emit machine-readable JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Write an annotated starter `.deslop.toml`.
    Init,
}

/// Process exit contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Loaded fine AND nothing at Tier 1/Tier 2.
    Clean = 0,
    /// At least one Tier 1 or Tier 2 finding.
    FindingsReported = 1,
    /// Usage error or rule-load failure: run aborted.
    LoadFailure = 2,
}

fn main() {
    let cli = Cli::parse();
    std::process::exit(run(cli));
}

fn run(cli: Cli) -> i32 {
    // --config must point at a real file when given.
    if let Some(cfg_path) = &cli.config {
        if !cfg_path.is_file() {
            eprintln!("deslop: --config file does not exist: {cfg_path}");
            return ExitCode::LoadFailure as i32;
        }
    }

    let missing: Vec<_> = cli.paths.iter().filter(|p| !p.exists()).collect();
    if let Some(first) = missing.first() {
        for path in &missing {
            eprintln!("deslop: path does not exist: {path}");
        }
        let _ = first;
        return ExitCode::LoadFailure as i32;
    }

    // Config resolution: explicit path wins; else walk up from cwd.
    let start = cli
        .config
        .clone()
        .unwrap_or_else(|| camino::Utf8PathBuf::from("."));
    let cfg = match deslop_core::config::discover(&start) {
        Ok(cfg) => cfg,
        Err(report) => {
            eprintln!("deslop: {report:?}");
            return ExitCode::LoadFailure as i32;
        }
    };
    let _ = &cfg;

    // --rules-dir must point at a real directory when given; it replaces the
    // whole pack-resolution chain so runs are hermetic to installed packs.
    let rules_dir = match &cli.rules_dir {
        Some(dir) if !dir.is_dir() => {
            eprintln!("deslop: --rules-dir directory does not exist: {dir}");
            return ExitCode::LoadFailure as i32;
        }
        // Canonicalize so the loader's parent-join is unambiguous even for
        // relative flags (`--rules-dir packs` from a nested cwd).
        Some(dir) => dir
            .canonicalize()
            .ok()
            .and_then(|p| camino::Utf8PathBuf::from_path_buf(p).ok()),
        None => None,
    };

    // --rule-file entries must exist; canonicalize so downstream joins and
    // diagnostics print stable absolute paths regardless of the caller's cwd.
    let rule_files = {
        let mut files = Vec::new();
        for f in &cli.rule_files {
            let Some(p) = f
                .canonicalize()
                .ok()
                .and_then(|p| camino::Utf8PathBuf::from_path_buf(p).ok())
            else {
                eprintln!("deslop: --rule-file does not exist: {f}");
                return ExitCode::LoadFailure as i32;
            };
            files.push(p);
        }
        files
    };
    let mut cfg = cfg;
    cfg.packs.extra_paths.extend(rule_files);

    match cli.command {
        Some(Command::Rules { json }) => {
            let mut cmd = cmd::rules_cmd::RulesCmd {
                cfg: &cfg,
                json,
                rules_dir: rules_dir.clone(),
            };
            match cmd.run() {
                Ok(code) => code,
                Err(_) => ExitCode::LoadFailure as i32,
            }
        }
        Some(Command::Fix {
            write,
            color,
            format,
        }) => {
            // Plugins load before the run; load failures warn, never abort.
            let (plugins, plugin_warnings) = deslop_core::plugin::load_plugins(&cfg.plugins);
            let stderr = std::io::stderr();
            {
                let mut lock = stderr.lock();
                for warning in &plugin_warnings {
                    let _ = writeln!(lock, "{warning}");
                }
            }
            let mut cmd = cmd::fix_cmd::FixCmd {
                cfg: &cfg,
                write,
                color_override: Some(color.into()),
                format_override: Some(format.into()),
                rules_dir: rules_dir.clone(),
                plugins,
            };
            match cmd.run() {
                Ok(code) => code,
                Err(_) => ExitCode::LoadFailure as i32,
            }
        }
        Some(Command::Init) => cmd::init_cmd::run(),
        None => {
            // Lint: load rules first; any load failure aborts with exit 2
            // before a single document is scanned (spec exit precedence).
            let loaded = cmd::rules_cmd::load_for_lint(&cfg, rules_dir.clone());
            if !loaded.errors.is_empty() {
                let stderr = std::io::stderr();
                let mut lock = stderr.lock();
                let _ = render::human::render_load_errors(&loaded.errors, &mut lock);
                return ExitCode::LoadFailure as i32;
            }
            // Plugins load after rule loading so rule failures keep their
            // precedence; plugin load failures only warn.
            let (plugins, plugin_warnings) = deslop_core::plugin::load_plugins(&cfg.plugins);
            let stderr = std::io::stderr();
            {
                let mut lock = stderr.lock();
                for warning in &plugin_warnings {
                    let _ = writeln!(lock, "{warning}");
                }
            }
            let run = cmd::lint_cmd::ScanRun {
                cfg: &cfg,
                paths: cli.paths.clone(),
                format_override: Some(cli.format.into()),
                color_override: Some(cli.color.into()),
                plugins,
            };
            run.run(loaded)
        }
    }
}
