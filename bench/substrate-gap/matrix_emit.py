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
import sys
from pathlib import Path

import matrix_vocab as vocab

HERE = Path(__file__).parent
RESULTS_PATH = HERE / "results-latest.json"
COVERAGE_PATH = HERE / "COVERAGE.md"
COHORT_DIR = HERE.parent / "cohort"
sys.path.insert(0, str(COHORT_DIR))
import cells as cohort_cells  # noqa: E402  -- bench/cohort/cells.py, stdlib only

# Schema 3 (FH.10) adds `cohort`: the outlier cohort's grade of each cell beside its fixture
# grade (bench/cohort/cells.py), the chain cells and the cells a cohort miss created.
SCHEMA = 3
# what build_payload reads when no cohort is passed: the newest bench/cohort/results/<r>/
_DEFAULT_COHORT = object()
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


def chains_digest():
    """sha256 over matrix_vocab.CHAINS, 12 hex — the chain rows, in the diff (FH.10)."""
    blob = json.dumps(vocab.CHAINS, sort_keys=True, ensure_ascii=False)
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


def load_cohort():
    """The newest committed cohort release's column over the full vocabulary, or None."""
    return cohort_cells.cohort(languages=vocab.LANGUAGES, mechanisms=vocab.MECHANISM_IDS)


def _fixture_level(cells, lang, mech):
    slot = cells.get((lang, mech))
    if slot is not None:
        return slot["level"]
    return vocab.NA_LEVEL if f"{lang}/{mech}" in vocab.NOT_APPLICABLE else "unknown"


GRADED = ("full", "partial", "none")


def cohort_section(cohort, cells, languages, mechanisms):
    """The payload's `cohort`: each graded cell gains its fixture level, the disagreements
    (both graded, levels differ) and the totals. None when there is no cohort."""
    if cohort is None:
        return None
    selected = {f"{lg}/{m}" for lg in languages for m in mechanisms}
    out_cells = {}
    for label, slot in sorted(cohort["cells"].items()):
        if not slot.get("created") and label not in selected:
            continue
        lang, mech = label.split("/", 1)
        out_cells[label] = {**slot, "fixture": "unknown" if slot.get("created")
                            else _fixture_level(cells, lang, mech)}
    chains = {k: {**v, "fixture": "unknown"} for k, v in sorted(cohort["chains"].items())}
    grid = [(k, v) for k, v in out_cells.items() if not v["created"]]
    disagree = sorted(k for k, v in grid if v["fixture"] in GRADED
                      and v["level"] in GRADED and v["fixture"] != v["level"])
    totals = {level: sum(1 for _, v in grid if v["level"] == level) for level in GRADED}
    totals.update(graded=sum(totals.values()), disagree=len(disagree),
                  unknown_graded=sum(1 for _, v in grid if v["fixture"] == "unknown"
                                     and v["level"] in GRADED),
                  created=len(cohort["created"]),
                  chain_cells=sum(1 for v in chains.values() if v["level"] in GRADED))
    return {"release": cohort["release"], "glia": cohort["glia"],
            "tasks_sha256": cohort["tasks_sha256"], "rule": cohort["rule"],
            "rows": cohort["rows"], "observed": cohort["observed"], "cells": out_cells,
            "chains": chains, "created": list(cohort["created"]),
            "outside_vocab_passing": list(cohort["outside_vocab_passing"]),
            "disagree": disagree, "totals": totals}


def cohort_marker(payload):
    """`[matrix] cells=<grid> fixture_full=.. cohort_graded=.. cohort_full=.. disagree=..
    chain_cells=.. created=..` -- FH.10's fired_on line."""
    t = (payload.get("cohort") or {}).get("totals") or {}
    return (f"[matrix] cells={payload['totals']['grid']} "
            f"fixture_full={payload['totals']['full']} cohort_graded={t.get('graded', 0)} "
            f"cohort_full={t.get('full', 0)} disagree={t.get('disagree', 0)} "
            f"chain_cells={t.get('chain_cells', 0)} created={t.get('created', 0)}")


def build_payload(cells, legacy_only, languages=None, mechanisms=None,
                  engine=None, cohort=_DEFAULT_COHORT):
    """The committed JSON object. Pure: same inputs -> same bytes, always.

    `cohort` is a bench/cohort/cells.cohort() result (None: no cohort column); by default
    the newest committed release is read. The FH.10 marker goes to stderr."""
    if cohort is _DEFAULT_COHORT:
        cohort = load_cohort()
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
    payload = {
        "schema": SCHEMA,
        "engine": engine if engine is not None else engine_version(),
        "vocabulary": {"mechanisms": len(mechanisms), "languages": len(languages),
                       "digest": vocabulary_digest(),
                       "not_applicable": len(vocab.NOT_APPLICABLE),
                       "na_digest": na_digest(), "chains": len(vocab.CHAINS),
                       "chains_digest": chains_digest()},
        "totals": totals,
        "cells": out_cells,
        "not_applicable": {k: not_applicable[k] for k in sorted(not_applicable)},
        "legacy_only": sorted(legacy_only),
        "cohort": cohort_section(cohort, cells, languages, mechanisms),
        "patterns": patterns_section(cells, languages),
    }
    print(cohort_marker(payload), file=sys.stderr)
    return payload


def patterns_section(cells, languages):
    """The use-pattern axis (0.5.5, matrix_vocab.PATTERNS): its own totals and
    cells, never counted in the 30-column `totals`."""
    totals = {level: 0 for level in ("full", "partial", "none", "unknown", "error")}
    out = {}
    for lang in languages:
        for pid in vocab.PATTERN_IDS:
            slot = cells.get((lang, pid))
            totals[slot["level"] if slot is not None else "unknown"] += 1
            if slot is None:
                continue
            out[f"{lang}/{pid}"] = {
                "level": slot["level"], "via": slot.get("via"),
                "extract": list(slot["extract"]), "literal": list(slot["literal"]),
                "route": list(slot["route"]), "forbid": list(slot["forbid"]),
                "fixtures": sorted(slot.get("fixtures", [])),
                "reasons": sorted(slot.get("reasons", [])),
            }
    return {"patterns": list(vocab.PATTERN_IDS), "totals": totals, "cells": out}


def _patterns_markdown(payload, languages):
    sec = payload.get("patterns")
    if not sec:
        return []
    lines = ["## Use patterns", "",
             "The second axis (0.5.5): how code is USED rather than how services talk. Graded "
             "by the same rules as a mechanism, filed under `patterns/<language>/<pattern>/`, "
             "never counted in the totals above (`matrix_vocab.PATTERNS`).", ""]
    for pid in sec["patterns"]:
        p = vocab.mechanism(pid)
        lines.append(f"- `{p['label']}` — **{pid}**: {p['note']}")
    lines.append("")
    lines.append("```")
    lines.append(" " * LANG_W + " ".join(vocab.mechanism(p)["label"].rjust(8)
                                         for p in sec["patterns"]))
    for lang in languages:
        glyphs = []
        for pid in sec["patterns"]:
            slot = sec["cells"].get(f"{lang}/{pid}")
            glyphs.append(GLYPH[slot["level"] if slot else "unknown"])
        lines.append(lang.ljust(LANG_W) + " ".join(g.rjust(8) for g in glyphs))
    lines.append("```")
    lines.append("")
    t = sec["totals"]
    lines.append(f"USE PATTERNS: {len(sec['cells'])}/{len(languages) * len(sec['patterns'])} "
                 f"cells have a fixture — {t['full']} full, {t['partial']} partial, "
                 f"{t['none']} none, {t['unknown']} unknown, {t['error']} error.")
    lines.append("")
    return lines


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


def _cohort_level(payload, lang, mech):
    """A cell's cohort level, `n/a` when no observed cohort row exercises it."""
    slot = ((payload.get("cohort") or {}).get("cells") or {}).get(f"{lang}/{mech}")
    return slot["level"] if slot else vocab.NA_LEVEL


def _pair(payload, lang, mech):
    """The rendered `fixture/cohort` glyph pair."""
    return f"{GLYPH[_level(payload, lang, mech)]}/{GLYPH[_cohort_level(payload, lang, mech)]}"


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
    lines.append("Each cell reads `fixture/cohort`: the fixture grade, then the outlier "
                 "cohort's grade of the same cell (`Cohort grades` below; `-` after the "
                 "slash: no cohort row exercises it). The rollups count the fixture grade.")
    lines.append("")
    lines.append("```")
    lines.append(f"LEGEND {GLYPH['full']} full  {GLYPH['partial']} partial  "
                 f"{GLYPH['none']} none (fixture exists, nothing emitted)  "
                 f"{GLYPH['unknown']} unknown (no fixture)  "
                 f"{GLYPH[vocab.NA_LEVEL]} n/a (see Not applicable)  {GLYPH['error']} error")
    lines.append(" " * LANG_W
                 + " ".join(vocab.mechanism(m)["label"].rjust(COL_W) for m in mechanisms))
    for lang in languages:
        glyphs = (_pair(payload, lang, m) for m in mechanisms)
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
    lines.extend(_patterns_markdown(payload, languages))
    lines.extend(_cohort_markdown(payload))
    return "\n".join(lines)


def _repos(slot):
    return ", ".join(f"{r} {p}/{n}" for r, (p, n) in slot["repos"].items())


def _classes(slot):
    return ", ".join(f"{k} {n}" for k, n in slot["classes"].items()) or "-"


def _cohort_markdown(payload):
    co = payload.get("cohort")
    lines = ["## Cohort grades", ""]
    if not co:
        lines += ["_No cohort results: bench/cohort/results/ holds no release._", ""]
        return lines
    t, rule = co["totals"], co["rule"]
    lines.append(f"Release `{co['release']}` (`{co['glia']}`, tasks digest "
                 f"`{co['tasks_sha256']}`): {co['observed']} of {co['rows']} probe rows observed "
                 "(bench/cohort/cells.py tags each with the cells and chains it exercises). A "
                 f"cell is full when >= {rule['full']:.0%} of its rows passed, partial at >= "
                 f"{rule['partial']:.0%}, none below. {t['graded']} grid cells graded "
                 f"({t['full']} full, {t['partial']} partial, {t['none']} none), "
                 f"{t['unknown_graded']} of them with no fixture; {t['disagree']} disagree with "
                 f"their fixture grade; {t['chain_cells']} chain cells graded; {t['created']} "
                 "cells created by a miss.")
    lines.append("")

    def table(title, intro, rows, empty):
        lines.extend([f"### {title}", "", intro, ""])
        if rows:
            lines.append("| cell | fixture | cohort | passed | repos | missed checks |")
            lines.append("|---|---|---|---|---|---|")
            for label, slot in rows:
                lines.append(f"| `{label}` | {GLYPH[slot['fixture']]} {slot['fixture']} | "
                             f"{GLYPH[slot['level']]} {slot['level']} | "
                             f"{slot['passed']}/{slot['rows']} | {_repos(slot)} | "
                             f"{_classes(slot)} |")
        else:
            lines.append(empty)
        lines.append("")

    cells = co["cells"]
    table("Disagreements", "Both grades exist and differ: the work list (README: reading "
          "a disagreement).", [(k, cells[k]) for k in co["disagree"]], "_None._")
    table("Cells with no fixture the cohort grades", "No fixture claims the cell; real repos "
          "exercise it.", [(k, v) for k, v in cells.items() if not v["created"]
                           and v["fixture"] == "unknown"], "_None._")
    table("Chain cells", "Where mechanisms chain (bench/cohort/cells.py CHAINS); graded by the "
          "cohort, fixture side by generated repos.", list(co["chains"].items()), "_None._")
    table("Cells the cohort created", "A cohort miss on a cell outside the vocabulary "
          "(a language glia has no parser for, a boundary no column models).",
          [(k, cells[k]) for k in co["created"]], "_None._")
    if co["outside_vocab_passing"]:
        lines.append("Outside the vocabulary, only ever passed (no cell created): "
                     + ", ".join(f"`{k}`" for k in co["outside_vocab_passing"]))
        lines.append("")
    return lines


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
    lines.extend(_cohort_drift(committed.get("cohort"), payload.get("cohort")))
    # 0.5.5: use-pattern cells drift the same way, labelled `<lang>/<pattern>`.
    old_p = (committed.get("patterns") or {}).get("cells") or {}
    new_p = (payload.get("patterns") or {}).get("cells") or {}
    for label in sorted(set(old_p) | set(new_p)):
        was = old_p.get(label, {}).get("level", "(absent)")
        now = new_p.get(label, {}).get("level", "(absent)")
        if was != now:
            lines.append(f"{label}: {was} -> {now}")
    return lines


def _cohort_drift(old, new):
    """`cohort <label>: <was> -> <now>` for each cohort or chain cell whose level moved; a
    cell no observed row exercises reads `n/a`."""
    def levels(co):
        co = co if isinstance(co, dict) else {}
        out = {k: v.get("level", vocab.NA_LEVEL) for k, v in (co.get("cells") or {}).items()}
        out.update({k: v.get("level", vocab.NA_LEVEL)
                    for k, v in (co.get("chains") or {}).items()})
        return out

    was, now = levels(old), levels(new)
    return [f"cohort {k}: {was.get(k, vocab.NA_LEVEL)} -> {now.get(k, vocab.NA_LEVEL)}"
            for k in sorted(set(was) | set(now))
            if was.get(k, vocab.NA_LEVEL) != now.get(k, vocab.NA_LEVEL)]


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
