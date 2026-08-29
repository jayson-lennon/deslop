# The Record

A curated list of factual, scoped statements asserting the application's **current** state. Authoritative for the present, never the future.

The planner consults this file before proposing a plan. If a feature **contradicts** an entry here, the contradiction is surfaced before the plan proceeds. If a feature **establishes a new high-level fact**, a verbatim entry is proposed for human approval as part of the plan.

## Format Rules

- **Factual.** Assert how things are _now_. Never future intent ("we will...", "should..."). Each entry is the current state of the application.
- **Scoped.** Name what each entry applies to — repo, app, frontend, or a named subsystem. An unscoped fact (e.g. "uses Fossil") is ambiguous: is that the repo, or the app's supported VCS list? Always disambiguate.
- **High-level.** One-liners (a few sentences at most). Capture decisions and facts a planner needs, not implementation minutiae.
- **Single tag.** Each entry carries exactly one subsystem tag as a `(tag)` prefix: `- (tools) The bash tool runs...`. One entry, one tag — this keeps tag usage a meaningful coverage metric (a tag growing large signals over-specification or a tag that should split). If you cannot decide between two tags for an entry, that is a signal to **re-evaluate the entry itself**, not to assign both. Use `(tag)` rather than `[tag]` to avoid colliding with markdown task-list (checkbox) syntax.
- **Singular concept.** Each entry should be a single sentence and only concerned with a single concept. Prefer multiple entries versus combining many things into one.

## Templates

| Pattern     | Form                                                             | Example                                                                                 |
| ----------- | ---------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| State       | `[Scope] currently [does X / is Y].`                             | "The TUI's first screen at startup is the chat screen."                                 |
| Persistence | `[Scope] persists [what] to [where].`                            | "Sessions persist to SQLite."                                                           |
| Flow        | `[Input/event] is handled by [actor/subsystem], which [action].` | "File edits route through the `edit` tool, which requires a unique match or `replace_all`."        |
| Boundary    | `[Scope] is bounded by [constraint].`                            | "Project discovery walks ancestors until a VCS root or `$HOME`, whichever comes first." |

## Absence

A missing record, or an un-recorded area, simply means the list has no entry there yet. Absence is not a constraint — it is an open question, and a feature that fills a gap may establish the first entry for that area (proposed for human approval as part of the plan).

## Editing

Entries are added or amended **only with human approval**.

---

- (config) Config discovery order is: `--config` flag, then `.deslop.toml` walking up from the working directory, then the user-global `~/.config/deslop/deslop.toml` (mirroring the user rules dir), then defaults.
- (config) A project config always wins over the user-global config.
- (config) Plugins can be declared in either the project or the user-global config.
- (lints) A lint ID is `<GROUP>#<slug>`.
- (lints) `[lints]` accepts GROUP or GROUP#slug, with slug winning.
- (lints) Findings carry a per-entry `category` override when set (e.g. the merged AISIGNS group keeps each artifact's original category).
- (metrics) Metric `terms` are lemma-expanded at load (`stems` semantics): inflected forms count as ONE distinct term in cluster scoring, so pack files list base words only.
- (metrics) Single common words lint only via the cluster metric in `cluster-terms.toml` (`term_cluster_max`, paragraph window, fires above 4 distinct terms), never per-instance.
- (metrics) Cluster findings anchor at the start of their window (not the last hit word).
- (metrics) A cluster finding's message carries the window kind and a 12-word preview; its evidence lists distinct terms indented under `Clustered terms:`, and its excerpt block renders without a caret.
- (plugins) deslop plugins are in-memory WASM modules run by the embedded wasmi interpreter.
- (plugins) Plugins are declared as `[plugin.<id>]` tables in `.deslop.toml`; the required `wasm` path is absolute as-given, `./`/`../` relative to the config file, or a bare name relative to the install dir `~/.local/share/deslop/plugins` (the XDG data dir — a convention only, never scanned).
- (plugins) The `.wasm` extension is mandatory on a plugin's `wasm` path.
- (plugins) Plugin identity (id_base, tier, category, ABI version) comes from the module's own `plugin_meta()` export and must match the table key case-insensitively.
- (plugins) Table keys remaining in `[plugin.<id>]` pass to the plugin verbatim; the host never interprets them.
- (plugins) `[plugin.<id>.runtime]` holds host-owned knobs (fuel, defaulting to a size-scaled high limit).
- (plugins) `enabled = false` removes the plugin at load level, unlike `[lints]` allow, which is scan-level.
- (plugins) Plugin findings use the same pipeline as TOML rules (`GROUP#slug` ids, `[lints]` overrides, tier-driven exit codes) but are report-only.
- (plugins) The host calls each plugin once per document; a plugin that traps, exhausts fuel, or returns invalid spans is skipped with a stderr warning and never changes the exit code.
- (plugins) `deslop plugin install <builtin>` writes builtin plugin modules into the user plugin install dir.
- (plugins) A plugin may document its params via the SDK's optional `PARAM_DOCS` const, exported as `plugin_params_schema` and rendered by install commands as commented defaults.
- (plugins) The SDK verifies documented param defaults against the `Params` type's serde defaults at module build time.
- (rules) Rule packs are single TOML files in `rules/` (aatell, slop, wsc, aisigns, cluster-terms, hedging); the file stem is the pack name.
- (rules) The hedging pack lints structural hedge formulas as tier-2 report-only pattern rules; its conclusive-pivot regex is the only pattern that matches across sentence boundaries (bounded character window).
- (rules) A rule pack contains any number of `[[group]]` tables.
- (rules) Group `id-base`s are globally unique across all packs.
- (rules) Entry slugs are unique within their group.
- (rules) Slugs are content-derived (term plus optional purpose tag); numeric suffixes appear only as `-2` collision disambiguators.
- (rules) At load, vocab and literal-ban terms deduplicate to a single owner: the most severe tier wins (a lower tier number beats a higher one), with config order breaking same-tier ties.
- (rules) Dedup drops losing terms, removes emptied entries/groups, and emits a `dedup:` line per drop.
- (rules) Identical pattern regex strings compile once and fan findings out to every owning rule.
- (rules) Metrics deduplicate on (stat, window, terms) with the strictest (smallest) threshold surviving.
- (rules) Metric/vocab `advice` strings follow a problem-plus-resolution model, cite no papers, and never carry TODO markers (loader-tested).
- (render) Human-format source lines are truncated to the terminal width only when stdout is a TTY or `--width` is explicit (`--width 0` disables); piped output stays untruncated.
- (render) Truncation windows anchor on the finding's primary span with `…` on each cut side so caret marks stay visible; anchorless excerpt lines truncate from the line head.
- (tests) The clean corpus (`tests/fixtures/clean_corpus/`) is a manually tracked false-positive baseline: tier hit counts are appended to `baseline.csv` via `just corpus-record` and diffed against the last row via `just corpus-check`, never asserted in cargo test.
- (tests) Each `baseline.csv` row records a timestamp, the deslop crate version, and corpus-wide finding counts per tier.
- (tests) Every clean-corpus file carries provenance (source URL, author, year, license, sha256) in `MANIFEST.toml` alongside the texts.
- (tests) Corpus recordings pin the full resolution chain (`--config scripts/corpus-baseline.deslop.toml --rules-dir ./rules`): `--rules-dir` alone still inherits the user-global config's pack list and plugin tables, which can differ per machine.
- (spans) All byte-offset slicing on source-derived strings in core and CLI goes through boundary-safe helpers in deslop-core; raw `str` range-indexing appears only in tests.
- (render) Human-format rendering floor-clamps byte offsets to char boundaries, so spans whose arithmetic lands mid-character always render instead of panicking.
