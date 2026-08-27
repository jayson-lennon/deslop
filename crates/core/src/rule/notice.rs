//! NOTICE.toml: per-pack attribution registry for converted rules.
//!
//! A pack directory with converted content carries a sibling NOTICE.toml:
//!
//! ```toml
//! license = "MIT"
//! [[origin]]
//! repo = "https://github.com/theserverlessdev/wsc"
//! commit = "202391de9e0020e8f23faab9ae10ae6c2601253c"
//! files = ["prose-constructions/*.toml"]   # optional glob hint
//! ```
//!
//! The loader refuses a rule file that declares `[origin]` unless the exact
//! (repo, commit) pair appears in its pack's NOTICE.

/// Parsed subset of NOTICE.toml we care about.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub license: Option<String>,
    #[serde(default)]
    pub origin: Vec<NoticeOrigin>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoticeOrigin {
    pub repo: String,
    pub commit: String,
    #[serde(default)]
    pub files: Vec<String>,
}

impl Notice {
    /// Parse NOTICE.toml text.
    pub fn parse(text: &str) -> Result<Notice, toml::de::Error> {
        toml::from_str(text)
    }

    /// Does this notice cover the (repo, commit) pair?
    pub fn covers(&self, repo: &str, commit: &str) -> bool {
        self.origin
            .iter()
            .any(|o| o.repo == repo && o.commit == commit)
    }
}
