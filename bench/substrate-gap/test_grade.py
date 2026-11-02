#!/usr/bin/env python3
"""Regression tests for grade.py's identity matchers.

Wave 2 shipped a correct java route composition and the harness accused it
anyway: `forbid {to: "UserController"}` matched the METHOD node whose qname is
`UserController::UserController::getUser`, because the precision gate had
inherited the recall gate's substring matcher. Leniency can only turn a recall
miss into a hit, but it turns a precision gate into a false accusation.

These tests pin both directions: the strict matcher must not over-match, and it
must still catch a real violation.

CA.8 added `"exact": true` on expect_nodes / expect_edges entries: the recall
gate then uses the strict matcher, so `get_user -> helper` is no longer
satisfied by `get_user_impl -> helper`. The checks below pin that over-match,
its fix, and that an entry without `exact` grades as before.

FD.24 added `rule` on forbid edge rows: the row fires only on an edge whose
EVIDENCE rule equals it (read through `PyGraph.why`). The checks below pin that
a json_response USES edge fires such a row while a composite_literal one between
the same two nodes does not, and that models-go-wire's real build both holds the
restored json_response guard and fires a composite_literal row.

HQ.4 added `site` on forbid edge rows: the row fires only on an edge whose `why`
site file equals it. The checks below pin that a site row fires on the edge
sited there and not on the same pair sited elsewhere, that `rule` and `site`
combine with AND, that a non-string `site` raises, that a key with no `site`
grades as before, and that solidity-twin-contracts' real build sites each
copy's HANDLED_BY in its own file.
"""
import contextlib
import io
import json
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from grade import (  # noqa: E402
    _CAT_BY_NAME, _KIND_BY_NAME, CELL_FIELDS, EDGE_FIELDS, FORBID_EDGE_FIELDS,
    FORBID_NODE_FIELDS, NODE_FIELDS, _node_matches, _node_matches_exact,
    _grade_forbid, _print_verbose, _recall_edge_hit, _recall_node_hit, grade_fixture,
)

FAILED = []


def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond:
        FAILED.append(name)


METHOD = {"name": "getUser", "qname": "UserController::UserController::getUser"}
CLASS = {"name": "UserController", "qname": "UserController"}
ROUTE = {"name": "/users", "qname": "route:/users"}

# The exact bug from wave 2: the class name is an ANCESTOR SEGMENT of the
# method's qname, so the lenient matcher claims the method IS the class.
check("lenient matcher over-matches an ancestor qname segment (the bug)",
      _node_matches(METHOD, "UserController"))
check("strict matcher does NOT match an ancestor qname segment",
      not _node_matches_exact(METHOD, "UserController"))

# ...while still identifying the thing it is actually named after.
check("strict matches the class by name", _node_matches_exact(CLASS, "UserController"))
check("strict matches a node by its qname", _node_matches_exact(ROUTE, "route:/users"))
check("strict matches the method by its full qname",
      _node_matches_exact(METHOD, "UserController::UserController::getUser"))

# Normalisation is preserved: :: and . both fold to /, case-insensitively.
check("strict still normalises :: and case",
      _node_matches_exact(METHOD, "usercontroller/usercontroller/getuser"))

# A prefix must not match either, in either direction.
check("strict rejects a proper prefix", not _node_matches_exact(ROUTE, "route:/use"))
check("strict rejects a superstring", not _node_matches_exact(CLASS, "UserControllerImpl"))


# ---- CA.8: `"exact": true` on recall entries ------------------------------
CALLS, FUNCTION, METHOD = _CAT_BY_NAME["CALLS"], _KIND_BY_NAME["FUNCTION"], _KIND_BY_NAME["METHOD"]
EXACT_NODE = {"id": 2, "kind": FUNCTION, "name": "get_user_impl",
              "qname": "user_controller::get_user_impl"}
GET_USER = {"id": 1, "kind": METHOD, "name": "get_user",
            "qname": "user_controller::UserController::get_user"}
HELPER = {"id": 3, "kind": FUNCTION, "name": "helper", "qname": "user_controller::helper"}
NODES = [GET_USER, EXACT_NODE, HELPER]
BY_ID = {n["id"]: n for n in NODES}
EDGES = [{"from": 2, "to": 3, "category": CALLS}]  # ONLY get_user_impl calls helper


def edge_hit(exp):
    return _recall_edge_hit("t", "expect_edges[0]", EDGES, BY_ID, CALLS,
                            {"category": "CALLS", **exp})


def node_hit(kind_id, exp):
    return _recall_node_hit("t", "expect_nodes[0]", NODES, kind_id, exp)


def raises(fn):
    try:
        fn()
    except ValueError:
        return True
    return False


GET_USER_EDGE = {"from": "get_user", "to": "helper"}
check("lenient recall over-matches get_user -> helper via get_user_impl (the bug)",
      edge_hit(GET_USER_EDGE))
check("exact recall does NOT satisfy get_user -> helper with get_user_impl -> helper",
      not edge_hit({**GET_USER_EDGE, "exact": True}))
check("exact recall finds the true edge by qname",
      edge_hit({"from": EXACT_NODE["qname"], "to": "user_controller::helper", "exact": True}))
check("exact recall finds the true edge by name",
      edge_hit({"from": "get_user_impl", "to": "helper", "exact": True}))
check("exact applies to the `to` end too",
      not edge_hit({"from": "get_user_impl", "to": "help", "exact": True}))
check("exact: false grades as lenient", edge_hit({**GET_USER_EDGE, "exact": False}))
for bad in ("yes", "true", 1):
    check(f"exact: {bad!r} raises ValueError",
          raises(lambda b=bad: edge_hit({**GET_USER_EDGE, "exact": b})))
check("exact: 'yes' raises on a node entry too",
      raises(lambda: node_hit(FUNCTION, {"name": "get_user", "exact": "yes"})))

check("lenient node recall over-matches FUNCTION get_user via get_user_impl",
      node_hit(FUNCTION, {"name": "get_user"}))
check("exact node recall rejects FUNCTION get_user",
      not node_hit(FUNCTION, {"name": "get_user", "exact": True}))
check("exact node recall finds EXACT_NODE by name and by qname",
      node_hit(FUNCTION, {"name": "get_user_impl", "exact": True})
      and node_hit(FUNCTION, {"name": EXACT_NODE["qname"], "exact": True}))
check("kind stays strict under exact",
      not node_hit(METHOD, {"name": "get_user_impl", "exact": True}))

# An entry without `exact` behaves exactly as the pre-CA.8 inline loops did.
for pat in ("get_user", "get_user_impl", "helper", "UserController", "nothing"):
    old_node = any(n["kind"] == FUNCTION and _node_matches(n, pat) for n in NODES)
    old_edge = any(_node_matches(BY_ID[e["from"]], pat) and _node_matches(BY_ID[e["to"]], "helper")
                   for e in EDGES)
    check(f"no `exact` == old lenient loop for {pat!r}",
          node_hit(FUNCTION, {"name": pat}) == old_node
          and edge_hit({"from": pat, "to": "helper"}) == old_edge)

check("`exact` is recall vocabulary only",
      "exact" in NODE_FIELDS and "exact" in EDGE_FIELDS
      and "exact" not in CELL_FIELDS and "exact" not in FORBID_NODE_FIELDS
      and "exact" not in FORBID_EDGE_FIELDS)

# End to end through grade_fixture: the real build, the fired_on marker, [exact].
with tempfile.TemporaryDirectory(prefix="ca8-exact-") as tmp:
    fx = Path(tmp) / "ca8-exact-e2e"
    fx.mkdir()
    (fx / "user_controller.py").write_text(
        "class UserController:\n    def get_user(self):\n        return 0\n\n\n"
        "def helper():\n    return 1\n\n\ndef get_user_impl():\n    return helper()\n")
    (fx / "key.json").write_text(json.dumps({
        "framework": "ca8-exact-e2e", "language": "python", "dirs": ["."],
        "expect_nodes": [{"kind": "FUNCTION", "name": "get_user_impl", "exact": True}],
        "expect_edges": [{**GET_USER_EDGE, "category": "CALLS"},
                         {**GET_USER_EDGE, "category": "CALLS", "exact": True}],
    }))
    err = io.StringIO()
    with contextlib.redirect_stderr(err):
        res = grade_fixture(fx)
    found = [r["found"] for r in res["edge_recall"]]
    check("grade_fixture: lenient row OK, exact row XX", found == [True, False])
    check("grade_fixture: exact node row found", res["node_recall"][0]["found"])
    check("grade_fixture: rows keep `exact`",
          res["edge_recall"][1].get("exact") is True and "exact" not in res["edge_recall"][0])
    check("grade_fixture prints the fired_on marker",
          "[substrate-gap] exact identity rows=2 in ca8-exact-e2e" in err.getvalue())
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        _print_verbose(res)
    check("an exact row prints [exact] after the category",
          "CALLS          [exact] 'get_user'" in out.getvalue())

# ---- FD.24: forbid edge rows filtered by EVIDENCE rule ---------------------
class _FakeWhy:
    """A graph stand-in: `why` answers one direct edge per (from, to) with a rule."""

    def __init__(self, rules):
        self.rules = rules  # (from_id, to_id) -> [rule, ..]
        self.calls = []

    def why(self, from_qname, to_qname, category):
        self.calls.append((from_qname, to_qname, category))
        return {"edges": [{"from_id": f, "to_id": t, "rule": r}
                          for (f, t), rs in sorted(self.rules.items()) for r in rs]}


GET = {"id": 1, "name": "Get", "qname": "web::fleet::Fleet::Get"}
PROBLEM = {"id": 2, "name": "Problem", "qname": "models::vehicle::Problem"}
USES = _CAT_BY_NAME["USES"]
FD24_NODES = [GET, PROBLEM]
FD24_EDGES = [{"from": 1, "to": 2, "category": USES}]
FD24_BY_ID = {1: GET, 2: PROBLEM}
RULE_ROW = {"from": "web::fleet::Fleet::Get", "to": "models::vehicle::Problem",
            "category": "USES", "rule": "json_response"}

check("FORBID_EDGE_FIELDS admits `rule`", "rule" in FORBID_EDGE_FIELDS)
fired = _grade_forbid("fd24", {"forbid": [RULE_ROW]}, FD24_NODES, FD24_EDGES, FD24_BY_ID,
                      _FakeWhy({(1, 2): ["json_response"]}))
check("a json_response USES edge to Problem fires the rule row",
      fired[0]["violated"] and fired[0]["matched"] == 1)
held = _grade_forbid("fd24", {"forbid": [RULE_ROW]}, FD24_NODES, FD24_EDGES, FD24_BY_ID,
                     _FakeWhy({(1, 2): ["composite_literal"]}))
check("a composite_literal USES edge to Problem does not fire it",
      not held[0]["violated"] and held[0]["matched"] == 0)
other = _grade_forbid("fd24", {"forbid": [RULE_ROW]}, FD24_NODES, FD24_EDGES, FD24_BY_ID,
                      _FakeWhy({(1, 3): ["json_response"]}))
check("a same-qname edge to another node id lends no rule", not other[0]["violated"])
plain = {k: v for k, v in RULE_ROW.items() if k != "rule"}
no_rule = _grade_forbid("fd24", {"forbid": [plain]}, FD24_NODES, FD24_EDGES, FD24_BY_ID)
check("a row without `rule` still fires on any matching edge, no graph needed",
      no_rule[0]["violated"])
for bad in ("", 1, None):
    try:
        _grade_forbid("fd24", {"forbid": [{**RULE_ROW, "rule": bad}]}, FD24_NODES,
                      FD24_EDGES, FD24_BY_ID, _FakeWhy({}))
        ok = False
    except ValueError:
        ok = True
    check(f"forbid rule={bad!r} is rejected (a rule is a non-empty string)", ok)
try:
    _grade_forbid("fd24", {"forbid": [RULE_ROW]}, FD24_NODES, FD24_EDGES, FD24_BY_ID)
    check("a rule row with no graph raises", False)
except ValueError:
    check("a rule row with no graph raises", True)
# End to end on models-go-wire's real build: the restored guard holds, and the
# same row with DT.5j's composite_literal rule fires on Get's real USES edge.
with tempfile.TemporaryDirectory(prefix="fd24-rule-") as tmp:
    fx = Path(tmp) / "models-go-wire"
    shutil.copytree(Path(__file__).parent / "fixtures" / "models-go-wire", fx)
    key = json.loads((fx / "key.json").read_text())
    check("models-go-wire key carries the json_response forbid row",
          any(r.get("rule") == "json_response" and r["to"] == "models::vehicle::Problem"
              for r in key["forbid"]))
    key["forbid"].append({**RULE_ROW, "rule": "composite_literal"})
    (fx / "key.json").write_text(json.dumps(key))
    with contextlib.redirect_stderr(io.StringIO()):
        res = grade_fixture(fx)
    by_rule = {r.get("rule"): r for r in res["forbid_results"] if r.get("rule")}
    check("models-go-wire: no json_response USES Get -> Problem",
          not by_rule["json_response"]["violated"])
    check("models-go-wire: the composite_literal USES Get -> Problem fires a rule row",
          by_rule["composite_literal"]["violated"] and by_rule["composite_literal"]["matched"] == 1)
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        _print_verbose(res)
    check("a rule row prints `forbid rule=<r>` in its label",
          "'models::vehicle::Problem' forbid rule=json_response  matched=0" in out.getvalue())

# ---- HQ.4: forbid edge rows filtered by the edge's site file ----------------
class _FakeWhySites:
    """A graph stand-in: `why` answers direct edges with a rule and a site file."""

    def __init__(self, rows):
        self.rows = rows  # (from_id, to_id) -> [(rule, file), ..]

    def why(self, from_qname, to_qname, category):
        return {"edges": [{"from_id": f, "to_id": t, "rule": r,
                           "site": None if fl is None else {"file": fl, "line": 8}}
                          for (f, t), rs in sorted(self.rows.items()) for r, fl in rs]}


PROC = {"id": 10, "name": "evm:IPriceOracle.latestPrice", "qname": "evm:IPriceOracle.latestPrice"}
VAULT = {"id": 11, "name": "latestPrice",
         "qname": "vault::src::IPriceOracle::IPriceOracle::latestPrice"}
HANDLED_BY = _CAT_BY_NAME["HANDLED_BY"]
HQ4_NODES = [PROC, VAULT]
HQ4_EDGES = [{"from": 10, "to": 11, "category": HANDLED_BY}]
HQ4_BY_ID = {10: PROC, 11: VAULT}
SITE_ROW = {"from": PROC["qname"], "to": VAULT["qname"], "category": "HANDLED_BY",
            "site": "lending/src/IPriceOracle.sol"}

check("FORBID_EDGE_FIELDS admits `site`", "site" in FORBID_EDGE_FIELDS)
wrong = _grade_forbid("hq4", {"forbid": [SITE_ROW]}, HQ4_NODES, HQ4_EDGES, HQ4_BY_ID,
                      _FakeWhySites({(10, 11): [(None, "lending/src/IPriceOracle.sol")]}))
check("a site row fires on the edge sited in that file",
      wrong[0]["violated"] and wrong[0]["matched"] == 1)
own = _grade_forbid("hq4", {"forbid": [SITE_ROW]}, HQ4_NODES, HQ4_EDGES, HQ4_BY_ID,
                    _FakeWhySites({(10, 11): [(None, "vault/src/IPriceOracle.sol")]}))
check("a site row does not fire on the same pair sited elsewhere",
      not own[0]["violated"] and own[0]["matched"] == 0)
unsited = _grade_forbid("hq4", {"forbid": [SITE_ROW]}, HQ4_NODES, HQ4_EDGES, HQ4_BY_ID,
                        _FakeWhySites({(10, 11): [(None, None)]}))
check("a site row does not fire on an edge with no site", not unsited[0]["violated"])
both = {**SITE_ROW, "rule": "abi_member"}
and_rule = _grade_forbid("hq4", {"forbid": [both]}, HQ4_NODES, HQ4_EDGES, HQ4_BY_ID,
                         _FakeWhySites({(10, 11): [("other", "lending/src/IPriceOracle.sol"),
                                                   ("abi_member", "vault/src/IPriceOracle.sol")]}))
check("rule and site combine with AND (each matches a different edge: no fire)",
      not and_rule[0]["violated"])
and_hit = _grade_forbid("hq4", {"forbid": [both]}, HQ4_NODES, HQ4_EDGES, HQ4_BY_ID,
                        _FakeWhySites({(10, 11): [("abi_member", "lending/src/IPriceOracle.sol")]}))
check("rule and site both matching one edge fire", and_hit[0]["violated"])
no_site = {k: v for k, v in SITE_ROW.items() if k != "site"}
plain_hit = _grade_forbid("hq4", {"forbid": [no_site]}, HQ4_NODES, HQ4_EDGES, HQ4_BY_ID)
check("a row without `site` grades as before (fires on any matching edge, no graph needed)",
      plain_hit[0]["violated"] and plain_hit[0]["matched"] == 1)
for bad in ("", 1, None, ["lending/src/IPriceOracle.sol"]):
    try:
        _grade_forbid("hq4", {"forbid": [{**SITE_ROW, "site": bad}]}, HQ4_NODES,
                      HQ4_EDGES, HQ4_BY_ID, _FakeWhySites({}))
        ok = False
    except ValueError:
        ok = True
    check(f"forbid site={bad!r} is rejected (a site is a non-empty string)", ok)
try:
    _grade_forbid("hq4", {"forbid": [SITE_ROW]}, HQ4_NODES, HQ4_EDGES, HQ4_BY_ID)
    check("a site row with no graph raises", False)
except ValueError:
    check("a site row with no graph raises", True)
out = io.StringIO()
with contextlib.redirect_stdout(out):
    _print_verbose({"framework": "hq4", "language": "solidity", "node_count": 2,
                    "edge_count": 1, "node_recall": [], "edge_recall": [],
                    "cell_recall": [], "forbid_results": [wrong[0]],
                    "forbid_violations": 1, "per_category": {}, "per_cell": {}})
check("a site row prints `forbid site=<file>` in its label",
      "forbid site=lending/src/IPriceOracle.sol  matched=1" in out.getvalue())
# End to end on solidity-twin-contracts' real build: each copy's HANDLED_BY is
# sited in its own file, so neither mirror forbid fires.
with contextlib.redirect_stderr(io.StringIO()):
    res = grade_fixture(Path(__file__).parent / "fixtures" / "solidity-twin-contracts")
site_rows = [r for r in res["forbid_results"] if r.get("site")]
check("solidity-twin-contracts carries two site rows", len(site_rows) == 2)
check("solidity-twin-contracts: no HANDLED_BY sited in the other copy's file",
      not any(r["violated"] for r in site_rows))

print(f"\n{len(FAILED)} failed" if FAILED else "\nall passed")
sys.exit(1 if FAILED else 0)
