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
  interpreter, declared under `[plugins]` in `.deslop.toml`; identity
  (id_base, tier, category, ABI version) comes from the module's own
  `plugin_meta()` export, `[plugins.<id>]` params pass to the plugin
  verbatim (host never interprets them), and `[plugins.<id>.runtime]`
  holds host-owned knobs (fuel, defaulting to a size-scaled high limit).

- Plugin findings use the same pipeline as TOML rules (`GROUP#slug` ids,
  `[lints]` overrides, tier-driven exit codes) but are report-only; the
  host calls each plugin once per document and a plugin that traps,
  exhausts fuel, or returns invalid spans is skipped with a stderr
  warning and never changes the exit code.
