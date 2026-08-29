#!/usr/bin/env python3
"""Manual drift tracker for the clean corpus.

The clean corpus (`tests/fixtures/clean_corpus/`) is the false-positive
baseline: human-written prose the linter must stay quiet about. Rule changes
can quietly push tier counts up over time; this tool makes that drift visible
by recording corpus-wide tier counts into an append-only CSV log and diffing
against the last recorded row. It is deliberately OUTSIDE `cargo test` —
run it by hand via `just corpus-record` / `just corpus-check`.

CSV schema: `timestamp,version,tier1,tier2,tier3`, one row per recording.
Tier numbering follows `Tier` in `crates/core/src/finding.rs`:
1 = artifact/error, 2 = tell/warning, 3 = density/note.

Both subcommands shell out to the release binary and parse STDOUT ONLY
(stderr carries dedup/plugin warnings and is never valid JSON).
"""

from __future__ import annotations

import csv
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_DIR = REPO_ROOT / "tests/fixtures/clean_corpus"
BASELINE_CSV = CORPUS_DIR / "baseline.csv"
CORPUS_ARG = CORPUS_DIR.relative_to(REPO_ROOT).as_posix()
# Pin the whole resolution chain, not just the pack directory: without
# `--config` the binary falls back to the user-global `~/.config/deslop/
# deslop.toml`, whose `builtin` pack list may differ from the repo's
# defaults (e.g. missing a newly added pack) - a recording would then
# reflect the invoking machine rather than this repo. Pack FILES come from
# `./rules` via `--rules-dir`.
CONFIG_ARG = REPO_ROOT / "scripts/corpus-baseline.deslop.toml"
# `--format json` is spelled out because the CLI flag's clap default
# (Human) overrides the config's [output].format whenever absent.
SCAN_ARGS = [
    "--config",
    CONFIG_ARG.relative_to(REPO_ROOT).as_posix(),
    "--rules-dir",
    "./rules",
    "--format",
    "json",
    CORPUS_ARG,
]

# Order matters: it is both the CSV column order and the drift report order.
TIERS = (1, 2, 3)


class ScanError(Exception):
    """Raised when the linter cannot produce a clean JSON scan."""


def scan() -> tuple[list, str]:
    """Lint the corpus and return (findings, version).

    # Errors

    Raises ScanError if the release binary cannot be built or run, or if the
    scan fails (pack load failure exits 2) or emits unparseable JSON. Cargo
    and the child share this process's stderr (no `-q`, no capturing), so
    compile progress streams live and the cause of a failure stays visible;
    only the child's STDOUT is captured, which is what carries the JSON.
    """
    built = subprocess.run(["cargo", "build", "--release"], cwd=REPO_ROOT)
    if built.returncode != 0:
        raise ScanError(f"cargo build --release failed with exit {built.returncode}")

    version = subprocess.run(
        ["cargo", "run", "--release", "--", "--version"],
        cwd=REPO_ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout.split()[1]

    scanned = subprocess.run(
        ["cargo", "run", "--release", "--", *SCAN_ARGS],
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        text=True,
    )
    # Exit contract (crates/cli/src/main.rs): 0 = clean, 1 = findings
    # reported (expected on a corpus), 2 = usage/pack-load failure. Only 2+
    # is an error; recording zeros for a failed load would corrupt the log.
    # The child's stderr (dedup/plugin/load warnings) already streamed live.
    if scanned.returncode not in (0, 1):
        raise ScanError(f"deslop scan failed with exit {scanned.returncode}")

    try:
        findings = json.loads(scanned.stdout)
    except json.JSONDecodeError as exc:
        raise ScanError(f"stdout is not valid JSON: {exc}") from exc
    if not isinstance(findings, list):
        raise ScanError("stdout is not a JSON array of findings")
    return findings, version


def tally(findings: list) -> dict[int, int]:
    """Count findings per tier; unknown tier values are a hard error."""
    counts = {tier: 0 for tier in TIERS}
    for finding in findings:
        tier = finding.get("tier")
        if tier not in counts:
            raise ScanError(f"finding reports unknown tier {tier!r}")
        counts[tier] += 1
    return counts


def last_row() -> list[str] | None:
    """Return the last non-empty CSV data row, or None if the log is absent.

    Ignores a trailing newline at EOF; a header-only file yields None.
    """
    if not BASELINE_CSV.exists():
        return None
    rows = [r for r in BASELINE_CSV.read_text(encoding="utf-8").splitlines() if r.strip()]
    return rows[-1].split(",") if len(rows) > 1 else None


def cmd_record() -> int:
    """Append one row: the corpus's current tier counts and this moment."""
    findings, version = scan()
    counts = tally(findings)
    timestamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    is_new = not BASELINE_CSV.exists()
    with BASELINE_CSV.open("a", newline="", encoding="utf-8") as fh:
        writer = csv.writer(fh, lineterminator="\n")
        if is_new:
            writer.writerow(["timestamp", "version", "tier1", "tier2", "tier3"])
        writer.writerow([timestamp, version] + [str(counts[t]) for t in TIERS])
    print(f"recorded {timestamp} deslop {version}: " + " ".join(f"tier{t}={counts[t]}" for t in TIERS))
    return 0


def cmd_check() -> int:
    """Compare current counts to the last row; silent on match, deltas on drift.

    # Errors

    Exits 1 when counts drift (printing per-tier +/- deltas) and 2 when the
    log is missing or unparseable. Never writes the CSV.
    """
    baseline = last_row()
    if baseline is None:
        print("no baseline recorded yet; run `just corpus-record` first", file=sys.stderr)
        return 2
    try:
        expected = {tier: int(baseline[2 + i]) for i, tier in enumerate(TIERS)}
    except (IndexError, ValueError) as exc:
        print(f"malformed baseline row {baseline!r}: {exc}", file=sys.stderr)
        return 2

    findings, version = scan()
    actual = tally(findings)

    drifted = [t for t in TIERS if actual[t] != expected[t]]
    if not drifted:
        return 0
    for tier in drifted:
        print(f"tier{tier}: {expected[tier]} -> {actual[tier]} ({actual[tier] - expected[tier]:+d})")
    print(
        f"corpus drift detected against baseline (deslop {version}); "
        "review the findings, then `just corpus-record` to accept.",
        file=sys.stderr,
    )
    return 1


def main() -> int:
    commands = {"record": cmd_record, "check": cmd_check}
    if len(sys.argv) != 2 or sys.argv[1] not in commands:
        print(f"usage: {sys.argv[0]} {'|'.join(commands)}", file=sys.stderr)
        return 2
    try:
        return commands[sys.argv[1]]()
    except ScanError as exc:
        print(f"corpus scan failed; nothing was recorded:\n{exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
