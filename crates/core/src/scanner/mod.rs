//! Scanning pipeline: regions -> route by kind -> findings.
//!
//! Orchestration order matters: normalize -> mask -> scan -> assemble -> sort.

pub mod literal_scan;
pub mod metrics;
pub mod pattern_scan;
pub mod regions;
pub mod use_mention;
pub mod vocab_scan;

use crate::config::Config;
use crate::doc::Doc;
use crate::finding::Finding;
use crate::rule::RuleSet;

/// Scan one document against a loaded ruleset.
pub fn scan(_doc: &Doc, _rules: &RuleSet, _cfg: &Config) -> Vec<Finding> {
    Vec::new()
}
