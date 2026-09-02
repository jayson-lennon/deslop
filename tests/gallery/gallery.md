# deslop output gallery

One document designed to trip every _output shape_ the linter can render —
severities, spanned vs document-level, help lines, context chains, urls,
fixable rewrites — without aiming to fire every rule.

The formats on display:

1. **Tier-1 error, spanned, with help + url** (literal-ban artifact)
2. **Tier-2 warning, spanned, with help + replacement** (fixable vocab tell)
3. **Tier-2 warning, spanned, no replacement** (report-only vocab)
4. **Tier-2 warning from a pattern group** (regex hit with named-capture message)
5. **Tier-3 note, spanned, metric with signal anchor** (curly-quote ratio at its first `"`)
6. **Tier-3 note, document-level (no caret), with context chain** (cluster, paragraph window)
7. **Tier-3 note, document-level cluster over sentences** (sentence window)
8. **Tier-3 note, document-level metric, bare** (distributional stat: no span, no context)

---

## Section: artifacts (tier 1, literal-ban)

This paragraph hides a copied chatbot URL:
[1](https://en.wikipedia.org/wiki/Signs_of_AI_writing?utm_source=chatgpt.com)
and a placeholder artifact: contentReference[oaicite:2]{index=2}.

## Section: vocab tells (tier 2)

We will leverage the team's synergy to delve into the testament of our
robust framework. The plan is crucial and it is important to note that we
must utilise a holistic approach going forward.

## Section: patterns (tier 2)

It is not merely a preference; it is a discipline.
This is not just a style guide, but a survival manual.

## Section: metric seeds (tier 3)

The “quoted” ratio here is deliberately “heavy”: “curly” quotes appear
“everywhere” in this “paragraph” — “ten” “pairs” “at” “least” “so” “the”
“ratio” “crosses” “its” “twenty-quote” floor and the note anchors at the
very first curly quote, so the caret sits on evidence, not the heading.

Also, one more thing: the em-dashes here — quite a few of them — should
trip the em-dash-rate metric if the density crosses its floor.

## Section: cluster paragraphs

Paragraph window: the team felt also aptly adept aims align across
crucial robust notably deep tapestry efforts, and the whole pitch read
as machine-generated emphasis from here to the end of the paragraph.

Second dense paragraph so you can see two independent cluster notes in
one document: delve crucial robust notably adept aims align tapestry
whilst additionally leveraging comprehensive pivotal landscape.

Sentence window next: crucial robust notably adept. Adept crucial
robust notably aims. Tapestry delve crucial robust notably adept aims.
Each dense sentence in this paragraph reports on its own. Boast
burgeoning camaraderie cutting-edge daunting efficacious claims stack
into one sentence here.

## Section: distributional metrics

Uniform rhythm carries this passage: every sentence holds exactly ten
words, so the document settles into one even, synthetic cadence. The
linter reads the flat distribution and notes the monotone sentence pulse.
Steady ten word lines land like ticks of a patient clock. Nothing here
swells or shrinks, so the variation statistic sinks below its cutoff.
Each sentence opens with the same two words in this closing run. Each
sentence keeps the repeated-opener streak alive for the scanner. Each
sentence completes the third consecutive hit, so the bare document-level
render shape stays on display.

## Section: exclaim-example

It's awesome! It's spectacular! It's stupendous! It's in a text file! It's an exclamation point!
