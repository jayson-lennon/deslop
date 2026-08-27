//! deslop CLI: argument surface, dispatch, exit codes.

use clap::{Parser, Subcommand};

/// A linter for AI-writing tells: dynamic TOML rules, three severity tiers.
#[derive(Debug, Parser)]
#[command(name = "deslop", version, about, propagate_version = true)]
struct Cli {
    /// Path(s) to lint; files or directories. Defaults to `.`.
    paths: Vec<camino::Utf8PathBuf>,

    /// Explicit config file (disables walk-up discovery).
    #[arg(long)]
    config: Option<camino::Utf8PathBuf>,

    /// Disable all Tier 3 hints.
    #[arg(long)]
    no_tier3: bool,

    /// Output format override.
    #[arg(long, value_enum, default_value_t = ArgFormat::Human)]
    format: ArgFormat,

    /// Color mode override.
    #[arg(long, value_enum, default_value_t = ArgColor::Auto)]
    color: ArgColor,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ArgFormat {
    Human,
    Json,
    Github,
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

    match cli.command {
        Some(Command::Rules { json: _ }) => unimplemented!("phase 2"),
        Some(Command::Fix { write: _ }) => unimplemented!("phase 5"),
        Some(Command::Init) => unimplemented!("phase 5"),
        None => {
            let _ = (&cli.no_tier3, &cli.format, &cli.color);
            unimplemented!("phase 3")
        }
    }
}
