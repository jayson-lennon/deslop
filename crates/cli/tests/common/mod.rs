//! Shared CLI-integration helpers.
//!
//! Integration tests must be HERMETIC: the binary under test resolves rule
//! packs from disk (`~/.config/deslop/rules`, `./rules`, ...) and would
//! otherwise silently source whatever the invoking user has installed.
//! [`hermetic`] provisions a tempdir pack copy and hands back the
//! `--rules-dir` argument pair so every run loads exactly what it should.

use std::process::Command;

/// One hermetic fixture: a tempdir with copies of the repo's builtin packs.
pub struct HermeticRules {
    pub dir: tempfile::TempDir,
}

impl HermeticRules {
    /// Copy `rules/*.toml` from the repo checkout into a fresh tempdir.
    pub fn provision() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo_rules = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules");
        for entry in std::fs::read_dir(&repo_rules).expect("repo rules dir") {
            let entry = entry.expect("readdir");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                std::fs::copy(&path, dir.path().join(entry.file_name())).expect("copy pack");
            }
        }
        Self { dir }
    }

    /// Prepend `--rules-dir <dir>` to a command so it loads ONLY these packs.
    pub fn apply(&self, cmd: &mut Command) {
        cmd.arg("--rules-dir")
            .arg(self.dir.path().to_str().expect("utf8 tempdir"));
    }

    /// Prepend `--config <dir>/seed-packs.toml`, pinning the seed pack
    /// list. Without an explicit config the binary walks up and inherits
    /// whatever the invoking user has installed - packs, lints, and any
    /// model-dependent repetition pack, which would drag the embedding
    /// model into every test run.
    // Suites that pin their own closest-config (lint_levels, plugins) keep
    // walk-up discovery and so never call this.
    #[allow(dead_code)]
    pub fn pin_seed_config(&self, cmd: &mut Command) {
        let cfg = self.dir.path().join("seed-packs.toml");
        std::fs::write(
            &cfg,
            "[packs]\nbuiltin = [\"aatell\", \"slop\", \"wsc\", \"aisigns\", \"cluster-terms\", \"hedging\"]\n",
        )
        .expect("write golden config");
        cmd.arg("--config").arg(cfg.to_str().expect("utf8 config"));
    }
}
