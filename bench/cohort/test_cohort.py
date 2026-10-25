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
