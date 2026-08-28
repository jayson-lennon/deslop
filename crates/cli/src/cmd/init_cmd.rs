//! `deslop init`: write an annotated starter `.deslop.toml`.

use std::io::Write;

/// Annotated template with a comment for every knob (spec: cmd/init_cmd.rs).
pub const TEMPLATE: &str = r#"# deslop configuration. Place at project root; the CLI walks up to find it.
# Delete this file to fall back to full defaults.

[packs]
# Which builtin packs load, in load order. A pack is ONE TOML file in
# rules/<stem>.toml containing any number of [[group]] tables. On load,
# every vocab/literal term gets ONE owner - the stricter (lower) tier wins,
# config order breaks ties - and identical pattern regexes compile once and
# fan their findings out to every owning rule.
#   aatell         - Tier 2: frequency-measured AI-tell words (seed rewrites)
#   slop           - Tier 2: AI-slop words and phrases
#   wsc            - Tier 2: vocabulary + structural prose patterns
#   aisigns        - Tier 1/3: chatbot markup artifacts + document metrics
#   cluster-terms  - Tier 3: single-word watch list (lints via cluster only)
builtin = ["aatell", "slop", "wsc", "aisigns", "cluster-terms"]
# Extra rule packs by file or directory path.
extra_paths = []

[scan]
tiers = [1, 2, 3]        # or cap with [1, 2] / [1]
respect_gitignore = true # skip gitignored paths when scanning directories
extra_globs = []         # extra include globs for directory scans

[output]
format = "human"         # human | json | github
color = "auto"           # auto | always | never

# Per-lint levels, clippy-style. Key is GROUP or GROUP#slug; value is
# allow | note | warn | error (default = the rule's tier).
#[lints]
#AATELL = "allow"                       # whole group off
#"AATELL#leverage" = "allow"            # one entry off
#"WSC-PAT-NOT-ONLY-BUT-ALSO" = "error"  # escalate to error

# Rule authoring quickstart (full guide in README):
#   - a rule FILE holds many [[group]] tables; each table is one group
#     with its own kind/tier/category and [[group.entries]],
#   - entries prove themselves via group.fixtures must_match/must_not_match
#     (a rule whose fixtures fail refuses to load),
#   - single words usually want `stems = true` (one entry, all inflections),
#   - enabled entries need `advice`; CI enforces DESLOP_REQUIRE_ADVICE=1.
"#;

/// Execute; returns process exit code (2 if file exists, else 0).
pub fn run() -> i32 {
    let path = std::path::Path::new(".deslop.toml");
    if path.exists() {
        eprintln!("deslop: .deslop.toml already exists here; not overwriting");
        return crate::ExitCode::LoadFailure as i32;
    }
    let wrote = std::fs::write(path, TEMPLATE).is_ok();
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(
        lock,
        "{}",
        if wrote {
            "wrote .deslop.toml - see comments inside for each knob"
        } else {
            "deslop: cannot write ./.deslop.toml"
        }
    );
    if wrote {
        crate::ExitCode::Clean as i32
    } else {
        crate::ExitCode::LoadFailure as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_writes_template_and_refuses_to_clobber() {
        // Given an empty directory.
        let dir = std::env::temp_dir().join(format!("deslop-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");

        // When running init twice.
        let first = run_in(&dir);
        let second = run_in(&dir);

        // Then the first writes, the second refuses, and the file parses.
        assert_eq!(first, 0, "first init succeeds");
        assert_eq!(second, 2, "second init refuses to clobber");
        let text = std::fs::read_to_string(dir.join(".deslop.toml")).expect("exists");
        assert!(
            deslop_core::config::parse_config_str(&text).is_ok(),
            "template parses"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Run the init routine with the process cwd temporarily elsewhere.
    fn run_in(dir: &std::path::Path) -> i32 {
        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir).expect("chdir");
        let code = run();
        std::env::set_current_dir(prev).expect("restore");
        code
    }

    #[test]
    fn template_documents_every_section() {
        // Given the shipped template.

        // When checking its sections.
        let has = [
            "[packs]",
            "[scan]",
            "[output]",
            "[lints]",
            "DESLOP_REQUIRE_ADVICE",
        ];

        // Then each knob area is present.
        for needle in has {
            assert!(TEMPLATE.contains(needle), "template missing {needle}");
        }
    }
}
