//! wasmi-backed [`LintPlugin`] implementation (filled in by the next phase).
//!
//! The concrete instantiation logic lives here; `load_plugins` in the parent
//! module calls into this module only, so swapping or extending the engine
//! later is a change confined to this file.

use deslop_plugin_protocol::{PluginFinding, PluginInput, PluginManifest};

use super::{PluginError, PluginRuntime};

/// A plugin executed by the embedded wasmi interpreter.
#[derive(Debug)]
pub struct WasmiPlugin {
    /// Fuel override from `[plugins.<id>.runtime]`, if any.
    pub(crate) fuel_override: PluginRuntime,
    pub(crate) manifest: PluginManifest,
    pub(crate) _engine: (),
}

impl WasmiPlugin {
    /// Fuel override applied on top of the size-scaled default.
    pub fn set_fuel_override(&mut self, fuel: Option<u64>) {
        self.fuel_override.fuel = fuel;
    }
}

impl super::LintPlugin for WasmiPlugin {
    fn meta(&self) -> &PluginManifest {
        &self.manifest
    }

    fn scan(&self, _input: &PluginInput) -> Result<Vec<PluginFinding>, PluginError> {
        Err(PluginError::Protocol {
            id: self.manifest.id.clone(),
            detail: "wasmi host not yet wired".into(),
        })
    }
}

/// Instantiate a plugin from a `.wasm` file on disk.
///
/// # Errors
///
/// Returns [`PluginError::Load`] for unreadable files, invalid modules,
/// missing/invalid metadata, or instantiation traps.
pub fn instantiate(_path: &camino::Utf8Path) -> Result<WasmiPlugin, PluginError> {
    Err(PluginError::Load {
        id: "?".into(),
        detail: "wasmi host not yet wired".into(),
    })
}
