//! CLI integration tests for `[plugins]`: end-to-end through the real
//! binary with `.wasm` files compiled from WAT in-process (no wasm
//! toolchain needed).
//!
//! Covers the test-matrix rows: missing plugin file (12), JSON renderer
//! output (13), and `deslop rules` listing (14).

mod common;

use std::process::Command;

/// Compile a WAT fixture from deslop-core's fixtures to wasm bytes.
fn fixture_wasm(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/core/tests/fixtures/plugins")
        .join(name);
    let wat = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    wat::parse_str(&wat).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

/// A project dir with a `.deslop.toml` declaring `paths` + one doc.
struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn with_config(config: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".deslop.toml"), config).expect("write config");
        std::fs::write(dir.path().join("doc.md"), "some words here\n").expect("write doc");
        Self { dir }
    }

    /// Absolute path for `paths = [...]` entries.
    fn spill_plugin(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, bytes).expect("write plugin");
        path.to_str().expect("utf8").to_owned()
    }

    fn run(&self, args: &[&str]) -> (String, String, i32) {
        let hermetic = common::HermeticRules::provision();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_deslop"));
        hermetic.apply(&mut cmd);
        let output = cmd
            .args(args)
            .current_dir(self.dir.path())
            .output()
            .expect("runs");
        (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            output.status.code().unwrap_or(-1),
        )
    }
}

#[test]
fn plugin_findings_render_in_json_with_plugin_kind() {
    // Given a project with a valid plugin.
    let project = Project::with_config("[plugins]\npaths = [\"plug.wasm\"]\n");
    let path = project.spill_plugin("plug.wasm", &fixture_wasm("happy.wat"));
    std::fs::write(
        project.dir.path().join(".deslop.toml"),
        format!("[plugins]\npaths = [\"{path}\"]\n"),
    )
    .expect("rewrite config");

    // When linting as JSON.
    let (stdout, _stderr, code) = project.run(&["--format", "json", "doc.md"]);

    // Then the finding carries kind "plugin" and the frozen field order.
    assert_eq!(code, 0);
    assert!(stdout.contains("\"rule_id\":\"FIXTURE#demo\""), "{stdout}");
    assert!(stdout.contains("\"kind\":\"plugin\""), "{stdout}");
    assert!(stdout.contains("\"tier\":3"), "{stdout}");
    assert!(stdout.contains("\"message\":\"demo hit\""), "{stdout}");
}

#[test]
fn missing_plugin_file_warns_and_does_not_change_exit_code() {
    // Given a project whose plugin path does not exist.
    let project = Project::with_config(
        "[plugins]\npaths = [\"/nonexistent/really-not-here.wasm\"]\n",
    );

    // When linting.
    let (_stdout, stderr, code) = project.run(&["doc.md"]);

    // Then stderr carries a warning naming the path and the exit is clean.
    assert!(stderr.contains("skipping plugin"), "{stderr}");
    assert!(stderr.contains("really-not-here.wasm"), "{stderr}");
    assert_eq!(code, 0);
}

#[test]
fn trapping_plugin_warns_per_document_and_exit_stays_clean() {
    // Given a project with a plugin whose scan traps.
    let project = Project::with_config("[plugins]\npaths = [\"plug.wasm\"]\n");
    project.spill_plugin("plug.wasm", &fixture_wasm("trap.wat"));

    // When linting.
    let (_stdout, stderr, code) = project.run(&["doc.md"]);

    // Then the trap is a warning, not a crash.
    assert!(stderr.contains("FIXTURE"), "{stderr}");
    assert!(stderr.contains("trapped") || stderr.contains("failed"), "{stderr}");
    assert_eq!(code, 0);
}

#[test]
fn rules_listing_includes_plugin_rows() {
    // Given a project with a valid plugin.
    let project = Project::with_config("[plugins]\npaths = [\"plug.wasm\"]\n");
    project.spill_plugin("plug.wasm", &fixture_wasm("happy.wat"));

    // When listing rules (table).
    let (stdout, _stderr, code) = project.run(&["rules"]);

    // Then the plugin appears as a row with kind "plugin".
    assert_eq!(code, 0);
    assert!(stdout.contains("FIXTURE"), "{stdout}");
    assert!(stdout.contains("plugin"), "{stdout}");

    // And in JSON listing.
    let (stdout, _stderr, code) = project.run(&["rules", "--json"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("{\"id\":\"FIXTURE\",\"tier\":3,\"kind\":\"plugin\""),
        "{stdout}"
    );
}
