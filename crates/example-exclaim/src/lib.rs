//! `exclaim`: a deslop plugin measuring exclamation density.
//!
//! This is the reference plugin: about fifty lines of ordinary Rust, none
//! of it about wasm, pointers, or serialization. It reports a Tier 3
//! document-level finding when a document's exclamation rate exceeds a
//! configurable threshold, anchored at the first `!`.
//!
//! Build (developer-only; CI never needs this):
//!
//! ```text
//! rustup target add wasm32-unknown-unknown
//! cargo build -p example-exclaim --target wasm32-unknown-unknown --release
//! ```
//!
//! Then wire it up:
//!
//! ```toml
//! [plugins]
//! paths = ["target/wasm32-unknown-unknown/release/example_exclaim.wasm"]
//!
//! [plugins.exclaim]
//! threshold_gt = 1.0   # findings per 1000 words; omit for the default
//! ```

use deslop_plugin_sdk::{Doc, Finding, Plugin, export};

/// `[plugins.<id>]` table. All fields default, so the section is optional.
#[derive(serde::Deserialize, Default)]
pub struct Params {
    /// Report when the rate exceeds this many exclamations per 1000 words.
    #[serde(default = "default_threshold")]
    pub threshold_gt: f64,
}

fn default_threshold() -> f64 {
    1.0
}

/// Documents under ~250 words have nonsense rates: two exclamations in a
/// short note is emphatic, not sloppy.
const MIN_WORDS: usize = 250;

pub struct Exclaim;

impl Plugin for Exclaim {
    const ID: &'static str = "EXCLAIM";
    const TIER: u8 = 3;
    const CATEGORY: &'static str = "emphasis";
    type Params = Params;

    fn scan(doc: &Doc, params: &Params) -> Vec<Finding> {
        let bangs = doc.text.bytes().filter(|&b| b == b'!').count();
        if bangs == 0 {
            return Vec::new();
        }
        let words = doc.text.split_whitespace().count();
        if words < MIN_WORDS {
            return Vec::new();
        }
        let rate = bangs as f64 / words as f64 * 1000.0;
        if rate <= params.threshold_gt {
            return Vec::new();
        }
        let at = doc.text.find('!').expect("nonzero above");
        vec![
            Finding::new(
                "exclamania",
                (at, at + 1),
                format!("exclamation rate {rate:.1} per 1000 words"),
            )
            .with_advice("cut most of these; one reads confident, ten reads shaky"),
        ]
    }
}

export!(Exclaim);
