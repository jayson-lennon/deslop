//! deslop-plugin-sdk: write a deslop plugin in plain Rust.
//!
//! A plugin is one struct implementing [`Plugin`] plus one macro call.
//! The SDK owns everything low-level: linear-memory allocation, JSON
//! marshalling, the `plugin_meta`/`alloc`/`scan` exports, and panic
//! behavior. A plugin author never sees a pointer.
//!
//! ```
//! use deslop_plugin_sdk::{export, Doc, Finding, Plugin};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize, Default)]
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
//! Then point `[plugins] paths` in `.deslop.toml` at the `.wasm` and give
//! the plugin its params under `[plugins.<your-id-lowercase>]`. See the
//! `example-exclaim` crate for a complete working plugin.
//!
//! # Panics
//!
//! A panic inside `scan` aborts the wasm module, which the host surfaces as
//! a per-document plugin failure: the plugin's findings for that document
//! are dropped with a warning. Panics are plugin bugs, not a control-flow
//! mechanism.

use deslop_plugin_protocol::{PROTOCOL_ABI, PluginFinding, PluginInput};

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

    /// The plugin's configuration, deserialized from `[plugins.<id>]`.
    /// Use `#[serde(default)]` fields so a missing table still works.
    type Params: serde::de::DeserializeOwned + Default;

    /// Analyze one document. Called once per scanned document.
    fn scan(doc: &Doc, params: &Self::Params) -> Vec<Finding>;
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

/// Expose a [`Plugin`] as the three wasm exports the deslop host speaks.
///
/// Generates a bump allocator (`alloc`, never freed — the host caps total
/// memory), `plugin_meta` (length-prefixed JSON manifest), and `scan`
/// (deserialize input + params, call [`Plugin::scan`], serialize findings).
///
/// Layout constants: the manifest lives at byte 64 (well past the length
/// prefix region and below any allocated buffer); the bump allocator starts
/// at byte 1024.
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

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn alloc(len: i32) -> i32 {
                let top: u32 = unsafe { core::ptr::read_volatile(&raw const HEAP_TOP) };
                unsafe {
                    core::ptr::write_volatile(&raw mut HEAP_TOP, top + len as u32);
                }
                top as i32
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn plugin_meta() -> i32 {
                // Serialize into the fixed slot as a 4-byte LE length prefix
                // followed by the manifest JSON.
                let manifest = manifest_bytes();
                let n = manifest.len() as u32;
                let memory = unsafe { $crate::memory_slice_mut(META_PTR, META_MAX) };
                memory[0..4].copy_from_slice(&n.to_le_bytes());
                memory[4..4 + manifest.len()].copy_from_slice(manifest.as_bytes());
                META_PTR as i32
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
