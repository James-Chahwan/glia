#!/usr/bin/env python3
"""pyo3 surface, py/src/review.rs (CC.6b): the module function
`review_vs_rev(repo_path, base="HEAD", format="dict", markdown_rows=20,
depth=4, max_tests=25, max_impact=25, models_min_tier="fact", ranking=None,
cochange=True, title=None, full=False, offset=0)` builds the git
rev `base` and the working tree of the repo at `repo_path` and returns the
engine's review as a native dict {base, counts, changed, impact, tests, edges,
new_violations, resolved_violations, model_breaks, check_errors, blocking}
(`blocking` is True only for a blocking violation the base did not have, or a
model break (DT.1p) whose pairing is at or above `models_min_tier`: by
default none in 0.5.2), or with format="markdown" the markdown PR report
`glia review` prints, as a str. Any other format or tier, or an engine error,
raises ValueError. EB.7: `intent` (the PR title's routed ids and the files
ranked LX[IDR,P1]) is a key only with a `title`, or a commit in base..HEAD
(its newest subject is the default title); EC.11: `selection_intent`
follows it (set SELt2's calibrated pick, EC.12, else LX[IDR,P1]). FB.12b:
`conventions` (after check_errors) lists the conventions governing the
changed files, only when one does. FH.8b: the dict is compact by default
(impact seeds capped at 25 since FH.8c, a seed's identity also_in past three members
folded to three plus also_in_count, a changed row that repeats a shown seed
reduced to {id, change}); full=True lifts every cap (max_impact and
max_tests included) and writes every row whole; one `[answer-size]
surface=review ..` line per dict. FH.8c: max_impact defaults to 25, a cut
impact carries `areas` and `truncated.next`, its rows are slim (`seed_index`),
and `offset=` skips impact rows (refused with full=True). FH.8d: max_tests
defaults to 25 (it was 50), name_breaks keeps 25 rows (folded rows first,
`name_breaks_truncated` counting the rest; full=True keeps all), and the
answer-size line adds names= / names_shown= / tests_shown= before bytes=.
Needs a `git` binary. Shared
helpers: test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

MANIFESTS = {
    "web/pyproject.toml": "[project]\nname = \"web\"\n",
    "services/api/pyproject.toml": "[project]\nname = \"api\"\n",
}
INTERNAL = "def charge(o):\n    return o\n"
INTERNAL_EDITED = "def charge(o):\n    return o or 0\n"
WEB_APP_CLEAN = "def pay(o):\n    return o\n"
WEB_APP = "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n"
TEST_PAY = "from web.app import pay\n\n\ndef test_pay():\n    assert pay(1) == 1\n"
RULES = ("version = 1\n\n[[constraint]]\nid = \"web-no-api-internals\"\nkind = \"forbid_edge\"\n"
         "from = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n")
CONVENTION = ("\n[[convention]]\nid = \"api-pure\"\nscope = \"services/api\"\n"
              "text = \"api internals stay pure\"\n")
KEYS = ["base", "counts", "changed", "impact", "tests", "edges", "new_violations",
        "resolved_violations", "model_breaks", "check_errors", "blocking"]
KW = [("repo_path", None), ("base", "HEAD"), ("format", "dict"), ("markdown_rows", 20), ("depth", 4),
      ("max_tests", 25), ("max_impact", 25), ("models_min_tier", "fact"), ("ranking", None), ("cochange", True), ("title", None),
      ("full", False), ("offset", 0)]
# DT.1p: the engine's model-breaks-shop fixture (py/tests/surface -> the repo root).
MODEL_SHOP = pathlib.Path(__file__).resolve().parents[3] / "engine" / "tests" / "data" / "model-breaks-shop"
TAG = '`json:"customerName"`'
RENAMED = '`json:"customer_name"`'
ADVISORY = "### Model breaks (advisory: an inferred client <-> server pairing below the models min tier)"
MARKER_TAIL = " edges +2 -0 (fact=2 derived=0 heuristic=0) violations new=1 resolved=0 blocking=true"
# HC.4: review's pyo3 markdown always carries the measured precision beside the tier.
ROW = "| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | fact (>= 0.91 on IMPORTS\\|fact, n=41) |"


def git(top: pathlib.Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    subprocess.run(["git", "-c", "user.name=glia", "-c", "user.email=glia@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", "-C", str(top), *args],
                   env=env, check=True, capture_output=True, text=True)


def committed(top: pathlib.Path, web_app: str) -> None:
    """CC.6a's tree with `web_app` as web/app.py, committed on main."""
    files = {**MANIFESTS, "services/api/internal.py": INTERNAL, "web/app.py": web_app,
             "tests/test_pay.py": TEST_PAY, ".glia/overlay.toml": RULES}
    for rel, text in files.items():
        p = top / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    git(top, "init", "-q")
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "clean")


def main() -> int:
    c = Checks("review")
    fn = getattr(rg, "review_vs_rev", None)
    c.check("review_vs_rev exists", fn is not None)
    if fn is None:
        return c.done()
    c.check("review_vs_rev params", params(fn) == KW, params(fn))

    with tempfile.TemporaryDirectory(prefix="glia-surface-review-") as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        gitconfig.write_text("")
        # The Rust side spawns git with this process's environment.
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"

        top = pathlib.Path(tmp) / "shop"
        committed(top, WEB_APP_CLEAN)
        (top / "web/app.py").write_text(WEB_APP)
        d, err = stderr_of(lambda: rg.review_vs_rev(str(top)))
        c.check("-> dict in engine field order", type(d) is dict and list(d) == KEYS, type(d) is dict and list(d))
        d = d if type(d) is dict else {}
        c.check("base as given", d.get("base") == "HEAD", d.get("base"))
        c.check("a new violation blocks", d.get("blocking") is True, d.get("blocking"))
        v = (d.get("new_violations") or [{}])[0]
        c.check("the new violation",
                (v.get("rule_id"), v.get("rule_kind"), v.get("decl"), v.get("count"))
                == ("web-no-api-internals", "forbid_edge", ".glia/overlay.toml:3", 2), v)
        rows = [(e.get("category"), e.get("file"), e.get("line")) for e in v.get("evidence", [])]
        c.check("its rows, 1-based", rows == [("IMPORTS", "web/app.py", 1), ("CALLS", "web/app.py", 5)], rows)
        c.check("nothing resolved", d.get("resolved_violations") == [], d.get("resolved_violations"))
        edges = [(e.get("change"), e.get("category"), e.get("tier")) for e in d.get("edges", [])]
        c.check("edge rows, facts first", edges == [("added", "CALLS", "fact"), ("added", "IMPORTS", "fact")],
                edges)
        counts = d.get("counts") or {}
        c.check("counts", (counts.get("new_violations"), counts.get("edges_added"), counts.get("tests"))
                == (1, 2, 1), counts)
        marker = [line for line in err.splitlines() if line.startswith("[review] base=")]
        c.check("engine marker", len(marker) == 1 and marker[0].endswith(MARKER_TAIL), err[-600:])
        c.check("surface marker", "[review] surface=py format=dict" in err, err[-400:])

        md, err = stderr_of(lambda: rg.review_vs_rev(str(top), format="markdown"))
        c.check("markdown is a str", type(md) is str, type(md))
        md = md if type(md) is str else ""
        c.check("markdown headline", md.startswith("## glia review vs `HEAD`\n**1 new violation(s)**"), md[:200])
        c.check("markdown blocking section", "### New violations (blocking)" in md, md)
        c.check("markdown violation row", ROW in md, md)
        c.check("markdown surface marker", "[review] surface=py format=markdown" in err, err[-400:])
        cut = rg.review_vs_rev(str(top), format="markdown", markdown_rows=1)
        c.check("markdown_rows cuts the tables",
                [line for line in cut.splitlines() if line.startswith("_(")] == ["_(1 of 2)_", "_(1 of 2)_",
                                                                                 "_(1 of 3)_"], cut)
        capped = rg.review_vs_rev(str(top), max_tests=0, max_impact=0, depth=1)
        c.check("max_tests / max_impact cut the rows, not the counts",
                capped["tests"]["tests"] == [] and capped["impact"]["results"] == []
                and capped["counts"] == d.get("counts"), capped.get("counts"))
        areas = capped["impact"].get("areas") or []
        c.check("FH.8c: a cut impact maps every impact row by area",
                sum(a.get("rows", 0) for a in areas) == capped["counts"]["impact"]
                and (capped["impact"].get("truncated") or {}).get("next", {}).get("offset") == 0, areas)
        c.raises("FH.8c: offset with full raises before any build", ValueError,
                 lambda: rg.review_vs_rev(str(pathlib.Path(tmp) / "nowhere"), offset=1, full=True),
                 "full keeps every row")

        c.check("FH.8b: markdown prints no answer-size line", "[answer-size]" not in err, err[-400:])
        _, err = stderr_of(lambda: rg.review_vs_rev(str(top)))
        size = [line for line in err.splitlines() if line.startswith("[answer-size] surface=review ")]
        c.check("FH.8b: one answer-size line per dict, seeds counted",
                len(size) == 1 and " seeds=" in size[0] and " also_in_folded=" in size[0], err[-600:])
        tests_shown = len(d.get("tests", {}).get("tests", []))
        c.check("FH.8d: the answer-size line counts names and shown tests",
                len(size) == 1 and f" names=0 names_shown=0 tests_shown={tests_shown} bytes=" in size[0],
                err[-600:])
        c.check("FH.8d: no name row, no name_breaks_truncated", "name_breaks_truncated" not in d, list(d))
        seed_ids = {s.get("id") for s in d.get("impact", {}).get("seeds", [])}
        c.check("FH.8b: by default a changed row that repeats a shown seed is {id, change}",
                seed_ids != set()
                and all(sorted(r) == ["change", "id"] for r in d.get("changed", []) if r.get("id") in seed_ids),
                d.get("changed"))
        whole, err = stderr_of(lambda: rg.review_vs_rev(str(top), max_tests=0, max_impact=0, full=True))
        c.check("FH.8b: full=True lifts max_impact / max_tests",
                len(whole["impact"]["results"]) == whole["counts"]["impact"]
                and len(whole["tests"]["tests"]) == whole["counts"]["tests"]
                and "truncated" not in whole["impact"] and "areas" not in whole["impact"],
                whole.get("counts"))
        c.check("FH.8b: full=True writes every changed row whole",
                whole["changed"] != [] and all("qname" in r for r in whole["changed"]), whole.get("changed"))
        c.check("FH.8b: full=True keeps every seed",
                len(whole["impact"]["seeds"]) == whole["counts"]["seeds"], whole["impact"].get("seeds"))
        c.check("FH.8b: full=True still marks the dict", "[answer-size] surface=review " in err, err[-400:])

        c.raises("format=xml raises", ValueError, lambda: rg.review_vs_rev(str(top), format="xml"),
                 "unknown format `xml`: expected one of dict, markdown")
        c.raises("unknown rev raises", ValueError, lambda: rg.review_vs_rev(str(top), base="no-such-rev"),
                 "no-such-rev")
        c.raises("bad models_min_tier raises before any build", ValueError,
                 lambda: rg.review_vs_rev(str(pathlib.Path(tmp) / "nowhere"), models_min_tier="x"),
                 "unknown models min tier `x`: expected one of fact, derived")
        c.raises("bad ranking raises before any build", ValueError,
                 lambda: rg.review_vs_rev(str(pathlib.Path(tmp) / "nowhere"), ranking="x"), "unknown ranking `x`: expected one of ppr, ppr-wide, bfs")

        # EB.7: a title adds `intent` (last key); its spec-kit id routes at the base
        tasks = top / "specs" / "001-pay" / "tasks.md"
        tasks.parent.mkdir(parents=True, exist_ok=True)
        tasks.write_text("# Tasks\n\n- [ ] T007 cover the payment in tests/test_pay.py\n")
        git(top, "add", "specs")
        git(top, "commit", "-q", "-m", "tasks")
        t, err = stderr_of(lambda: rg.review_vs_rev(str(top), title="T007 payment"))
        t = t if type(t) is dict else {}
        c.check("title -> intent then selection_intent last", list(t) == KEYS + ["intent", "selection_intent"],
                list(t))
        sel = t.get("selection_intent") or {}
        c.check("EC.11: uncalibrated selection_intent",
                (sel.get("set"), sel.get("pick"), sel.get("source"), sel.get("reason"))
                == ("SELt2", "LX[IDR,P1]", "fallback", "absent"), sel)
        intent = t.get("intent") or {}
        c.check("intent fields", (intent.get("title"), intent.get("title_source"), intent.get("fusion"),
                                  intent.get("text")) == ("T007 payment", "arg", "LX[IDR,P1]", "absent"), intent)
        files = intent.get("files") or [{}]
        c.check("T007 routes to its file", (files[0].get("file"), files[0].get("idr"), files[0].get("ids"))
                == ("tests/test_pay.py", 1.0, ["T007"]), files)
        c.check("[review-intent] marker",
                "[review-intent] base=HEAD title_source=arg fusion=LX[IDR,P1] ids=1 idr_files=1 " in err, err[-800:])
        c.check("no title, empty base..HEAD: no intent", "intent" not in rg.review_vs_rev(str(top)),
                "intent present")
        md = rg.review_vs_rev(str(top), format="markdown", title="T007 payment")
        c.check("markdown: ### Intent", "### Intent\n\n_title (arg): T007 payment_" in md, md[-800:])

        # FB.12b: a change under a [[convention]]'s scope adds `conventions`
        # (after check_errors), never blocking, and `### Conventions`.
        conv = pathlib.Path(tmp) / "conv"
        committed(conv, WEB_APP)
        (conv / ".glia/overlay.toml").write_text(RULES + CONVENTION)
        git(conv, "commit", "-q", "-am", "convention")
        (conv / "services/api/internal.py").write_text(INTERNAL_EDITED)
        cd, err = stderr_of(lambda: rg.review_vs_rev(str(conv)))
        c.check("conventions key after check_errors",
                list(cd) == KEYS[:10] + ["conventions"] + KEYS[10:], list(cd))
        rows = [(x.get("id"), x.get("tier"), x.get("files")) for x in cd.get("conventions", [])]
        c.check("the convention row", rows == [("api-pure", "declared", ["services/api/internal.py"])], rows)
        c.check("a convention never blocks", cd.get("blocking") is False, cd.get("blocking"))
        c.check("[review] conventions marker", "[review] conventions=1" in err, err[-600:])
        md = rg.review_vs_rev(str(conv), format="markdown")
        c.check("markdown: ### Conventions",
                "| api internals stay pure | `services/api` | 1 | overlay/api-pure (declared) at .glia/overlay.toml:10 |"
                in md, md[-800:])

        old = pathlib.Path(tmp) / "old"
        committed(old, WEB_APP)
        (old / "services/api/internal.py").write_text(INTERNAL_EDITED)
        pre = rg.review_vs_rev(str(old))
        c.check("a pre-existing violation does not block",
                pre.get("blocking") is False and pre.get("new_violations") == []
                and pre.get("resolved_violations") == [], pre)

        plain = pathlib.Path(tmp) / "plain"
        plain.mkdir()
        c.raises("not a git work tree raises", ValueError, lambda: rg.review_vs_rev(str(plain)),
                 "not a git work tree")

        # DT.1p: the model-breaks-shop fixture, v1 then the json tag rename as v2.
        shop = pathlib.Path(tmp) / "model-shop"
        for src in sorted(MODEL_SHOP.rglob("*")):
            if src.is_file():
                dst = shop / src.relative_to(MODEL_SHOP)
                dst.parent.mkdir(parents=True, exist_ok=True)
                dst.write_text(src.read_text())
        git(shop, "init", "-q")
        git(shop, "add", "-A")
        git(shop, "commit", "-q", "-m", "v1")
        order = shop / "api" / "internal" / "models" / "order.go"
        order.write_text(order.read_text().replace(TAG, RENAMED, 1))
        git(shop, "commit", "-q", "-am", "v2")
        r = rg.review_vs_rev(str(shop), base="HEAD~1")
        breaks = r.get("model_breaks") or [{}]
        c.check("a commit in base..HEAD is the default title",
                ((r.get("intent") or {}).get("title"), (r.get("intent") or {}).get("title_source"))
                == ("v2", "commit"), r.get("intent"))
        c.check("an inferred model break is listed and advisory",
                (r.get("blocking"), len(r.get("model_breaks") or []), breaks[0].get("pairing"),
                 (r.get("counts") or {}).get("model_breaks"))
                == (False, 1, "http_model", 1), r.get("model_breaks"))
        md = rg.review_vs_rev(str(shop), base="HEAD~1", format="markdown")
        c.check("markdown: the advisory model section", ADVISORY in md, md[:800])
        d = rg.review_vs_rev(str(shop), base="HEAD~1", models_min_tier="derived")
        c.check("models_min_tier='derived' makes it block", d.get("blocking") is True, d.get("model_breaks"))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
