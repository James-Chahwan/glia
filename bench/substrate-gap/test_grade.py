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
"""
import contextlib
import io
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from grade import (  # noqa: E402
    _CAT_BY_NAME, _KIND_BY_NAME, CELL_FIELDS, EDGE_FIELDS, FORBID_EDGE_FIELDS,
    FORBID_NODE_FIELDS, NODE_FIELDS, _node_matches, _node_matches_exact,
    _print_verbose, _recall_edge_hit, _recall_node_hit, grade_fixture,
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

print(f"\n{len(FAILED)} failed" if FAILED else "\nall passed")
sys.exit(1 if FAILED else 0)
