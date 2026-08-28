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
}
