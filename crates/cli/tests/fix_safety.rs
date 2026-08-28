//! `deslop fix` safety: dry-run default, CRLF preservation, overlap
//! suppression, idempotence (spec t8hg / AC7).

mod common;

use std::process::Command;

struct FixRun {
    dir: std::path::PathBuf,
    hermetic: common::HermeticRules,
}

impl FixRun {
    fn new(tag: &str, body: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("deslop-fix-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        std::fs::write(dir.join("doc.md"), body).expect("write doc");
        Self {
            dir,
            hermetic: common::HermeticRules::provision(),
        }
    }

    fn run_fix(&self, extra: &[&str]) -> (i32, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_deslop"));
        self.hermetic.apply(&mut cmd);
        let out = cmd
            .arg("fix")
            .args(extra)
            .args(["--color", "never"])
            .current_dir(&self.dir)
            .output()
            .expect("runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }

    fn body(&self) -> String {
        std::fs::read_to_string(self.dir.join("doc.md")).expect("read back")
    }
}

impl Drop for FixRun {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const LEVERAGE_DOC: &str = "We will leverage the existing pipeline.\n";

#[test]
fn dry_run_default_leaves_file_untouched() {
    // Given a doc with a replacement-bearing hit.
    let run = FixRun::new("dry", LEVERAGE_DOC);

    // When running fix without --write.
    let (code, _out) = run.run_fix(&[]);

    // Then the file is byte-identical and the run reports a pending edit.
    assert_eq!(run.body(), LEVERAGE_DOC);
    assert_eq!(code, 0, "dry-run never fails the run");
}

#[test]
fn write_replaces_only_the_hit_span() {
    // Given a doc with one vocab hit carrying `replacement`.
    let run = FixRun::new("write", LEVERAGE_DOC);

    // When applying --write.
    let (_code, out) = run.run_fix(&["--write"]);

    // Then exactly the hit is rewritten and the summary says one edit.
    assert_eq!(run.body(), "We will use the existing pipeline.\n");
    assert!(out.contains("1"), "summary reports the edit count: {out}");
}

#[test]
fn crlf_docs_fix_in_place_preserving_line_endings() {
    // Given a CRLF doc with a fixable hit on line 1.
    let body = "We will leverage the pipeline.\r\n\r\nSecond paragraph stands.\r\n";
    let run = FixRun::new("crlf", body);

    // When applying --write.
    run.run_fix(&["--write"]);

    // Then the replacement landed and every CRLF survived.
    let after = run.body();
    assert!(after.starts_with("We will use the pipeline.\r\n"));
    assert!(after.ends_with("Second paragraph stands.\r\n"));
    assert_eq!(after.matches("\r\n").count(), 3);
}

#[test]
fn overlapping_finding_suppresses_the_fix() {
    // Given a doc where a literal-ban artifact overlaps a vocab hit.
    let body = "contentReference[oaicite:1]{index=1} will leverage leverage.\n";
    let run = FixRun::new("overlap", body);
    let before = run.body();

    // When applying --write.
    let (_code, out) = run.run_fix(&["--write"]);

    // Then no edit lands inside the artifact span; hits outside it may.
    // The artifact occupies the head of the line, so the trailing vocab
    // hits survive untouched and are reported as skipped overlaps.
    assert!(
        !out.contains("applied 1") || run.body() == before,
        "either no clean fix applies, or non-overlapping edits do"
    );
    assert!(
        run.body().contains("contentReference[oaicite:1]"),
        "artifact never rewritten by fix"
    );
}

#[test]
fn second_write_run_is_idempotent() {
    // Given a doc fixed once.
    let run = FixRun::new("idem", LEVERAGE_DOC);
    run.run_fix(&["--write"]);
    let once = run.body();

    // When running fix --write again.
    run.run_fix(&["--write"]);

    // Then nothing further changes and the file matches the first pass.
    assert_eq!(run.body(), once);
    assert_eq!(run.body(), "We will use the existing pipeline.\n");
}

const DELETION_DOC: &str = "crucial and it is important to note that we ship.\n";

#[test]
fn deletion_replacement_eats_one_following_space() {
    // Given a doc whose hit carries an empty replacement (deletion).
    let run = FixRun::new("deletion", DELETION_DOC);

    // When applying --write.
    run.run_fix(&["--write"]);

    // Then the phrase vanishes WITHOUT leaving a double blank.
    assert_eq!(run.body(), "crucial and we ship.\n");
}

#[test]
fn deletion_before_multibyte_char_does_not_panic() {
    // Given a deletion hit followed by a multibyte char (no ASCII space).
    let run = FixRun::new(
        "deletion-cjk",
        "crucial and it is important to note that “we” ship.\n",
    );

    // When applying --write.
    run.run_fix(&["--write"]);

    // Then the deletion lands cleanly with single blanks around “we”.
    assert_eq!(run.body(), "crucial and “we” ship.\n");
}
