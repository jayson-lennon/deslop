//! Host-level plugin tests: real wasmi execution against WAT fixtures.
//!
//! The fixtures under `tests/fixtures/plugins` are hand-written WAT modules
//! compiled in-process by the `wat` crate, so no wasm32 toolchain is needed.

use deslop_core::plugin::{load_plugins, LintPlugin, PluginConfig, PluginError, PluginInput};

/// Compile a WAT fixture from `tests/fixtures/plugins` to wasm bytes.
fn fixture(name: &str) -> Vec<u8> {
    let path = camino::Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plugins")
        .join(name);
    let wat = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    wat::parse_str(&wat).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

/// Write bytes to a temp file (models a user's `plugins.paths` entry).
fn spill(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> camino::Utf8PathBuf {
    let path = camino::Utf8PathBuf::from_path_buf(dir.path().join(name)).expect("utf8 path");
    std::fs::write(&path, bytes).expect("write fixture");
    path
}

fn sample_input() -> PluginInput {
    PluginInput {
        text: "demo text here".into(),
        ..PluginInput::default()
    }
}

#[test]
fn happy_fixture_returns_its_single_finding() {
    // Given a compiled valid plugin fixture.
    let wasm = fixture("happy.wat");
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = spill(&tmp, "happy.wasm", &wasm);

    // When instantiating and scanning.
    let plugin = deslop_core::plugin::wasmi_host::instantiate(&path).expect("instantiate");
    let manifest = plugin.meta().clone();
    let findings = plugin.scan(&sample_input()).expect("scan");

    // Then the manifest comes from the module itself and the finding is verbatim.
    assert_eq!(manifest.id, "FIXTURE");
    assert_eq!(manifest.tier, 3);
    assert_eq!(manifest.category, "test");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].slug, "demo");
    assert_eq!(findings[0].span, (0, 4));
    assert_eq!(findings[0].message, "demo hit");
}

#[test]
fn manifest_with_wrong_abi_is_a_load_error() {
    // Given a fixture whose manifest declares a future ABI.
    let wasm = fixture("bad_meta_abi.wat");
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = spill(&tmp, "bad_abi.wasm", &wasm);

    // When instantiating.
    let error = deslop_core::plugin::wasmi_host::instantiate(&path).expect_err("must reject");

    // Then it is a Load error naming the ABI mismatch.
    match error {
        PluginError::Load { id, detail } => {
            assert_eq!(id, "bad_abi.wasm");
            assert!(detail.contains("abi"), "detail: {detail}");
        }
        other => panic!("expected Load error, got {other:?}"),
    }
}

#[test]
fn manifest_with_out_of_range_tier_is_a_load_error() {
    // Given a fixture whose manifest declares tier 9.
    let wasm = fixture("bad_meta_tier.wat");
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = spill(&tmp, "bad_tier.wasm", &wasm);

    // When instantiating.
    let error = deslop_core::plugin::wasmi_host::instantiate(&path).expect_err("must reject");

    // Then it is a Load error naming the tier.
    match error {
        PluginError::Load { detail, .. } => {
            assert!(detail.contains("tier"), "detail: {detail}");
        }
        other => panic!("expected Load error, got {other:?}"),
    }
}

#[test]
fn trapping_scan_reports_a_trap_error() {
    // Given a plugin whose scan always traps.
    let wasm = fixture("trap.wat");
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = spill(&tmp, "trap.wasm", &wasm);
    let plugin = deslop_core::plugin::wasmi_host::instantiate(&path).expect("instantiate");

    // When scanning.
    let error = plugin.scan(&sample_input()).expect_err("must trap");

    // Then the failure is a Trap (not fuel, not load).
    assert!(matches!(error, PluginError::Trap { .. }), "{error:?}");
}

#[test]
fn runaway_scan_is_stopped_by_the_fuel_budget() {
    // Given a plugin whose scan loops forever.
    let wasm = fixture("infinite_loop.wat");
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = spill(&tmp, "loop.wasm", &wasm);
    let plugin = deslop_core::plugin::wasmi_host::instantiate(&path).expect("instantiate");

    // When scanning (with a small explicit budget so the test stays fast).
    let mut small = deslop_core::plugin::wasmi_host::instantiate(&path).expect("instantiate");
    small.set_fuel_override(Some(1_000_000));
    let error = small.scan(&sample_input()).expect_err("must exhaust fuel");
    drop(plugin);

    // Then the failure is fuel exhaustion.
    assert!(matches!(error, PluginError::Fuel { .. }), "{error:?}");
}

#[test]
fn scan_is_reusable_across_documents() {
    // Given a valid plugin that already scanned once.
    let wasm = fixture("happy.wat");
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = spill(&tmp, "happy.wasm", &wasm);
    let plugin = deslop_core::plugin::wasmi_host::instantiate(&path).expect("instantiate");
    let first = plugin.scan(&sample_input()).expect("first scan");

    // When scanning a second, larger document.
    let bigger = PluginInput {
        text: format!("{} more words", "x ".repeat(500)),
        ..PluginInput::default()
    };
    let second = plugin.scan(&bigger).expect("second scan");

    // Then both scans succeed with identical scripted output.
    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
}

#[test]
fn duplicate_plugin_ids_keep_the_first_and_warn() {
    // Given two different files declaring the same plugin id.
    let wasm = fixture("happy.wat");
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = spill(&tmp, "first.wasm", &wasm);
    let b = spill(&tmp, "second.wasm", &wasm);
    let cfg = PluginConfig {
        paths: vec![a, b],
        ..PluginConfig::default()
    };

    // When loading both.
    let (plugins, warnings) = load_plugins(&cfg);

    // Then only the first loads and the duplicate is warned about.
    assert_eq!(plugins.len(), 1);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("second.wasm"), "{}", warnings[0]);
    assert!(warnings[0].contains("FIXTURE"), "{}", warnings[0]);
}
