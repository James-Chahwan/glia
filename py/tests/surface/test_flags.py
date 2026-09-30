#!/usr/bin/env python3
"""pyo3 surface, py/src/flags.rs (CC.7c): `flags(quiet_days=90, scope=None)`
returns the stale feature-flag report `{flags, definitions_in_graph,
quiet_evaluated, history_now, quiet_days, counts, absence}` as a native dict.
The tree is CC.7b's acceptance tree; its history is real, a git repo synced
with `history_sync(..., blame=True)` before the build. Shared helpers:
test_build.py."""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

from test_build import Checks, params, rg, stderr_of

T0, DAY = 1_767_225_600, 86_400  # 2026-01-01T00:00:00Z
IDENTITY = {
    "GIT_AUTHOR_NAME": "Flag Fixture",
    "GIT_AUTHOR_EMAIL": "flag.fixture@identity.invalid",
    "GIT_COMMITTER_NAME": "Flag Fixture",
    "GIT_COMMITTER_EMAIL": "flag.fixture@identity.invalid",
}

# service/checkout.py reads `new-checkout` in `checkout` and `promo-banner` in
# `banner`, service/promo.py reads `promo-banner` in `show`, and
# flipt/features.yaml defines `new-checkout` and `legacy-search`: legacy-search
# is dead, promo-banner undefined, new-checkout single-site.
CHECKOUT_PY = ('import ldclient\n\nclient = ldclient.get()\n\n\ndef checkout(user):\n'
               '    if client.variation("new-checkout", user, False):\n        return 1\n    return 0\n\n\n'
               'def banner(user):\n    return client.variation("promo-banner", user, False)\n')
PROMO_PY = ('import ldclient\n\nclient = ldclient.get()\n\n\ndef show(user):\n'
            '    return client.variation("promo-banner", user, False)\n')
FEATURES_YAML = ("namespace: default\nflags:\n  - key: new-checkout\n    name: New checkout\n"
                 "  - key: legacy-search\n    name: Legacy search\n")
FILES = {"service/checkout.py": CHECKOUT_PY, "service/promo.py": PROMO_PY, "flipt/features.yaml": FEATURES_YAML}

REPORT_KEYS = ["flags", "definitions_in_graph", "quiet_evaluated", "history_now", "quiet_days", "counts",
               "absence"]
ROW_KEYS = ["key", "providers", "definitions", "reads", "readers", "last_read_change", "findings"]
SITE_KEYS = ["qname", "kind", "file", "line"]
FINDING_KEYS = ["status", "tier", "note"]
DEFINED = {"qname": "flipt::features.yaml", "kind": "MODULE", "file": "flipt/features.yaml", "line": None}


def git(top: pathlib.Path, *args: str, t: int = T0) -> None:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    env.update(IDENTITY, GIT_AUTHOR_DATE=f"@{t} +0000", GIT_COMMITTER_DATE=f"@{t} +0000")
    subprocess.run(["git", "-C", str(top), *args], env=env, check=True, capture_output=True, text=True)


def write_tree(top: pathlib.Path) -> None:
    for rel, src in FILES.items():
        (top / rel).parent.mkdir(parents=True, exist_ok=True)
        (top / rel).write_text(src)


def history_repo(top: pathlib.Path) -> None:
    """The tree committed at T0, then promo.py's read line edited 200 days
    later: new-checkout's one reader is 200 days older than the snapshot's
    newest change, promo-banner's `show` is not."""
    top.mkdir(parents=True)
    git(top, "init", "-q", "-b", "main")
    write_tree(top)
    git(top, "add", "-A")
    git(top, "commit", "-q", "-m", "one")
    promo = top / "service/promo.py"
    promo.write_text(PROMO_PY.replace('user, False)', 'user, None)'))
    git(top, "add", "-A", t=T0 + 200 * DAY)
    git(top, "commit", "-q", "-m", "two", t=T0 + 200 * DAY)


def keys_of(report: dict) -> list:
    return [f.get("key") for f in report.get("flags", [])]


def statuses(row: dict) -> list:
    return [(f.get("status"), f.get("tier")) for f in row.get("findings", [])]


def main() -> int:
    c = Checks("flags")
    c.check("flags signature", params(rg.PyGraph.flags) == [("quiet_days", 90), ("scope", None)],
            params(rg.PyGraph.flags))
    with tempfile.TemporaryDirectory(prefix="glia-surface-flags-") as tmp:
        bare = pathlib.Path(tmp) / "bare"
        write_tree(bare)
        g = rg.generate(str(bare))

        r, err = stderr_of(g.flags)
        c.check("marker", "[flags] keys=3 defined_files=1 dead=1 undefined=1 single_site=1 quiet=0 "
                "quiet_evaluated=false" in err, err[-400:])
        c.check("flags -> dict in engine field order", type(r) is dict and list(r) == REPORT_KEYS, r)
        c.check("flags[0] is legacy-search", r["flags"][0]["key"] == "legacy-search", keys_of(r))
        c.check("rows: findings first, then by key", keys_of(r) == ["legacy-search", "new-checkout", "promo-banner"],
                keys_of(r))
        legacy, checkout, promo = r["flags"]
        c.check("row keys in engine field order", list(legacy) == ROW_KEYS, list(legacy))
        c.check("site keys in engine field order", list(checkout["reads"][0]) == SITE_KEYS, checkout["reads"])
        c.check("finding keys in engine field order", list(legacy["findings"][0]) == FINDING_KEYS,
                legacy["findings"])
        c.check("dead", statuses(legacy) == [("dead", "derived")] and legacy["definitions"] == [DEFINED]
                and legacy["reads"] == [] and legacy["readers"] == 0 and legacy["providers"] == ["flipt"], legacy)
        c.check("single_site, read at the call's line (int)",
                statuses(checkout) == [("single_site", "fact")]
                and checkout["reads"] == [{"qname": "service::checkout::checkout", "kind": "FUNCTION",
                                           "file": "service/checkout.py", "line": 7}]
                and checkout["providers"] == ["flipt", "launchdarkly"], checkout)
        c.check("undefined, two readers", statuses(promo) == [("undefined", "derived")] and promo["readers"] == 2
                and [(s["file"], s["line"]) for s in promo["reads"]]
                == [("service/checkout.py", 13), ("service/promo.py", 7)] and promo["definitions"] == [], promo)
        c.check("report-wide fields", (r["definitions_in_graph"], r["quiet_evaluated"], r["history_now"],
                                       r["quiet_days"], r["absence"]) == (1, False, None, 90, None), r)
        c.check("counts hold every status", r["counts"] == {"dead": 1, "quiet": 0, "single_site": 1, "undefined": 1},
                r["counts"])
        c.check("last_read_change None without history",
                all(f["last_read_change"] is None for f in r["flags"]), r["flags"])

        c.check("quiet_days echoed", g.flags(quiet_days=30)["quiet_days"] == 30)
        c.check("scope keeps a flag with a site under it, whole",
                keys_of(g.flags(scope="service/promo.py")) == ["promo-banner"]
                and g.flags(scope="service/promo.py")["flags"][0]["readers"] == 2)
        c.check("scope by position", keys_of(g.flags(90, "flipt")) == ["legacy-search", "new-checkout"])
        none = g.flags(scope="docs")
        c.check("an emptied scope is an absence", none["flags"] == []
                and (none.get("absence") or {}).get("reason") == "no_match", none)

        top = pathlib.Path(tmp) / "shop"
        history_repo(top)
        rg.history_sync(str(top), blame=True)
        h = rg.generate(str(top))
        r, err = stderr_of(h.flags)
        later = T0 + 200 * DAY
        c.check("history marker", "[flags] keys=3 defined_files=1 dead=1 undefined=1 single_site=1 quiet=1 "
                "quiet_evaluated=true" in err, err[-400:])
        c.check("history_now is int unix seconds", r["history_now"] == later and r["quiet_evaluated"] is True, r)
        rows = {f["key"]: f for f in r["flags"]}
        checkout, promo = rows.get("new-checkout", {}), rows.get("promo-banner", {})
        c.check("quiet new-checkout", statuses(checkout) == [("single_site", "fact"), ("quiet", "heuristic")]
                and checkout["last_read_change"] == T0, checkout)
        c.check("quiet note", checkout["findings"][1]["note"]
                == "no reading line changed in 200 days before the snapshot's newest change "
                   "(git blame, not runtime use)", checkout["findings"])
        c.check("promo-banner not quiet", statuses(promo) == [("undefined", "derived")]
                and promo["last_read_change"] == later, promo)
        c.check("dead flags are never quiet", statuses(rows.get("legacy-search", {})) == [("dead", "derived")])
        past = h.flags(quiet_days=250)
        c.check("quiet_days=250: evaluated, none quiet", past["counts"]["quiet"] == 0 and past["quiet_evaluated"]
                and past["quiet_days"] == 250, past["counts"])
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
