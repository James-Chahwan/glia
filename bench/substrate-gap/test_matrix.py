#!/usr/bin/env python3
"""Zero-dependency tests for matrix.py — `python3 test_matrix.py`, exit 0 on green.

Two halves, deliberately kept apart:

  DERIVATION tests are hermetic. They drive `derive` / `aggregate` / `collect`
  against canned grade rows and a StubGrader over a temp tree, so the MIN rule,
  the four assertions, the mis-declared-cell hard error and the "an error is
  never swallowed" rule are proven without building a single graph. They cannot
  flap when a sibling packet lands a fixture.

  ANCHOR tests measure four real cells whose levels the packet spec names:
      go/nats      => full via=queue
      python/kafka => partial
      csharp/kafka => none
      clojure/kafka => unknown
  The first three need fixtures that later corpus packets (A15.6 legacy `cells`
  tags, A15.7 matrix/ authoring) still have to land. Until then each SKIPS with
  a line naming exactly what is missing — never a silent pass, and never a
  fabricated number. `clojure/kafka => unknown` needs nothing and always runs:
  it is the assertion that `unknown` really is distinguishable from `none`.

An anchor grades ONLY the fixtures that declare its cell, not the whole corpus,
so this file stays cheap and does not re-grade 79 fixtures run.py already owns.
"""
import io
import json
import shutil
import sys
import tempfile
import traceback
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import matrix  # noqa: E402
import matrix_vocab as vocab  # noqa: E402

# What a skipped anchor must name, so "SKIP" is always actionable.
ANCHOR_SOURCE = {
    "go/nats": ("fixtures/xcut-queue-queue_flows — A15.6 must add "
                "\"cells\": [\"go/nats\"] to its key.json"),
    "python/kafka": ("matrix/python/kafka — A15.7 must author the source and key; "
                     "the scaffold's TODO placeholders are not a fixture"),
    "csharp/kafka": ("fixtures/xcut-queue-csharp-kafka — declares the "
                     "non-vocabulary cell 'csharp/queue' today, so it scores nothing"),
}


class Skip(Exception):
    """Raised by a test that cannot run yet; the reason must name the blocker."""


# ---------------------------------------------------------------------------
# Canned grade rows
# ---------------------------------------------------------------------------

def _res(nodes=(), edges=(), forbid=()):
    """A grade_fixture() return value, trimmed to the keys matrix.py reads."""
    return {
        "node_recall": [{"kind": k, "name": n, "found": f} for k, n, f in nodes],
        "edge_recall": [{"category": c, "from": a, "to": b, "found": f}
                        for c, a, b, f in edges],
        "forbid_results": [{"kind": k, "matched": m, "violated": v}
                           for k, m, v in forbid],
    }


PRODUCED = ("QUEUE_PRODUCER", "orders", True)
CONSUMED = ("QUEUE_CONSUMER", "orders", True)
FLOWS = ("QUEUE_FLOWS", "orders", "orders", True)
QUEUE_KINDS = {"QUEUE_PRODUCER", "QUEUE_CONSUMER"}


class StubGrader:
    """Stands in for grade.py: canned rows + canned node kinds, no engine.

    `grade_with_kinds` observes the kinds by monkeypatching `build_graph`, so
    the stub must route through its own attribute for the spy to see anything —
    which is also what proves the spy is wired to the real call path.
    """

    _KIND_BY_ID = {1: "QUEUE_PRODUCER", 2: "QUEUE_CONSUMER", 9: "CLASS"}
    _ID_BY_KIND = {v: k for k, v in _KIND_BY_ID.items()}

    def __init__(self, plan):
        self.plan = plan            # {fixture dir name: (res, kinds) | Exception}
        self.graded = []

    def build_graph(self, fixture_dir, key):
        _, kinds = self.plan[Path(fixture_dir).name]
        nodes = [{"kind": self._ID_BY_KIND[k]} for k in kinds]
        return None, nodes, [], {}

    def grade_fixture(self, fixture_dir):
        entry = self.plan[Path(fixture_dir).name]
        if isinstance(entry, Exception):
            raise entry
        self.graded.append(Path(fixture_dir).name)
        self.build_graph(Path(fixture_dir), {})
        return entry[0]


def _write_key(root, rel, cells, framework=None, language="go"):
    d = Path(root) / rel
    d.mkdir(parents=True, exist_ok=True)
    (d / "key.json").write_text(json.dumps({
        "framework": framework or d.name, "language": language,
        "dirs": ["."], "cells": cells,
    }))
    return d


# ---------------------------------------------------------------------------
# Vocabulary wiring — matrix.py must CONSUME matrix_vocab, never restate it
# ---------------------------------------------------------------------------

def test_vocab_is_the_only_vocabulary():
    src = (HERE / "matrix.py").read_text()
    assert "import matrix_vocab" in src, "matrix.py must consume matrix_vocab"
    assert len(vocab.LANGUAGES) == 16 and len(vocab.MECHANISM_IDS) == 30
    # No second copy of the column list smuggled in as a literal.
    for mid in ("http_client", "sqs_sns", "subproject"):
        assert f'"{mid}"' not in src and f"'{mid}'" not in src, (
            f"matrix.py hardcodes mechanism id {mid!r}; the vocabulary is A15.2's")


def test_parse_cell():
    assert matrix.parse_cell("python/kafka") == ("python", "kafka")
    assert matrix.parse_cell("ts/ws") == ("typescript", "ws")       # alias row
    for bad in ("python", "python/queue", "kotlin/kafka", ""):
        try:
            matrix.parse_cell(bad)
        except ValueError:
            continue
        raise AssertionError(f"parse_cell({bad!r}) should have raised")


def test_mechanism_from_dir_allows_a_variant_suffix():
    assert matrix.mechanism_from_dir("kafka") == "kafka"
    assert matrix.mechanism_from_dir("sqs_sns") == "sqs_sns"        # longest id wins
    assert matrix.mechanism_from_dir("kafka-two") == "kafka"
    assert matrix.mechanism_from_dir("http_server_composed") == "http_server"
    try:
        matrix.mechanism_from_dir("queue")
    except ValueError as exc:
        assert "30" in str(exc) or "http_client" in str(exc), exc
    else:
        raise AssertionError("mechanism_from_dir('queue') should have raised")


# ---------------------------------------------------------------------------
# The four graded assertions -> one level
# ---------------------------------------------------------------------------

def test_derive_full():
    got = matrix.derive("nats", _res([PRODUCED, CONSUMED], [FLOWS]), QUEUE_KINDS)
    assert got["level"] == "full", got
    assert got["via"] == "queue", got
    assert got["extract"] == [2, 2] and got["route"] == [1, 1], got


def test_derive_none_when_nothing_of_the_kind_was_emitted():
    got = matrix.derive("nats", _res([PRODUCED, CONSUMED], [FLOWS]), {"CLASS"})
    assert got["level"] == "none", got
    assert got["via"] is None, got


def test_derive_partial_when_the_literal_is_lost():
    rows = [("QUEUE_PRODUCER", "orders", False), ("QUEUE_CONSUMER", "orders", False)]
    got = matrix.derive("nats", _res(rows, [FLOWS]), QUEUE_KINDS)
    assert got["level"] == "partial", got       # kinds present, names are not
    assert "literal lost" in got["reasons"], got


def test_derive_partial_when_routing_is_missing():
    miss = ("QUEUE_FLOWS", "orders", "orders", False)
    got = matrix.derive("nats", _res([PRODUCED, CONSUMED], [miss]), QUEUE_KINDS)
    assert got["level"] == "partial" and "routing edge missing" in got["reasons"], got


def test_derive_caps_at_partial_when_no_routing_is_expected():
    got = matrix.derive("nats", _res([PRODUCED, CONSUMED]), QUEUE_KINDS)
    assert got["level"] == "partial", got
    assert "no routing expected" in got["reasons"], got


def test_derive_forbid_violation_caps_a_phantom_cell_at_partial():
    res = _res([PRODUCED, CONSUMED], [FLOWS], [("QUEUE_PRODUCER", 1, True)])
    got = matrix.derive("nats", res, QUEUE_KINDS)
    assert got["level"] == "partial" and "forbid violated" in got["reasons"], got
    assert got["forbid"] == [0, 1], got


def test_derive_records_which_path_actually_fired():
    # mqtt's MEASURED path is the generic eventbus needles, not the queue registry.
    got = matrix.derive("mqtt", _res(), {"EVENT_EMITTER"})
    assert got["via"] == "eventbus", got
    assert got["level"] == "partial", got       # extracted, but nothing routed
    got = matrix.derive("mqtt", _res(), {"QUEUE_PRODUCER"})
    assert got["via"] == "queue", got


def test_derive_probes_mechanism_kinds_when_the_fixture_scopes_no_nodes():
    # A legacy fixture tagged with `cells` but whose expect_nodes are all of
    # another mechanism's kinds still gets a real extract verdict.
    got = matrix.derive("nats", _res([("CLASS", "Foo", True)]), set())
    assert got["level"] == "none", got
    assert any("probed" in r for r in got["reasons"]), got


# ---------------------------------------------------------------------------
# Aggregation: MIN across fixtures
# ---------------------------------------------------------------------------

def _rec(level, fixture, cell=("go", "nats")):
    return {"level": level, "cell": cell, "fixture": fixture, "via": "queue",
            "extract": [1, 1], "literal": [1, 1], "route": [1, 1], "forbid": [0, 0],
            "reasons": [f"from {fixture}"]}


def test_aggregate_takes_the_min_level():
    for worse, better, want in (("partial", "full", "partial"),
                                ("none", "partial", "none"),
                                ("error", "none", "error"),
                                ("full", "full", "full")):
        cells = matrix.aggregate([_rec(better, "fixtures/b"), _rec(worse, "fixtures/a")])
        slot = cells[("go", "nats")]
        assert slot["level"] == want, (worse, better, slot["level"])
        assert slot["fixtures"] == ["fixtures/a", "fixtures/b"], slot["fixtures"]
        assert slot["reasons"] == ["from fixtures/a", "from fixtures/b"], slot


def test_aggregate_sums_the_assertion_denominators():
    cells = matrix.aggregate([_rec("full", "fixtures/a"), _rec("partial", "fixtures/b")])
    assert cells[("go", "nats")]["extract"] == [2, 2]


# ---------------------------------------------------------------------------
# Discovery, cell declaration, error propagation
# ---------------------------------------------------------------------------

def test_a_fixture_filed_under_the_wrong_cell_is_a_hard_error():
    tmp = Path(tempfile.mkdtemp(prefix="matrix-test-"))
    try:
        d = _write_key(tmp, "matrix/go/nats", ["python/kafka"])
        try:
            matrix.declared_cells(d / "key.json", root=tmp)
        except ValueError as exc:
            assert "go/nats" in str(exc) and "python/kafka" in str(exc), exc
        else:
            raise AssertionError("a mis-filed fixture must not score its declared cell")
        # ... and the same directory declaring its own cell is fine.
        ok = _write_key(tmp, "matrix/go/nats-2", ["go/nats"])
        _, cells = matrix.declared_cells(ok / "key.json", root=tmp)
        assert cells == [("go", "nats")], cells
    finally:
        shutil.rmtree(tmp)


def test_collect_aggregates_two_fixtures_onto_one_cell():
    tmp = Path(tempfile.mkdtemp(prefix="matrix-test-"))
    try:
        _write_key(tmp, "fixtures/q-good", ["go/nats"])
        _write_key(tmp, "fixtures/q-degraded", ["go/nats"])
        _write_key(tmp, "fixtures/q-legacy", [])          # no cells => legacy_only
        stub = StubGrader({
            "q-good": (_res([PRODUCED, CONSUMED], [FLOWS]), QUEUE_KINDS),
            "q-degraded": (_res([("QUEUE_PRODUCER", "orders", False)], [FLOWS]),
                           {"QUEUE_PRODUCER"}),
        })
        records, bad, legacy_only = matrix.collect(root=tmp, grader=stub)
        assert bad == [], bad
        assert legacy_only == ["fixtures/q-legacy"], legacy_only
        assert sorted(stub.graded) == ["q-degraded", "q-good"], stub.graded
        slot = matrix.aggregate(records)[("go", "nats")]
        assert slot["level"] == "partial", slot            # MIN, not the best one
        assert slot["fixtures"] == ["fixtures/q-degraded", "fixtures/q-good"], slot
        assert matrix.cell_marker(("go", "nats"), slot).startswith(
            "[matrix] cell go/nats = partial ("), matrix.cell_marker(("go", "nats"), slot)
    finally:
        shutil.rmtree(tmp)


def test_a_raising_fixture_is_recorded_as_error_not_swallowed():
    tmp = Path(tempfile.mkdtemp(prefix="matrix-test-"))
    try:
        _write_key(tmp, "matrix/go/nats", ["go/nats"])
        stub = StubGrader({"nats": ValueError("unknown key.json field 'expect_literals'")})
        records, bad, legacy_only = matrix.collect(root=tmp, grader=stub)
        assert len(records) == 1, records
        assert records[0]["level"] == "error", records
        assert "expect_literals" in records[0]["reasons"][0], records
        cells = matrix.aggregate(records)
        assert cells[("go", "nats")]["level"] == "error"
        assert matrix._counts(cells, [("go", "nats")])["error"] == 1
    finally:
        shutil.rmtree(tmp)


def test_an_invalid_cell_declaration_is_reported_not_dropped():
    tmp = Path(tempfile.mkdtemp(prefix="matrix-test-"))
    try:
        _write_key(tmp, "fixtures/bad", ["csharp/queue"])   # 'queue' is not a column
        records, bad, legacy_only = matrix.collect(root=tmp, grader=StubGrader({}))
        assert records == [] and legacy_only == [], (records, legacy_only)
        assert len(bad) == 1 and bad[0][0] == "fixtures/bad", bad
        assert "queue" in bad[0][1], bad
    finally:
        shutil.rmtree(tmp)


# ---------------------------------------------------------------------------
# Render + marker shape
# ---------------------------------------------------------------------------

def test_render_uses_the_review_glyphs_and_prints_all_four_counts():
    cells = matrix.aggregate([_rec("full", "fixtures/a"),
                              _rec("partial", "fixtures/b", ("python", "kafka"))])
    out = io.StringIO()
    matrix.render(cells, ["go", "python", "clojure"], ["nats", "kafka"], [], [], out=out)
    text = out.getvalue()
    rows = {ln.split()[0]: ln.split()[1:] for ln in text.splitlines()
            if ln[:1].isalpha() and set(ln.split()[1:]) <= set("●◐·?!")}
    assert rows["go"] == ["●", "?"], rows            # nats full, kafka unknown
    assert rows["python"] == ["?", "◐"], rows
    assert rows["clojure"] == ["?", "?"], rows       # no fixture => unknown, not none
    assert "COVERAGE OF THE COVERAGE: 2/6 cells have a fixture (33.3%)" in text, text
    for glyph in "●◐·?!":
        assert glyph in text, f"rollups must always print {glyph}"


def test_the_fired_on_marker_is_a_whole_line_at_16x30():
    err = io.StringIO()
    out = io.StringIO()
    real_err, real_out = sys.stderr, sys.stdout
    sys.stderr, sys.stdout = err, out
    try:
        rc = matrix.main([])
    finally:
        sys.stderr, sys.stdout = real_err, real_out
    assert rc == 0, rc
    hits = [ln for ln in err.getvalue().splitlines()
            if ln.startswith("[matrix] 16 languages x 30 mechanisms")]
    assert len(hits) == 1, err.getvalue()
    assert "full," in hits[0] and "unknown" in hits[0], hits[0]


def test_an_unknown_cell_still_gets_a_marker():
    line = matrix.cell_marker(("clojure", "kafka"))
    assert line == ("[matrix] cell clojure/kafka = unknown (extract=0/0 literal=0/0 "
                    "route=0/0 forbid=0/0) via=None fixtures="), line


def test_a_bad_cli_selection_exits_2_without_grading():
    err = io.StringIO()
    real = sys.stderr
    sys.stderr = err
    try:
        rc = matrix.main(["--cell", "go/queue"])
    finally:
        sys.stderr = real
    assert rc == 2, rc
    assert "unknown mechanism" in err.getvalue(), err.getvalue()


# ---------------------------------------------------------------------------
# run.py is the frozen regression gate — this module must not perturb it
# ---------------------------------------------------------------------------

def test_matrix_never_touches_run_py_or_results_jsonl():
    src = (HERE / "matrix.py").read_text()
    assert "import run" not in src and "results.jsonl" not in src, (
        "matrix.py must not import run.py or write the append-only results log")
    results = HERE / "results.jsonl"
    before = (results.stat().st_size, results.stat().st_mtime_ns) if results.exists() else None
    strays_before = set(HERE.glob("matrix/*/*/*.gmap")) | set(HERE.glob("fixtures/*/*.gmap"))
    out, err = io.StringIO(), io.StringIO()
    real_out, real_err = sys.stdout, sys.stderr
    sys.stdout, sys.stderr = out, err
    try:
        matrix.main([])
    finally:
        sys.stdout, sys.stderr = real_out, real_err
    after = (results.stat().st_size, results.stat().st_mtime_ns) if results.exists() else None
    assert before == after, "matrix.py appended to results.jsonl"
    strays = (set(HERE.glob("matrix/*/*/*.gmap")) | set(HERE.glob("fixtures/*/*.gmap"))) - strays_before
    assert not strays, f"GLIA_NO_PERSIST leaked .gmap dirs into the corpus: {strays}"


# ---------------------------------------------------------------------------
# Anchors — four real cells
# ---------------------------------------------------------------------------

def _is_scaffold(key):
    """True for a scaffold.py skeleton nobody has authored yet."""
    if str(key.get("note", "")).startswith("SCAFFOLDED, NOT AUTHORED"):
        return True
    for group, fields in (("expect_nodes", ("name",)), ("expect_edges", ("from", "to")),
                          ("expect_cells", ("node",))):
        for entry in key.get(group) or []:
            if any(entry.get(f) == "TODO" for f in fields):
                return True
    return False


def measure(cell_label):
    """Grade ONLY the fixtures that declare `cell_label`. -> (level, via).

    Raises Skip when a contributor exists but is not authored yet, so the
    difference between "not measured" and "measured none" is never blurred.
    """
    want = matrix.parse_cell(cell_label)
    contributors = []
    for key_path in matrix.discover():
        try:
            key, cells = matrix.declared_cells(key_path)
        except (ValueError, OSError):
            continue                       # reported by matrix.py's `bad` list
        if want in cells:
            rel = str(key_path.parent.resolve().relative_to(HERE.resolve()))
            if _is_scaffold(key):
                raise Skip(f"{rel} is an unauthored scaffold — {ANCHOR_SOURCE[cell_label]}")
            contributors.append((key_path, rel))
    if not contributors:
        return "unknown", None
    records = []
    for key_path, rel in contributors:
        res, kinds = matrix.grade_with_kinds(key_path.parent)
        rec = matrix.derive(want[1], res, kinds)
        rec.update({"cell": want, "fixture": rel})
        records.append(rec)
    slot = matrix.aggregate(records)[want]
    return slot["level"], slot["via"]


def _anchor(cell_label, want_level, want_via=None):
    level, via = measure(cell_label)
    if level == "unknown":
        raise Skip(f"{cell_label} is unknown — {ANCHOR_SOURCE[cell_label]}")
    assert level == want_level, f"{cell_label} = {level}, expected {want_level}"
    if want_via is not None:
        assert via == want_via, f"{cell_label} via={via}, expected {want_via}"


def test_anchor_go_nats_is_full():
    _anchor("go/nats", "full", "queue")


def test_anchor_python_kafka_is_partial():
    _anchor("python/kafka", "partial")


def test_anchor_csharp_kafka_is_none():
    _anchor("csharp/kafka", "none")


def test_anchor_clojure_kafka_is_unknown():
    # The fourth anchor needs no fixture: it IS the claim that a cell nobody has
    # measured reads `unknown`, not `none`. If this ever reads `none`, some
    # fixture started scoring a cell it does not cover.
    level, via = measure("clojure/kafka")
    assert (level, via) == ("unknown", None), (level, via)


# ---------------------------------------------------------------------------

def main():
    tests = [(n, f) for n, f in sorted(globals().items())
             if n.startswith("test_") and callable(f)]
    passed = skipped = failed = 0
    for name, fn in tests:
        try:
            fn()
        except Skip as exc:
            skipped += 1
            print(f"SKIP {name}: {exc}")
        except Exception:                                    # noqa: BLE001
            failed += 1
            print(f"FAIL {name}")
            traceback.print_exc()
        else:
            passed += 1
            print(f"PASS {name}")
    print(f"\n{passed} passed, {skipped} skipped, {failed} failed "
          f"({len(tests)} tests)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
