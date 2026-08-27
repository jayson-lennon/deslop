//! deslop-core: engine for the deslop linter.
//!
//! Everything about lint rules is external data: TOML packs loaded at startup.
//! The binary embeds no compiled-in rules; this crate is their interpreter.

pub mod config;
pub mod doc;
pub mod finding;
pub mod metric_stats;
pub mod rule;
pub mod scanner;

/// Walk every `*.toml` beneath `root` in deterministic (sorted) order.
///
/// Shared by the rule loader and by converters: determinism here is what makes
/// generated pack output byte-stable.
pub fn sorted_toml_files(root: &camino::Utf8Path) -> Vec<camino::Utf8PathBuf> {
    walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|entry| {
            let path = entry.ok()?.into_path();
            let is_file_toml = path.is_file() && path.extension().is_some_and(|ext| ext == "toml");
            is_file_toml.then(|| camino::Utf8PathBuf::from_path_buf(path).ok())?
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_toml_files_lists_only_tomls_in_sorted_order() {
        // Given a temp tree with unsorted names and mixed extensions.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        for name in ["zz.toml", "aa.toml", "mid.txt", "sub/mm.toml"] {
            std::fs::write(dir.path().join(name), "").expect("write");
        }

        // When listing.
        let files = sorted_toml_files(camino::Utf8Path::from_path(dir.path()).expect("utf8"));

        // Then only tomls appear, parents before children, sorted.
        let names: Vec<_> = files.iter().map(|p| p.as_str().to_owned()).collect();
        assert_eq!(
            names,
            vec!["aa.toml", "sub/mm.toml", "zz.toml"]
                .into_iter()
                .map(|n| format!("{}/{}", dir.path().display(), n))
                .collect::<Vec<_>>()
        );
    }
}
