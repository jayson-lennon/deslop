# deslop

A prose linter for AI-generated text. It scans Markdown documents for
chatbot artifacts (leaked referral URLs, `[cite: 3]` scaffolds), AI
vocabulary ("delve", "leverage"), AI sentence patterns ("Whether you're a
beginner or an expert…"), and document-level statistical signals (bold
density, em-dash rate, vocabulary clustering).

```console
$ deslop doc.md                 # lint (exit 1 if anything at tier 1/2)
$ deslop fix --write doc.md     # apply mechanical replacements
$ deslop rules                  # list the effective merged ruleset
$ deslop init                   # write an annotated .deslop.toml
```

Exit codes: `0` clean, `1` findings reported, `2` usage error / failed rule
load.

---

## Rule packs

A **pack is one TOML file**. The file stem is the pack name; a comment
header credits the source:

```
rules/
├── aatell.toml          # github.com/MikkoParkkola/anti-ai-tell
├── slop.toml            # github.com/walidboulanouar/anti-ai-slop
├── wsc.toml             # github.com/theserverlessdev/wsc
├── aisigns.toml         # this project (WP:AISIGNS artifacts + metrics)
└── cluster-terms.toml   # this project (single-word cluster watch list)
```

Packs are read from disk at startup and **deduplicated during load**:

- **vocab / literal-ban terms** get a single owner. The first pack in
  configured order to claim a term wins; later packs' copies are dropped
  with a `dedup:` note on stderr. Order = `[packs]` in `.deslop.toml`
  (default: `aatell`, `slop`, `wsc`, `aisigns`, `cluster-terms`).
- **pattern regexes** deduplicate by exact string: identical regex strings
  compile once and every owning rule reports the hit.
- **metrics** deduplicate on `(stat, window, terms)` — two groups measuring
  the same thing at the same threshold keep one survivor.

Where packs live (first hit wins):

1. `~/.config/deslop/rules/` (user packs — drop TOML files here)
2. `./rules/` (repo development layout)
3. `<exe_dir>/rules/` (installed layout)

---

## Creating rules

Every rule lives in a `[[group]]` table. The group carries the shared
settings; `[[group.entries]]` blocks carry the individual triggers. A file
may contain any number of groups. The schema rejects unknown fields — a
typo'd key refuses the pack at load rather than being silently ignored.

### Group fields (shared by all kinds)

```toml
[[group]]
id-base = 'AATELL'              # required — ID prefix; findings and [lints] keys
                                #   are '<id-base>#<slug>'. Must be unique across
                                #   ALL loaded files (two groups with the same
                                #   id-base refuse to load together).
kind = 'vocab'                  # required — vocab | pattern | literal-ban | metric
tier = 2                        # required — 1 = artifact -> error
                                #            2 = tell      -> warning
                                #            3 = density   -> hint
                                #   Tier 1/2 findings make the run exit 1.
category = 'ai-vocabulary'      # required — free-form label shown by `deslop rules`
                                #   (entries may override per finding).
message = '…{match}…'           # optional — finding headline template.
                                #   Placeholders depend on kind (see below).
advice = '…'                    # optional — fix guidance shown under the finding.
enabled = true                  # optional — default true. false ships the group
                                #   switched off (visible in `deslop rules`).
scope = 'prose'                 # optional — where the scan looks:
                                #   'prose'     visible prose outside code fences
                                #               (default for vocab/pattern)
                                #   'heading'   heading text only
                                #   'list-item' list item text only
                                #   'anywhere'  whole document
                                #               (default for literal-ban)

[group.url]                     # optional — reference rendered with findings.
text = 'WP:AISIGNS'
href = 'https://en.wikipedia.org/wiki/Wikipedia:Signs_of_AI_writing'

[group.fixtures]                # optional but expected — self-test samples.
must_match = ['...']            #   every enabled entry MUST hit every
must_not_match = ['...']        #   must_match sample and miss every
                                #   must_not_match sample, or the pack
                                #   REFUSES TO LOAD. A rule that matches
                                #   nothing is not a rule.
```

### Placeholders in `message` / `advice`

| kind | allowed placeholders |
|---|---|
| `vocab`, `literal-ban` | `{match}` — the exact matched text |
| `pattern` | any named capture from the regex, e.g. `{payload}` |
| `metric` | `{value}`, `{per_words}` |

Literal braces are doubled: `{{` renders as `{`. Placeholder typos refuse
the pack at load time.

---

### kind = "vocab"

Word- and phrase-level tells, matched on word boundaries in prose.
Case-insensitive.

```toml
[[group.entries]]
slug = 'leverage'               # required — ID suffix, unique within the FILE.
                                #   Content-derived: 'leverage', not 'fix-14'.
advice = 'Replace "{match}" with "use"'   # optional — overrides group advice
terms = ['leverage']            # required — words or phrases. One entry per
                                #   concept; list all spellings here, do not
                                #   create near-duplicate entries.
stems = true                    # optional — default false. Expands inflections
                                #   mechanically (leverage -> leverages,
                                #   leveraged, leveraging): one entry, one ID,
                                #   all forms caught. Use for single words.
word_boundary = true            # optional — default true. false = substring
                                #   match (rarely what you want for words).
replacement = 'use'             # optional — enables `deslop fix` to rewrite
                                #   hits mechanically. Requires the entry to
                                #   have EXACTLY ONE term so inflected forms
                                #   rewrite consistently.
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
terms = [                       # required — literal markers.
  '[cite: {N}, {N}, {N}]',      #   {N} = wildcard for a run of digits
  'utm_source=chatgpt.com',     #   {{ and }} escape literal braces
]
```

### kind = "pattern"

Regex-anchored sentence constructions. Syntax is the Rust `regex` crate
(no lookaround); matching is case-insensitive.

```toml
[[group]]
id-base = 'WSC-PAT-AUDIENCE-HEDGE'
kind = 'pattern'
tier = 2
category = 'audience-hedge'

[[group.entries]]
slug = 'main'
regex = "(?P<hedge>\\bwhether you[''’]re ...\\bor\\b)"   # required.
                                #   Name your interesting parts:
                                #   (?P<name>...) — they become {name}
                                #   placeholders in message/advice.
advice = '"{hedge}" flattens the audience; address THIS reader'

[group.fixtures]                # patterns especially want fixtures —
must_match = ["Whether you're a beginner or an expert, this guide helps."]
must_not_match = ['She could not decide whether the coat or the jacket suited the weather.']
```

`captures = 'echo'` (the default) is what surfaces named captures into
templates; `engine` is accepted for forward compatibility and currently
ignored — the engine is the `regex` crate, full stop.

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
stat = 'bold_density'           # required — one of the closed registry:
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
threshold-gt = 3.0              # required — fire when the value EXCEEDS this
                                #   (strictly greater; equal does not fire).
per-words = 100                 # optional — default 1000. Scale factor for
                                #   {value} in the message.
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
threshold-gt = 4                # fires at 5+ distinct watch words...
window = 'paragraph'            # ...within one window:
                                #   'paragraph' (default) blank-line separated
                                #   'sentence'            one sentence
                                #   'document'            whole text pools
terms = [                       # required for term_cluster_max — the watch
  'crucial', 'robust',          #   list. Inflections are auto-expanded and
  'notably',                    #   count as ONE distinct word (lemma
]                               #   identity): delve + delves = 1.
```

Metric groups skip string fixtures — their input is a whole document, not a
sample string.

---

## Silencing rules

```toml
[lints]
AATELL = "allow"                # whole group off
WSC-PAT-AUDIENCE-HEDGE = "allow"# one pattern group off
AATELL#delve = "allow"          # one entry off
```

Because vocab terms deduplicate to a single owner, allowing the owner
(`AATELL`) fully silences the word — there is no shadow copy in another
pack.

## Testing a pack

```console
$ deslop rules                  # lists entries or shows load errors
$ deslop doc.md                 # fixtures re-verify on every load
```

If `must_match`/`must_not_match` samples fail, the pack refuses to load
with a named entry and sample — that is the pack's unit test.
