gallery:
  cargo r --release -- tests/gallery/gallery.md --rules-dir ./rules --rule-file tests/gallery/_gallery_format_triggers.toml

# Lint the clean corpus for eyeballing false positives (pinned pack set).
corpus:
  # Pinned config keeps this aligned with what the baseline log tracks;
  # unpinned runs would silently use ~/.config/deslop/deslop.toml instead.
  cargo r --release -- --config scripts/corpus-baseline.deslop.toml --rules-dir ./rules tests/fixtures/clean_corpus

# Append one corpus tier-count row to the baseline CSV log.
corpus-record:
  python3 scripts/corpus_track.py record

# Diff current corpus tier counts against the last recorded row.
corpus-check:
  python3 scripts/corpus_track.py check
