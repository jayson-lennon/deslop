//! wasmi-backed [`LintPlugin`] implementation.
//!
//! One [`WasmiPlugin`] owns a wasmi `Instance` (instantiated once, reused
//! across every scanned document) plus the interpreter `Engine` it runs on.
//! `scan` resets the fuel budget per call, so a plugin cannot bank budget
//! across documents. The store sits behind a mutex because the [`LintPlugin`]
//! trait hands out `&self` while wasmi calls need `&mut Store`; a lint run is
//! single-threaded per document, so the lock is uncontended in practice.
//!
//! ## Low-level ABI (v1)
//!
//! The module must export a linear `memory` and three functions:
//!
//! - `plugin_meta() -> i32` — pointer to a length-prefixed JSON buffer
//!   (4-byte little-endian length, then a `PluginManifest` object).
//! - `alloc(len: i32) -> i32` — returns a write slot for `len` bytes of
//!   input. No `dealloc` exists: the guest uses a bump allocator and never
//!   frees; the host's memory limiter bounds runaway growth.
//! - `scan(ptr: i32, len: i32) -> i64` — input JSON at `ptr..ptr+len`;
//!   returns `(ptr << 32) | len` of the output JSON `Vec<PluginFinding>`.
//!
//! There are no host imports in v1: plugins are pure computation.

use std::sync::Mutex;

use deslop_plugin_protocol::{PluginFinding, PluginInput, PluginManifest, PROTOCOL_ABI};

use super::{
    fuel_for, validate_finding_slug, validate_manifest, PluginError, PluginRuntime, MAX_FINDINGS,
    MAX_MEMORY_BYTES,
};

/// A plugin executed by the embedded wasmi interpreter.
pub struct WasmiPlugin {
    /// Shared interpreter engine. Instances borrow from it, so it must
    /// outlive the store.
    _engine: wasmi::Engine,
    /// Fuel override from `[plugins.<id>.runtime]`, if any.
    fuel_override: PluginRuntime,
    /// Identity read from the module's own `plugin_meta` export.
    manifest: PluginManifest,
    /// Everything wasmi needs `&mut` access to, guarded for `&self` scans.
    ctx: Mutex<GuestCtx>,
}

/// Store plus cached guest exports, locked as a unit.
struct GuestCtx {
    store: wasmi::Store<StoreState>,
    memory: wasmi::Memory,
    alloc: wasmi::TypedFunc<i32, i32>,
    scan: wasmi::TypedFunc<(i32, i32), i64>,
}

/// State attached to the wasmi store (the resource limiter).
#[derive(Debug)]
struct StoreState {
    limits: wasmi::StoreLimits,
}

impl std::fmt::Debug for WasmiPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmiPlugin")
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}

impl WasmiPlugin {
    /// Fuel override applied on top of the size-scaled default.
    pub fn set_fuel_override(&mut self, fuel: Option<u64>) {
        self.fuel_override.fuel = fuel;
    }

    /// Instantiate a plugin from compiled wasm bytes.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Load`] for invalid modules, missing exports,
    /// invalid manifests, or traps during instantiation/metadata reading.
    fn from_bytes(id_hint: &str, wasm: &[u8]) -> Result<WasmiPlugin, PluginError> {
        let load = |detail: String| PluginError::Load {
            id: id_hint.to_owned(),
            detail,
        };

        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, wasm).map_err(|e| load(e.to_string()))?;

        let mut store = {
            let limits = wasmi::StoreLimitsBuilder::new()
                .memory_size(usize::try_from(MAX_MEMORY_BYTES).unwrap_or(usize::MAX))
                .build();
            let mut store = wasmi::Store::new(&engine, StoreState { limits });
            store.limiter(|state| &mut state.limits);
            store
        };

        let linker = wasmi::Linker::new(&engine);
        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| load(format!("instantiation failed: {e}")))?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| load("module does not export a linear memory".into()))?;
        let meta_fn = instance
            .get_typed_func::<(), i32>(&store, "plugin_meta")
            .map_err(|e| load(format!("missing plugin_meta export: {e}")))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "alloc")
            .map_err(|e| load(format!("missing alloc export: {e}")))?;
        let scan_fn = instance
            .get_typed_func::<(i32, i32), i64>(&store, "scan")
            .map_err(|e| load(format!("missing scan export: {e}")))?;

        // Fuel for instantiation + the metadata call: the fixed baseline,
        // independent of any document.
        store
            .set_fuel(super::FUEL_BASE)
            .map_err(|e| load(format!("fuel setup failed: {e}")))?;
        let ptr = meta_fn
            .call(&mut store, ())
            .map_err(map_call_error(id_hint, "plugin_meta"))?;
        let manifest_bytes = read_length_prefixed(&store, &memory, ptr)?;
        let manifest: PluginManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|e| load(format!("plugin_meta is not a valid manifest: {e}")))?;
        validate_manifest(&manifest).map_err(load)?;

        Ok(WasmiPlugin {
            _engine: engine,
            fuel_override: PluginRuntime::default(),
            manifest,
            ctx: Mutex::new(GuestCtx {
                store,
                memory,
                alloc,
                scan: scan_fn,
            }),
        })
    }
}

impl super::LintPlugin for WasmiPlugin {
    fn meta(&self) -> &PluginManifest {
        &self.manifest
    }

    fn scan(&self, input: &PluginInput) -> Result<Vec<PluginFinding>, PluginError> {
        let id = &self.manifest.id;
        let protocol = |detail: String| PluginError::Protocol {
            id: id.to_owned(),
            detail,
        };
        let input_bytes = serde_json::to_vec(input)
            .map_err(|e| protocol(format!("host failed to serialize input: {e}")))?;
        if input_bytes.len() > u32::MAX as usize {
            return Err(protocol("document input too large".into()));
        }
        let fuel = fuel_for(Some(&self.fuel_override), input_bytes.len());
        let mut guard = self.ctx.lock().map_err(|_| {
            protocol("plugin store lock poisoned by a previous panic".into())
        })?;
        // Destructure so Rust sees disjoint borrows of the fields.
        let GuestCtx {
            store,
            memory,
            alloc,
            scan,
        } = &mut *guard;

        // Per-call fuel reset: a plugin cannot bank budget across documents,
        // and each document gets the full size-scaled budget.
        store
            .set_fuel(fuel)
            .map_err(|e| protocol(format!("fuel setup failed: {e}")))?;

        let ptr = alloc
            .call(&mut *store, input_bytes.len() as i32)
            .map_err(map_call_error(id, "alloc"))?;
        let ptr = usize::try_from(ptr as u32)
            .map_err(|_| protocol(format!("alloc returned invalid pointer {ptr}")))?;
        bounds(id, memory.data(&*store), ptr, input_bytes.len())?;
        memory
            .write(&mut *store, ptr, &input_bytes)
            .map_err(|e| protocol(format!("host failed to write input: {e}")))?;

        let packed = scan
            .call(&mut *store, (ptr as i32, input_bytes.len() as i32))
            .map_err(map_call_error(id, "scan"))?;
        let (out_ptr, out_len) = unpack_ptr_len(packed, id)?;
        let out_bytes = slice(id, memory.data(&*store), out_ptr, out_len)?;
        let findings: Vec<PluginFinding> = serde_json::from_slice(out_bytes)
            .map_err(|e| protocol(format!("invalid findings JSON: {e}")))?;
        if findings.len() > MAX_FINDINGS {
            return Err(protocol(format!(
                "returned {} findings (max {MAX_FINDINGS})",
                findings.len()
            )));
        }
        for finding in &findings {
            validate_finding_slug(&finding.slug)
                .map_err(|detail| protocol(format!("invalid slug {:?}: {detail}", finding.slug)))?;
        }
        Ok(findings)
    }
}

/// Validate that `ptr..ptr+len` lies inside the given guest memory image.
fn bounds(id: &str, data: &[u8], ptr: usize, len: usize) -> Result<(), PluginError> {
    let end = ptr
        .checked_add(len)
        .ok_or_else(|| PluginError::Protocol {
            id: id.to_owned(),
            detail: "buffer pointer overflows usize".into(),
        })?;
    if end > data.len() {
        return Err(PluginError::Protocol {
            id: id.to_owned(),
            detail: format!("buffer [{ptr}..{end}) exceeds guest memory of {} bytes", data.len()),
        });
    }
    Ok(())
}

/// Slice a previously validated range of guest memory.
fn slice<'a>(id: &str, data: &'a [u8], ptr: usize, len: usize) -> Result<&'a [u8], PluginError> {
    data.get(ptr..ptr + len).ok_or_else(|| PluginError::Protocol {
        id: id.to_owned(),
        detail: format!("buffer [{ptr}..{}) out of bounds", ptr + len),
    })
}

/// Read a 4-byte LE length prefix followed by that many bytes at `ptr`,
/// returning just the body bytes.
fn read_length_prefixed<'a>(
    store: &'a wasmi::Store<StoreState>,
    memory: &'a wasmi::Memory,
    ptr: i32,
) -> Result<&'a [u8], PluginError> {
    let protocol = |detail: String| PluginError::Protocol {
        id: "unknown plugin".into(),
        detail,
    };
    let ptr = (ptr as u32) as usize;
    let data = memory.data(store);
    let head = data
        .get(ptr..ptr + 4)
        .ok_or_else(|| protocol(format!("metadata header [{ptr}..{}] out of bounds", ptr + 4)))?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    data.get(ptr + 4..ptr + 4 + len).ok_or_else(|| {
        protocol(format!(
            "metadata body [{},{}) out of bounds",
            ptr + 4,
            ptr + 4 + len
        ))
    })
}

/// Unpack a guest-returned `(ptr << 32) | len` i64.
fn unpack_ptr_len(packed: i64, id: &str) -> Result<(usize, usize), PluginError> {
    if packed < 0 {
        return Err(PluginError::Protocol {
            id: id.to_owned(),
            detail: format!("scan returned invalid packed pointer {packed}"),
        });
    }
    let packed = packed as u64;
    let ptr = (packed >> 32) as u32 as usize;
    let len = packed as u32 as usize;
    Ok((ptr, len))
}

/// Translate a wasmi call error into the right [`PluginError`], separating
/// fuel exhaustion from other traps.
fn map_call_error<'a>(id: &'a str, what: &'static str) -> impl Fn(wasmi::Error) -> PluginError + 'a {
    move |error| {
        if matches!(error.as_trap_code(), Some(wasmi::TrapCode::OutOfFuel)) {
            PluginError::Fuel {
                id: id.to_owned(),
            }
        } else {
            PluginError::Trap {
                id: id.to_owned(),
                detail: format!("{what} failed: {error}"),
            }
        }
    }
}

/// Instantiate a plugin from a `.wasm` file on disk.
///
/// # Errors
///
/// Returns [`PluginError::Load`] for unreadable files, invalid modules,
/// missing/invalid metadata, or instantiation traps.
pub fn instantiate(path: &camino::Utf8Path) -> Result<WasmiPlugin, PluginError> {
    let id_hint = path.file_name().unwrap_or("plugin").to_owned();
    let wasm = std::fs::read(path).map_err(|e| PluginError::Load {
        id: id_hint.clone(),
        detail: format!("failed to read: {e}"),
    })?;
    WasmiPlugin::from_bytes(&id_hint, &wasm)
}

const _: () = {
    // Compile-time reminder: the ABI this file speaks must match the
    // protocol crate version it was compiled against.
    assert!(PROTOCOL_ABI == 1);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpack_ptr_len_splits_high_and_low_words() {
        // Given a packed pointer/length in the scan return format.
        // When unpacking.
        // Then both halves are recovered.
        let packed = ((4096u64) << 32) | 65;
        assert_eq!(unpack_ptr_len(packed as i64, "X").expect("unpack"), (4096, 65));
    }

    #[test]
    fn unpack_ptr_len_rejects_negative_packed_values() {
        // Given a packed value with the sign bit set (impossible from u32 pairs).
        // When unpacking.
        // Then it is a protocol violation.
        assert!(unpack_ptr_len(-1, "X").is_err());
    }

    #[test]
    fn out_of_bounds_scan_output_is_a_protocol_error() {
        // Given one page of real guest memory.
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let wasm = wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
            )"#,
        )
        .expect("wat");
        let module = wasmi::Module::new(&engine, &wasm).expect("module");
        let mut store = wasmi::Store::new(&engine, StoreState {
            limits: wasmi::StoreLimitsBuilder::new().build(),
        });
        let instance = wasmi::Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .expect("instantiate");
        let memory = instance.get_memory(&store, "memory").expect("memory");
        let (ptr, len) = (2048usize, u32::MAX as usize);

        // When validating a claimed buffer far larger than guest memory.
        let error = bounds("BOGUS", memory.data(&store), ptr, len).expect_err("must reject");

        // Then it is a protocol violation, not a panic.
        assert!(matches!(error, PluginError::Protocol { .. }), "{error:?}");
    }

    #[test]
    fn bounds_accepts_ranges_inside_guest_memory() {
        // Given one page of guest memory.
        let data = vec![0u8; 65536];
        // When validating an in-bounds and an out-of-bounds range.
        // Then only the in-bounds range passes.
        assert!(bounds("X", &data, 65500, 36).is_ok());
        assert!(bounds("X", &data, 65500, 37).is_err());
    }
}
