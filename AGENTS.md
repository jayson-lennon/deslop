# Style Guide

This document defines the _coding conventions_, _patterns_, and _architecture_ for the `deslop` codebase.

## 1. Overview

`deslop` is a prose linter for AI-generated text. Two properties shape everything in this guide:

- **All rules are external data.** The binary embeds zero compiled-in rules; it is an interpreter for TOML packs loaded at startup. Rule behavior changes happen in `rules/*.toml`, not in Rust.
- **Determinism.** Packs load in sorted order, findings sort deterministically, and all output formats are byte-stable. Golden tests pin this.

The workspace has five crates:

| Crate | Path | Role |
|---|---|---|
| `deslop-core` | `crates/core` | Engine: config, rule loading, markdown-aware scanning, metrics, findings |
| `deslop` | `crates/cli` | Binary: argument surface, dispatch, exit codes, rendering |
| `deslop-plugin-protocol` | `plugins/protocol` | Wire types + ABI version shared by host and guests |
| `deslop-plugin-sdk` | `plugins/sdk` | Guest-side SDK: `Plugin` trait, `export!` macro |
| `example-exclaim` | `plugins/example-exclaim` | Reference plugin |

## 2. Core Patterns

### Error Handling

Use `wherror::Error` with `error_stack::Report` for all fallible operations.

**Colocate errors with their related types.** Never create standalone `error.rs` or `errors.rs` files. `ConfigError` lives in `config.rs`; load errors live in the rule loader. The CLI boundary is the one exception: `crates/cli/src/cmd/mod.rs` defines `CmdError` plus the `fail()` shorthand because command errors are "already reported to stderr" messages, not domain data.

```rust
#[derive(Debug, wherror::Error)]
#[error(debug)]
pub enum ConfigError {
    #[error("config file {path} is not valid UTF-8")]
    NotUtf8 { path: camino::Utf8PathBuf },
    // ...
}
```

**Document errors in functions:**

```rust
/// # Errors
///
/// Returns an error if the module cannot be instantiated or the export
/// is missing.
pub fn scan(&self, input: &PluginInput) -> Result<Vec<PluginFinding>, Report<PluginError>>
```

### External Data Is Validated At Load, Never Mid-Scan

Anything a user can write — TOML packs, config files, regexes, message templates — is checked once, at load, with a pointing diagnostic, and the pack is refused rather than silently ignored or half-loaded:

- Unknown schema fields refuse the pack (a typo'd key is an error, not a default).
- Regexes pass the policy checker (`rule/policy.rs`) before compiling; forbidden constructs get a byte offset, not a panic mid-scan.
- Template placeholders are validated against the allowed set per kind (`rule/template.rs`); unknown `{name}` refuses the pack.
- Fixtures (`[group.fixtures]`) are the pack's unit test: every enabled entry must hit every `must_match` and miss every `must_not_match` sample or the pack refuses to load.

The scan phase may then assume well-formed input. Do not add defensive re-validation to scanners.

### Determinism

- Rule packs are discovered via `sorted_toml_files` — file order is always sorted, never `read_dir` order.
- Findings are sorted deterministically before rendering (position, then identity — see `scanner.rs`).
- All three output formats must be byte-identical run to run for the same input. Golden tests enforce this; if your change alters output bytes, that's a golden regeneration (see Tests), not a silent update.
- Never use `HashMap` iteration order where output or dedup decisions depend on it; use `BTreeMap` / sorted vectors.

### Module System

Follow the layout already in place; don't introduce deeper hierarchies.

- `crates/core/src/` holds one flat module per concern (`doc.rs`, `eol.rs`, `finding.rs`, `config.rs`, `metric_stats.rs`), with submodule directories for the two big concerns: `rule/` (loading: schema, loader, dedup, fixtures, policy, stems, literals, template) and `scanner/` (the four scanners + `regions.rs`, `use_mention.rs`, `metrics/`).
- `metrics/` is **one submodule per stat**. Each exposes a `measure()` that fills its field of `DocStats`. Adding a stat means: a new submodule, a `DocStats` field, one delegate line in `metrics.rs`, and a `Stat` enum variant (the registry is closed — adding one is a deliberate act).
- Plugins live outside `crates/`: `plugins/protocol` (shared types), `plugins/sdk` (guest SDK), `plugins/<name>/` (plugin crates). Builtin plugin modules are committed as `plugins/*.wasm` and embedded with `include_bytes!` — see `crates/core/src/plugin/builtin.rs` for the full checklist.

### Byte Offsets and Text Handling

- All spans are **byte offsets** into the original source. `Span::slice` is char-safe and returns `Option` — internal invariant violations surface as errors, never panics, on multibyte documents.
- NEVER manually split a string using `.chars` or by indexing. Use the `unicode-segmentation` crate.
- CRLF handling goes through `eol::normalize`, which returns the offset remap. Findings and fixes always apply to ORIGINAL bytes; translation happens only at the boundary.

## 3. Architecture

### Pipeline

```
  doc.md (original bytes)
     │
     ▼
  eol::normalize         CRLF → LF, bidirectional offset remap
     │
     ▼
  regions                markdown walk: scopes + length-preserving masking
     │
     ▼
  use_mention            strip quoted-term mentions
     │
     ▼
  scanners               vocab | pattern | literal-ban | metric  (+ plugin pass)
     │
     ▼
  finding assembly       spans translated back to ORIGINAL coordinates
     │
     ▼
  deterministic sort → render (human | github | json) → tier-driven exit code
```

Unidirectional. Scanners never re-read the original file, never mutate config, never talk to each other.

### THE Masking Invariant

`RegionMap.masked.len() == src.len()` — byte for byte. Masked ranges (code fences/spans, link targets) become NULs, newlines inside them preserved. Because lengths match, **masked positions ARE original positions**; no translation is needed between region map and scanners. Every scanner runs over the masked text; nothing may match inside code, URLs, or quoted mentions. Any new scanner must consume `RegionMap`, never raw `src`.

### Rules, IDs, and Tiers

- A pack is one TOML file; the file stem is the pack name (`rules/aatell.toml` → pack `aatell`).
- A lint ID is `<ID-BASE>#<slug>`. `id-base` is unique across ALL loaded packs; `slug` is unique within its file and is **content-derived** (`delve`, not `fix-14`). Numeric suffixes appear only as `-2` collision disambiguators.
- Tiers are ordered by false-positive risk: 1 = artifact → error, 2 = tell → warning, 3 = density → hint. Tier 1/2 findings make the run exit 1.
- Exit contract (`crates/cli/src/main.rs`): `0` clean, `1` findings reported, `2` usage error / failed rule load. A load failure aborts before a single document is scanned.
- `[lints]` overrides accept `GROUP` or `GROUP#slug`; the exact slug wins (`LintSettings::level_for`). Entry keys must match the full ID (`SLOP#delve-into`, not `SLOP#delve`).

### Dedup Is Ownership

Packs are deduplicated at load so every term has exactly one owner:

- **vocab / literal-ban** terms: first pack in configured order to claim a term wins — except tier severity wins over order (tier 1 beats tier 3). Losing copies are dropped with a `dedup:` note on stderr. Consequence: allowing the owner in `[lints]` fully silences the word.
- **pattern** regexes: identical regex strings compile once; every owning rule reports the hit.
- **metrics**: deduplicate on `(stat, window, terms)`; the strictest (smallest) threshold survives.

Single common words are linted ONLY via the cluster metric (`term_cluster_max` in `cluster-terms.toml`), never as individual vocab entries. Metric `terms` are lemma-expanded at load, so inflections count as one distinct word and pack files list base forms only.

### Advice Text

`advice` and `message` follow a problem-plus-resolution model: say what's wrong, then what to do. They never cite papers and never carry TODO markers. Vocab entries that declare `replacement` (exactly one term) are the only machine-fixable rules — `deslop fix` rewrites nothing else, and plugin findings are always report-only.

### The Plugin Seam

The stable seam is the runtime-agnostic `LintPlugin` trait (`crates/core/src/plugin/mod.rs`):

- `wasmi_host::WasmiPlugin` — production (embedded `wasmi` interpreter, fuel budget, memory cap, findings cap).
- `fake::FakePlugin` — in-memory, for tests without a wasm toolchain.
- `builtin.rs` — modules embedded in the binary via `include_bytes!`.

Everything above the trait (scanner integration, CLI) never knows which implementation it is talking to. Plugins run in a bounded sandbox: a plugin that traps, exceeds fuel, over-allocates, or emits invalid spans is skipped for that document with a stderr warning and NEVER changes the exit code. Plugin identity comes from the module's own `plugin_meta()` export and must match the `[plugin.<id>]` config key case-insensitively; the host never interprets plugin params.

New plugin logic goes in the SDK/protocol crates only if the wire protocol actually grows; otherwise write a new plugin crate against `deslop-plugin-sdk` (copy `plugins/example-exclaim`).

## 4. Tests

Important:

- Tests should only verify _observable behavior_
- Testing internal details is an _anti-pattern_.
- If observable behavior cannot be tested, an abstraction is needed — the plugin `FakePlugin` seam is the model to follow.

### One Test, One Behavior

Every test asserts exactly one semantic concept: one `// When` and one `// Then` block. A failing test's name alone must say what broke. Checking multiple fields of the same result is one concept; a state change **and** a command/emission are two tests. Duplicated setup across tests is acceptable — do not merge tests to avoid it.

### BDD-Style (Given/When/Then)

```rust
#[test]
fn slice_returns_exact_excerpt() {
    // Given a document.
    let doc = Doc::from_source("t.md".into(), "héllo world");

    // When slicing a multibyte-spanning range.
    let got = doc.slice(Span::new(0, 6));

    // Then the full "héllo" comes back intact.
    assert_eq!(got, Some("héllo"));
}
```

Name tests so they read as program behavior in the test report: `submit_message_rejected_when_buffer_empty`, `masked_code_is_ignored_by_vocab_scan`.

### Test Placement

| What | Where |
|---|---|
| Unit tests for a module | `#[cfg(test)] mod tests` in the same file |
| Loader/schema/dedup/scanner integration | `crates/core/tests/*.rs` |
| Plugin pass logic (no wasm) | `crates/core/tests/plugin_scan.rs` (uses `FakePlugin`) |
| Plugin host / wasm ABI | `crates/core/tests/plugin_host.rs` (builds `wat` modules) |
| Binary contract, exit codes, output formats | `crates/cli/tests/*.rs` via `assert_cmd` |
| Renderer byte-stability | `crates/cli/tests/goldens.rs` + `tests/fixtures/goldens/` |

**CLI integration tests must be hermetic.** The binary resolves packs from disk (`~/.config/deslop/rules`, `./rules`, ...), so tests must provision a tempdir pack copy via `HermeticRules::provision()` (`crates/cli/tests/common/mod.rs`) and pass `--rules-dir`. A test that silently reads the invoking user's installed packs is broken.

**Golden tests are byte-identical and approval-gated.** Regenerating a `.golden.txt` after an intentional renderer change requires updating the files AND presenting the change for user approval before proceeding. Never regenerate goldens as a side effect of an unrelated change.

### Parameterized Tests with rstest

Use `#[rstest]` when the same assertion logic runs against different inputs; each `#[case]` must test the same property. Edge cases that don't fit an "expected" value get their own BDD-styled test instead.

### Test Documents

Filenames in `tests/fixtures/docs/` are contract — goldens and test IDs reference them. The clean corpus (`tests/fixtures/clean_corpus/`, Project Gutenberg texts) proves linters stay silent on human writing; extend it rather than loosening a rule.

## 5. Documentation

### Module-Level Documentation

Explain purpose and high-level behavior; technical detail only as needed to make the high level understandable. State invariants prominently and name them (see `regions.rs`: "THE invariant").

```rust
//! Regex policy: static analysis that keeps user-supplied patterns safe.
//!
//! The engine is the `regex` crate: linear-time, no backtracking, so the
//! catastrophic-backtracking class of failures cannot occur.
```

### Type Documentation

```rust
/// Severity tier. Ordered by false-positive risk, not by count: Tier 1
/// artifacts are unambiguous; Tier 3 density signals matter only in aggregate.
pub enum Tier { ... }
```

Fallible functions get a `# Errors` doc section (see Error Handling).

## 6. Modification Guide

Locate concerns by convention, not hardcoded paths — use `rg` if unsure.

1. **Add/adjust lint rules** — edit a pack in `rules/*.toml`. Run `deslop rules` (or lint any doc) to re-verify fixtures; a fixture failure refuses the pack, which IS the pack's test. Run `just gallery` to eyeball rendering.
2. **Add a new rule kind** (rare) — schema (`rule/schema.rs`), loader, a scanner module under `scanner/`, `KindTag` in `finding.rs`, render support, then tests at every layer. This is a deliberate design change; discuss first.
3. **Add a metric stat** — new submodule under `scanner/metrics/`, a `DocStats` field, a delegate line in `metrics.rs`, a `Stat` variant. Remember the floor in `DocStats::get` so short docs can't trip it.
4. **Add a builtin plugin** — follow the checklist in `crates/core/src/plugin/builtin.rs` (crate under `plugins/`, build for `wasm32-unknown-unknown`, copy the `.wasm` into `plugins/`, add a `BUILTINS` entry).
5. **Add a CLI surface / output format** — args in `crates/cli/src/main.rs`, subcommand in `crates/cli/src/cmd/`, renderer in `crates/cli/src/render/`. New formats need golden coverage.
6. **Write tests** — BDD style, one behavior per test, hermetic for the CLI. Update `tests/fixtures/docs/` + goldens only with approval when renderer output intentionally changes.
7. **Update `.agents/RECORD.md`** — when a change lands that alters stated behavior (config keys, dedup semantics, exit codes, plugin contract), append the new decision. It is the distilled record agents read first.

## 7. Tooling

The `justfile` is minimal; use `cargo` directly for the rest.

| Role | Command | Description |
|---|---|---|
| `vcs` | `git` | `git status`, `git diff`, `git log`, ... |
| `check` | `cargo check --workspace` | Fast compile check |
| `test` | `cargo test --workspace` | **All tests must pass before committing** |
| `lint` | `cargo clippy --workspace` | Warnings are not negotiable |
| `format` | `cargo fmt` | Apply formatting |
| `commit` | `git add -A && git commit -m '<message>'` | Stage and commit all work |
| `sync-trunk` | `git rebase main` | Run on your working branch to bring in the latest `main` (resolve conflicts, re-run tests, commit). NEVER merge or push your branch onto `main` |
| gallery | `just gallery` | Lint `tests/gallery/gallery.md` with the trigger pack — visual regression check for rendered output |

Wasm plugin development additionally needs `rustup target add wasm32-unknown-unknown` (developer machine only; building `deslop` itself never requires it).

### Plan Directory

Task plans live in `.plans/<task>/` (gitignored) with `plan.md` as the specification. The spec is an immutable reference — annotate it with divergence notes, never rewrite it.

## 8. Misc

- NEVER manually split a string using `.chars` or by indexing. Use the `unicode-segmentation` crate.
- `deslop fix` must stay idempotent, dry-run by default, CRLF-preserving, and overlap-suppressing — the `fix_safety` tests pin these; extend the tests when touching fix logic.
- Regexes in packs use the Rust `regex` crate: no lookaround, no backreferences, no atomic groups; unbounded `*`/`+` are policy-restricted in favor of `{m,n}` bounds or character classes.
- Environment and global config paths (`dirs` crate: config dir, data dir) are resolved during config discovery only, then passed down as values.
- DO NOT USE CODE COMMENTS TO WRITE ABOUT "SPEC DIVERGENCES" OR "DIVERGENCES". Code comments are not the place for planning information. PLANS ARE NOT PERSISTED.
- Prefer `match` over `if` where appropriate. Use `where` clauses for all generics.
