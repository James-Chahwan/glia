#!/usr/bin/env python3
"""pyo3 surface, py/src/identity.rs (DD.15): `identity_classes(tier=None,
rule=None, diverged_only=False, limit=None)` returns the `glia identity`
report `{counts, classes, omitted, absence}` as a native dict, keys in engine
field order; an unknown tier or rule raises ValueError naming the valid
values. The fixtures are temp copies of DD.5's identity-npm-package (two FACT
package_name classes) and DD.6's identity-vendored-subtree (HEURISTIC
body_hash / path_suffix candidates, parse.ts diverged). Shared helpers:
test_build.py."""
from __future__ import annotations

import pathlib
import shutil
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

FIXTURES = pathlib.Path(__file__).resolve().parents[3] / "bench" / "substrate-gap" / "fixtures"
REPORT_KEYS = ["counts", "classes", "omitted", "absence"]
COUNT_KEYS = ["classes", "members", "fact", "derived", "heuristic", "diverged", "by_rule"]
CLASS_KEYS = ["key", "tier", "rules", "collapses", "walks", "diverged", "variants", "members"]
MEMBER_KEYS = ["repo", "qname", "kind", "file", "line", "variant"]
RULES = ["package_name", "manifest_name", "message_fqname", "contract_op", "contract_route",
         "scip_symbol", "body_hash", "path_suffix", "http_model", "graphql_model"]


def copy(fixture: str, top: pathlib.Path) -> pathlib.Path:
    dst = top / fixture
    shutil.copytree(FIXTURES / fixture, dst, ignore=shutil.ignore_patterns("key.json"))
    return dst


def keyed(report) -> dict:
    return {c["key"]: c for c in report.get("classes", [])}


def main() -> int:
    c = Checks("identity")
    c.check("identity_classes signature",
            params(rg.PyGraph.identity_classes)
            == [("tier", None), ("rule", None), ("diverged_only", False), ("limit", None)],
            params(rg.PyGraph.identity_classes))
    with tempfile.TemporaryDirectory(prefix="glia-surface-identity-") as tmp:
        top = pathlib.Path(tmp)
        npm = copy("identity-npm-package", top)
        g = rg.generate_many([str(npm / "web"), str(npm / "admin")])

        r, err = stderr_of(g.identity_classes)
        c.check("marker", "[identity] report classes=2 fact=2 derived=0 heuristic=0 diverged=0" in err,
                err[-400:])
        c.check("report -> dict in engine field order", type(r) is dict and list(r) == REPORT_KEYS, r)
        counts = r.get("counts", {})
        c.check("counts keys", list(counts) == COUNT_KEYS, counts)
        c.check("counts: two fact classes, four members",
                (counts.get("classes"), counts.get("members"), counts.get("fact"), counts.get("heuristic"))
                == (2, 4, 2, 0), counts)
        c.check("by_rule is [rule, n] lists, every rule in table order",
                [b[0] for b in counts.get("by_rule", [])] == RULES
                and counts["by_rule"][0] == ["package_name", 2], counts.get("by_rule"))
        classes = r.get("classes", [])
        c.check("classes keyed by the display qname",
                [k.get("key") for k in classes] == ["package:npm:lodash", "package:npm:react"], classes)
        first = classes[0] if classes else {}
        c.check("class keys in engine field order", list(first) == CLASS_KEYS, list(first))
        c.check("fact package_name classes collapse",
                all((k["tier"], k["rules"], k["collapses"], k["diverged"]) == ("fact", ["package_name"], True, False)
                    for k in classes), classes)
        members = first.get("members", [])
        c.check("member keys in engine field order", members and list(members[0]) == MEMBER_KEYS, members)
        c.check("members labelled by repo, display first",
                [m.get("repo") for m in members] == ["web", "admin"], members)
        c.check("absence None, nothing omitted", r.get("absence", 1) is None and r.get("omitted") == 0, r)

        one = g.identity_classes(limit=1)
        c.check("limit cuts classes, not counts",
                (len(one["classes"]), one["omitted"], one["counts"]["classes"]) == (1, 1, 2), one)
        c.check("limit=0 is every class", len(g.identity_classes(limit=0)["classes"]) == 2)
        c.check("rule filter", [k["key"] for k in g.identity_classes(rule="package_name")["classes"]]
                == ["package:npm:lodash", "package:npm:react"])
        none = g.identity_classes(tier="derived")
        c.check("filters removing every class -> absence",
                none["classes"] == [] and (none.get("absence") or {}).get("reason") == "no_match"
                and "filters removed 2 classes" in (none.get("absence") or {}).get("note", ""), none)
        c.raises("unknown rule -> ValueError naming the rules", ValueError,
                 lambda: g.identity_classes(rule="nope"), "path_suffix")
        c.raises("unknown tier -> ValueError", ValueError,
                 lambda: g.identity_classes(tier="maybe"), "fact, derived or heuristic")
        c.check("deterministic", g.identity_classes() == g.identity_classes())

        alone = rg.generate(str(npm / "web")).identity_classes()
        c.check("one repo, no SAME_AS -> absence",
                alone["classes"] == [] and "no SAME_AS edge" in (alone.get("absence") or {}).get("note", ""),
                alone)

        vend = copy("identity-vendored-subtree", top)
        v = rg.generate_many([str(vend / "money"), str(vend / "shop")])
        h = keyed(v.identity_classes(tier="heuristic"))
        fmt = h.get("src::format::formatCents", {})
        c.check("formatCents: body_hash candidate, never collapsed",
                (fmt.get("tier"), fmt.get("rules"), fmt.get("collapses"), fmt.get("walks"), fmt.get("variants"))
                == ("heuristic", ["body_hash"], False, False, 1), fmt)
        parse = h.get("src::parse::parseCents", {})
        c.check("parseCents: diverged path_suffix candidate, 2 variants",
                (parse.get("rules"), parse.get("diverged"), parse.get("variants"))
                == (["path_suffix"], True, 2), parse)
        c.check("parseCents members located with their variants",
                [(m["repo"], m["file"], m["line"], m["variant"]) for m in parse.get("members", [])]
                == [("money", "src/parse.ts", 1, 0), ("shop", "third_party/money/src/parse.ts", 1, 1)], parse)
        d = v.identity_classes(diverged_only=True)
        c.check("diverged_only keeps only diverged classes",
                d["classes"] and all(k["diverged"] for k in d["classes"])
                and "src::format::formatCents" not in keyed(d), d)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
