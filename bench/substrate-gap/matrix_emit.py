#!/usr/bin/env python3
"""Deterministic, COMMITTED artefacts for the language x mechanism coverage matrix.

`results.jsonl` is machine-local (.gitignore:36 ignores that one file), so the
matrix has so far been invisible to review and to the next session: the only
durable statement of what glia extracts was the hand-made table in
dev-notes/review-2026-09-15-coverage-and-issues.md, which rots silently the
moment a fix lands. This module writes two artefacts that DO commit:

  results-latest.json   the machine record -- every covered cell with its four
                        graded assertions, the totals, and a digest of the
                        vocabulary that scored them.
  COVERAGE.md           the human record -- the same matrix rendered in the
                        review's own glyph table, so `git diff` shows a cell
                        flipping `·` -> `●` in a reviewable one-character change.

DETERMINISM IS THE DESIGN CONSTRAINT, not a nicety. These files are committed,
so any run-to-run wobble would leave them permanently dirty and destroy the
diff-as-signal property that is the entire point. Therefore:

  * NO wall-clock timestamp anywhere. Re-running with no behaviour change must
    produce byte-identical files across two SEPARATE processes (separate
    processes matter: PYTHONHASHSEED is randomised per process, so a leaked set
    or dict iteration order only shows up across processes, never within one).
  * every mapping is dumped with sort_keys=True, every list is sorted or comes
    from an already-sorted source (matrix.discover() globs sorted;
    matrix.aggregate() sorts reasons and orders fixtures by path).
  * json.dumps(..., indent=2, sort_keys=True, ensure_ascii=False) + "\n" is the
    ONE spelling used, so the file also round-trips through an editor unchanged.

`vocabulary.digest` is a sha256 over the MECHANISMS table. Without it a
vocabulary edit -- a changed `kinds` group, a new `category` -- would silently
re-score every cell and the diff would show only the moved glyphs, with no
statement that the ruler itself changed. With it, the ruler is in the diff.
`vocabulary.na_digest` does the same for matrix_vocab.NOT_APPLICABLE (schema 2,
CF.13a), which is kept out of the ruler's digest: listing a cell n/a scores
nothing, so it must not read as a re-scored vocabulary.

  covered = full + partial + none + error = the number of cells with a fixture,
  i.e. exactly len(cells). `error` is counted as covered because a cell whose
  fixture blew up HAS a fixture; dropping it would make `covered` disagree with
  the cell list it sits next to.
  n/a     = the NOT_APPLICABLE cells with no fixture. It is counted apart from
  `unknown`, so `unknown` stays "work left"; the coverage line divides `covered`
  by the applicable cells (grid - n/a). A listed cell WITH a fixture is not n/a:
  matrix.apply_not_applicable has already made it an `error`, so it is covered.

COVERAGE.md is spliced, never overwritten: everything before BEGIN and after END
is hand-maintained prose that regeneration must not eat. A file with no markers
(or no file at all) is created from PREAMBLE.

This module must NOT import matrix.py -- matrix.py imports it, and matrix.py is
normally __main__, so the reverse import would load a second copy of it under a
different module name and re-run collection. The glyph map therefore lives HERE
and matrix.py imports it from here.
"""
import hashlib
import json
from pathlib import Path

import matrix_vocab as vocab

HERE = Path(__file__).parent
RESULTS_PATH = HERE / "results-latest.json"
COVERAGE_PATH = HERE / "COVERAGE.md"

SCHEMA = 2
BEGIN = "<!-- BEGIN generated: matrix.py --emit -->"
END = "<!-- END generated -->"

# The review's glyph set (review:134 LEGEND), plus `?` unknown and `!` error --
# the two verdicts the hand-made table could not express -- and `-` n/a, a cell
# matrix_vocab.NOT_APPLICABLE says the language cannot express (CF.13a).
GLYPH = {"full": "●", "partial": "◐", "none": "·", "unknown": "?",
         vocab.NA_LEVEL: "-", "error": "!"}
LEVELS = ("full", "partial", "none", "unknown", vocab.NA_LEVEL, "error")

# Column geometry, reverse-engineered from the review's own block so a rendered
# header is byte-identical to review:135 and the two tables diff against each
# other: language column left-justified to 11, then each glyph/label right-
# justified in 7 and joined by a single space.
LANG_W = 11
COL_W = 7

PREAMBLE = f"""# Coverage — {len(vocab.LANGUAGES)} languages x {len(vocab.MECHANISM_IDS)} mechanisms

**Generated. Do not hand-edit the block below.** Run

```
python3 bench/substrate-gap/matrix.py --emit
```

before committing ANY change to extraction, resolution or the matrix
vocabulary, and commit the regenerated `COVERAGE.md` + `results-latest.json` in
the same commit. `python3 bench/substrate-gap/matrix.py --check` regenerates
both in memory, prints every cell whose level moved, and exits 1 on drift. It is
a pre-commit discipline, not a CI job: the only workflow in this repo
(.github/workflows/wheels-py.yml) builds wheels.

This file supersedes the hand-made table in
`dev-notes/review-2026-09-15-coverage-and-issues.md` section 3. That table was a
human reading a fixture and writing a glyph; every glyph here is DERIVED from
four assertions grade.py already grades (extract / literal / route / forbid) —
see `matrix.py`'s module docstring for the level rule.

`?` is not a weaker `·`. `·` means a fixture exists for the cell and nothing of
the right kind was emitted — a MEASURED blind spot. `?` means no fixture claims
the cell, so no claim is made in either direction. The review's blanks could not
tell those apart, which is what made them unfalsifiable.

`-` is not a `?` either. `-` means the language or runtime cannot express the
mechanism at all (`matrix_vocab.NOT_APPLICABLE`, listed under "Not applicable"
below with its reason), so the cell is left out of the coverage denominator. The
list is falsifiable: a fixture that claims a `-` cell is a cell error until its
entry is deleted in the same commit.

Prose outside the generated block below is hand-maintained and survives
regeneration. (This paragraph deliberately does not quote the marker strings:
the splice partitions on the FIRST marker it finds, so a literal marker inside
the prose would make the preamble eat itself.)

{BEGIN}
{END}
"""


# ---------------------------------------------------------------------------
# Payload
# ---------------------------------------------------------------------------

def vocabulary_digest():
    """sha256 over the MECHANISMS table, 12 hex — the ruler, in the diff."""
    blob = json.dumps(vocab.MECHANISMS, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()[:12]


def na_digest():
    """sha256 over matrix_vocab.NOT_APPLICABLE, 12 hex — the n/a list, in the diff.

    Its own digest, not part of vocabulary_digest(): listing a cell n/a changes
    no cell's score, so it must not read as a re-scored ruler.
    """
    blob = json.dumps(vocab.NOT_APPLICABLE, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()[:12]


def engine_version():
    """The installed wheel's version, or `unknown` if it will not import.

    Never raises: --check must be able to report drift on a box where the wheel
    is mid-rebuild, and an emit that dies on an import is worse than one that
    records the gap.
    """
    try:
        import glia_py as rg
        return str(rg.version())
    except Exception:  # noqa: BLE001 -- recorded, never fatal
        return "unknown"


def build_payload(cells, legacy_only, languages=None, mechanisms=None,
                  engine=None):
    """The committed JSON object. Pure: same inputs -> same bytes, always."""
    languages = list(languages or vocab.LANGUAGES)
    mechanisms = list(mechanisms or vocab.MECHANISM_IDS)
    grid = len(languages) * len(mechanisms)

    totals = {level: 0 for level in LEVELS}
    not_applicable = {}
    for lang in languages:
        for mech in mechanisms:
            label = f"{lang}/{mech}"
            entry = vocab.NOT_APPLICABLE.get(label)
            if entry is not None:
                not_applicable[label] = {"class": entry[0], "reason": entry[1]}
            # A listed cell WITH a fixture counts at its graded level (an error,
            # once matrix.apply_not_applicable ran), never as n/a.
            slot = cells.get((lang, mech))
            totals[slot["level"] if slot is not None
                   else vocab.NA_LEVEL if entry is not None else "unknown"] += 1

    out_cells = {}
    for (lang, mech), slot in sorted(cells.items()):
        if lang not in languages or mech not in mechanisms:
            continue
        out_cells[f"{lang}/{mech}"] = {
            "level": slot["level"],
            "via": slot.get("via"),
            "extract": list(slot["extract"]), "literal": list(slot["literal"]),
            "route": list(slot["route"]), "forbid": list(slot["forbid"]),
            "fixtures": sorted(slot.get("fixtures", [])),
            "reasons": sorted(slot.get("reasons", [])),
        }

    totals["covered"] = len(out_cells)
    totals["grid"] = grid
    return {
        "schema": SCHEMA,
        "engine": engine if engine is not None else engine_version(),
        "vocabulary": {"mechanisms": len(mechanisms), "languages": len(languages),
                       "digest": vocabulary_digest(),
                       "not_applicable": len(vocab.NOT_APPLICABLE),
                       "na_digest": na_digest()},
        "totals": totals,
        "cells": out_cells,
        "not_applicable": {k: not_applicable[k] for k in sorted(not_applicable)},
        "legacy_only": sorted(legacy_only),
    }


def render_results(payload):
    """The exact bytes of results-latest.json. One spelling, everywhere."""
    return json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


# ---------------------------------------------------------------------------
# Markdown
# ---------------------------------------------------------------------------

def _rollup(tally):
    na = vocab.NA_LEVEL
    return (f"{GLYPH['full']} {tally['full']:<3} {GLYPH['partial']} {tally['partial']:<3} "
            f"{GLYPH['none']} {tally['none']:<3} {GLYPH['unknown']} {tally['unknown']:<3} "
            f"{GLYPH[na]} {tally[na]:<3} {GLYPH['error']} {tally['error']}")


def _level(payload, lang, mech):
    """A cell's level: graded, else `n/a` when listed, else `unknown`."""
    label = f"{lang}/{mech}"
    if label in payload["cells"]:
        return payload["cells"][label]["level"]
    return vocab.NA_LEVEL if label in payload.get("not_applicable", {}) else "unknown"


def _tally(payload, pairs):
    tally = {level: 0 for level in LEVELS}
    for lang, mech in pairs:
        tally[_level(payload, lang, mech)] += 1
    return tally


def _alternative_via(payload):
    """Cells whose extraction fired through a NON-primary path.

    `mqtt` measured as `full via=eventbus` is not the same claim as `full via
    queue` -- it means the broker identity was never in play. This is where such
    a cell gets its honest asterisk instead of hiding behind a `●`.
    """
    rows = []
    for label, slot in sorted(payload["cells"].items()):
        via = slot.get("via")
        if not via:
            continue
        mech = vocab.mechanism(label.split("/", 1)[1])
        primary = mech["via_labels"][0]
        if via != primary:
            rows.append((label, via, primary, slot["level"]))
    return rows


def render_markdown(payload, languages=None, mechanisms=None):
    """The generated block, WITHOUT the BEGIN/END markers."""
    languages = list(languages or vocab.LANGUAGES)
    mechanisms = list(mechanisms or vocab.MECHANISM_IDS)
    lines = []

    lines.append(f"Engine `{payload['engine']}` · vocabulary digest "
                 f"`{payload['vocabulary']['digest']}` · schema {payload['schema']}")
    lines.append("")
    lines.append("Columns, left to right (the review's own abbreviations):")
    lines.append("")
    for mech in mechanisms:
        m = vocab.mechanism(mech)
        lines.append(f"- `{m['label']}` — **{m['id']}** ({m['family']})")
    lines.append("")
    lines.append("```")
    lines.append(f"LEGEND {GLYPH['full']} full  {GLYPH['partial']} partial  "
                 f"{GLYPH['none']} none (fixture exists, nothing emitted)  "
                 f"{GLYPH['unknown']} unknown (no fixture)  "
                 f"{GLYPH[vocab.NA_LEVEL]} n/a (see Not applicable)  {GLYPH['error']} error")
    lines.append(" " * LANG_W
                 + " ".join(vocab.mechanism(m)["label"].rjust(COL_W) for m in mechanisms))
    for lang in languages:
        glyphs = (GLYPH[_level(payload, lang, m)] for m in mechanisms)
        lines.append(lang.ljust(LANG_W) + " ".join(g.rjust(COL_W) for g in glyphs))
    lines.append("")
    lines.append(f"PER-MECHANISM  across {len(languages)} languages:")
    for mech in mechanisms:
        lines.append(f"  {mech:<12} "
                     + _rollup(_tally(payload, [(lg, mech) for lg in languages])))
    lines.append("")
    lines.append(f"PER-LANGUAGE  across {len(mechanisms)} mechanisms:")
    for lang in languages:
        lines.append(f"  {lang:<12} "
                     + _rollup(_tally(payload, [(lang, m) for m in mechanisms])))
    lines.append("```")
    lines.append("")

    totals = payload["totals"]
    na = totals.get(vocab.NA_LEVEL, 0)
    covered, applicable = totals["covered"], totals["grid"] - na
    pct = (100.0 * covered / applicable) if applicable else 0.0
    lines.append(f"COVERAGE OF THE COVERAGE: {covered}/{applicable} applicable cells have a "
                 f"fixture ({pct:.1f}%) — {totals['full']} full, {totals['partial']} partial, "
                 f"{totals['none']} none, {totals['unknown']} unknown, "
                 f"{totals['error']} error, {na} n/a.")
    lines.append("")
    lines.append(f"`legacy_only` (fixtures with no `cells`, graded by run.py only): "
                 f"{len(payload['legacy_only'])}")
    lines.append("")

    lines.append("## Cells routed via an alternative mechanism")
    lines.append("")
    lines.append("A `●` here does not mean the intended path fired — it means SOME path "
                 "did. These cells resolved through their fallback registry.")
    lines.append("")
    rows = _alternative_via(payload)
    if rows:
        lines.append("| cell | level | via | primary |")
        lines.append("|---|---|---|---|")
        for label, via, primary, level in rows:
            lines.append(f"| `{label}` | {GLYPH[level]} {level} | `{via}` | `{primary}` |")
    else:
        lines.append("_None: every covered cell resolved through its primary path._")
    lines.append("")

    errors = sorted(k for k, s in payload["cells"].items() if s["level"] == "error")
    lines.append("## Cell errors")
    lines.append("")
    if errors:
        for label in errors:
            reasons = "; ".join(payload["cells"][label]["reasons"])
            lines.append(f"- `{label}`: {reasons}")
    else:
        lines.append("_None._")
    lines.append("")

    lines.append("## Not applicable")
    lines.append("")
    lines.append(f"`{GLYPH[vocab.NA_LEVEL]}` cells: the language or runtime cannot express "
                 "the mechanism (`matrix_vocab.NOT_APPLICABLE`, digest "
                 f"`{payload['vocabulary'].get('na_digest', '')}`), so they leave the "
                 "coverage denominator. A fixture that claims one is a cell error: delete "
                 "the entry in the commit that adds it.")
    lines.append("")
    listed = payload.get("not_applicable", {})
    if listed:
        lines.append("| cell | class | reason |")
        lines.append("|---|---|---|")
        for label in sorted(listed):
            lines.append(f"| `{label}` | {listed[label]['class']} | {listed[label]['reason']} |")
    else:
        lines.append("_None in this selection._")
    lines.append("")
    return "\n".join(lines)


def splice(existing, block):
    """Replace the generated region, preserving hand-maintained prose."""
    if existing is None or BEGIN not in existing or END not in existing:
        existing = PREAMBLE
    head, _, rest = existing.partition(BEGIN)
    _, _, tail = rest.partition(END)
    return f"{head}{BEGIN}\n{block}{END}{tail}"


def render_coverage(payload, existing=None, languages=None, mechanisms=None):
    """The exact bytes of COVERAGE.md, given whatever is on disk today."""
    return splice(existing, render_markdown(payload, languages, mechanisms))


# ---------------------------------------------------------------------------
# Write / compare
# ---------------------------------------------------------------------------

def _read(path):
    try:
        return Path(path).read_text(encoding="utf-8")
    except OSError:
        return None


def write_results(payload, path=RESULTS_PATH):
    text = render_results(payload)
    Path(path).write_text(text, encoding="utf-8")
    return text


def write_markdown(payload, path=COVERAGE_PATH, languages=None, mechanisms=None):
    text = render_coverage(payload, _read(path), languages, mechanisms)
    Path(path).write_text(text, encoding="utf-8")
    return text


def level_drift(committed, payload):
    """Per-cell level changes, `<lang>/<mech>: <committed> -> <measured>`.

    A cell that gained or lost its fixture reads `(absent)` on that side, so an
    emit someone forgot to commit is as loud as a regression. A cell entering
    or leaving matrix_vocab.NOT_APPLICABLE is drift too: `unknown -> n/a` /
    `n/a -> unknown`, or `n/a -> <level>` when its entry went in the commit
    that added its fixture. A schema-1 file carries no list, so every listed
    cell reads `unknown -> n/a` against it.
    """
    committed = committed if isinstance(committed, dict) else {}
    old, new = committed.get("cells") or {}, payload["cells"]
    old_na = committed.get("not_applicable") or {}
    new_na = payload.get("not_applicable") or {}

    def state(cells, na, label):
        if label in cells:
            return cells[label].get("level", "(absent)")
        return vocab.NA_LEVEL if label in na else "(absent)"

    lines = []
    for label in sorted(set(old) | set(new) | set(old_na) | set(new_na)):
        was, now = state(old, old_na, label), state(new, new_na, label)
        if was == now:
            continue
        if vocab.NA_LEVEL in (was, now):
            # an unlisted cell with no fixture is `unknown` in the n/a vocabulary
            was, now = (("unknown" if s == "(absent)" else s) for s in (was, now))
        lines.append(f"{label}: {was} -> {now}")
    return lines


def check(payload, results_path=RESULTS_PATH, coverage_path=COVERAGE_PATH,
          languages=None, mechanisms=None):
    """(drift_lines, n_cells_changed). Empty list == committed bytes are current."""
    raw = _read(results_path)
    try:
        committed = json.loads(raw) if raw is not None else None
    except json.JSONDecodeError as exc:
        return ([f"{Path(results_path).name}: not valid JSON ({exc})"], 0)

    cell_lines = level_drift(committed, payload)
    if cell_lines:
        # Level changes SUBSUME the byte diff they caused: reporting both would
        # bury the one line a reader needs under boilerplate.
        return (cell_lines, len(cell_lines))

    lines = []
    if raw != render_results(payload):
        lines.append(f"{Path(results_path).name}: bytes differ (no cell level changed)")
    if _read(coverage_path) != render_coverage(payload, _read(coverage_path),
                                               languages, mechanisms):
        lines.append(f"{Path(coverage_path).name}: generated block differs")
    return (lines, 0)
