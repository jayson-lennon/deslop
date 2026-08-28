//! deslop-plugin-protocol: the frozen wire contract between host and plugins.
//!
//! This crate is the single source of truth for everything that crosses the
//! host↔plugin boundary. It contains types and constants only — no logic — so
//! it can be depended on by both sides without pulling in the engine:
//!
//! - `deslop-core` (the host) depends on it to build [`PluginInput`] and
//!   interpret [`PluginFinding`]s.
//! - `deslop-plugin-sdk` (guest side) depends on it so plugin binaries stay
//!   lean and never link the engine.
//!
//! The wire format is JSON over the plugin's linear memory (see the `deslop`
//! plugin spec): serde derives are the ABI. All integers are [`u64`] because
//! they serialize as JSON numbers that both a 64-bit host and a 32-bit
//! wasm guest can handle.

/// Version of the low-level ABI this crate describes.
///
/// Bumped when the shape of the exports/imports contract changes (not when
/// fields are added to these structs — guests must ignore unknown JSON
/// fields, so additive wire evolution needs no bump).
pub const PROTOCOL_ABI: u32 = 1;

/// Static identity of a plugin, embedded in the module itself.
///
/// Produced by the guest's `plugin_meta()` export; validated by the host at
/// load time. One plugin = one manifest: sharing a plugin across projects
/// never requires re-declaring identity in each project's config.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginManifest {
    /// Stable id used everywhere else: `[lints]` keys, `entry_id` prefixes.
    /// Conventionally UPPER_SNAKE (config keys are matched case-insensitively).
    pub id: String,
    /// Severity tier (1–3) assigned to every finding this plugin emits.
    pub tier: u8,
    /// Free-form grouping label shown by `deslop rules` (e.g. `"emphasis"`).
    pub category: String,
    /// ABI version the guest was compiled against. Must equal
    /// [`PROTOCOL_ABI`] for the host to instantiate the plugin.
    pub abi: u32,
}

/// The per-document envelope the host hands to a plugin's `scan` export.
///
/// One call per document. All coordinates are byte offsets into [`Self::text`]
/// — the normalized, use-mention-masked document text — and there is exactly
/// one coordinate space on this side of the boundary (native `prose` regions
/// are deliberately not exposed). Masked bytes appear as `'\0'`.
///
/// Structs are additively extensible: the host may add fields in future
/// versions; guests (via the SDK) deserialize into a type that ignores
/// unknown fields, so old plugins keep working against newer hosts.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginInput {
    /// Normalized (LF-only) document text, with quoted-term masking applied.
    /// Same length as the normalized original; masked bytes are `'\0'`.
    pub text: String,
    /// Byte ranges of ATX headings in `text` coordinates.
    pub heading_ranges: Vec<(u64, u64)>,
    /// Byte ranges of bold segments (`**…**`) in `text` coordinates.
    pub bold_spans: Vec<(u64, u64)>,
    /// Byte ranges of list item bodies in `text` coordinates.
    pub list_items: Vec<(u64, u64)>,
    /// The plugin's `[plugins.<id>]` config table, verbatim and opaque.
    /// `{}` when the user declared nothing.
    pub config: serde_json::Value,
}

impl Default for PluginInput {
    fn default() -> Self {
        PluginInput {
            text: String::new(),
            heading_ranges: Vec::new(),
            bold_spans: Vec::new(),
            list_items: Vec::new(),
            // Always an object, never null: guests may deserialize it as a map.
            config: serde_json::json!({}),
        }
    }
}

/// One finished finding produced by a plugin.
///
/// The plugin owns its entire pipeline (metrics, thresholds, wording): the
/// host performs no template rendering. `message` is final; metric numbers
/// are baked into it because the JSON finding schema is frozen.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginFinding {
    /// Short stable slug; the host joins it into `"<PLUGIN_ID>#<slug>"`.
    /// Must be non-empty, whitespace-free, and `'#'`-free (enforced by the
    /// SDK and re-checked by the host).
    pub slug: String,
    /// Half-open span `[start, end)` in [`PluginInput::text`] coordinates.
    /// Serialized as a JSON array (`"span": [120, 124]`).
    pub span: (u64, u64),
    /// Fully formatted message (metric numbers included verbatim).
    pub message: String,
    /// Optional advice line, rendered like native pack advice.
    pub advice: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_json_roundtrips_field_order_stable() {
        // Given a manifest with all fields set.
        let manifest = PluginManifest {
            id: "EXCLAIM".into(),
            tier: 3,
            category: "emphasis".into(),
            abi: PROTOCOL_ABI,
        };

        // When serializing to JSON.
        let json = serde_json::to_string(&manifest).expect("serialize");

        // Then field order is declaration order and values survive a round trip.
        assert_eq!(
            json,
            r#"{"id":"EXCLAIM","tier":3,"category":"emphasis","abi":1}"#
        );
        let back: PluginManifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, manifest);
    }

    #[test]
    fn input_config_defaults_to_empty_object() {
        // Given no config at all.
        let input = PluginInput::default();

        // When serializing.
        let json = serde_json::to_string(&input).expect("serialize");

        // Then config is an empty JSON object, not null.
        assert!(json.contains(r#""config":{}"#));
    }

    #[test]
    fn finding_span_serializes_as_json_array() {
        // Given a finding with a tuple span.
        let finding = PluginFinding {
            slug: "demo".into(),
            span: (120, 124),
            message: "demo hit".into(),
            advice: None,
        };

        // When serializing.
        let json = serde_json::to_string(&finding).expect("serialize");

        // Then the span renders as an array.
        assert!(json.contains(r#""span":[120,124]"#));
    }

    #[test]
    fn finding_deserialize_ignores_unknown_fields() {
        // Given JSON with a field a future host might add.
        let json = r#"{"slug":"s","span":[0,1],"message":"m","advice":null,"future":true}"#;

        // When deserializing.
        let finding: PluginFinding = serde_json::from_str(json).expect("deserialize");

        // Then the known fields parse and the unknown one is dropped.
        assert_eq!(finding.slug, "s");
    }
}
