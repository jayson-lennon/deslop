# Record

## deslop project

- deslop rule packs are single TOML files in `rules/` (aatell, slop, wsc,
  aisigns, cluster-terms), each containing any number of `[[group]]` tables;
  the file stem is the pack. Group `id-base`s are globally unique across all
  packs; entry slugs are unique within their group.

- A lint ID is `<GROUP>#<slug>`; `[lints]` accepts GROUP or GROUP#slug (slug
  wins). Findings carry a per-entry `category` override when set
  (e.g. the merged AISIGNS group keeps each artifact's original category).

- Slugs are content-derived (term plus optional purpose tag); numeric
  suffixes appear only as `-2` collision disambiguators.

- At load, vocab and literal-ban terms deduplicate to a single owner: the
  most severe tier wins (tier 1 = error is stricter than tier 3; a lower
  tier number beats a higher one), config order breaking same-tier ties.
  Losing terms are dropped, emptied entries/groups removed, and each drop
  emits a `dedup:` line. Identical pattern regex strings compile once and
  fan findings out to every owning rule; metrics deduplicate on
  (stat, window, terms) with the strictest (smallest) threshold surviving.

- Metric `terms` are lemma-expanded at load (`stems` semantics): inflected
  forms count as ONE distinct term in cluster scoring, so pack files list
  base words only.

- Single common words lint only via the cluster metric in
  `cluster-terms.toml` (`term_cluster_max`, paragraph window, fires above 4
  distinct terms), never per-instance.

- Cluster findings anchor at the start of their window (not the last hit
  word): the message carries the window kind and a 12-word preview, the
  evidence lists distinct terms indented under `Clustered terms:`, and the
  excerpt block renders without a caret.

- Metric/vocab `advice` strings follow a problem-plus-resolution model,
  cite no papers, and never carry TODO markers (loader-tested).

- deslop plugins are in-memory WASM modules run by the embedded wasmi
  interpreter, declared as `[plugin.<id>]` tables in `.deslop.toml`
  (required `wasm` path: absolute as-given, `./`/`../` relative to the
  config file, bare names relative to the install dir
  `~/.local/share/deslop/plugins`; `.wasm` extension mandatory); identity
  (id_base, tier, category, ABI version) comes from the module's own
  `plugin_meta()` export and must match the table key case-insensitively,
  remaining keys in the table pass to the plugin verbatim (host never
  interprets them), `[plugin.<id>.runtime]` holds host-owned knobs (fuel,
  defaulting to a size-scaled high limit), and `enabled = false` removes
  the plugin at load level (unlike `[lints]` allow, which is scan-level).

- Plugin findings use the same pipeline as TOML rules (`GROUP#slug` ids,
  `[lints]` overrides, tier-driven exit codes) but are report-only; the
  host calls each plugin once per document and a plugin that traps,
  exhausts fuel, or returns invalid spans is skipped with a stderr
  warning and never changes the exit code.

- Config discovery order is: `--config` flag, then `.deslop.toml` walking up from the working directory, then the user-global `~/.config/deslop/deslop.toml` (mirroring the user rules dir), then defaults. A project config always wins; plugins can be declared in either config, and bare `wasm` names resolve against the user plugin install dir `~/.local/share/deslop/plugins` (the XDG data dir — a convention only, never scanned; `deslop plugin install <builtin>` writes there). A plugin may document its params via the SDK's optional `PARAM_DOCS` const, exported as `plugin_params_schema` and rendered by install commands as commented defaults; the SDK verifies documented defaults against the `Params` type's serde defaults at module build time.
