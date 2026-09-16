#!/usr/bin/env python3
"""Regression tests for grade.py's identity matchers.

Wave 2 shipped a correct java route composition and the harness accused it
anyway: `forbid {to: "UserController"}` matched the METHOD node whose qname is
`UserController::UserController::getUser`, because the precision gate had
inherited the recall gate's substring matcher. Leniency can only turn a recall
miss into a hit, but it turns a precision gate into a false accusation.

These tests pin both directions: the strict matcher must not over-match, and it
must still catch a real violation.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from grade import _node_matches, _node_matches_exact  # noqa: E402

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

print(f"\n{len(FAILED)} failed" if FAILED else "\nall passed")
sys.exit(1 if FAILED else 0)
