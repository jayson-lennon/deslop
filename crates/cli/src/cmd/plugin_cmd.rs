//! `deslop plugin`: install builtin plugin modules to the user plugin dir.
//!
//! Install writes the embedded module to
//! `<data_dir>/deslop/plugins/<name>.wasm` (atomic: temp file + rename), so
//! a project config's bare `wasm = "<name>.wasm"` resolves to it. Installing
//! is inert until a config declares the plugin — there is no auto-discovery.

use deslop_plugin_protocol::PluginManifest;

use crate::ExitCode;

/// The user plugin install directory under a resolved data dir.
fn plugin_dir(data_dir: &camino::Utf8Path) -> camino::Utf8PathBuf {
    data_dir.join("deslop").join("plugins")
}

/// Validate the embedded module and return its manifest.
///
/// A builtin that fails to instantiate is a build/commit bug; the error goes
/// to stderr with the id hint (install name) for context.
fn validated_manifest(name: &str) -> Result<PluginManifest, ExitCode> {
    let Some(builtin) = crate::builtin_registry::find(name) else {
        eprintln!("deslop: unknown builtin plugin {name:?}");
        eprintln!("try one of:");
        for candidate in crate::builtin_registry::all() {
            eprintln!("  {}", candidate.name);
        }
        return Err(ExitCode::LoadFailure);
    };
    match deslop_core::plugin::wasmi_host::instantiate_bytes(name, builtin.bytes) {
        Ok(plugin) => {
            use deslop_core::plugin::LintPlugin as _;
            Ok(plugin.meta().clone())
        }
        Err(error) => {
            eprintln!("deslop: builtin plugin {name:?} is invalid: {error}");
            Err(ExitCode::LoadFailure)
        }
    }
}

/// Write one builtin's bytes to the install dir atomically.
fn write_module(
    name: &str,
    bytes: &[u8],
    dir: &camino::Utf8Path,
) -> Result<camino::Utf8PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
    let target = dir.join(format!("{name}.wasm"));
    // Temp file + rename so an interrupted install never leaves a
    // truncated module at the canonical path.
    let tmp = dir.join(format!(".{name}.wasm.tmp"));
    {
        use std::io::Write as _;
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| format!("cannot create {}: {e}", tmp.as_str()))?;
        file.write_all(bytes)
            .map_err(|e| format!("cannot write {}: {e}", tmp.as_str()))?;
    }
    std::fs::rename(&tmp, &target)
        .map_err(|e| format!("cannot finalize {}: {e}", target.as_str()))?;
    Ok(target)
}

/// Entry point for `deslop plugin install <name>`.
pub fn install_cmd(name: &str) -> i32 {
    let Some(data_dir) = dirs::data_dir().and_then(|p| camino::Utf8PathBuf::from_path_buf(p).ok())
    else {
        eprintln!("deslop: no user data directory available on this platform");
        return ExitCode::LoadFailure as i32;
    };

    let manifest = match validated_manifest(name) {
        Ok(manifest) => manifest,
        Err(code) => return code as i32,
    };
    let builtin = crate::builtin_registry::find(name).expect("just validated");
    let dir = plugin_dir(&data_dir);
    let target = match write_module(builtin.name, builtin.bytes, &dir) {
        Ok(target) => target,
        Err(detail) => {
            eprintln!("deslop: {detail}");
            return ExitCode::LoadFailure as i32;
        }
    };

    // Usage hint from the module's own manifest id; the config key matches
    // it case-insensitively, so lowercase is always valid.
    let key_hint = manifest.id.to_lowercase();
    println!("installed {name} -> {target}");
    println!("enable it in a project's .deslop.toml with:");
    println!();
    println!("  [plugin.{key_hint}]");
    println!("  wasm = \"{}.wasm\"", builtin.name);
    ExitCode::Clean as i32
}

/// Entry point for `deslop plugin list`.
pub fn list_cmd() -> i32 {
    let Some(data_dir) = dirs::data_dir().and_then(|p| camino::Utf8PathBuf::from_path_buf(p).ok())
    else {
        eprintln!("deslop: no user data directory available on this platform");
        return ExitCode::LoadFailure as i32;
    };
    let dir = plugin_dir(&data_dir);
    println!("builtin plugins (install dir: {}):", dir.as_str());
    for builtin in crate::builtin_registry::all() {
        let installed = dir.join(format!("{}.wasm", builtin.name));
        let state = if installed.is_file() {
            "installed"
        } else {
            "-"
        };
        println!(
            "  {:<24} {:>10}  {}",
            builtin.name,
            state,
            format_size(builtin.bytes.len())
        );
    }
    ExitCode::Clean as i32
}

fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `f` with a fresh temp dir standing in for the data dir; the
    /// tempdir lives for the whole closure body.
    fn with_dir<F: FnOnce(&camino::Utf8Path)>(f: F) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        f(&path);
    }

    #[test]
    fn write_module_places_the_file_and_cleans_up() {
        // Given a name, bytes, and an empty install dir.
        with_dir(|dir| {
            // When writing the module.
            let target = write_module("demo", b"\0asm-demo-bytes", dir).expect("write");

            // Then the file lands at <dir>/<name>.wasm and no temp remains.
            assert_eq!(target, dir.join("demo.wasm"));
            assert_eq!(std::fs::read(&target).expect("read"), b"\0asm-demo-bytes");
            assert!(!dir.join(".demo.wasm.tmp").exists());
        });
    }

    #[test]
    fn write_module_overwrites_an_existing_module() {
        // Given an install dir that already holds an older copy.
        with_dir(|dir| {
            std::fs::create_dir_all(dir).expect("mkdir");
            std::fs::write(dir.join("demo.wasm"), b"stale").expect("seed");

            // When rewriting the module.
            write_module("demo", b"fresh", dir).expect("write");

            // Then the content is replaced.
            assert_eq!(
                std::fs::read(dir.join("demo.wasm")).expect("read"),
                b"fresh"
            );
        });
    }

    #[test]
    fn validated_manifest_rejects_unknown_names() {
        // Given a name that is not in the builtin registry.
        // When validating.
        // Then it fails with the load-failure code.
        assert_eq!(
            validated_manifest("not-a-plugin"),
            Err(ExitCode::LoadFailure)
        );
    }

    #[test]
    fn validated_manifest_reads_the_real_builtin() {
        // Given the shipped example plugin.
        // When validating.
        // Then its manifest id comes back from the module itself.
        let manifest = validated_manifest("example-exclaim").expect("valid builtin");
        assert_eq!(manifest.id, "EXCLAIM");
    }

    #[test]
    fn plugin_dir_nests_deslop_plugins_under_the_data_dir() {
        // Given a data dir.
        with_dir(|dir| {
            // When resolving the install dir.
            // Then it follows the documented layout.
            assert_eq!(plugin_dir(dir), dir.join("deslop").join("plugins"));
        });
    }
}
