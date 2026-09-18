#!/usr/bin/env python3
"""Derive the 16-language x 30-mechanism coverage matrix from graded fixtures.

The review's section-3 table (dev-notes/review-2026-09-15-coverage-and-issues.md:127-143)
is a hand-made snapshot: a human reads a fixture, judges it, writes a glyph. It
goes stale the moment a fix lands. This module replaces the judgement with a
MEASUREMENT: every cell verdict is derived from assertions grade.py already
grades, against the vocabulary matrix_vocab.py already pins down.

The decisive distinction it buys is `none` vs `unknown`:
  `none`     a fixture exists for this cell and NOTHING of the right kind was
             emitted. A measured blind spot.
  `unknown`  no fixture claims this cell. No claim is made, in either direction.
The hand-made table could not tell those apart, so its blanks were unfalsifiable.

FOUR GRADED ASSERTIONS per (fixture, cell), all from ONE grade_fixture() call:

  extract  kind-only, no identity: does a node of the expected KIND exist
           anywhere in the graph? This is the level grade.py's `_node_matches`
           cannot express -- it always matches on identity, so it cannot separate
           "emitted with the wrong name" from "not emitted at all". When the
           fixture scopes no expect_nodes to this cell, the probe falls back to
           the mechanism's own kind groups: is ANY of them present?
           A ROLE mechanism (matrix_vocab `role_cells`, e.g. service) is also
           extracted by a `found` expect_cells row {cell: ROLE, contains: <one
           of role_cells>}: LB.3's fold leaves the declaration with its own kind
           plus a ROLE cell, so after the fold the kind group alone matches only
           standalone overlays. Those rows count beside the kind-scoped rows.
  literal   the identifying literal survived into the node name/qname -- i.e.
           every scoped expect_nodes entry is `found` under grade.py's identity
           matcher, plus every scoped ROLE row (its `node` is matched on the same
           identity). Vacuously true when the fixture scopes no nodes here, which
           is what makes the legacy corpus gradeable at this level with no source
           edits (A15.6 only adds `cells`).
  route     the scoped expect_edges are NON-EMPTY and all `found`. An empty set
           is not-applicable, not a pass: the cell then caps at `partial` and
           records the reason, because a mechanism nobody routed is not proven.
           The one exception is an ANCHOR mechanism (matrix_vocab `anchor: True`,
           e.g. subproject): its node is edge-less by design, so it has no
           routing vocabulary and route is not-applicable BY DEFINITION -- which
           is different from "nobody routed it", so it does not cap the cell.
  forbid    zero violations among the scoped `forbid` entries. This is what caps
           a phantom-emitting cell at `partial`; the review's matrix was
           recall-only and could not express precision at all.

LEVEL:  none    iff not extract
        full    iff extract and literal and route and forbid
                (anchor mechanism: iff extract and literal and forbid)
        partial otherwise
        error   if grade_fixture RAISED. Deliberately NOT swallowed the way
                run.py does -- an unbuildable matrix fixture that quietly read
                `unknown` would make the matrix under-report, which is the exact
                failure this harness exists to prevent.

SCOPING. The frozen key.json vocabulary has no per-assertion cell tag, so an
assertion belongs to a cell when its kind/category is in that mechanism's
vocabulary (matrix_vocab.MECHANISMS[*].kinds / .categories), or, for a ROLE
row, when its `contains` names one of the mechanism's `role_cells`. A fixture
declaring two cells whose mechanisms differ therefore splits cleanly; one declaring two
cells on the SAME mechanism in two languages grades both identically, which is
honest -- one fixture genuinely proves the same thing about both rows.

AGGREGATION is MIN: when several fixtures claim one cell the WORST level wins
(error > none > partial > full). The matrix is a blind-spot map, so a cell is
`full` only when EVERY fixture for it is full; a stretch fixture that drags a
cell down is not noise, it is the finding. The contributing fixture paths are
recorded on every cell so a drag is always traceable back to one fixture.

Discovery covers `fixtures/*/key.json` (the legacy corpus, read-only here) and
`matrix/<language>/<mechanism>/key.json` (the canonical per-cell layout). A
fixture participates iff its key.json carries a non-empty `cells`; the rest are
counted as `legacy_only` in the footer. For a matrix/ fixture the directory
names are cross-checked against the declared `cells` and a mismatch is a hard
error naming both, so a copy-pasted fixture cannot silently score another cell.

run.py is NOT touched by this module. It is the frozen 0-blind-spot regression
gate and its stdout must stay byte-identical.

Usage:
  python3 matrix.py                      # render the matrix + markers
  python3 matrix.py --json               # machine dump to stdout
  python3 matrix.py --cell go/nats       # one cell, in detail
  python3 matrix.py --language python    # filter rows
  python3 matrix.py --mechanism kafka    # filter columns
  python3 matrix.py --fail-on-error      # exit 1 on any error/invalid cell
  python3 matrix.py --emit               # rewrite the COMMITTED artefacts
  python3 matrix.py --check              # exit 1 if those artefacts are stale

--emit / --check write and verify bench/substrate-gap/results-latest.json and
COVERAGE.md (see matrix_emit.py). They always cover the FULL 16x30 grid, so they
refuse to run alongside --cell/--language/--mechanism: a committed artefact
rendered through a filter would silently claim every excluded cell is unknown.
"""
import argparse
import glob
import json
import sys
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import matrix_vocab as vocab  # noqa: E402
import matrix_emit  # noqa: E402  -- the committed-artefact emitter
import grade as _grade  # noqa: E402  -- sets GLIA_NO_PERSIST at import

# Owned by matrix_emit so the terminal render and the committed COVERAGE.md
# cannot drift apart. matrix_emit deliberately does NOT import this module back:
# matrix.py is normally __main__, so the reverse import would load a second copy
# under a different name and re-run collection.
GLYPH = matrix_emit.GLYPH
# increasing badness; MIN aggregation takes the highest rank.
RANK = {"full": 0, "partial": 1, "none": 2, "error": 3}
SEPARATORS = "-_."


# --------------------------------------------------------------------------
# Discovery
# --------------------------------------------------------------------------

def discover(root=HERE):
    """Every key.json, legacy corpus first then the canonical matrix tree."""
    root = Path(root)
    legacy = sorted(glob.glob(str(root / "fixtures" / "*" / "key.json")))
    canonical = sorted(glob.glob(str(root / "matrix" / "*" / "*" / "key.json")))
    return [Path(p) for p in legacy + canonical]


def mechanism_from_dir(segment):
    """`kafka` -> kafka; `http_server_composed` -> http_server (variant suffix).

    A second fixture for one cell needs a second directory, so a trailing
    `-`/`_`/`.` variant is allowed after the mechanism id. Longest id wins, so
    `sqs_sns` is never read as `sqs` + `_sns`.
    """
    seg = str(segment)
    if seg in vocab.MECHANISM_IDS:
        return seg
    best = None
    for mid in vocab.MECHANISM_IDS:
        if (seg.startswith(mid) and len(seg) > len(mid) and seg[len(mid)] in SEPARATORS
                and (best is None or len(mid) > len(best))):
            best = mid
    if best is None:
        raise ValueError(
            f"matrix directory {seg!r} does not name a mechanism; the "
            f"{len(vocab.MECHANISM_IDS)} columns are: {', '.join(vocab.MECHANISM_IDS)}"
        )
    return best


def parse_cell(label):
    """'<language>/<mechanism>' -> (language, mechanism_id). Raises on either."""
    lang, sep, mech = str(label).partition("/")
    if not sep:
        raise ValueError(f"cell {label!r} is not '<language>/<mechanism>'")
    return vocab.normalize_language(lang), vocab.mechanism(mech)["id"]


def declared_cells(key_path, root=HERE):
    """The cells this fixture claims, cross-checked against its directory."""
    key = json.loads(Path(key_path).read_text())
    cells = [parse_cell(c) for c in (key.get("cells") or [])]
    rel = Path(key_path).parent.resolve().relative_to(Path(root).resolve())
    if rel.parts and rel.parts[0] == "matrix" and len(rel.parts) == 3:
        want = (vocab.normalize_language(rel.parts[1]), mechanism_from_dir(rel.parts[2]))
        if want not in cells:
            raise ValueError(
                f"{rel}: directory declares cell {want[0]}/{want[1]} but key.json "
                f"`cells` says {[f'{a}/{b}' for a, b in cells] or '[]'} -- a fixture "
                f"must not score a cell it is not filed under"
            )
    return key, cells


# --------------------------------------------------------------------------
# Grading
# --------------------------------------------------------------------------

def grade_with_kinds(fixture_dir, grader=_grade):
    """grade_fixture() plus the set of node KINDS present in the built graph.

    grade.py returns recall rows but not the graph, and kind-presence is exactly
    the signal its identity matcher cannot express. Rather than build the graph
    a second time (or edit grade.py, which is another packet's file), the one
    build grade_fixture already does is observed in passing.
    """
    seen = {}
    original = grader.build_graph

    def spy(*args, **kwargs):
        out = original(*args, **kwargs)
        if isinstance(out, tuple) and len(out) >= 2:
            seen["nodes"] = out[1]
        return out

    grader.build_graph = spy
    try:
        res = grader.grade_fixture(fixture_dir)
    finally:
        grader.build_graph = original
    kinds = {grader._KIND_BY_ID.get(n.get("kind"), str(n.get("kind")))
             for n in seen.get("nodes", [])}
    return res, kinds


def derive(mech_id, res, present_kinds):
    """The four assertions -> one level, for ONE (fixture, cell) pair."""
    m = vocab.mechanism(mech_id)
    mech_kinds = {k for group in m["kinds"] for k in group}
    mech_cats = set(m["categories"])

    node_rows = [r for r in res.get("node_recall", []) if r.get("kind") in mech_kinds]
    edge_rows = [r for r in res.get("edge_recall", []) if r.get("category") in mech_cats]
    forbid_rows = [r for r in res.get("forbid_results", [])
                   if r.get("category") in mech_cats or r.get("kind") in mech_kinds]
    # A role mechanism (matrix_vocab `role_cells`) is also proven by a ROLE cell
    # on the declaration LB.3 folded the overlay into. Empty for every other
    # mechanism, which therefore derives exactly as before.
    roles = {r.casefold(): r for r in m.get("role_cells", [])}
    role_rows = [r for r in res.get("cell_recall", [])
                 if roles and r.get("cell") == vocab.ROLE_CELL
                 and (r.get("contains") or "").casefold() in roles]

    reasons = []
    if node_rows or role_rows:
        extract_n = (sum(1 for r in node_rows if r["kind"] in present_kinds)
                     + sum(1 for r in role_rows if r["found"]))
        extract_d = len(node_rows) + len(role_rows)
    else:
        # No node claim scoped here: probe the mechanism's own kind groups.
        extract_n, extract_d = (1 if present_kinds & mech_kinds else 0), 1
        reasons.append("extract probed from mechanism kinds (fixture scopes no nodes)")
    if role_rows:
        reasons.append(f"extract read from {vocab.ROLE_CELL} cells naming "
                       f"{', '.join(sorted(roles.values()))}")
    # A found ROLE row matched its `node` on identity, so it proves the literal
    # too; it also counts as the role kind being present when naming `via`.
    literal_n = sum(1 for r in node_rows if r["found"]) + sum(1 for r in role_rows if r["found"])
    literal_d = len(node_rows) + len(role_rows)
    played = {roles[r["contains"].casefold()] for r in role_rows if r["found"]}
    route_n, route_d = sum(1 for r in edge_rows if r["found"]), len(edge_rows)
    forbid_n, forbid_d = sum(1 for r in forbid_rows if not r["violated"]), len(forbid_rows)

    extract_ok = extract_n == extract_d
    literal_ok = literal_n == literal_d          # vacuously true when nothing scoped
    # Opt-in per mechanism: an ANCHOR mechanism has no routing vocabulary, so
    # route is not-applicable rather than a cap. Every other mechanism keeps the
    # rule that an unrouted cell is not proven.
    anchor = m.get("anchor", False)
    route_ok = True if anchor else (route_d > 0 and route_n == route_d)
    forbid_ok = forbid_n == forbid_d

    if anchor:
        reasons.append("anchor mechanism: routing not applicable")
    elif route_d == 0:
        reasons.append("no routing expected")
    if not extract_ok:
        level = "none"
        if extract_n:
            # `none` is an ALL-OF verdict, so a half-present kind set still reads
            # `none`. Say so out loud: `via` will name a path that DID fire, and a
            # reader must not have to reconcile that against a bare glyph.
            reasons.append(f"extract partial: {extract_n}/{extract_d} expected "
                           f"kinds present; `none` is the all-of rule, not silence")
    elif literal_ok and route_ok and forbid_ok:
        level = "full"
    else:
        level = "partial"
        if not literal_ok:
            reasons.append("literal lost")
        if route_d and not route_ok:
            reasons.append("routing edge missing")
        if not forbid_ok:
            reasons.append("forbid violated")

    return {
        "level": level,
        "via": vocab.resolve_via(mech_id, set(present_kinds) | played),
        "extract": [extract_n, extract_d], "literal": [literal_n, literal_d],
        "route": [route_n, route_d], "forbid": [forbid_n, forbid_d],
        "reasons": reasons,
    }


def collect(root=HERE, grader=_grade):
    """Grade every participating fixture -> per-(cell, fixture) records."""
    records, bad, legacy_only = [], [], []
    for key_path in discover(root):
        rel = str(Path(key_path).parent.resolve().relative_to(Path(root).resolve()))
        try:
            key, cells = declared_cells(key_path, root)
        except (ValueError, OSError, json.JSONDecodeError) as exc:
            bad.append((rel, f"{type(exc).__name__}: {exc}"))
            continue
        if not cells:
            legacy_only.append(rel)
            continue
        try:
            res, kinds = grade_with_kinds(Path(key_path).parent, grader)
        except Exception as exc:  # noqa: BLE001 -- recorded as `error`, never swallowed
            for cell in cells:
                records.append({"cell": cell, "fixture": rel, "level": "error",
                                "via": None, "extract": [0, 0], "literal": [0, 0],
                                "route": [0, 0], "forbid": [0, 0],
                                "reasons": [f"{type(exc).__name__}: {exc}"]})
            continue
        for lang, mech in cells:
            rec = derive(mech, res, kinds)
            rec.update({"cell": (lang, mech), "fixture": rel})
            records.append(rec)
    return records, bad, legacy_only


def aggregate(records):
    """MIN across fixtures: the worst level wins. See the module docstring."""
    cells = {}
    for rec in sorted(records, key=lambda r: (r["cell"], r["fixture"])):
        slot = cells.setdefault(rec["cell"], {"fixtures": [], "contributions": []})
        slot["fixtures"].append(rec["fixture"])
        slot["contributions"].append(rec)
    for slot in cells.values():
        # max() returns the FIRST maximum, so ties go to the earliest fixture path.
        worst = max(slot["contributions"], key=lambda c: RANK[c["level"]])
        slot["level"] = worst["level"]
        slot["via"] = worst["via"] or next(
            (c["via"] for c in slot["contributions"] if c["via"]), None)
        for field in ("extract", "literal", "route", "forbid"):
            slot[field] = [sum(c[field][0] for c in slot["contributions"]),
                           sum(c[field][1] for c in slot["contributions"])]
        slot["reasons"] = sorted({r for c in slot["contributions"] for r in c["reasons"]})
    return cells


# --------------------------------------------------------------------------
# Render
# --------------------------------------------------------------------------

def _counts(cells, pairs):
    tally = {"full": 0, "partial": 0, "none": 0, "unknown": 0, "error": 0}
    for pair in pairs:
        tally[cells.get(pair, {}).get("level", "unknown")] += 1
    return tally


def _rollup(tally):
    return (f"{GLYPH['full']} {tally['full']:<3} {GLYPH['partial']} {tally['partial']:<3} "
            f"{GLYPH['none']} {tally['none']:<3} {GLYPH['unknown']} {tally['unknown']:<3} "
            f"{GLYPH['error']} {tally['error']}")


def render(cells, languages, mechanisms, bad, legacy_only, out=None):
    # Resolved per call, NOT as a default argument: a default would bind the
    # real stdout at import time and ignore any later redirection.
    out = sys.stdout if out is None else out
    labels = [vocab.mechanism(m)["label"] for m in mechanisms]
    width = max([len(lbl) for lbl in labels] + [1])
    pad = max([len(lang) for lang in languages] + [9])
    for i in range(width):
        print(" " * pad + "  " + " ".join(lbl[i] if i < len(lbl) else " " for lbl in labels),
              file=out)
    print("-" * (pad + 2 + 2 * len(mechanisms) - 1), file=out)
    for lang in languages:
        row = " ".join(GLYPH[cells.get((lang, m), {}).get("level", "unknown")]
                       for m in mechanisms)
        print(lang.ljust(pad) + "  " + row, file=out)
    print("-" * (pad + 2 + 2 * len(mechanisms) - 1), file=out)

    print("\nPER MECHANISM:", file=out)
    for m in mechanisms:
        print(f"  {m:<12} " + _rollup(_counts(cells, [(lg, m) for lg in languages])), file=out)
    print("\nPER LANGUAGE:", file=out)
    for lang in languages:
        print(f"  {lang:<12} " + _rollup(_counts(cells, [(lang, m) for m in mechanisms])),
              file=out)

    total = len(languages) * len(mechanisms)
    have = sum(1 for lang in languages for m in mechanisms if (lang, m) in cells)
    pct = (100.0 * have / total) if total else 0.0
    print(f"\nCOVERAGE OF THE COVERAGE: {have}/{total} cells have a fixture ({pct:.1f}%)",
          file=out)
    print(f"legacy_only (no `cells`, graded by run.py only): {len(legacy_only)}", file=out)
    print(f"\nINVALID CELL DECLARATIONS: {len(bad)}", file=out)
    for rel, msg in bad:
        # vocab spells out all 30 ids on a miss; that belongs in --json, not 9x here.
        print(f"  - {rel}: {msg.split('; ')[0]}", file=out)
    errors = sorted(p for p, s in cells.items() if s["level"] == "error")
    print(f"\nCELL ERRORS: {len(errors)}", file=out)
    for lang, m in errors:
        print(f"  - {lang}/{m}: {'; '.join(cells[(lang, m)]['reasons'])}", file=out)


def cell_marker(pair, slot=None):
    """The per-cell fired_on marker, the grep handle every corpus packet uses.

    A cell with NO fixture still gets one (`= unknown`, empty fixtures), so a
    packet can grep for its own cell before its fixture lands and see the
    measurement move, rather than grepping for silence.
    """
    lang, mech = pair
    slot = slot or {"level": "unknown", "via": None, "fixtures": [],
                    "extract": [0, 0], "literal": [0, 0],
                    "route": [0, 0], "forbid": [0, 0]}
    return (f"[matrix] cell {lang}/{mech} = {slot['level']} "
            f"(extract={slot['extract'][0]}/{slot['extract'][1]} "
            f"literal={slot['literal'][0]}/{slot['literal'][1]} "
            f"route={slot['route'][0]}/{slot['route'][1]} "
            f"forbid={slot['forbid'][0]}/{slot['forbid'][1]}) "
            f"via={slot['via']} fixtures={','.join(slot['fixtures'])}")


def select(args):
    """Resolve the row/column filters. Raises ValueError naming the vocabulary."""
    languages, mechanisms = list(vocab.LANGUAGES), list(vocab.MECHANISM_IDS)
    if args.cell:
        lang, mech = parse_cell(args.cell)
        languages, mechanisms = [lang], [mech]
    if args.language:
        languages = [vocab.normalize_language(args.language)]
    if args.mechanism:
        mechanisms = [vocab.mechanism(args.mechanism)["id"]]
    return languages, mechanisms


def main(argv=None):
    ap = argparse.ArgumentParser(description="derive the coverage matrix")
    ap.add_argument("--json", action="store_true", help="machine dump to stdout")
    ap.add_argument("--cell", help="'<language>/<mechanism>' — one cell in detail")
    ap.add_argument("--language", help="filter rows")
    ap.add_argument("--mechanism", help="filter columns")
    ap.add_argument("--fail-on-error", action="store_true",
                    help="exit 1 on any cell error or invalid cell declaration")
    ap.add_argument("--emit", action="store_true",
                    help="rewrite the committed results-latest.json + COVERAGE.md")
    ap.add_argument("--check", action="store_true",
                    help="exit 1 if the committed artefacts are stale")
    args = ap.parse_args(argv)

    # A filtered artefact would record every excluded cell as `unknown`, which is
    # a lie the committed file would then carry until someone re-emitted unfiltered.
    if (args.emit or args.check) and (args.cell or args.language or args.mechanism
                                      or args.json):
        print("[matrix] --emit/--check cover the full 16x30 grid and cannot be "
              "combined with --cell/--language/--mechanism/--json", file=sys.stderr)
        return 2

    # Resolve the selection BEFORE grading: a typo in --cell should cost nothing
    # and must not surface as a traceback out of the vocabulary module.
    try:
        languages, mechanisms = select(args)
    except ValueError as exc:
        print(f"[matrix] {exc}", file=sys.stderr)
        return 2

    records, bad, legacy_only = collect()
    cells = aggregate(records)

    tally = _counts(cells, [(lg, m) for lg in languages for m in mechanisms])
    print(f"[matrix] {len(languages)} languages x {len(mechanisms)} mechanisms, "
          f"{tally['full']} full, {tally['partial']} partial, {tally['none']} none, "
          f"{tally['unknown']} unknown", file=sys.stderr)

    if args.emit or args.check:
        payload = matrix_emit.build_payload(cells, legacy_only)
        if args.emit:
            matrix_emit.write_results(payload)
            matrix_emit.write_markdown(payload)
            print(f"[matrix] emit: {matrix_emit.RESULTS_PATH.name} "
                  f"{len(payload['cells'])} cells, {matrix_emit.COVERAGE_PATH.name} "
                  f"{len(languages)}x{len(mechanisms)}", file=sys.stderr)
            return 0
        drift, changed = matrix_emit.check(payload)
        if not drift:
            print("[matrix] check: OK", file=sys.stderr)
            return 0
        # stdout carries ONLY the drift lines: the falsification test greps it
        # for one `<lang>/<mech>: <committed> -> <measured>` and nothing else.
        for line in drift:
            print(line)
        print(f"[matrix] check: DRIFT {changed} cells changed", file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps({
            "languages": languages, "mechanisms": mechanisms,
            "cells": {f"{lg}/{m}": {k: v for k, v in s.items() if k != "contributions"}
                      for (lg, m), s in sorted(cells.items())
                      if lg in languages and m in mechanisms},
            "invalid_cell_declarations": [{"fixture": f, "error": e} for f, e in bad],
            "legacy_only": legacy_only, "totals": tally,
        }, indent=2, sort_keys=True))
    elif args.cell:
        pair = (languages[0], mechanisms[0])
        slot = cells.get(pair)
        print(cell_marker(pair, slot), file=sys.stderr)
        print(f"{pair[0]}/{pair[1]} = {slot['level'] if slot else 'unknown'}  "
              f"via={slot['via'] if slot else None}")
        if slot is None:
            print("  reason   no fixture declares this cell")
        else:
            for field in ("extract", "literal", "route", "forbid"):
                print(f"  {field:<8} {slot[field][0]}/{slot[field][1]}")
            for reason in slot["reasons"]:
                print(f"  reason   {reason}")
            for contribution in slot["contributions"]:
                print(f"  fixture  {contribution['fixture']} = {contribution['level']}")
    else:
        for pair in sorted(p for p in cells if p[0] in languages and p[1] in mechanisms):
            print(cell_marker(pair, cells[pair]), file=sys.stderr)
        render(cells, languages, mechanisms, bad, legacy_only)

    if args.fail_on_error and (bad or tally["error"]):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
