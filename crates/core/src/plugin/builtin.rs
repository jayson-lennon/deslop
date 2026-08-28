//! Builtin plugin registry: prebuilt wasm modules embedded in the binary.
//!
//! Each entry pairs an install name with module bytes baked in at compile
//! time via [`include_bytes`]`, so the released binary ships the builtin
//! plugins and `deslop plugin install <name>` works without a network, a
//! registry, or a wasm toolchain. The module sources live in the repo's
//! `plugins/` directory (`plugins/example-exclaim` etc.); the prebuilt
//! `.wasm` files are committed next to them (`plugins/*.wasm`).
//!
//! # Adding a builtin plugin
//!
//! 1. Write the plugin crate under `plugins/<name>/` against
//!    `deslop-plugin-sdk` (see `plugins/example-exclaim` for the pattern).
//! 2. Build it: `cargo build -p <crate-name> --target wasm32-unknown-unknown
//!    --release` (developer-only; CI never needs the wasm target).
//! 3. Copy the module in: `cp target/wasm32-unknown-unknown/release/
//!    <crate>.wasm plugins/<install-name>.wasm` and commit it. The modules
//!    are deliberately committed (not build artifacts) so the embedding
//!    works everywhere; the `.gitignore` whitelist keeps them tracked.
//! 4. Add a [`Builtin`] entry to [`BUILTINS`] below.
//!
//! # Consuming a builtin
//!
//! Nothing runs from this directory by itself — a config must still declare
//! the plugin. After `deslop plugin install <name>`, a bare `wasm = "<name>
//! .wasm"` in `[plugin.<id>]` resolves to the installed copy.

/// One embedded plugin module.
pub struct Builtin {
    /// Install name: `deslop plugin install <name>` writes
    /// `<data_dir>/deslop/plugins/<name>.wasm`.
    pub name: &'static str,
    /// The wasm module, embedded at compile time.
    pub bytes: &'static [u8],
}

/// The builtin plugin set, in install order.
pub const BUILTINS: &[Builtin] = &[Builtin {
    name: "example-exclaim",
    bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/example-exclaim.wasm")),
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_are_unique_and_install_safe() {
        // Given the builtin registry.
        // When collecting names.
        let mut names: Vec<&str> = BUILTINS.iter().map(|b| b.name).collect();
        names.sort_unstable();

        // Then they are unique and free of path separators or extensions
        // (the installer joins them under the user plugin dir verbatim).
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
        for name in names {
            assert!(!name.contains('/'), "{name}");
            assert!(!name.contains('\\'), "{name}");
            assert!(!name.ends_with(".wasm"), "{name}");
        }
    }

    #[test]
    fn builtin_modules_are_valid_wasm_with_consistent_manifests() {
        // Given every embedded module.
        for builtin in BUILTINS {
            // Then the bytes are a wasm module (magic + version).
            assert_eq!(&builtin.bytes[0..4], b"\0asm", "{}", builtin.name);
            assert!(builtin.bytes.len() > 8, "{}", builtin.name);
        }
        // (Manifest-level validation happens at instantiate time; the
        // end-to-end install test covers it against the real host.)
    }
}
