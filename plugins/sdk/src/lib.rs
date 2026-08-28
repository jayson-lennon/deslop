//! deslop-plugin-sdk: write a deslop plugin in plain Rust.
//!
//! A plugin is one struct implementing [`Plugin`] plus one macro call.
//! The SDK owns everything low-level: linear-memory allocation, JSON
//! marshalling, the `plugin_meta`/`alloc`/`scan` exports, and panic
//! behavior. A plugin author never sees a pointer.
//!
//! ```
//! use deslop_plugin_sdk::{export, Doc, Finding, Plugin};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, Serialize, Default)]
//! struct Params {}
//!
//! struct MyPlugin;
//!
//! impl Plugin for MyPlugin {
//!     const ID: &'static str = "MYPLUGIN";
//!     const TIER: u8 = 3;
//!     const CATEGORY: &'static str = "style";
//!     type Params = Params;
//!
//!     fn scan(doc: &Doc, _params: &Params) -> Vec<Finding> {
//!         Vec::new() // analyze doc.text here
//!     }
//! }
//!
//! export!(MyPlugin);
//! ```
//!
//! Build (developer-only; CI and `cargo test` never need this):
//!
//! ```text
//! rustup target add wasm32-unknown-unknown
//! cargo build -p your-plugin --target wasm32-unknown-unknown --release
//! ```
//!
//! Then declare the plugin in `.deslop.toml` and give it its params in the
//! same table. The `wasm` path resolves by form: absolute paths are used
//! exactly, `./`/`../` paths are relative to the config file, and bare
//! names look in the install dir `~/.local/share/deslop/plugins/`. See the
//! `example-exclaim` crate for a complete working plugin.
//!
//! ```toml
//! [plugin.exclaim]
//! wasm = "exclaim.wasm"     # → ~/.local/share/deslop/plugins/exclaim.wasm
//! threshold_gt = 1.0        # anything but wasm/enabled/runtime is a param
//! ```
//!
//! # Panics
//!
//! A panic inside `scan` aborts the wasm module, which the host surfaces as
//! a per-document plugin failure: the plugin's findings for that document
//! are dropped with a warning. Panics are plugin bugs, not a control-flow
//! mechanism.

use deslop_plugin_protocol::{PROTOCOL_ABI, ParamOption, PluginFinding, PluginInput};

/// Author-facing doc for one configurable param (const-constructible).
///
/// `default` is the param's default value written as a JSON literal —
/// `1.0`, `true`, `"verbose"` — so `PARAM_DOCS` can live in a `const`. The
/// `export!` macro verifies each literal against the `Params` type's real
/// serde default and converts to the wire [`ParamOption`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDoc {
    /// Param name as it appears in `[plugin.<id>]`.
    pub name: &'static str,
    /// Default value as a JSON literal.
    pub default: &'static str,
    /// One-line human description (empty string = none).
    pub description: &'static str,
}

/// The document envelope a plugin analyzes. Coordinates are byte offsets
/// into `text`; masked (use-mention suppressed) bytes appear as `'\0'`.
pub type Doc = PluginInput;

/// One finding a plugin emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Short stable slug, joined by the host into `"<ID>#<slug>"`. Must be
    /// non-empty, whitespace-free, and `'#'`-free.
    pub slug: &'static str,
    /// Half-open span `[start, end)` in `text` coordinates.
    pub span: (usize, usize),
    /// Final message; metric numbers are baked in here.
    pub message: String,
    /// Optional advice line.
    pub advice: Option<String>,
}

impl Finding {
    /// Convenience constructor for findings without advice.
    pub fn new(slug: &'static str, span: (usize, usize), message: impl Into<String>) -> Self {
        Finding {
            slug,
            span,
            message: message.into(),
            advice: None,
        }
    }

    /// Attach advice to this finding (builder style).
    #[must_use]
    pub fn with_advice(mut self, advice: impl Into<String>) -> Self {
        self.advice = Some(advice.into());
        self
    }
}

impl From<Finding> for PluginFinding {
    fn from(f: Finding) -> Self {
        PluginFinding {
            slug: f.slug.to_owned(),
            span: (f.span.0 as u64, f.span.1 as u64),
            message: f.message,
            advice: f.advice,
        }
    }
}

/// Alias used by the `export!` macro for the wire conversion, keeping the
/// generated code self-documenting without importing the protocol crate.
pub type RepackFinding = PluginFinding;

/// The plugin contract. Implement this, then call [`export!`].
pub trait Plugin {
    /// Stable id (`[lints]` key, `entry_id` prefix). No `'#'`, no whitespace.
    const ID: &'static str;
    /// Severity tier 1–3 (artifact / tell / density).
    const TIER: u8;
    /// Free-form grouping label shown by `deslop rules`.
    const CATEGORY: &'static str;
    /// ABI version this SDK speaks; bumping is a breaking release.
    const ABI_VERSION: u32 = PROTOCOL_ABI;

    /// The plugin's configuration, deserialized from `[plugin.<id>]`.
    /// Use `#[serde(default)]` fields so a missing table still works.
    /// Must also implement `Serialize` (used to surface param defaults).
    type Params: serde::de::DeserializeOwned + serde::Serialize + Default;

    /// Documentation for the params, shown by `deslop plugin install` as
    /// commented defaults. Optional — the default is empty, and plugins
    /// without configurable params need not mention this.
    ///
    /// ```ignore
    /// const PARAM_DOCS: &[ParamDoc] = &[ParamDoc {
    ///     name: "threshold_gt",
    ///     default: "1.0",
    ///     description: "exclamations per 1000 words before findings start",
    /// }];
    /// ```
    ///
    /// Each `default` literal is verified against the `Params` type's real
    /// serde default when the module is built (`export!` calls
    /// [`param_options`]); a mismatch aborts the module, which fails CI for
    /// builtins and load for third-party plugins.
    const PARAM_DOCS: &[ParamDoc] = &[];

    /// Analyze one document. Called once per scanned document.
    fn scan(doc: &Doc, params: &Self::Params) -> Vec<Finding>;
}

/// Build the wire param schema from [`Plugin::PARAM_DOCS`], guaranteeing
/// zero drift from the `Params` type.
///
/// Deserializes `Params` from an empty JSON object — serde then applies
/// every field's `#[serde(default)]`, i.e. exactly what a config omitting
/// the params would produce — and serializes it back out. Each doc's
/// `default` literal is parsed and compared against that result: an unknown
/// name, or a literal that differs from the type's real default, aborts the
/// module (the host surfaces the failure at load; a shipped builtin fails
/// in CI the moment it is embedded).
///
/// Called by the `export!` macro; plugin authors never call this directly.
pub fn param_options<T: Plugin>(docs: &[ParamDoc]) -> Vec<ParamOption> {
    // The Params-as-{} round trip is the source of truth for defaults.
    let realized: serde_json::Value = match serde_json::from_str::<T::Params>("{}") {
        Ok(params) => serde_json::to_value(&params).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    };
    docs.iter()
        .map(|doc| {
            let actual = realized.get(doc.name).unwrap_or_else(|| {
                panic!(
                    "PARAM_DOCS names param {:?}, which is not a field of {} (or has no serde \
                     default); PARAM_DOCS must mirror the Params type",
                    doc.name,
                    core::any::type_name::<T::Params>()
                )
            });
            let claimed: serde_json::Value =
                serde_json::from_str(doc.default).unwrap_or_else(|e| {
                    panic!(
                        "PARAM_DOCS default for {:?} is not valid JSON ({:?}): {e}",
                        doc.name, doc.default
                    )
                });
            if actual != &claimed {
                panic!(
                    "PARAM_DOCS says {} defaults to {claimed}, but the Params type defaults to \
                     {actual}; fix PARAM_DOCS or the serde default so they agree",
                    doc.name
                );
            }
            ParamOption {
                name: doc.name.to_owned(),
                default: claimed,
                description: (!doc.description.is_empty()).then(|| doc.description.to_owned()),
            }
        })
        .collect()
}

/// First whole-word occurrence of `term_lower` (already lower-case) in
/// `text`, matching the native vocab scanner's token semantics: the hit
/// must not be preceded or followed by another ASCII alphanumeric.
pub fn find_word(text: &str, term_lower: &str) -> Option<(usize, usize)> {
    find_word_from(text, term_lower, 0)
}

/// All whole-word occurrences of `term_lower`, in order.
pub fn find_all_words(text: &str, term_lower: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((start, end)) = find_word_from(text, term_lower, from) {
        out.push((start, end));
        from = end.max(start + 1);
    }
    out
}

fn find_word_from(text: &str, term_lower: &str, from: usize) -> Option<(usize, usize)> {
    let term = term_lower.as_bytes();
    let bytes = text.as_bytes();
    let mut from = from;
    while let Some(rel) = text[from..].find(term_lower) {
        let start = from + rel;
        let end = start + term.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return Some((start, end));
        }
        from = start + 1;
    }
    None
}

/// Read linear memory as a byte slice at `[base, base+size)`.
///
/// # Safety
///
/// `base..base+size` must be within the wasm linear memory.
pub unsafe fn memory_slice<'a>(base: u32, size: u32) -> &'a [u8] {
    unsafe { core::slice::from_raw_parts(base as *const u8, size as usize) }
}

/// Read linear memory as a mutable byte slice at `[base, base+size)`.
///
/// # Safety
///
/// `base..base+size` must be within the wasm linear memory, and no other
/// live reference may alias it.
pub unsafe fn memory_slice_mut<'a>(base: u32, size: u32) -> &'a mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(base as *mut u8, size as usize) }
}

/// Expose a [`Plugin`] as the wasm exports the deslop host speaks.
///
/// Generates a bump allocator (`alloc`, never freed — the host caps total
/// memory), `plugin_meta` (length-prefixed JSON manifest), `scan`
/// (deserialize input + params, call [`Plugin::scan`], serialize findings),
/// and — when the plugin documents params via [`Plugin::PARAM_DOCS`] —
/// `plugin_params_schema` (length-prefixed JSON `ParamOption` list, from
/// which the host renders config hints with real defaults).
///
/// Layout constants: the manifest lives at byte 64 and the params schema at
/// byte 640 (well past the length-prefix regions and below any allocated
/// buffer); the bump allocator starts at byte 1024.
#[macro_export]
macro_rules! export {
    ($ty:ty) => {
        const _: () = {
            /// Bump allocator top; buffers never free. The host's memory
            /// limit bounds total growth.
            static mut HEAP_TOP: u32 = 1024;

            /// Where the length-prefixed manifest lives (fixed slot).
            const META_PTR: u32 = 64;
            const META_MAX: u32 = 512;

            /// Where the length-prefixed params schema lives (fixed slot).
            const SCHEMA_PTR: u32 = 640;
            const SCHEMA_MAX: u32 = 384;

            /// Serialize the manifest JSON (host ABI: length-prefixed).
            fn manifest_bytes() -> std::string::String {
                format!(
                    "{{\"id\":\"{}\",\"tier\":{},\"category\":\"{}\",\"abi\":{}}}",
                    <$ty as $crate::Plugin>::ID,
                    <$ty as $crate::Plugin>::TIER,
                    <$ty as $crate::Plugin>::CATEGORY,
                    <$ty as $crate::Plugin>::ABI_VERSION,
                )
            }

            /// The documented params, defaults verified against `Params`.
            fn schema_bytes() -> std::vec::Vec<u8> {
                let options = $crate::param_options::<$ty>(<$ty as $crate::Plugin>::PARAM_DOCS);
                serde_json::to_vec(&options).unwrap_or_else(|_| std::vec::Vec::new())
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn alloc(len: i32) -> i32 {
                let top: u32 = unsafe { core::ptr::read_volatile(&raw const HEAP_TOP) };
                unsafe {
                    core::ptr::write_volatile(&raw mut HEAP_TOP, top + len as u32);
                }
                top as i32
            }

            /// Write a length-prefixed payload into a fixed memory slot.
            ///
            /// Returns the slot pointer, or 0 when the payload does not fit
            /// (the host reads 0 as "no such metadata").
            fn write_prefixed(payload: &[u8], ptr: u32, max: u32) -> i32 {
                if 4 + payload.len() as u32 > max {
                    return 0;
                }
                let memory = unsafe { $crate::memory_slice_mut(ptr, max) };
                memory[0..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
                memory[4..4 + payload.len()].copy_from_slice(payload);
                ptr as i32
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn plugin_meta() -> i32 {
                write_prefixed(manifest_bytes().as_bytes(), META_PTR, META_MAX)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn plugin_params_schema() -> i32 {
                write_prefixed(&schema_bytes(), SCHEMA_PTR, SCHEMA_MAX)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn scan(ptr: i32, len: i32) -> i64 {
                let input_bytes = unsafe { $crate::memory_slice(ptr as u32, len as u32) };
                let doc: $crate::Doc = match serde_json::from_slice(input_bytes) {
                    Ok(doc) => doc,
                    Err(_) => return 0,
                };
                let params = match serde_json::from_value(doc.config.clone()) {
                    Ok(params) => params,
                    Err(_) => return 0,
                };
                let findings: Vec<$crate::Finding> = <$ty as $crate::Plugin>::scan(&doc, &params);
                let wire: Vec<$crate::RepackFinding> =
                    findings.into_iter().map(Into::into).collect();
                let out = match serde_json::to_vec(&wire) {
                    Ok(out) => out,
                    Err(_) => return 0,
                };
                if out.len() > u32::MAX as usize {
                    return 0;
                }
                // Output buffer from the bump allocator.
                let out_ptr: u32 = unsafe {
                    let top = core::ptr::read_volatile(&raw const HEAP_TOP);
                    core::ptr::write_volatile(&raw mut HEAP_TOP, top + out.len() as u32);
                    top
                };
                unsafe {
                    $crate::memory_slice_mut(out_ptr, out.len() as u32).copy_from_slice(&out);
                }
                ((out_ptr as i64) << 32) | (out.len() as i64)
            }
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A params type with a nontrivial serde default.
    #[derive(serde::Deserialize, serde::Serialize, Default)]
    struct TestParams {
        #[serde(default = "default_rate")]
        rate: f64,
        #[serde(default)]
        verbose: bool,
    }

    fn default_rate() -> f64 {
        2.5
    }

    struct TestPlugin;

    impl Plugin for TestPlugin {
        const ID: &'static str = "TEST";
        const TIER: u8 = 3;
        const CATEGORY: &'static str = "test";
        type Params = TestParams;

        fn scan(_doc: &Doc, _params: &TestParams) -> Vec<Finding> {
            Vec::new()
        }
    }

    #[test]
    fn param_options_passes_through_docs_matching_real_defaults() {
        // Given PARAM_DOCS whose literals match the Params serde defaults.
        let docs = vec![
            ParamDoc {
                name: "rate",
                default: "2.5",
                description: "the rate",
            },
            ParamDoc {
                name: "verbose",
                default: "false",
                description: "",
            },
        ];

        // When building the wire schema.
        let options = param_options::<TestPlugin>(&docs);

        // Then every doc passes through with its description intact and an
        // empty description dropped.
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].name, "rate");
        assert_eq!(options[0].default, serde_json::json!(2.5));
        assert_eq!(options[0].description.as_deref(), Some("the rate"));
        assert_eq!(options[1].name, "verbose");
        assert_eq!(options[1].default, serde_json::json!(false));
        assert_eq!(options[1].description, None);
    }

    #[test]
    #[should_panic(expected = "defaults to 3, but the Params type defaults to 2.5")]
    fn param_options_rejects_a_drifted_default_literal() {
        // Given a doc claiming a default the Params type does not have.
        let docs = vec![ParamDoc {
            name: "rate",
            default: "3",
            description: "",
        }];

        // When building the wire schema.
        // Then it aborts naming the disagreement.
        param_options::<TestPlugin>(&docs);
    }

    #[test]
    #[should_panic(expected = "is not a field of")]
    fn param_options_rejects_an_unknown_param_name() {
        // Given a doc naming a param that does not exist.
        let docs = vec![ParamDoc {
            name: "nope",
            default: "1",
            description: "",
        }];

        // When building the wire schema.
        // Then it aborts.
        param_options::<TestPlugin>(&docs);
    }

    #[test]
    #[should_panic(expected = "is not valid JSON")]
    fn param_options_rejects_an_invalid_default_literal() {
        // Given a doc whose default literal is not JSON.
        let docs = vec![ParamDoc {
            name: "rate",
            default: "about 2",
            description: "",
        }];

        // When building the wire schema.
        // Then it aborts.
        param_options::<TestPlugin>(&docs);
    }

    #[test]
    fn param_options_with_no_docs_is_empty() {
        // Given no PARAM_DOCS.
        // When building the wire schema.
        // Then it is empty (plugins without params need nothing).
        assert!(param_options::<TestPlugin>(&[]).is_empty());
    }
}
