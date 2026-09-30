#!/usr/bin/env python3
"""Zero-dependency tests for matrix.py — `python3 test_matrix.py`, exit 0 on green.

Two halves, deliberately kept apart:

  DERIVATION tests are hermetic. They drive `derive` / `aggregate` / `collect`
  against canned grade rows and a StubGrader over a temp tree, so the MIN rule,
  the four assertions, the mis-declared-cell hard error and the "an error is
  never swallowed" rule are proven without building a single graph. They cannot
  flap when a sibling packet lands a fixture.

  ANCHOR tests measure real cells whose levels the packet spec names:
      go/nats      => full via=queue
      python/kafka => partial
      csharp/kafka => none
      clojure/kafka => unknown
      go/subproject => full via=project   (LA.9: an ANCHOR mechanism, graded
                                           with route not-applicable)
  The first three need fixtures that later corpus packets (A15.6 legacy `cells`
  tags, A15.7 matrix/ authoring) still have to land. Until then each SKIPS with
  a line naming exactly what is missing — never a silent pass, and never a
  fabricated number. `clojure/kafka => unknown` needs nothing and always runs:
  it is the assertion that `unknown` really is distinguishable from `none`.

An anchor grades ONLY the fixtures that declare its cell, not the whole corpus,
so this file stays cheap and does not re-grade 79 fixtures run.py already owns.

  PAIRING (LA.10) builds a fixed list of two-dir fixtures once with two dirs
  and once as one, and holds matrix_vocab.REPO_PAIRWISE_CATEGORIES to what the
  engine does: stack edges pair single-dir, SHARES_* edges need two repos.
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
    "go/subproject": ("matrix/go/subproject — LA.9 re-authored it against the "
                      "PROJECT anchor (node_kind 45)"),
}


class Skip(Exception):
    """Raised by a test that cannot run yet; the reason must name the blocker."""


# ---------------------------------------------------------------------------
# Canned grade rows
# ---------------------------------------------------------------------------

def _res(nodes=(), edges=(), forbid=(), cells=()):
    """A grade_fixture() return value, trimmed to the keys matrix.py reads."""
    return {
        "node_recall": [{"kind": k, "name": n, "found": f} for k, n, f in nodes],
        "edge_recall": [{"category": c, "from": a, "to": b, "found": f}
                        for c, a, b, f in edges],
        "forbid_results": [{"kind": k, "matched": m, "violated": v}
                           for k, m, v in forbid],
        "cell_recall": [{"kind": k, "node": n, "cell": c, "contains": s, "found": f}
                        for k, n, c, s, f in cells],
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


# subproject is an ANCHOR mechanism (matrix_vocab `anchor: True`): PROJECT is an
# edge-less anchor by design, so routing is not-applicable rather than a cap. The
# nats cap test above is the proof the rule did NOT loosen for routed mechanisms.
PROJECTS = [("PROJECT", "project:svc-a", True), ("PROJECT", "project:svc-b", True)]


def test_derive_anchor_mechanism_is_full_without_routing():
    res = _res(PROJECTS, forbid=[("PROJECT", 0, False), ("PROJECT", 1, False)])
    got = matrix.derive("subproject", res, {"PROJECT", "MODULE"})
    assert got["level"] == "full", got
    assert got["via"] == "project", got
    assert got["route"] == [0, 0] and got["forbid"] == [2, 2], got
    assert "anchor mechanism: routing not applicable" in got["reasons"], got
    assert "no routing expected" not in got["reasons"], got


def test_derive_anchor_still_fails_on_a_forbid():
    # e.g. a vendored manifest minted its own PROJECT: precision still caps.
    res = _res(PROJECTS, forbid=[("PROJECT", 1, True), ("PROJECT", 1, False)])
    got = matrix.derive("subproject", res, {"PROJECT"})
    assert got["level"] == "partial" and "forbid violated" in got["reasons"], got
    assert got["forbid"] == [1, 2], got


def test_derive_anchor_is_none_without_the_anchor_node():
    # route not-applicable must not paper over extraction: no PROJECT, no cell.
    got = matrix.derive("subproject", _res(PROJECTS), {"MODULE", "REGION"})
    assert got["level"] == "none" and got["via"] is None, got


def test_scaffold_gives_an_anchor_mechanism_no_edge_skeleton():
    import grade
    import scaffold
    stubs = [(".", "client.go", "caller"), (".", "server.go", "callee")]
    key = scaffold.build_key("go", vocab.mechanism("subproject"), ["."], stubs)
    assert key["expect_edges"] == [], key
    assert key["forbid"] == [{"kind": "PROJECT", "name": "TODO", "max_nodes": 1,
                              "note": key["forbid"][0]["note"]}], key
    # Every field stays inside the frozen key.json vocabulary grade.py enforces.
    assert set(key) <= grade.TOP_FIELDS, set(key) - grade.TOP_FIELDS
    for f in key["forbid"]:
        assert set(f) <= grade.FORBID_NODE_FIELDS, f
    # A routed mechanism is untouched: one routing-proof edge, no forbid.
    nats = scaffold.build_key("go", vocab.mechanism("nats"), ["."], stubs)
    assert [e["category"] for e in nats["expect_edges"]] == ["QUEUE_FLOWS"], nats
    assert nats["forbid"] == [], nats


# service is a ROLE mechanism (matrix_vocab `role_cells`): LB.3 folds the
# SERVICE overlay into its declaration, which keeps its kind (CLASS here) and
# gains a ROLE cell naming SERVICE. No SERVICE / REGION node is left to count.
SVC_DECL = ("CLASS", "UserService", True)
SVC_ROUTE = ("HTTP_CALLS", "endpoint:GET:/users", "GET /users", True)


def test_derive_counts_role_cells_for_role_mechanisms():
    role = ("CLASS", "UserService", "ROLE", "SERVICE", True)
    got = matrix.derive("service", _res([SVC_DECL], [SVC_ROUTE], cells=[role]),
                        {"CLASS", "ENDPOINT", "ROUTE"})
    assert got["level"] == "full", got
    assert got["extract"] == [1, 1] and got["literal"] == [1, 1], got
    assert got["route"] == [1, 1], got
    assert got["via"] == "service", got     # the found role names the path
    assert "extract read from ROLE cells naming SERVICE" in got["reasons"], got
    assert not any("probed" in r for r in got["reasons"]), got

    # `contains` is case-folded, as grade.py folds the payload.
    lower = ("CLASS", "UserService", "ROLE", "service", True)
    got = matrix.derive("service", _res([SVC_DECL], [SVC_ROUTE], cells=[lower]), {"CLASS"})
    assert got["level"] == "full", got

    # The same fixture with the ROLE cell missing is a measured blind spot.
    miss = ("CLASS", "UserService", "ROLE", "SERVICE", False)
    got = matrix.derive("service", _res([SVC_DECL], [SVC_ROUTE], cells=[miss]),
                        {"CLASS", "ENDPOINT", "ROUTE"})
    assert got["level"] == "none", got
    assert got["extract"] == [0, 1] and got["literal"] == [0, 1], got
    assert got["via"] is None, got

    # A ROLE row naming another role proves nothing about this column, and a
    # non-ROLE cell row carrying the word is not a role row either.
    other = ("CLASS", "UserCard", "ROLE", "COMPONENT", True)
    doc = ("CLASS", "UserService", "DOC", "SERVICE", True)
    got = matrix.derive("service", _res([SVC_DECL], [SVC_ROUTE], cells=[other, doc]),
                        {"CLASS"})
    assert got["level"] == "none", got
    assert got["extract"] == [0, 1] and got["literal"] == [0, 0], got
    assert any("probed" in r for r in got["reasons"]), got
    assert not any("ROLE" in r for r in got["reasons"]), got

    # A standalone SERVICE overlay (no same-qname declaration to fold into)
    # still proves the column through the kind group, as before.
    got = matrix.derive("service", _res([("SERVICE", "Billing", True)], [SVC_ROUTE]),
                        {"SERVICE"})
    assert got["level"] == "full" and got["via"] == "service", got


def test_role_cells_leave_every_other_mechanism_untouched():
    # A mechanism without `role_cells` must derive byte-identically whether or
    # not the fixture also asserts ROLE rows: the role branch is opt-in.
    roled = [m["id"] for m in vocab.MECHANISMS if m.get("role_cells")]
    assert roled == ["service"], roled
    role = ("CLASS", "OrdersService", "ROLE", "SERVICE", True)
    for mid, res, kinds in (
        ("nats", _res([PRODUCED, CONSUMED], [FLOWS]), QUEUE_KINDS),
        ("nats", _res([("CLASS", "Foo", True)]), set()),
        ("subproject", _res(PROJECTS, forbid=[("PROJECT", 0, False)]), {"PROJECT"}),
    ):
        plain = matrix.derive(mid, res, kinds)
        with_role = matrix.derive(mid, {**res, "cell_recall": [
            {"kind": role[0], "node": role[1], "cell": role[2],
             "contains": role[3], "found": role[4]}]}, kinds)
        assert plain == with_role, (mid, plain, with_role)


def test_scaffold_gives_a_role_mechanism_the_folded_shape():
    import grade
    import scaffold
    stubs = [("client", "client.py", "service A"), ("server", "server.py", "service B")]
    key = scaffold.build_key("python", vocab.mechanism("service"), ["client", "server"], stubs)
    assert [(n["kind"], n["name"]) for n in key["expect_nodes"]] == [("CLASS", "TODO")], key
    assert [(c["kind"], c["node"], c["cell"], c["contains"]) for c in key["expect_cells"]] == [
        ("CLASS", "TODO", "ROLE", "SERVICE")], key
    assert [e["category"] for e in key["expect_edges"]] == ["HTTP_CALLS"], key
    assert set(key) <= grade.TOP_FIELDS, set(key) - grade.TOP_FIELDS
    for c in key["expect_cells"]:
        assert set(c) <= grade.CELL_FIELDS, c
    assert _is_scaffold(key), "a role skeleton must still read as unauthored"
    # A kind-only mechanism keeps its kind skeleton and no ROLE row.
    nats = scaffold.build_key("go", vocab.mechanism("nats"), ["."], stubs)
    assert nats["expect_cells"] == [], nats
    assert [n["kind"] for n in nats["expect_nodes"]] == ["QUEUE_PRODUCER", "QUEUE_CONSUMER"], nats


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


# ---------------------------------------------------------------------------
# Not applicable (CF.13a): matrix_vocab.NOT_APPLICABLE, falsified by a fixture
# ---------------------------------------------------------------------------

# The 4 azure_sb cells whose language lacks a client library: NOT listed (James
# 2026-09-30). A client can appear, and a wrong n/a hides a measurable cell.
NO_CLIENT_CELLS = ("ruby/azure_sb", "php/azure_sb", "dart/azure_sb", "elixir/azure_sb")


def test_na_list_names_canonical_cells_with_reasons():
    na = vocab.NOT_APPLICABLE
    assert len(na) == 28, len(na)
    assert list(na) == sorted(na), "NOT_APPLICABLE rows must be written sorted"
    per_lang = {}
    for label, entry in na.items():
        # parse_cell normalizes and raises on a bad row or column; a canonical
        # key must survive the round trip unchanged (no alias spellings).
        lang, mech = matrix.parse_cell(label)
        assert f"{lang}/{mech}" == label, (label, lang, mech)
        cls, reason = entry
        assert cls == "structural", (label, cls)
        assert isinstance(reason, str) and len(reason) >= 20, (label, reason)
        assert vocab.not_applicable(lang, mech) == reason, label
        per_lang[lang] = per_lang.get(lang, 0) + 1
    assert per_lang == {"solidity": 22, "terraform": 6}, per_lang
    # the unknown anchor and the no-client cells stay measurable
    assert "clojure/kafka" not in na
    for label in NO_CLIENT_CELLS:
        assert label not in na, label
        assert vocab.not_applicable(*label.split("/")) is None, label
    assert vocab.NA_LEVEL == "n/a"


def test_an_na_cell_reads_na_not_unknown():
    assert matrix.cell_marker(("solidity", "kafka")) == (
        "[matrix] cell solidity/kafka = n/a (extract=0/0 literal=0/0 "
        "route=0/0 forbid=0/0) via=None fixtures="), matrix.cell_marker(("solidity", "kafka"))
    assert "= unknown" in matrix.cell_marker(("dart", "azure_sb"))
    tally = matrix._counts({}, [("solidity", "kafka"), ("dart", "azure_sb")])
    assert tally["n/a"] == 1 and tally["unknown"] == 1, tally
    out = io.StringIO()
    matrix.render({}, ["solidity"], ["kafka"], [], [], out=out)
    text = out.getvalue()
    rows = {ln.split()[0]: ln.split()[1:] for ln in text.splitlines()
            if ln[:1].isalpha() and ln.split()[1:]
            and set(ln.split()[1:]) <= set("●◐·?-!")}
    assert rows.get("solidity") == ["-"], text
    # the coverage line divides by APPLICABLE cells and names the n/a count
    assert "COVERAGE OF THE COVERAGE: 0/0 cells have a fixture (0.0%); 1 n/a" in text, text


def test_a_fixture_on_an_na_cell_is_an_error():
    tmp = Path(tempfile.mkdtemp(prefix="matrix-test-"))
    try:
        _write_key(tmp, "matrix/solidity/kafka", ["solidity/kafka"], language="solidity")
        _write_key(tmp, "matrix/go/nats", ["go/nats"])
        good = (_res([PRODUCED, CONSUMED], [FLOWS]), QUEUE_KINDS)
        stub = StubGrader({"kafka": good, "nats": good})
        records, bad, _ = matrix.collect(root=tmp, grader=stub)
        assert bad == [], bad
        cells = matrix.aggregate(records)
        # graded as usual: the list does not change how a fixture scores ...
        assert cells[("solidity", "kafka")]["level"] == "full", cells
        matrix.apply_not_applicable(cells)
        # ... but a fixture claiming an n/a cell contradicts the list: an error
        slot = cells[("solidity", "kafka")]
        assert slot["level"] == "error", slot
        assert slot["reasons"][0].startswith("n/a contradicted: "), slot["reasons"]
        assert "remove the NOT_APPLICABLE entry" in slot["reasons"][0], slot["reasons"]
        assert cells[("go", "nats")]["level"] == "full", cells  # unlisted: untouched
        tally = matrix._counts(cells, [("solidity", "kafka"), ("go", "nats")])
        assert tally["error"] == 1 and tally["n/a"] == 0, tally
        out = io.StringIO()
        matrix.render(cells, ["solidity"], ["kafka"], [], [], out=out)
        assert "  - solidity/kafka: n/a contradicted: " in out.getvalue(), out.getvalue()
    finally:
        shutil.rmtree(tmp)


def test_emit_counts_na_apart_from_unknown():
    import matrix_emit
    payload = matrix_emit.build_payload({}, [], languages=["solidity"],
                                        mechanisms=["kafka", "calls"], engine="test")
    assert payload["schema"] == 2, payload["schema"]
    totals = payload["totals"]
    assert totals["n/a"] == 1 and totals["unknown"] == 1, totals
    assert totals["covered"] == 0 and totals["grid"] == 2, totals
    assert list(payload["not_applicable"]) == ["solidity/kafka"], payload["not_applicable"]
    assert payload["not_applicable"]["solidity/kafka"]["class"] == "structural"
    voc = payload["vocabulary"]
    assert voc["not_applicable"] == 28, voc
    assert len(voc["na_digest"]) == 12 and voc["na_digest"] == matrix_emit.na_digest(), voc
    # the n/a list has its own digest: the scoring ruler does not move with it
    assert voc["digest"] == matrix_emit.vocabulary_digest(), voc
    md = matrix_emit.render_markdown(payload, ["solidity"], ["kafka", "calls"])
    assert "0/1 applicable cells have a fixture" in md and ", 1 n/a" in md, md
    assert "## Not applicable" in md and "| `solidity/kafka` | structural |" in md, md
    grid_row = next(ln for ln in md.splitlines() if ln.startswith("solidity "))
    assert grid_row.split() == ["solidity", "-", "?"], grid_row
    # a cell entering the list is drift; a schema-1 payload has no list at all
    old = {k: v for k, v in payload.items() if k != "not_applicable"}
    assert matrix_emit.level_drift(old, payload) == ["solidity/kafka: unknown -> n/a"]
    assert matrix_emit.level_drift(payload, old) == ["solidity/kafka: n/a -> unknown"]
    assert matrix_emit.level_drift(payload, payload) == []
    # dropped from the list in the commit that adds its fixture
    measured = {**payload, "not_applicable": {},
                "cells": {"solidity/kafka": {"level": "full"}}}
    assert matrix_emit.level_drift(payload, measured) == ["solidity/kafka: n/a -> full"]


def test_scaffold_refuses_an_na_cell():
    import scaffold
    tmp = Path(tempfile.mkdtemp(prefix="matrix-test-"))
    # scaffold() writes under the module-level MATRIX: point it at a temp tree
    # so neither outcome can touch the real matrix/ corpus.
    real = (scaffold.MATRIX, sys.stderr, sys.stdout)
    scaffold.MATRIX, sys.stderr, sys.stdout = tmp, io.StringIO(), io.StringIO()
    try:
        rc = scaffold.main(["solidity", "kafka"])
        err = sys.stderr.getvalue()
        written = sorted(p.relative_to(tmp).as_posix() for p in tmp.rglob("*"))
        # an applicable cell still scaffolds (proves the redirect took effect)
        ok = scaffold.main(["dart", "azure_sb"])
        ok_written = (tmp / "dart" / "azure_sb" / "key.json").exists()
    finally:
        scaffold.MATRIX, sys.stderr, sys.stdout = real
        shutil.rmtree(tmp)
    assert rc == 2, (rc, err)
    assert "NOT_APPLICABLE" in err and "solidity/kafka" in err, err
    assert written == [], written
    assert ok == 0 and ok_written, ok


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
    # Was `partial` when this anchor was written: extract_topic_near took the
    # FIRST occurrence of `KafkaConsumer`, which is the import line, so the
    # consumer collapsed to the framework tag and never paired. A2.1 (b8b00aa)
    # replaced that scanner with one that walks every occurrence, before any
    # fixture for this cell existed. A15.7 authored matrix/python/kafka with the
    # import line kept, and it measures `full via=queue`. The anchor moves with
    # the fix; the name is kept so the history of the cell stays greppable.
    _anchor("python/kafka", "full", "queue")


def test_anchor_csharp_kafka_is_none():
    # Was `none` when this anchor was written: C# Kafka emitted no queue nodes
    # at all. A2.2 (wave 2, 71052ff) shipped the receiver-agnostic needles and
    # case-insensitive framework gates, and A2.3 (wave 3, dfe3969) made the
    # topic-agnostic tag node unpairable, so the cell is now `full via=queue`
    # on fixtures/xcut-queue-csharp-kafka. The anchor moves with the fix; the
    # name is kept so the history of the cell stays greppable.
    _anchor("csharp/kafka", "full", "queue")


def test_anchor_go_subproject_is_full():
    # Read `none via=contain` before LA.9: the column measured REGION/CONTAINS,
    # written before PROJECT (A8.5) shipped, while `glia projects` listed both
    # go.mod roots. Re-authored against PROJECT; the vendored go.mod under
    # svc-a/vendor is the forbid that keeps the cell honest.
    _anchor("go/subproject", "full", "project")


def test_anchor_clojure_kafka_is_unknown():
    # The fourth anchor needs no fixture: it IS the claim that a cell nobody has
    # measured reads `unknown`, not `none`. If this ever reads `none`, some
    # fixture started scoring a cell it does not cover.
    level, via = measure("clojure/kafka")
    assert (level, via) == ("unknown", None), (level, via)


# ---------------------------------------------------------------------------
# Single-dir pairing — which cross-graph edges need two RepoIds (LA.10)
# ---------------------------------------------------------------------------

# Every category a cross-graph resolver (graph/src/resolvers/) emits. Intra-graph
# categories such as IMPORTS are left out: single-dir qnames carry the dir
# prefix, so their edges move for reasons that have nothing to do with RepoIds.
CROSS_GRAPH_CATEGORIES = frozenset({
    "HTTP_CALLS", "GRPC_CALLS", "RPC_CALLS", "QUEUE_FLOWS", "GRAPHQL_CALLS",
    "WS_CONNECTS", "EVENT_FLOWS", "CLI_INVOKES",
    "SHARES_SCHEMA", "SHARES_DATA_ENTITY", "SHARES_DATA_SOURCE", "SHARES_CONFIG",
    "SHARES_INFRA_REF", "SHARES_DEPENDENCY", "SHARES_CRON_SCHEDULE",
})

# Two-dir fixtures, each with the cross-graph category it exists to show.
PAIRING_PROBES = [
    ("xcut-proto-shared", "SHARES_SCHEMA"),
    ("xdata-source-shares", "SHARES_DATA_SOURCE"),
    ("xiac-terraform-k8s", "SHARES_INFRA_REF"),
    ("xpoly-data-entity", "SHARES_DATA_ENTITY"),
    ("xcli-invokes", "CLI_INVOKES"),
    ("xstack-go-http", "HTTP_CALLS"),
    ("xcut-grpc-grpc_calls", "GRPC_CALLS"),
    ("xcut-trpc", "RPC_CALLS"),
    ("xcut-queue-queue_flows", "QUEUE_FLOWS"),
    ("xcut-websocket-ws_connects", "WS_CONNECTS"),
    ("xcut-graphql-graphql_calls", "GRAPHQL_CALLS"),
    ("xcut-eventbus-spring", "EVENT_FLOWS"),
]


def test_single_dir_pairing_matches_the_vocabulary():
    # Builds each probe twice -- generate_many(dirs) and generate(root) -- and
    # holds every cross-graph category of the two-dir build to one rule: it
    # appears single-dir exactly when REPO_PAIRWISE_CATEGORIES omits it. Fails
    # the day a stack resolver starts needing two RepoIds, or a SHARES_*
    # resolver starts pairing inside one repo.
    rg = matrix._grade.rg          # grade's import set GLIA_NO_PERSIST first
    names = dict(rg.category_names())
    unknown = (CROSS_GRAPH_CATEGORIES | vocab.REPO_PAIRWISE_CATEGORIES) - set(names.values())
    assert not unknown, f"not in the locked edge_category registry: {sorted(unknown)}"
    assert vocab.REPO_PAIRWISE_CATEGORIES <= CROSS_GRAPH_CATEGORIES, (
        "REPO_PAIRWISE_CATEGORIES names a category no cross-graph resolver emits")

    def cross(g):
        return {names[e["category"]] for e in json.loads(g.edges_json())} & CROSS_GRAPH_CATEGORIES

    def persisted():
        return {(p, p.stat().st_mtime_ns) for f, _ in PAIRING_PROBES
                for p in (HERE / "fixtures" / f).rglob("*.gmap")}

    before, wrong = persisted(), []
    for fixture, shown in PAIRING_PROBES:
        root = (HERE / "fixtures" / fixture).resolve()
        dirs = json.loads((root / "key.json").read_text())["dirs"]
        assert len(dirs) >= 2, f"{fixture}: a pairing probe needs two dirs, has {dirs}"
        two = cross(rg.generate_many([str((root / d).resolve()) for d in dirs]))
        assert shown in two, (
            f"{fixture}: the two-dir build no longer shows {shown}, so it proves nothing")
        one = cross(rg.generate(str(root), False))
        for cat in sorted(two):
            pairwise = cat in vocab.REPO_PAIRWISE_CATEGORIES
            if (cat in one) == pairwise:
                wrong.append(f"{fixture}: {cat} {'appears' if cat in one else 'is absent'} "
                             f"single-dir, but REPO_PAIRWISE_CATEGORIES "
                             f"{'lists' if pairwise else 'omits'} it")
    assert not wrong, "single-dir pairing disagrees with the vocabulary:\n  " + "\n  ".join(wrong)
    assert persisted() == before, "GLIA_NO_PERSIST leaked .gmap writes into the probe fixtures"


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
