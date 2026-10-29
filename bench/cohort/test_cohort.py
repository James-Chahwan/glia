#!/usr/bin/env python3
"""Hermetic tests for cohort.py (FH.1a): two tiny scripted git repos, an app with a server
and a client side and a library with neither, no network, no gh, no glia.

    python3 bench/cohort/test_cohort.py
"""
import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import cohort as C  # noqa: E402

GIT_ENV = {"GIT_AUTHOR_NAME": "dev", "GIT_AUTHOR_EMAIL": "dev@example.invalid",
           "GIT_COMMITTER_NAME": "dev", "GIT_COMMITTER_EMAIL": "dev@example.invalid",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
ISSUE = {"number": 12, "title": "Widget count overflows past a thousand",
         "body": "Steps: open the dashboard with many widgets."}


def git(cwd, *args, when=None):
    env = {**os.environ, **GIT_ENV}
    if when is not None:
        env.update(GIT_AUTHOR_DATE=f"@{when} +0000", GIT_COMMITTER_DATE=f"@{when} +0000")
    p = subprocess.run(["git", *args], cwd=cwd, env=env, capture_output=True, text=True)
    if p.returncode:
        raise RuntimeError(f"git {args}: {p.stderr}")
    return p.stdout.strip()


def commit(work, files, message, when, delete=()):
    for path, text in files.items():
        f = Path(work) / path
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(text)
    for path in delete:
        (Path(work) / path).unlink()
    git(work, "add", "-A")
    git(work, "commit", "-q", "-m", message, when=when)
    return git(work, "rev-parse", "HEAD")


def lines(prefix, n):
    return "".join(f"{prefix} line {i}\n" for i in range(n))


def make_app(root):
    """An app: server/ (Go) and web/src/ (TS) with a generated SDK under web/src/gen/."""
    work = Path(root) / "app"
    work.mkdir()
    git(work, "init", "-q", "-b", "main")
    shas = {}
    shas["init"] = commit(work, {
        "server/api.go": "package server\n\nfunc ListWidgets() []Widget {\n\treturn nil\n}\n"
                         + lines("// api", 6),
        "server/handler.go": "package server\n\nfunc Handle() {\n\tListWidgets()\n}\n",
        "server/total.go": "package server\n\nfunc computeTotal(a int) int {\n\treturn a\n}\n",
        "server/a.go": "package server\n\nfunc A() int { return computeTotal(1) }\n",
        "server/b.go": "package server\n\nfunc B() int { return computeTotal(2) }\n",
        "web/src/client.ts": "export function listWidgets() {\n  return fetch('/widgets');\n}\n"
                             + lines("// client", 6),
        "web/src/view.ts": "import { listWidgets } from './client';\nlistWidgets();\n",
        "web/src/gen/api.ts": "// generated\nexport const WIDGETS = '/widgets';\n",
        "README.md": "app\n"}, "initial import of the app", 1_000)
    shas["drift"] = commit(work, {
        "server/api.go": "package server\n\nfunc ListWidgets(page int) []Widget {\n\treturn nil\n}\n"
                         + lines("// api v2", 6),
        "server/handler.go": "package server\n\nfunc Handle() {\n\tListWidgets(1)\n}\n",
        "web/src/gen/api.ts": "// generated\nexport const WIDGETS = '/v2/widgets';\n",
        "web/src/client.ts": "export function listWidgets(page: number) {\n"
                             "  return fetch('/v2/widgets?page=' + page);\n}\n"
                             + lines("// client v2", 6),
        "web/src/pager.ts": "export const FIRST_PAGE = 1;\n"},
        "feat(api): page the widget list endpoint", 2_000)
    shas["ripple"] = commit(work, {
        "server/total.go": "package server\n\nfunc computeSum(a int) int {\n\treturn a + 0\n}\n",
        "server/a.go": "package server\n\nfunc A() int { return computeSum(1) }\n",
        "server/b.go": "package server\n\nfunc B() int { return computeSum(2) }\n"},
        "refactor: rename the total helper everywhere", 3_000)
    shas["symptom"] = commit(work, {
        "server/tally.go": "package server\n\nfunc Count(n int64) int64 {\n\treturn n\n}\n",
        "server/handler.go": "package server\n\nfunc Handle() {\n\tListWidgets(1)\n\tCount(2)\n}\n"},
        "fix: use a wider integer for counting widgets\n\nFixes #12", 4_000)
    shas["docs"] = commit(work, {"README.md": "app, documented\n"},
                          "docs: describe the app in the readme", 5_000)
    bare = Path(root) / "cohort" / "app.git"
    git(root, "clone", "-q", "--bare", str(work), str(bare))
    return bare, shas


def make_lib(root):
    """A library: one package, no boundary sides; a ripple and a symptom."""
    work = Path(root) / "lib"
    work.mkdir()
    git(work, "init", "-q", "-b", "main")
    shas = {"init": commit(work, {
        "pkg/core.py": "def parse_value(x):\n    return x\n",
        "pkg/one.py": "from pkg.core import parse_value\nparse_value(1)\n",
        "pkg/two.py": "from pkg.core import parse_value\nparse_value(2)\n"},
        "initial import of the library", 1_000)}
    shas["ripple"] = commit(work, {
        "pkg/core.py": "def parse_number(x, base=10):\n    return int(x, base)\n",
        "pkg/one.py": "from pkg.core import parse_number\nparse_number('1')\n",
        "pkg/two.py": "from pkg.core import parse_number\nparse_number('2')\n"},
        "refactor: parse numbers with an explicit base", 2_000)
    bare = Path(root) / "cohort" / "lib.git"
    git(root, "clone", "-q", "--bare", str(work), str(bare))
    return bare, shas


def repo_row(label, sha, role, sides):
    return {"label": label, "url": f"https://github.com/example/{label}.git", "sha": sha,
            "lang": "polyglot", "role": role, "sides": sides, "shape": "test", "public": True}


APP_SIDES = {"server": ["server/"], "client": ["web/src/"], "client_names": "web app"}


def fake_fetch(calls):
    def fetch(url, refs):
        calls.append((url, tuple(refs)))
        return (dict(ISSUE), "ok") if 12 in refs else (None, "none")
    return fetch


class CohortTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="cohort-test-")
        self.env = {k: os.environ.get(k) for k in ("GLIA_COHORT_DIR", "GLIA_COHORT_OFFLINE",
                                                    "GLIA_AGENT_AB_DIR")}
        os.environ["GLIA_COHORT_DIR"] = str(Path(self.tmp) / "cohort")
        os.environ["GLIA_AGENT_AB_DIR"] = str(Path(self.tmp) / "no-agent-ab")
        os.environ["GLIA_COHORT_OFFLINE"] = "1"
        self.app, self.shas = make_app(self.tmp)
        self.lib, self.lib_shas = make_lib(self.tmp)
        self.app_repo = repo_row("app", self.shas["docs"], "app", APP_SIDES)
        self.lib_repo = repo_row("lib", self.lib_shas["ripple"], "library", {})

    def tearDown(self):
        for k, v in self.env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        shutil.rmtree(self.tmp)

    def mine_app(self, **kw):
        calls = []
        rows = C.mine(self.app_repo, self.app, fetch=fake_fetch(calls), **kw)
        return rows, calls

    def test_mining_per_kind(self):
        rows, calls = self.mine_app()
        by = {r["shape"]: r for r in rows}
        self.assertEqual([r["shape"] for r in rows], ["drift", "ripple", "symptom"])
        d = by["drift"]
        self.assertEqual(d["commit"], self.shas["drift"])
        self.assertEqual(d["parent"], f"refs/cohort/drift/app-{self.shas['drift'][:12]}")
        self.assertEqual(d["start"]["real_parent"], self.shas["init"])
        self.assertEqual(d["start"]["client_prefixes"], ["web/src/"])
        # the generated SDK goes with the server; the truth is the hand-written client half
        self.assertEqual(d["probe"]["client_files"], ["web/src/client.ts", "web/src/pager.ts"])
        self.assertIn("web/src/gen/api.ts", d["probe"]["server_files"])
        self.assertEqual([f["path"] for f in d["truth"]["code"]],
                         ["web/src/client.ts", "web/src/pager.ts"])
        self.assertEqual([f["status"] for f in d["truth"]["code"]], ["M", "A"])
        self.assertIn("page the widget list endpoint", d["prompt_subject"])
        self.assertIn("web app", d["prompt_subject"])
        r = by["ripple"]
        self.assertEqual(r["probe"], {"symbol": "computeSum", "defined_in": "server/total.go",
                                      "callers": ["server/a.go", "server/b.go"]})
        self.assertEqual(r["parent"], self.shas["drift"])
        s = by["symptom"]
        self.assertEqual(s["issue"], ISSUE)
        self.assertEqual(s["prompt_subject"], "Resolve the GitHub issue below.")
        self.assertNotIn("wider integer", json.dumps(s["issue"]))
        self.assertEqual(calls, [("https://github.com/example/app.git", (12,))])

    def test_library_has_no_drift(self):
        rows = C.mine(self.lib_repo, self.lib, fetch=fake_fetch([]))
        self.assertEqual([r["shape"] for r in rows], ["ripple"])
        self.assertEqual(rows[0]["probe"]["symbol"], "parse_number")
        self.assertFalse(C.drift_eligible(self.lib_repo, C.T.changed_files(
            self.lib, self.lib_shas["init"], self.lib_shas["ripple"])))

    def test_start_rebuilds_byte_identical(self):
        rows, _ = self.mine_app()
        d = rows[0]
        first = C.git_out(self.app, "rev-parse", d["parent"])
        C.git_out(self.app, "update-ref", "-d", d["parent"])
        self.assertFalse(C.has_commit(self.app, d["parent"]))
        self.assertEqual(C.checkout_rev(self.app, d), first)
        # a fresh clone of the repo rebuilds the same commit from the row alone
        other = Path(self.tmp) / "other.git"
        git(self.tmp, "clone", "-q", "--bare", str(self.app), str(other))
        self.assertEqual(C.build_start(other, d), first)
        # S = C's tree with the client half as it was at P: server and generated SDK moved,
        # the hand-written client did not, and a file the commit added is absent
        show = lambda rev, p: C.git_out(other, "show", f"{rev}:{p}")  # noqa: E731
        self.assertEqual(show(first, "server/api.go"), show(d["commit"], "server/api.go"))
        self.assertEqual(show(first, "web/src/gen/api.ts"),
                         show(d["commit"], "web/src/gen/api.ts"))
        self.assertEqual(show(first, "web/src/client.ts"),
                         show(self.shas["init"], "web/src/client.ts"))
        self.assertEqual(C.git_out(other, "ls-tree", first, "--", "web/src/pager.ts"), "")
        self.assertEqual(C.git_out(other, "rev-parse", f"{first}^"), self.shas["init"])
        diff = C.git_out(other, "diff", "--name-only", first, d["commit"]).split()
        self.assertEqual(diff, ["web/src/client.ts", "web/src/pager.ts"])

    def test_determinism_and_check(self):
        out = Path(self.tmp) / "tasks"
        repos = Path(self.tmp) / "repos.json"
        repos.write_text(json.dumps({"repos": [self.app_repo, self.lib_repo]}))
        argv = ["build-tasks", "--repos", str(repos), "--out", str(out), "--jobs", "2"]
        rows, _ = self.mine_app()
        # build-tasks fetches nothing offline: seed the symptom's issue as a carried row
        out.mkdir()
        (out / "app.jsonl").write_text(C.render(rows))
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            self.assertEqual(C.main(argv), 0)
        first = {p.name: p.read_bytes() for p in out.iterdir()}
        self.assertIn("[cohort] tasks repo=app drift=1 ripple=1 symptom=1 starts=1",
                      buf.getvalue())
        self.assertIn("[cohort] tasks repo=lib drift=0 ripple=1 symptom=0 starts=0",
                      buf.getvalue())
        self.assertEqual((out / "app.jsonl").read_text(), C.render(rows))
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(C.main(argv), 0)
            self.assertEqual(C.main(argv + ["--check"]), 0)
        self.assertEqual({p.name: p.read_bytes() for p in out.iterdir()}, first)
        tampered = [json.loads(x) for x in (out / "app.jsonl").read_text().splitlines()]
        tampered[1]["prompt_subject"] = "something else"
        (out / "app.jsonl").write_text(C.render(tampered))
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            self.assertEqual(C.main(argv + ["--check"]), 1)
        self.assertIn(f"check differs: app-{self.shas['ripple'][:12]}", err.getvalue())

    def test_picks_come_first_and_cap(self):
        repo = dict(self.app_repo, picks={"drift": [self.shas["drift"]]})
        rows = C.mine(repo, self.app, per_kind=1, fetch=fake_fetch([]))
        self.assertEqual(sum(r["shape"] == "drift" for r in rows), 1)
        self.assertEqual(rows[0]["commit"], self.shas["drift"])
        rows2 = C.mine(self.app_repo, self.app, per_kind=1, max_commits=2,
                       fetch=fake_fetch([]))
        self.assertEqual([r["shape"] for r in rows2], ["symptom"])

    def test_validate_refuses(self):
        bad = [dict(self.app_repo, public=False), dict(self.app_repo, sha="abc"),
               dict(self.app_repo, role="tool"), dict(self.app_repo, url="http://x/y"),
               dict(self.lib_repo, sides=APP_SIDES), dict(self.app_repo, sides={}),
               dict(self.app_repo, sides=dict(APP_SIDES, client=["web"]))]
        for row in bad:
            with self.assertRaises(C.CohortError):
                C.validate(row)


FAKE_GLIA = r'''#!{python}
"""A fake glia (FH.1b tests): canned JSON per (HEAD sha, answer key) from $FAKE_GLIA_SPEC."""
import json, os, subprocess, sys
spec = json.load(open(os.environ["FAKE_GLIA_SPEC"]))
args = sys.argv[1:]
if args == ["--version"]:
    print(spec["version"]); sys.exit(0)
cmd = args[0]
if cmd == "build":
    os.makedirs(".glia/graph", exist_ok=True); sys.exit(0)
if cmd == "history":
    sys.exit(0)
if cmd == "query" and "--gql" in args:
    text = args[args.index("-e") + 1]
    key = "gql:pairs" if "HTTP_CALLS" in text else "gql:endpoints" if "ENDPOINT" in text \
        else "gql:routes"
elif cmd == "query":
    key = "dl:" + ",".join(args[i + 1] for i, a in enumerate(args) if a == "--output")
elif cmd == "diff-impact":
    key = "diff-impact:names" if "--depth" in args else "diff-impact"
else:
    key = cmd
head = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
ans = spec["by_head"].get(head, {{}}).get(key, spec["default"].get(key))
if ans is None:
    print(f"no canned answer for {{key}} at {{head}}", file=sys.stderr); sys.exit(1)
if key.startswith("dl:"):
    ans = {{"results": [{{"predicate": p, "values": [{{"int": v}} if isinstance(v, int)
                                                  else {{"str": v}} for v in row]}}
                        for p, rows in ans.items() for row in rows]}}
print(json.dumps(ans))
'''


def impact(files=(), rows=()):
    return {"impact": {"files": [{"file": f} for f in files],
                       "results": [{"file": f} for f in rows]}}


def cb(rows, breaking):
    return {"schemas": rows, "models": [], "orphaned_clients": [], "breaking": breaking,
            "advisory": 0}


def strip_ms(obj):
    if isinstance(obj, dict):
        return {k: strip_ms(v) for k, v in obj.items() if not k.endswith("ms")}
    if isinstance(obj, list):
        return [strip_ms(v) for v in obj]
    return obj


class RunTest(unittest.TestCase):
    """The no-model runner (FH.1b) over the scripted app and library with a fake glia: one
    task per kind, each check's miss class, the report, determinism and check."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="cohort-run-test-")
        keys = ("GLIA_COHORT_DIR", "GLIA_COHORT_OFFLINE", "GLIA_AGENT_AB_DIR",
                "FAKE_GLIA_SPEC")
        self.env = {k: os.environ.get(k) for k in keys}
        os.environ["GLIA_COHORT_DIR"] = str(Path(self.tmp) / "cohort")
        os.environ["GLIA_AGENT_AB_DIR"] = str(Path(self.tmp) / "no-agent-ab")
        os.environ["GLIA_COHORT_OFFLINE"] = "1"
        self.app, self.shas = make_app(self.tmp)
        self.lib, self.lib_shas = make_lib(self.tmp)
        app_repo = repo_row("app", self.shas["docs"], "app", APP_SIDES)
        lib_repo = repo_row("lib", self.lib_shas["ripple"], "library", {})
        self.repos = Path(self.tmp) / "repos.json"
        self.repos.write_text(json.dumps({"repos": [app_repo, lib_repo]}))
        self.tasks = Path(self.tmp) / "tasks"
        self.tasks.mkdir()
        rows = C.mine(app_repo, self.app, fetch=fake_fetch([]))
        (self.tasks / "app.jsonl").write_text(C.render(rows))
        (self.tasks / "lib.jsonl").write_text("")
        self.rows = {r["shape"]: r for r in rows}
        self.start = C.checkout_rev(self.app, self.rows["drift"])
        self.results = Path(self.tmp) / "results"
        self.glia = Path(self.tmp) / "glia"
        self.glia.write_text(FAKE_GLIA.format(python=sys.executable))
        self.glia.chmod(0o755)
        self.write_spec()

    def tearDown(self):
        for k, v in self.env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        shutil.rmtree(self.tmp)

    def write_spec(self, version="glia 9.9.9 (build test)"):
        same = [{"kind": "op", "key": "GET /widgets", "change": "removed",
                 "file": "server/api.go"}]
        drift_s = {
            # 40 rows of one change, no client file: noisy
            "contract-breaks": cb(same * 40, 40),
            "review": {"counts": {"impact": 1}, "impact": [{"file": "server/api.go"}]},
            "dl:parsed,cnode,paired": {"parsed": [["web/src/client.ts"]],
                                       "cnode": [["web/src/client.ts", "ENDPOINT"]],
                                       "paired": []},
            # the client is reached, and holds no removed name: names glia_wrong
            "diff-impact:names": impact(["web/src/client.ts"]),
            "find": {"results": [{"file": "web/src/client.ts"}]},
            # the client is in the rows, below 29 others: noisy
            "diff-impact": impact([], ["web/src/client.ts"] + ["server/x.go"] * 29)}
        five = [{"kind": "op", "key": f"k{i}", "change": "removed", "file": "server/api.go"}
                for i in range(5)]
        drift_c = {"contract-breaks": cb(five, 50), "review": {"counts": {}},
                   "diff-impact:names": impact()}
        ripple = {"dl:parsed,cnode,paired": {"parsed": [], "cnode": [], "paired": []},
                  "find": {"results": []}, "diff-impact": impact()}
        symptom = {"dl:parsed,cnode,paired": {
                       "parsed": [["server/handler.go"]], "cnode": [], "paired": []},
                   "find": {"results": [{"file": "server/other.go"}]},
                   "diff-impact": impact(["server/x.go"])}
        health = {"gaps": {"counts": {"unpaired_route": 1, "dead_symbol": 2}},
                  "coverage": {"rows": [{}]},
                  "gql:endpoints": {"results": [{"values": [{"str": "endpoint:GET:/w"}]}]},
                  "gql:routes": {"results": [{"values": [{"str": "route:GET:/w"}]}]},
                  "gql:pairs": {"results": []},
                  "dl:boundary_nodes,boundary_pairs": {
                      "boundary_nodes": [["ENDPOINT", 1]], "boundary_pairs": []}}
        lib_health = dict(health, **{
            "gaps": {"counts": {"unpaired_route": 2, "dead_symbol": 5}},
            "gql:endpoints": {"results": []},
            "gql:routes": {"results": [{"values": [{"str": "route:GET:/a"}]},
                                       {"values": [{"str": "route:GET:/b"}]}]},
            "dl:boundary_nodes,boundary_pairs": {"boundary_nodes": [],
                                                 "boundary_pairs": []}})
        spec = {"version": version, "default": health,
                "by_head": {self.start: drift_s,
                            self.shas["drift"]: {**drift_c, **ripple},
                            self.shas["ripple"]: symptom,
                            self.lib_shas["ripple"]: lib_health}}
        path = Path(self.tmp) / "spec.json"
        path.write_text(json.dumps(spec))
        os.environ["FAKE_GLIA_SPEC"] = str(path)

    def main(self, *args, release="t"):
        argv = [*args, "--release", release, "--repos", str(self.repos), "--out",
                str(self.tasks), "--results", str(self.results), "--glia", str(self.glia)]
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = C.main(argv)
        return rc, out.getvalue(), err.getvalue()

    def read(self, name, release="t"):
        return [json.loads(x) for x in
                (self.results / release / name).read_text().splitlines()]

    def test_run_classifies_each_miss_class(self):
        rc, out, err = self.main("run", "--jobs", "3")
        self.assertEqual(rc, 0, err)
        self.assertRegex(out, r"\[cohort\] run release=t repos=2 tasks=3 misses=9 "
                              r"\(blind=4 wrong=3 noisy=2\) ms=\d+")
        tasks = {t["shape"]: t for t in self.read("tasks.jsonl")}
        got = {s: {c["check"]: c["class"] or c["status"] for c in t["checks"]}
               for s, t in tasks.items()}
        self.assertEqual(got["drift"], {"locate": "pass", "rest": "noisy", "gate": "noisy",
                                        "clear": "glia_wrong", "names": "glia_wrong"})
        self.assertEqual(got["ripple"], {"locate": "glia_blind", "rest": "glia_blind",
                                         "gate": "n/a", "clear": "n/a", "names": "n/a"})
        # the core is handler.go (a tie goes to the first path), the rest a file it adds
        self.assertEqual(got["symptom"], {"locate": "glia_wrong", "rest": "n/a",
                                          "gate": "n/a", "clear": "n/a", "names": "n/a"})
        d = tasks["drift"]
        # pager.ts is added by the change: it does not exist at S, so nothing can name it
        self.assertEqual(d["S"]["evidence"]["web/src/pager.ts"]["exists"], False)
        self.assertEqual(d["S"]["gate"]["cb"]["distinct_changes"], 1)
        # FH.1c's marker: the drift's contract-breaks held no name row; others ran no gate
        self.assertRegex(out, rf"\[cohort\] task {d['task']} drift .* names=glia_wrong "
                              r"cb_names=0\n")
        self.assertRegex(out, rf"\[cohort\] task {tasks['ripple']['task']} ripple .* "
                              r"cb_names=-\n")
        self.assertEqual(d["S"]["names"]["boundary_has_truth"], ["web/src/client.ts"])
        ripple = {c["check"]: c for c in tasks["ripple"]["checks"]}
        self.assertIn("server/a.go: unparsed", ripple["rest"]["why"])
        sym = tasks["symptom"]
        self.assertEqual(sym["R"]["rest_files"], ["server/tally.go"])
        self.assertEqual(d["R"]["rest_in_answer"], ["web/src/client.ts"])
        health = {h["repo"]: h for h in self.read("health.jsonl")}
        self.assertEqual(health["app"]["blind"], ["routes_unreached", "clients_unpaired"])
        self.assertEqual(health["app"]["boundary_nodes"], {"ENDPOINT": 1})
        # a library is never judged on client reach
        self.assertEqual(health["lib"]["flags"], ["routes_unreached", "no_client_endpoints"])
        self.assertEqual(health["lib"]["blind"], [])
        misses = self.read("misses.jsonl")
        self.assertEqual(len(misses), 9)
        self.assertEqual(misses[0], {"repo": "app", "task": None, "scope": "repo",
                                     "check": "health", "class": "glia_blind",
                                     "why": "clients_unpaired"})
        self.assertFalse(list((Path(self.tmp) / "cohort" / "work").rglob("*.go")))

    def test_report(self):
        self.assertEqual(self.main("run")[0], 0)
        rc, out, _ = self.main("report")
        self.assertEqual(rc, 0)
        self.assertEqual(out, (self.results / "t" / "report.md").read_text())
        self.assertIn("| app | app | 1 | 1 | 0 | 0.0 | 0.0 | 2 | ENDPOINT 1 | - | "
                      "routes_unreached, clients_unpaired |", out)
        self.assertIn("| app | 3 | 1/3 | 0/2 | 0/1 | 0/1 | 0/1 | 4 | 3 | 2 |", out)
        self.assertIn("| lib | 0 | - | - | - | - | - | 0 | 0 | 0 |", out)
        self.assertIn("contract-breaks at S named a client truth file on 0 of 1 drift tasks",
                      out)
        for k, n in (("glia_blind", 4), ("glia_wrong", 3), ("noisy", 2)):
            self.assertIn(f"### {k} ({n})", out)
        self.assertIn("40 rows for 1 distinct changes", out)

    def test_determinism_partial_and_check(self):
        self.assertEqual(self.main("run", "--jobs", "1")[0], 0)
        names = ("health.jsonl", "tasks.jsonl", "misses.jsonl", "summary.json", "report.md",
                 "meta.json")
        first = {n: (self.results / "t" / n).read_text() for n in names}
        self.assertEqual(self.main("run", "--jobs", "4")[0], 0)
        for n in names:
            now = (self.results / "t" / n).read_text()
            if n.endswith(".jsonl"):
                self.assertEqual([strip_ms(json.loads(x)) for x in now.splitlines()],
                                 [strip_ms(json.loads(x)) for x in first[n].splitlines()], n)
            else:
                self.assertEqual(now, first[n], n)
        rc, out, _ = self.main("check")
        self.assertEqual(rc, 0)
        self.assertIn("[cohort] check release=t ok - repos=2 tasks=3 misses=9", out)
        # a partial run replaces only its own rows
        rid = self.rows["ripple"]["id"]
        rc, out, _ = self.main("run", "--ids", f"^{rid}$")
        self.assertEqual(rc, 0)
        self.assertIn("tasks=3 misses=9", out)
        self.assertEqual(self.main("check")[0], 0)
        # a changed check is caught
        rows = self.read("tasks.jsonl")
        rows[0]["checks"][1]["status"] = "pass"  # the drift's rest was a noisy miss
        (self.results / "t" / "tasks.jsonl").write_text(C.jsonl(rows))
        rc, _, err = self.main("check")
        self.assertEqual(rc, 1)
        self.assertIn(f"checks differ: {rows[0]['task']}", err)
        # one release's results come from one glia
        self.write_spec(version="glia 0.0.1 (build other)")
        rc, _, err = self.main("run")
        self.assertEqual(rc, 2)
        self.assertIn("one release's results come from one glia", err)

    def test_classify_errors_and_added_files(self):
        base = {"task": "x", "repo": "app", "shape": "ripple", "truth": ["a.go"]}
        self.assertEqual({c["status"] for c in C.classify(dict(base, error="start: gone"))},
                         {"error"})
        row = dict(base, start={"error": "build rc=1 boom"})
        got = {c["check"]: c["status"] for c in C.classify(row)}
        self.assertEqual(got["locate"], "error")
        self.assertEqual(got["gate"], "n/a")
        ev = {"a.go": {"exists": False, "parsed": False, "boundary": [], "paired": []}}
        row = dict(base, start={"evidence": ev},
                   L={"glia": [0.0, None], "glia_err": None},
                   R={"glia": [0.0, None], "rest": 1, "rest_files": ["a.go"],
                      "rest_in_answer": [], "rows": 0})
        got = {c["check"]: c["status"] for c in C.classify(row)}
        self.assertEqual((got["locate"], got["rest"]), ("n/a", "n/a"))


class CannedGlia:
    """gate_state's glia: one canned answer per command (FH.1c)."""

    def __init__(self, answers):
        self.answers = answers

    def json(self, *args):
        d = self.answers.get(args[0])
        return d, len(json.dumps(d)), 0, None if d is not None else "rc=1 none"


def name_row(name, file):
    return {"name": name, "normalised": name.lower(), "renamed_to": None, "repo": "app",
            "file": file, "line": 3, "position": "property", "client": f"{file}::f",
            "server": "server/api.go::H", "tier": "heuristic", "blocking": False}


class GateStateTest(unittest.TestCase):
    """gate_state reads contract-breaks' name rows and the shared-schema fold (FH.1c)."""

    REVIEW = {"counts": {}}

    def state(self, answer, truth):
        return C.gate_state(CannedGlia({"contract-breaks": answer, "review": self.REVIEW}),
                            truth)["cb"]

    def test_names_rows_count_toward_truth_named(self):
        ans = dict(cb([], 0), names=[name_row("userName", "web/src/client.ts"),
                                     name_row("userName", "web/src/other.ts"),
                                     name_row("mail", "web/src/client.ts")])
        got = self.state(ans, ["web/src/client.ts", "web/src/pager.ts"])
        self.assertEqual(got["truth_named"], ["web/src/client.ts"])
        self.assertEqual(got["names"], 3)
        self.assertEqual(got["rows"], 3)
        self.assertEqual(got["files_named"], 2)
        # one distinct change per removed name, however many client sites use it
        self.assertEqual(got["distinct_changes"], 2)

    def test_missing_names_key_reads_empty(self):
        got = self.state(cb([], 0), ["web/src/client.ts"])
        self.assertEqual((got["names"], got["rows"], got["truth_named"]), (0, 0, []))

    def test_folded_schema_row_counts_once_per_operation(self):
        change = [{"section": "response", "field": "code", "change": "removed"}]
        row = {"kind": "operation", "key": "GET /a", "change": "modified",
               "changes": change, "file": "api/openapi.yaml"}
        folded = dict(row, operations=["GET /a", "GET /b", "GET /c"])
        self.assertEqual(self.state(cb([folded], 3), [])["distinct_changes"], 3)
        # unfolded, the same change on three operations is one distinct change
        three = [dict(row, key=k) for k in ("GET /a", "GET /b", "GET /c")]
        self.assertEqual(self.state(cb(three, 3), [])["distinct_changes"], 1)
        self.assertEqual(self.state(cb([row], 1), [])["distinct_changes"], 1)
        # an empty operations list folds nothing
        self.assertEqual(self.state(cb([dict(row, operations=[])], 1), [])
                         ["distinct_changes"], 1)


class CommittedTest(unittest.TestCase):
    """The committed corpus: repos.json and tasks/ are well formed (no clone needed)."""

    def test_repos_json(self):
        repos = C.load_repos()
        self.assertGreaterEqual(len(repos), 20)
        labels = {r["label"] for r in repos}
        for need in ("shell", "websocket", "samples", "commons-text", "microservices-demo",
                     "click", "immich", "twenty", "photoprism", "gin-vue-admin",
                     "scaffold-eth-2"):
            self.assertIn(need, labels)
        readme = (HERE / "README.md").read_text()
        for r in repos:
            self.assertIn(f"`{r['label']}`", readme, r["label"])
            self.assertGreater(len(r["shape"]), 20, r["label"])

    def test_fe_drift_picks(self):
        probe = HERE.parent / "agent-ab" / "probe" / "drift-tasks"
        if not probe.exists():
            self.skipTest("bench/agent-ab/probe/drift-tasks is gone")
        repos = {r["label"]: r for r in C.load_repos()}
        for f in sorted(probe.glob("*.jsonl")):
            rows = [json.loads(x) for x in f.read_text().splitlines()]
            r = repos[f.stem]
            self.assertEqual(set(r["picks"]["drift"]), {x["commit"] for x in rows})
            for x in rows:
                self.assertTrue(r["sides"]["client"], f.stem)
                self.assertTrue(all(c.startswith(tuple(r["sides"]["client"]))
                                    for c in x["probe"]["client_files"]))
                self.assertEqual(C.gen_rule(r), C.GEN_DEFAULT)

    def test_task_files(self):
        repos = {r["label"]: r for r in C.load_repos()}
        files = sorted((HERE / "tasks").glob("*.jsonl"))
        self.assertEqual({f.stem for f in files}, set(repos))
        ids = set()
        for f in files:
            rows = [json.loads(x) for x in f.read_text().splitlines()]
            self.assertEqual(f.read_text(), C.render(rows))
            shapes = [r["shape"] for r in rows]
            self.assertEqual(shapes, sorted(shapes, key=C.KINDS.index))
            for k in C.KINDS:
                self.assertLessEqual(shapes.count(k), C.MAX_PER_KIND)
            if repos[f.stem]["role"] == "library":
                self.assertNotIn("drift", shapes)
            for r in rows:
                self.assertNotIn(r["id"], ids)
                ids.add(r["id"])
                self.assertTrue(r["truth"]["code"], r["id"])
                if r["shape"] == "drift":
                    st = r["start"]
                    self.assertEqual(r["parent"], C.START_REF + r["id"])
                    self.assertEqual(st["commit"], r["commit"])
                    self.assertNotIn("start", r["probe"])
                    self.assertEqual(st["client_prefixes"],
                                     repos[f.stem]["sides"]["client"])
                    self.assertEqual(st["generated"], C.gen_rule(repos[f.stem]))
                else:
                    self.assertTrue(C.HEX40.match(r["parent"]))
                if r["shape"] == "symptom":
                    self.assertTrue(r["issue"]["title"])


if __name__ == "__main__":
    unittest.main(verbosity=1)
