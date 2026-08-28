//! In-memory [`LintPlugin`] for tests: scripted findings, no WASM involved.
//!
//! Public (not `#[cfg(test)]`) so integration tests in `crates/core/tests`
//! and the CLI test suite can assemble plugin lists without shipping a wasm
//! toolchain.

use deslop_plugin_protocol::{PluginFinding, PluginInput, PluginManifest};

use super::{PluginError, PluginRuntime};

/// A [`super::LintPlugin`] that replays a scripted response.
#[derive(Debug)]
pub struct FakePlugin {
    manifest: PluginManifest,
    /// Findings returned verbatim on every `scan` call.
    pub findings: Vec<PluginFinding>,
    /// When set, every `scan` fails with this error instead of returning
    /// `findings`.
    pub failure: Option<PluginError>,
    /// Optional fuel override to apply (mirrors wasmi host wiring).
    pub runtime: PluginRuntime,
    /// Set once and then permanently fails with a protocol violation
    /// (models a plugin that turns bad after first use).
    pub fail_after_first_call: bool,
    calls: std::sync::atomic::AtomicUsize,
}

impl FakePlugin {
    pub fn new(manifest: PluginManifest) -> Self {
        FakePlugin {
            manifest,
            findings: Vec::new(),
            failure: None,
            runtime: PluginRuntime::default(),
            fail_after_first_call: false,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Number of `scan` calls so far (asserts the host's call pattern).
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl super::LintPlugin for FakePlugin {
    fn meta(&self) -> &PluginManifest {
        &self.manifest
    }

    fn scan(&self, _input: &PluginInput) -> Result<Vec<PluginFinding>, PluginError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if self.fail_after_first_call && self.calls() > 1 {
            return Err(PluginError::Protocol {
                id: self.manifest.id.clone(),
                detail: "fake plugin expired".into(),
            });
        }
        Ok(self.findings.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            tier: 2,
            category: "test".into(),
            abi: deslop_plugin_protocol::PROTOCOL_ABI,
        }
    }

    #[test]
    fn fake_plugin_returns_scripted_findings_and_counts_calls() {
        // Given a fake plugin with one scripted finding.
        let plugin = FakePlugin::new(manifest("FAKE"));
        let input = PluginInput::default();

        // When scanning twice.
        let first = super::super::LintPlugin::scan(&plugin, &input).expect("first scan");
        let second = super::super::LintPlugin::scan(&plugin, &input).expect("second scan");

        // Then both calls return the scripted findings and the call count is 2.
        assert_eq!(first, plugin.findings);
        assert_eq!(second, plugin.findings);
        assert_eq!(plugin.calls(), 2);
    }

    #[test]
    fn fake_plugin_failure_suppresses_findings() {
        // Given a fake plugin configured to fail.
        let mut plugin = FakePlugin::new(manifest("FAKE"));
        plugin.findings.push(PluginFinding {
            slug: "s".into(),
            span: (0, 1),
            message: "m".into(),
            advice: None,
        });
        plugin.failure = Some(PluginError::Fuel {
            id: "FAKE".into(),
        });

        // When scanning.
        let result = super::super::LintPlugin::scan(&plugin, &PluginInput::default());

        // Then the failure surfaces and no findings escape.
        assert!(matches!(result, Err(PluginError::Fuel { .. })));
    }

    #[test]
    fn fake_plugin_fail_after_first_call_expires_on_second_scan() {
        // Given a fake plugin that expires after its first call.
        let mut plugin = FakePlugin::new(manifest("FAKE"));
        plugin.fail_after_first_call = true;

        // When scanning twice.
        let first = super::super::LintPlugin::scan(&plugin, &PluginInput::default());
        let second = super::super::LintPlugin::scan(&plugin, &PluginInput::default());

        // Then the first succeeds and the second reports the protocol failure.
        assert!(first.is_ok());
        assert!(matches!(second, Err(PluginError::Protocol { .. })));
    }
}
