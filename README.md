# deslop

Slop prose linter.

```sh
$ deslop doc.md                 # lint
$ deslop fix --write doc.md     # apply mechanical replacements
$ deslop rules                  # list the effective merged ruleset
$ deslop init                   # write an annotated .deslop.toml
```

---

## Rule packs

```
rules/
├── aatell.toml          # github.com/MikkoParkkola/anti-ai-tell
├── slop.toml            # github.com/walidboulanouar/anti-ai-slop
├── wsc.toml             # github.com/theserverlessdev/wsc
├── aisigns.toml         # this project (WP:AISIGNS artifacts + metrics)
└── cluster-terms.toml   # this project (single-word cluster watch list)
```

Where packs live (first hit wins):

1. `--rules-dir DIR` (CLI flag — DIR itself is the pack directory; skips
   everything below, for CI and tests)
2. `~/.config/deslop/rules/` (user packs — drop TOML files here)
3. `./rules/` (repo development layout)
4. `<exe_dir>/rules/` (installed layout)

---

## Creating rules

Every rule lives in a `[[group]]` table. The group carries the shared settings; `[[group.entries]]` blocks carry the individual triggers. A file may contain any number of groups. The schema rejects unknown fields. A typo'd key refuses the pack at load rather than being silently ignored.

### Group fields (shared by all kinds)

```toml
[[group]]
# required — ID prefix; findings and [lints] keys are '<id-base>#<slug>'.
# Must be unique across ALL loaded files (two groups with the same id-base
# refuse to load together).
id-base = 'AATELL'

# required — vocab | pattern | literal-ban | metric
kind = 'vocab'

# required — 1 = artifact -> error
#            2 = tell      -> warning
#            3 = density   -> hint
# Tier 1/2 findings make the run exit 1.
tier = 2

# required — free-form label shown by `deslop rules`
# (entries may override per finding).
category = 'ai-vocabulary'

# optional — finding headline template.
# Placeholders depend on kind (see below).
message = '…{match}…'

# optional — fix guidance shown under the finding.
advice = '…'

# optional — default true. false ships the group switched off
# (visible in `deslop rules`).
enabled = true

# optional — where the scan looks:
#   'prose'     visible prose outside code fences
#               (default for vocab/pattern)
#   'heading'   heading text only
#   'list-item' list item text only
#   'anywhere'  whole document
#               (default for literal-ban)
scope = 'prose'

[group.url]
# optional — reference rendered with findings.
text = 'WP:AISIGNS'
href = 'https://en.wikipedia.org/wiki/Wikipedia:Signs_of_AI_writing'

[group.fixtures]
# optional but expected — self-test samples. Every enabled entry MUST hit
# every must_match sample and miss every must_not_match sample, or the
# pack REFUSES TO LOAD. A rule that matches nothing is not a rule.
must_match = ['...']
must_not_match = ['...']
```

### Placeholders in `message` / `advice`

| kind                   | allowed placeholders                               |
| ---------------------- | -------------------------------------------------- |
| `vocab`, `literal-ban` | `{match}` — the exact matched text                 |
| `pattern`              | any named capture from the regex, e.g. `{payload}` |
| `metric`               | `{value}`, `{per_words}`                           |

Literal braces are doubled: `{{` renders as `{`. Placeholder typos refuse
the pack at load time.

---

### kind = "vocab"

Word- and phrase-level tells, matched on word boundaries in prose.
Case-insensitive.

```toml
[[group.entries]]
# required — ID suffix, unique within the FILE.
# Content-derived: 'leverage', not 'fix-14'.
slug = 'leverage'

# optional — overrides group advice.
advice = 'Replace "{match}" with "use"'

# required — words or phrases. One entry per concept; list all spellings
# here, do not create near-duplicate entries.
terms = ['leverage']

# optional — default false. Expands inflections mechanically
# (leverage -> leverages, leveraged, leveraging): one entry, one ID,
# all forms caught. Use for single words.
stems = true

# optional — default true. false = substring match
# (rarely what you want for words).
word_boundary = true

# optional — enables `deslop fix` to rewrite hits mechanically. Requires
# the entry to have EXACTLY ONE term so inflected forms rewrite
# consistently.
replacement = 'use'
```

### kind = "literal-ban"

Exact substrings that must never appear — chatbot markup, leaked URLs,
rendered citations. Case-insensitive; matched anywhere in the document
including headings and list items (default `scope = 'anywhere'`, vs `prose`
for other kinds). Like every kind, it never matches inside code spans,
fences, or link targets — those regions are masked out before scanning.

```toml
[[group.entries]]
slug = 'gemini-cite'
advice = 'Replace the rendered citation scaffold with a real reference'

# required — literal markers.
#   {N} = wildcard for a run of digits
#   {{ and }} escape literal braces
terms = [
  '[cite: {N}, {N}, {N}]',
  'utm_source=chatgpt.com',
]
```

### kind = "pattern"

Regex-anchored sentence constructions. Syntax is the Rust `regex` crate and matching is case-insensitive.

```toml
[[group]]
id-base = 'WSC-PAT-AUDIENCE-HEDGE'
kind = 'pattern'
tier = 2
category = 'audience-hedge'

[[group.entries]]
slug = 'main'

# required. Name your interesting parts: (?P<name>...) — they become
# {name} placeholders in message/advice.
regex = "(?P<hedge>\\bwhether you[''’]re ...\\bor\\b)"

advice = '"{hedge}" flattens the audience; address THIS reader'

# Test cases. Rule will not be applied if these fail.
[group.fixtures]
must_match = ["Whether you're a beginner or an expert, this guide helps."]
must_not_match = ['She could not decide whether the coat or the jacket suited the weather.']
```

### kind = "metric"

Document-level statistical signals. The formula lives in Rust
(`crates/core/src/scanner/metrics/<stat>.rs`); TOML picks which stat to
evaluate and where the threshold sits. Metric groups have NO `[[entries]]` —
everything is group-level.

```toml
[[group]]
id-base = 'AISIGNS-METRIC-BOLD-DENSITY'
kind = 'metric'
tier = 3
category = 'document-signals'
message = 'Bold spans at {value} per {per_words} words'
advice = 'Bold only genuinely pivotal terms'

# required — one of the closed registry:
#   em_dash_rate                  em dashes / 1000 words
#   curly_double_ratio            curly share of " quotes
#   bold_density                  **bold** spans / 100 words
#   heading_titlecase_fraction    Title Case heading share
#   emoji_decoration_count        emoji in headings/bullets
#   bullet_boldlead_fraction      bullets opening with bold
#   tricolon_max_streak           longest "x, y, and z" run
#   sent_len_cv                   sentence-length variation
#   opening_ngram_repeat          repeated sentence openers
#   term_cluster_max              distinct watch words in one window
stat = 'bold_density'

# required — fire when the value EXCEEDS this
# (strictly greater; equal does not fire).
threshold-gt = 3.0

# optional — default 1000. Scale factor for {value} in the message.
per-words = 100
```

Short docs are exempt by design — density stats stay silent below floor
(e.g. 250 words for rate metrics, 6 sentences for `sent_len_cv`) so a
two-line file can't trip them.

`term_cluster_max` is the cluster metric; it takes two extra fields and is
the ONLY sanctioned way to lint single common words — never as individual
vocab entries:

```toml
[[group]]
id-base = 'CLUSTER'
kind = 'metric'
tier = 3
stat = 'term_cluster_max'

# fires at 5+ distinct watch words...
threshold-gt = 4

# ...within one window:
#   'paragraph' (default) blank-line separated
#   'sentence'            one sentence
#   'document'            whole text pools
window = 'paragraph'

# required for term_cluster_max — the watch list. Inflections are
# auto-expanded and count as ONE distinct word (lemma identity):
# delve + delves = 1.
terms = [
  'crucial', 'robust',
  'notably',
]
```

---

## Silencing rules

```toml
[lints]
AATELL = "allow"                 # whole group off
WSC-PAT-AUDIENCE-HEDGE = "allow" # one pattern group off
"SLOP#delve-into" = "allow"      # one entry off (quote keys containing #)
```

Because vocab terms deduplicate to a single owner, allowing the owner (`AATELL`) fully silences the word — there is no shadow copy in another pack. Entry keys match the full ID exactly (`SLOP#delve-into`, not `SLOP#delve`): run `deslop rules` to see the IDs you can silence.

## Testing a pack

```console
$ deslop rules                  # lists entries or shows load errors
$ deslop doc.md                 # fixtures re-verify on every load
```

If `must_match`/`must_not_match` samples fail, the pack refuses to load with a named entry and sample.

---

## WASM plugins

Packs express what a TOML rule can say. When a check needs logic a pack can't express — a custom document metric, stateful iteration over structure, anything programmatic — write a plugin: a Rust struct plus one macro call, compiled to `.wasm` and declared in `.deslop.toml`.

**Pack or plugin?** If a `vocab`/`pattern`/`literal-ban`/`metric` entry can express it, write a pack (less machinery, config-only tuning). Plugins are for logic packs can't reach.

### The whole plugin

```rust,ignore
// plugins/example-exclaim/src/lib.rs — the reference plugin, unabridged
use deslop_plugin_sdk::{export, Doc, Finding, Plugin};

#[derive(serde::Deserialize, Default)]
struct Params {
    #[serde(default = "default_threshold")]
    threshold_gt: f64,               // report above this many "!"s per 1000 words
}
fn default_threshold() -> f64 { 1.0 }

struct Exclaim;

impl Plugin for Exclaim {
    const ID: &'static str = "EXCLAIM";     // [lints] key, `deslop rules` name
    const TIER: u8 = 3;                     // 1 artifact / 2 tell / 3 density
    const CATEGORY: &'static str = "emphasis";
    type Params = Params;

    fn scan(doc: &Doc, params: &Params) -> Vec<Finding> {
        let bangs = doc.text.bytes().filter(|&b| b == b'!').count();
        let words = doc.text.split_whitespace().count();
        if bangs == 0 || words < 250 { return vec![]; }
        let rate = bangs as f64 / words as f64 * 1000.0;
        if rate <= params.threshold_gt { return vec![]; }
        let at = doc.text.find('!').unwrap();
        vec![Finding::new("exclamania", (at, at + 1),
                format!("exclamation rate {rate:.1} per 1000 words"))
            .with_advice("cut most of these; one reads confident, ten reads shaky")]
    }
}

export!(Exclaim);
```

To document your params, add a `PARAM_DOCS` const to the impl. `deslop plugin install` renders it as commented defaults in the printed config block, and the SDK verifies each `default` literal against your `Params` type's serde defaults at build time — a mismatch aborts the module, so the docs can never drift from the code:

```rust,ignore
const PARAM_DOCS: &[ParamDoc] = &[ParamDoc {
    name: "threshold_gt",
    default: "1.0",
    description: "exclamations per 1000 words before findings start",
}];
```

### Build and install

```console
$ rustup target add wasm32-unknown-unknown      # once, developer machine only
$ cargo build -p example-exclaim --target wasm32-unknown-unknown --release
```

Builtin plugins ship inside the deslop binary and install without any
toolchain:

```console
$ deslop plugin list                    # what's available, and what's installed
$ deslop plugin install example-exclaim # install one
$ deslop plugin install-all             # install every builtin at once
```

That writes `~/.local/share/deslop/plugins/example-exclaim.wasm` (the platform data dir) and prints the `[plugin.<id>]` snippet to enable it. `install-all` writes every builtin and prints a single copy-paste block declaring all of them. Installing is inert by itself — the directory is never scanned; a plugin runs only where a config declares it.

```toml
# .deslop.toml
[plugin.exclaim]
wasm = "exclaim.wasm"    # bare name → ~/.local/share/deslop/plugins/exclaim.wasm
threshold_gt = 1.0       # everything except wasm/enabled/runtime is an opaque param

# [plugin.exclaim.runtime]   # host knobs (optional)
# fuel = 100_000_000         # per-call compute budget; default scales with document size
```

The `wasm` path resolves by form (must always end in `.wasm`):

| Form                           | Resolves against                 | Use                          |
| ------------------------------ | -------------------------------- | ---------------------------- |
| `/abs/path.wasm`               | exactly as written               | machine-specific locations   |
| `./rel.wasm`, `../up/rel.wasm` | the `.deslop.toml`'s directory   | repo-committed plugins       |
| `name.wasm`                    | `~/.local/share/deslop/plugins/` | personally installed plugins |

Plugins can also be declared in the user-global config, `~/.config/deslop/deslop.toml` — the same file layout the rules dir uses. Config discovery is: `--config` flag, then a `.deslop.toml` walking up from the working directory, then that user-global file, then defaults. A project config always wins over the user-global one.

Add `enabled = false` to switch a plugin off at load level — the module is never read and it disappears from `deslop rules` (whereas `[lints] ID = "allow"` keeps it loaded and listed but silent during scans).

Plugin findings behave exactly like native findings: they show up in `deslop rules`, can be silenced or re-tiered via `[lints]` (`EXCLAIM = "allow"`, `EXCLAIM = "error"`, or per-slug `"EXCLAIM#exclamania"`), feed the exit code by tier, and render in all formats. They are report-only — `deslop fix` never rewrites text because of a plugin.

A plugin that traps, exceeds its fuel budget, or emits invalid spans is skipped for that document with a warning on stderr; it never changes the exit code or aborts the run.
