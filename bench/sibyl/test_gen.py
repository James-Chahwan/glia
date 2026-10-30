#!/usr/bin/env python3
"""Hermetic tests for bench/sibyl/gen.py (FH.11a, FH.11c) and run.py (FH.11b): the cover holds every
expressible axis pair and no unexpressible one, every transport's cases render deterministically
with their drift by construction, the git form has P / S / C, every rendered file parses where a
parse-only toolchain is present (each checker first proven to reject a broken file), and run.py
grades a case end to end against a canned glia; the evm transport (FH.11c, in the default cover
since FH.23) renders its drift by construction, and a cover without it is the one FH.11a / FH.11b
wrote. No real glia, no network.

  python3 bench/sibyl/test_gen.py

Prints `[sibyl] syntax checked=<n> skipped=<m> (<lang>:<reason>, ..)`; a skipped language is
counted, never passed silently."""

import ast
import glob
import itertools
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import gen  # noqa: E402
from templates import evm  # noqa: E402
from templates.common import NEW, OLD  # noqa: E402

ALL = ",".join(gen.AXES["transport"])
EVERY = ",".join(gen.ALL_TRANSPORTS)
# The transports whose case is the `User` field exchange (RenderTest's shape); an evm case is a
# contract with a member drift, tested by EvmTest.
CHANNEL = [t for t in gen.AXES["transport"] if t != "evm"]


def tmpdir():
    base = Path(os.environ.get("SIBYL_TMP", Path.home() / ".cache" / "glia-sibyl-test"))
    base.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(dir=base))


def brute_pairs(axes, forbid):
    """Every pair some full forbidden-free case holds, by enumerating the whole product."""
    names = list(axes)
    bad = {frozenset(p) for p in forbid}
    held = set()
    for combo in itertools.product(*(axes[n] for n in names)):
        items = list(zip(names, combo))
        pairs = [frozenset(p) for p in itertools.combinations(items, 2)]
        if any(p in bad for p in pairs):
            continue
        held.update(pairs)
    return held


def case_pairs(cases):
    out = set()
    for c in cases:
        out.update(frozenset(p) for p in itertools.combinations(sorted(c["axes"].items()), 2))
    return out


class CoverTest(unittest.TestCase):
    def check_cover(self, transports):
        cases = gen.cover(transports.split(","), quiet=True)
        axes = gen.axes_for(transports.split(","))
        forbid = [(a, b) for a, b, _ in gen.unexpressible()]
        want = brute_pairs(axes, forbid)
        got = case_pairs(cases)
        self.assertEqual(want - got, set(), "pairs the cover misses")
        for a, b in forbid:
            for c in cases:
                self.assertFalse(c["axes"].get(a[0]) == a[1] and c["axes"].get(b[0]) == b[1],
                                 f"{c['id']} holds unexpressible {a} x {b}")
        self.assertEqual(len({c["id"] for c in cases}), len(cases))
        return cases

    def test_rest_cover_holds_every_pair(self):
        cases = self.check_cover("rest")
        self.assertTrue(all(c["axes"]["transport"] == "rest" for c in cases))
        # Pairwise is bounded below by the two largest axes (server x client).
        self.assertGreaterEqual(len(cases), 7 * 5)
        self.assertLessEqual(len(cases), 60)

    def test_all_transport_cover_holds_every_pair(self):
        self.check_cover(ALL)

    def test_evm_cover_holds_every_pair(self):
        cases = self.check_cover("evm")
        for c in cases:
            ax = c["axes"]
            self.assertEqual(ax["transport"], "evm")
            self.assertIn(ax["client"], ("ts", "go"), c["id"])
            self.assertEqual(ax["server"], "solidity", c["id"])
            self.assertNotEqual(ax["wiring_source"], "enum-prefix", c["id"])
            self.assertNotEqual(ax["contract"], "declared-type", c["id"])
        # every expressible value appears; each drift kind is exercised
        for k, vs in gen.axes_for(["evm"]).items():
            held = {v for v in vs if any((k, v) in p for p in brute_pairs(
                gen.axes_for(["evm"]), [(a, b) for a, b, _ in gen.unexpressible()]))}
            self.assertEqual({c["axes"][k] for c in cases}, held, k)
        self.assertEqual({evm.drift_of(c) for c in cases}, set(evm.DRIFTS))

    def test_every_transport_cover_holds_every_pair(self):
        cases = self.check_cover(EVERY)
        evm_cases = [c for c in cases if c["axes"]["transport"] == "evm"]
        self.assertTrue(evm_cases)
        for c in cases:
            on_chain = c["axes"]["transport"] == "evm"
            self.assertEqual(c["axes"]["server"] == "solidity", on_chain, c["id"])
            if c["axes"]["client"] == "go":
                self.assertTrue(on_chain, c["id"])

    def test_narrowed_values_join_only_with_their_transport(self):
        """evm's values (go, solidity) join an axis only when evm is selected, so a cover without
        evm is the one FH.11a / FH.11b wrote; the default cover (evm in it since FH.23) holds
        them."""
        self.assertIn("evm", gen.AXES["transport"])
        self.assertIn("evm", gen.ALL_TRANSPORTS)
        for ts in [CHANNEL] + [[t] for t in CHANNEL]:
            axes = gen.axes_for(ts)
            for k in ("client", "server"):
                self.assertEqual(axes[k], [v for v in gen.AXES[k]
                                           if v in set(gen.values_of(ts[0], k))
                                           or len(ts) > 1], (ts, k))
            self.assertNotIn("go", axes["client"])
            self.assertNotIn("solidity", axes["server"])
        self.assertEqual(gen.axes_for(["evm"])["client"], ["ts", "go"])
        self.assertEqual(gen.axes_for(gen.ALL_TRANSPORTS)["client"], gen.AXES["client"] + ["go"])
        self.assertEqual(gen.axes_for(gen.AXES["transport"])["server"],
                         gen.AXES["server"] + ["solidity"])

    def test_opt_in_matches_run_tables(self):
        """evm is in the default run exactly when run.py's cell tables have its rows (MECH["evm"],
        LANG["go"], LANG["solidity"]; FH.23 added both together, gen.py's module doc)."""
        import run
        ready = "evm" in run.MECH and {"go", "solidity"} <= set(run.LANG)
        self.assertEqual(ready, "evm" in gen.AXES["transport"],
                         "run.py's MECH / LANG and gen.AXES['transport'] disagree on evm")

    def test_cover_is_deterministic_and_seeded(self):
        a = gen.cover(["rest"], quiet=True)
        self.assertEqual(a, gen.cover(["rest"], quiet=True))
        self.assertNotEqual(a, gen.cover(["rest"], seed=gen.SEED + 1, quiet=True))

    def test_marker(self):
        r = subprocess.run([sys.executable, str(HERE / "gen.py"), "cover"], capture_output=True,
                           text=True, check=True)
        m = re.search(r"^\[sibyl\] cover cases=(\d+) pairs=(\d+)/(\d+) transports=rest$",
                      r.stderr, re.M)
        self.assertIsNotNone(m, r.stderr)
        self.assertEqual(m.group(2), m.group(3))
        self.assertEqual(int(m.group(1)), len(gen.cover(["rest"], quiet=True)))

    def test_unknown_transport_is_refused(self):
        with self.assertRaises(SystemExit):
            gen.render_case({"id": "x", "axes": {"transport": "carrier-pigeon"}})

    def test_every_transport_has_templates(self):
        self.assertEqual(sorted(gen.templates.TRANSPORTS), sorted(gen.ALL_TRANSPORTS))

    def test_marker_names_evm(self):
        r = subprocess.run([sys.executable, str(HERE / "gen.py"), "cover", "--transports", EVERY],
                           capture_output=True, text=True, check=True)
        m = re.search(r"^\[sibyl\] cover cases=(\d+) pairs=(\d+)/(\d+) "
                      r"transports=rest,grpc,graphql,queue,websocket,evm( |$)", r.stderr, re.M)
        self.assertIsNotNone(m, r.stderr)
        self.assertEqual(m.group(2), m.group(3))

    def test_readme_lists_axes_and_unexpressible_pairs(self):
        text = (HERE / "README.md").read_text()
        for axis in gen.AXES:
            self.assertIn(f"`{axis}`", text)
            for v in gen.all_values(axis):
                self.assertIn(f"`{v}`", text, f"README misses {axis} value {v}")
        rendered = set(gen.unrendered())
        for a, b, _ in gen.unexpressible():
            if (a, b, _) in rendered:
                continue
            self.assertRegex(text, rf"`{re.escape(a[1])}`[^\n]*`{re.escape(b[1])}`",
                             f"README misses unexpressible {a} x {b}")
        for t in gen.ALL_TRANSPORTS:  # a transport that narrows its values says which
            for axis, vs in getattr(gen.templates.TRANSPORTS[t], "VALUES", {}).items():
                self.assertRegex(text, rf"`{t}`[^\n]*{axis}[^\n]*"
                                 + "".join(rf"[^\n]*`{re.escape(v)}`" for v in vs),
                                 f"README misses {t}'s {axis} values")


SPELL_OLD = OLD.spellings()
SPELL_NEW = NEW.spellings()
# A bytes literal's escapes (protoc's serialized descriptor in users_pb2.py) read as a gap.
ESCAPE = re.compile(r"\\(x[0-9a-fA-F]{2}|[ntr'\"\\])")


def holds(text, names):
    """A name as a word, or after an accessor's get / set (protobuf-java's setFullName)."""
    text = ESCAPE.sub(" ", text)
    return any(re.search(rf"(?:\b|(?<=get)|(?<=set)){re.escape(n)}\b", text) for n in names)


def every_case():
    """The channel transports' cover plus each one's own (a pairwise cover inside one transport
    reaches every pair of its template variants), deduplicated by id; evm's are EvmTest's."""
    out = {}
    for ts in [CHANNEL] + [[t] for t in CHANNEL]:
        for c in gen.cover(ts, quiet=True):
            out.setdefault(c["id"] + ("" if len(ts) > 1 else "@" + ts[0]), c)
    return list(out.values())


# What a transport's call file must name: the exchange as that transport spells it.
CALL_NAMES = {"grpc": "UserService", "graphql": "GetUser", "queue": "user",
              "websocket": "users"}


class RenderTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = every_case()
        cls.renders = [(c, gen.render_case(c)) for c in cls.cases]

    def test_every_value_renders(self):
        """Every axis value a transport can express appears in that transport's cover."""
        for t in CHANNEL:
            axes = gen.axes_for([t])
            forbid = [(a, b) for a, b, _ in gen.unexpressible()]
            want = {p for p in brute_pairs(axes, forbid)}
            seen = {k: set() for k in gen.AXES}
            for c in self.cases:
                if c["axes"]["transport"] == t:
                    for k, v in c["axes"].items():
                        seen[k].add(v)
            for k, vs in gen.AXES.items():
                held = {v for v in vs if any((k, v) in p for p in want)}
                self.assertEqual(seen[k], held, (t, k))

    def test_render_is_deterministic(self):
        for t in CHANNEL:
            c, r = next((c, r) for c, r in self.renders if c["axes"]["transport"] == t)
            self.assertEqual(gen.render_case(c), r, t)

    def test_expected_by_construction(self):
        for c, (states, exp) in self.renders:
            cid = c["id"]
            for state in ("P", "S", "C"):
                for key in ("client", "reader"):
                    ref = exp[key] if key == "reader" else exp["pair"]["client"]
                    self.assertIn(ref["file"], states[state][ref["repo"]], f"{cid} {key}")
                srv = exp["pair"]["server"]
                for f in ("route_file", "handler_file", "model_file"):
                    self.assertIn(srv[f], states[state][srv["repo"]], f"{cid} {f}")
                self.assertIn(srv["handler"], states[state][srv["repo"]][srv["handler_file"]])
                for pf in srv.get("pair_files", []):
                    self.assertIn(pf, states[state][srv["repo"]], f"{cid} pair file")
            self.assertEqual(exp["transport"], c["axes"]["transport"])
            self.assertEqual(exp["pair"]["category"], {
                "rest": "HTTP_CALLS", "grpc": "GRPC_CALLS", "graphql": "GRAPHQL_CALLS",
                "queue": "QUEUE_FLOWS", "websocket": "WS_CONNECTS"}[exp["transport"]])
            drift = exp["drift"]
            self.assertTrue(drift["server_files"], cid)
            self.assertTrue(drift["client_files"], cid)
            srv_repo = exp["pair"]["server"]["repo"]
            cli_repo = exp["pair"]["client"]["repo"]
            server_root = exp["pair"]["server"]["route_file"].split("/")[0]
            for ref in drift["server_files"]:
                self.assertEqual(ref["repo"], srv_repo, cid)
                if exp["axes"]["packaging"] != "multi-repo":
                    self.assertTrue(ref["file"].startswith(server_root + "/"), (cid, ref))
            for ref in drift["client_files"]:
                self.assertEqual(ref["repo"], cli_repo, cid)
                if exp["axes"]["packaging"] != "multi-repo":
                    self.assertFalse(ref["file"].startswith(server_root + "/"), (cid, ref))
            # S: the server holds the new name only, every changed client file the old one.
            for ref in drift["server_files"]:
                s = states["S"][ref["repo"]][ref["file"]]
                self.assertFalse(holds(s, SPELL_OLD), (cid, ref))
                self.assertTrue(holds(s, SPELL_NEW), (cid, ref))
            for ref in drift["client_files"]:
                s = states["S"][ref["repo"]][ref["file"]]
                cc = states["C"][ref["repo"]][ref["file"]]
                self.assertTrue(holds(s, SPELL_OLD) and not holds(s, SPELL_NEW), (cid, ref))
                self.assertTrue(holds(cc, SPELL_NEW), (cid, ref))
            # The call file names the exchange; a literal REST path whole.
            call = states["P"][cli_repo][exp["pair"]["client"]["file"]]
            if exp["transport"] == "rest":
                self.assertIn("users", call.lower(), cid)
                if c["axes"]["wiring_source"] == "literal":
                    self.assertIn("api/v1/users", call, cid)
            else:
                self.assertIn(CALL_NAMES[exp["transport"]].lower(), call.lower(), cid)
                self.assertTrue(exp["exchange"]["match"], cid)

    def test_layouts(self):
        for c, (states, exp) in self.renders:
            pk = c["axes"]["packaging"]
            repos = sorted(states["P"])
            self.assertEqual(repos, ["client", "server"] if pk == "multi-repo" else ["repo"])
            if pk == "workspace":
                files = states["P"]["repo"]
                self.assertTrue(exp["reader"]["file"].startswith("apps/web/"), c["id"])
                self.assertTrue(exp["pair"]["client"]["file"].startswith("packages/"), c["id"])
                # A SwiftPM app reaches its local package by path; there is no root manifest.
                if c["axes"]["client"] != "swift":
                    self.assertTrue(any("/" not in f for f in files), (c["id"], "root manifest"))


class EvmTest(unittest.TestCase):
    """The evm transport's renders: the contract drifts one way per case at S, the client follows
    at C, and the expected answer is read off the renders."""

    @classmethod
    def setUpClass(cls):
        cls.cases = gen.cover(["evm"], quiet=True)
        cls.renders = [(c, gen.render_case(c)) for c in cls.cases]

    def test_render_is_deterministic(self):
        for c, r in self.renders:
            self.assertEqual(gen.render_case(c), r, c["id"])

    def test_expected_by_construction(self):
        for c, (states, exp) in self.renders:
            cid, kind = c["id"], exp["drift"]["kind"]
            self.assertEqual(kind, evm.drift_of(c))
            self.assertEqual(exp["transport"], "evm")
            self.assertEqual(exp["pair"]["category"], "RPC_CALLS")
            self.assertEqual(exp["exchange"]["procedure"], "evm:Escrow.deposit")
            self.assertTrue(re.search(exp["exchange"]["match"][0], "evm:Escrow.deposit"))
            self.assertFalse(re.search(exp["exchange"]["match"][0], "evm:Escrow.depositAll"))
            cl, srv = exp["pair"]["client"], exp["pair"]["server"]
            for state in ("P", "S", "C"):
                self.assertIn(cl["file"], states[state][cl["repo"]], cid)
                self.assertIn(exp["reader"]["file"], states[state][cl["repo"]], cid)
                sol = states[state][srv["repo"]][srv["route_file"]]
                self.assertIn("function deposit(address payee) external payable", sol, cid)
                # the pair's call names deposit in every state: it never drifts
                self.assertRegex(states[state][cl["repo"]][cl["file"]],
                                 r"""\.deposit\(|['"]deposit['"]""", cid)
            self.assertEqual({e["category"] for e in exp["edges"]}, {"RPC_CALLS", "EVENT_FLOWS"})
            sp_p, sp_s = (states[s][srv["repo"]][srv["route_file"]] for s in ("P", "S"))
            new, old = exp["drift"]["new"], exp["drift"]["old"]
            if kind == "fn_rename":
                self.assertIn("function settle(", sp_s)
                self.assertNotIn("function release(", sp_s)
            elif kind == "event_rename":
                self.assertIn("event Settled(", sp_s)
                self.assertNotIn("Released", sp_s)
            else:
                self.assertIn("bytes32 memo", sp_s)
                self.assertNotIn("memo", sp_p)
                self.assertEqual(exp["drift"]["arity"], {"old": 2, "new": 3})
            self.assertEqual(exp["drift"]["server_files"],
                             [{"repo": srv["repo"], "file": srv["route_file"]}], cid)
            files = exp["drift"]["client_files"]
            self.assertTrue(files, cid)
            self.assertIn(cl, files, cid)
            stale = re.compile(r"Released\b" if kind == "event_rename" else r"\b[Rr]elease\b")
            for ref in files:
                self.assertEqual(ref["repo"], cl["repo"], cid)
                s_text = states["S"][ref["repo"]][ref["file"]]
                c_text = states["C"][ref["repo"]][ref["file"]]
                self.assertNotEqual(s_text, c_text)
                if kind != "arity":
                    self.assertFalse(stale.search(c_text), (cid, ref, "C still holds " + old))
                    self.assertIn(new.lower(), c_text.lower(), (cid, ref))
                else:
                    self.assertRegex(c_text, r"(?i)memo", (cid, ref))
            # the client and contracts do not change from P to S on the client's side
            self.assertEqual(states["P"][cl["repo"]].get(cl["file"]),
                             states["S"][cl["repo"]].get(cl["file"]), cid)
            sites = exp["drift"]["client_sites"]
            self.assertTrue(sites, cid)
            for site in sites:
                lines = states["S"][site["repo"]][site["file"]].splitlines()
                self.assertEqual(lines[site["line"] - 1].strip(), site["text"], cid)
                self.assertTrue(stale.search(site["text"]), (cid, site))
            self.assertIn(cl["file"], {s["file"] for s in sites}, cid)

    def test_layouts(self):
        for c, (states, exp) in self.renders:
            pk, files = c["axes"]["packaging"], None
            repos = sorted(states["P"])
            self.assertEqual(repos, ["client", "server"] if pk == "multi-repo" else ["repo"])
            files = states["P"]["repo"] if pk != "multi-repo" else states["P"]["client"]
            if pk == "workspace":
                root = "go.work" if c["axes"]["client"] == "go" else "pnpm-workspace.yaml"
                self.assertIn(root, files, c["id"])
                self.assertTrue(exp["reader"]["file"].startswith("apps/web/"), c["id"])
            manifest = "go.mod" if c["axes"]["client"] == "go" else "package.json"
            self.assertTrue(any(f.endswith(manifest) for f in files), c["id"])
            self.assertTrue(exp["pair"]["server"]["route_file"].endswith(
                "contracts/Escrow.sol"), c["id"])

    def test_abi_forms(self):
        """Each `contract` value writes the ABI form the module doc names."""
        for c, (states, exp) in self.renders:
            files = states["P"][exp["pair"]["client"]["repo"]]
            text = "\n".join(files.values())
            form, lang = c["axes"]["contract"], c["axes"]["client"]
            if form == "generated":
                self.assertTrue("Escrow__factory" in text if lang == "ts"
                                else "Code generated - DO NOT EDIT." in text, c["id"])
            elif form == "schema-object":
                self.assertTrue("escrowAbi" in text if lang == "ts"
                                else any(f.endswith("chain/abi.go") for f in files), c["id"])
            else:
                self.assertTrue("'function deposit(address payee) payable'" in text
                                if lang == "ts" else "const EscrowABI = `[" in text, c["id"])


class GitTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("git"), "git absent")
    def test_git_form(self):
        cases = gen.cover(["rest"], quiet=True)
        multi = next(c for c in cases if c["axes"]["packaging"] == "multi-repo")
        single = next(c for c in cases if c["axes"]["packaging"] == "single")
        heads = []
        for _ in range(2):
            d = tmpdir()
            try:
                gen.main(["render", "--out", str(d), "--case", multi["id"],
                          "--case", single["id"]])
                exp = json.loads((d / multi["id"] / "expected.json").read_text())
                self.assertEqual(exp["primary"], "server")
                got = []
                for cid, repos in ((multi["id"], ["client", "server"]), (single["id"], ["repo"])):
                    for repo in repos:
                        g = d / cid / repo
                        log = subprocess.run(["git", "-C", str(g), "log", "--format=%s"],
                                             capture_output=True, text=True, check=True).stdout
                        self.assertEqual(len(log.splitlines()), 3, (cid, repo))
                        for state in ("P", "S", "C"):
                            subprocess.run(["git", "-C", str(g), "rev-parse",
                                            f"refs/sibyl/{state}"], check=True,
                                           capture_output=True)
                        got.append(subprocess.run(["git", "-C", str(g), "rev-parse", "HEAD"],
                                                  capture_output=True, text=True,
                                                  check=True).stdout.strip())
                # S..C touches only the client repo of a multi-repo case.
                diff = subprocess.run(["git", "-C", str(d / multi["id"] / "server"), "diff",
                                       "--name-only", "refs/sibyl/S", "refs/sibyl/C"],
                                      capture_output=True, text=True, check=True).stdout
                self.assertEqual(diff, "")
                heads.append(got)
            finally:
                shutil.rmtree(d)
        self.assertEqual(heads[0], heads[1], "git form is byte-identical across renders")


# ------------------------------------------------------------------------------- run.py

FAKE_GLIA = r"""#!/usr/bin/env python3
# A canned glia for run.py's tests: answers from $FAKE_DIR, the state read off the commit.
import json, os, subprocess, sys
a = sys.argv[1:]
if a == ["--version"]:
    print("glia 0.0.0 (fake)")
    sys.exit(0)
d = os.environ["FAKE_DIR"]
if a[0] == "query":
    print(open(os.path.join(d, "query.json")).read())
    sys.exit(0)
if a[0] == "contract-breaks":
    subj = subprocess.run(["git", "-C", a[1], "log", "-1", "--format=%s"], capture_output=True,
                          text=True, check=True).stdout
    print(open(os.path.join(d, "cb_" + subj.split()[1].rstrip(":") + ".json")).read())
    sys.exit(1 if "sibyl S" in subj else 0)
sys.exit(2)
"""


def node(qname, file):
    return {"node": {"qname": qname, "kind": "X", "file": file, "line": 1}}


class RunTest(unittest.TestCase):
    """run.py's grading against a canned glia (no real build): every check passes on a right
    answer, and each fails on the answer that breaks it."""

    @classmethod
    def setUpClass(cls):
        import run
        cls.R = run
        cls.cases = gen.cover(gen.AXES["transport"], quiet=True)

    def setUp(self):
        self.tmp = tmpdir()
        fake = self.tmp / "glia"
        fake.write_text(FAKE_GLIA)
        fake.chmod(0o755)
        self.answers = self.tmp / "answers"
        self.answers.mkdir()
        os.environ["FAKE_DIR"] = str(self.answers)
        self.glia = self.R.Glia(fake)

    def tearDown(self):
        shutil.rmtree(self.tmp)
        os.environ.pop("FAKE_DIR", None)

    def answer(self, exp, edge=True, s_breaking=1, named=True, c_breaking=0):
        cl, srv = exp["pair"]["client"], exp["pair"]["server"]
        idx = {r: i for i, r in enumerate([exp["primary"], *exp["with"]])}
        sfile = (srv.get("pair_files") or [srv["route_file"]])[0]
        if exp["transport"] == "rest":
            qa, qb = "endpoint:GET:/api/v1/users/${…} @c", "GET /api/v1/users/:id @s"
        elif exp["transport"] == "evm":  # the server end is the procedure, as glia names it
            qa, qb = "client_of " + exp["exchange"]["key"], exp["exchange"]["procedure"]
        else:
            qa, qb = "client_of " + exp["exchange"]["key"], "server_of " + exp["exchange"]["key"]
        rows = [] if not edge else [{"values": [
            node(qa, cl["file"]), node(qb, sfile), {"str": cl["file"]}, {"str": sfile},
            {"str": f"repo#{idx[cl['repo']]}"}, {"str": f"repo#{idx[srv['repo']]}"}]}]
        (self.answers / "query.json").write_text(json.dumps({"results": rows}))
        client_file = exp["drift"]["client_files"][0]["file"]
        for state, n in (("S", s_breaking), ("C", c_breaking)):
            row = {"status": "breaking", "client": {"file": client_file if named else "x"}}
            (self.answers / f"cb_{state}.json").write_text(json.dumps(
                {"schemas": [], "models": [row] * n, "orphaned_clients": [], "breaking": n,
                 "advisory": 0}))

    def grade(self, case, **kw):
        _, exp = gen.render_case(case)
        self.answer(exp, **kw)
        return self.R.run_case(self.glia, case, self.tmp / "work")

    @unittest.skipUnless(shutil.which("git"), "git absent")
    def test_right_answer_passes_every_transport(self):
        for t in gen.AXES["transport"]:
            for pack in ("single", "multi-repo"):
                case = next((c for c in self.cases if c["axes"]["transport"] == t
                             and c["axes"]["packaging"] == pack), None)
                if case is None:
                    continue
                row = self.grade(case)
                if t == "evm":
                    self.assertEqual(row["cells"], sorted({
                        f"{self.R.LANG[case['axes']['client']]}/evm", "solidity/evm"}))
                self.assertEqual(row["checks"], {c: True for c in self.R.CHECKS},
                                 (case["id"], row["evidence"]))
                self.assertTrue(row["pass"])
                self.assertNotIn("errors", row)

    @unittest.skipUnless(shutil.which("git"), "git absent")
    def test_evm_case_grades_with_run_tables(self):
        """An evm-only cover's case follows run.py's grading interface with run.py's own cell
        tables (FH.23): a right answer passes every check."""
        cases = gen.cover(["evm"], quiet=True)
        for pack in ("single", "multi-repo"):
            case = next(c for c in cases if c["axes"]["packaging"] == pack)
            row = self.grade(case)
            self.assertEqual(row["checks"], {c: True for c in self.R.CHECKS},
                             (case["id"], row["evidence"]))
            self.assertIn(row["cells"][0].split("/")[1], {"evm"})

    @unittest.skipUnless(shutil.which("git"), "git absent")
    def test_each_check_fails_alone(self):
        case = next(c for c in self.cases if c["axes"]["transport"] == "rest")
        bad = {"pair": {"edge": False}, "drift_s": {"s_breaking": 0},
               "clear_c": {"c_breaking": 1}, "client": {"named": False}}
        for check, kw in bad.items():
            row = self.grade(case, **kw)
            failed = sorted(k for k, v in row["checks"].items() if not v)
            # a gate that never broke at S cannot clear at C, nor name a client
            want = ["clear_c", "client", "drift_s"] if check == "drift_s" else [check]
            self.assertEqual(failed, want, check)
            self.assertFalse(row["pass"])

    def test_channel_match(self):
        exp = {"transport": "rest", "exchange": {"path_norm": "/api/v1/users/{}"}}
        m = self.R.channel_match
        self.assertEqual(m(exp, "endpoint:GET:/api/v1/users/${…} @c", "x"), "exact")
        self.assertEqual(m(exp, "GET /api/v1/users/{id}", "x"), "exact")
        self.assertEqual(m(exp, "endpoint:GET:/users/${…} @c", "GET /users/:id @s"), "suffix")
        self.assertIsNone(m(exp, "endpoint:GET:/{id} @c", "GET /:id @s"))
        self.assertIsNone(m(exp, "endpoint:GET:/api/v1/orders/${…}", "GET /orders/:id"))
        g = {"transport": "graphql", "exchange": {"match": [r"\bGetUser\b"]}}
        self.assertEqual(m(g, "graphql_op:GetUser", "graphql_resolver:user"), "exact")
        self.assertIsNone(m(g, "graphql_op:GetUsers", "graphql_resolver:users"))

    def test_summary_and_marker(self):
        rows = []
        for i, c in enumerate(self.cases):
            ok = i % 2 == 0
            rows.append({"id": c["id"], "axes": c["axes"], "chains": self.R.chains_of(c["axes"]),
                         "cells": self.R.cells_of(c["axes"]), "pass": ok,
                         "checks": {k: ok for k in self.R.CHECKS}})
        s = self.R.summarise(rows, gen.AXES["transport"], gen.SEED)
        self.assertEqual(s["pairs_covered"], s["pairs_total"])
        self.assertEqual(sorted(s["chains"]), sorted(self.R.cells.CHAIN_IDS))
        self.assertEqual(s["chains"]["sfc_service_endpoint"]["cases"], 0)
        for p in s["pairs"]:
            self.assertEqual(p["share"], round(p["passed"] / p["cases"], 3))
        k = sum(1 for r in rows if r["pass"])
        self.assertEqual(self.R.marker(rows, s),
                         f"[sibyl] cases={len(rows)} pairs_covered={s['pairs_total']}/"
                         f"{s['pairs_total']} pass={k} fail={len(rows) - k} (pair={k} "
                         f"drift_s={k} clear_c={k} client={k})")
        for cell in s["cells"]:
            lang, mech = cell.split("/")
            self.assertIn(mech, {"http_client", "http_server", "grpc", "graphql", "kafka", "ws",
                                 "evm"})
        self.assertTrue(any(c.endswith("/evm") for c in s["cells"]), sorted(s["cells"]))


# ---------------------------------------------------------------------------- syntax checks

TS_CHECK = r"""
const ts = require(process.argv[2]);
const fs = require('fs');
let bad = 0;
for (const f of process.argv.slice(3)) {
  const out = ts.transpileModule(fs.readFileSync(f, 'utf8'), {
    reportDiagnostics: true, fileName: f,
    compilerOptions: {target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS,
                      experimentalDecorators: true},
  });
  for (const d of out.diagnostics || []) {
    bad++;
    console.log(f + ': ' + ts.flattenDiagnosticMessageText(d.messageText, '\n'));
  }
}
process.exit(bad ? 1 : 0);
"""

JAVA_CHECK = r"""
import com.sun.source.util.JavacTask;
import java.util.List;
import javax.tools.*;

public class Parse {
    public static void main(String[] args) throws Exception {
        JavaCompiler c = ToolProvider.getSystemJavaCompiler();
        DiagnosticCollector<JavaFileObject> d = new DiagnosticCollector<>();
        StandardJavaFileManager fm = c.getStandardFileManager(d, null, null);
        JavacTask t = (JavacTask) c.getTask(null, fm, d, List.of("-proc:none"), null,
                fm.getJavaFileObjects(args));
        t.parse();
        int bad = 0;
        for (Diagnostic<? extends JavaFileObject> x : d.getDiagnostics()) {
            if (x.getKind() == Diagnostic.Kind.ERROR) {
                bad++;
                System.out.println(x);
            }
        }
        System.exit(bad == 0 ? 0 : 1);
    }
}
"""


def typescript_module():
    env = os.environ.get("SIBYL_TYPESCRIPT")
    if env:
        return env if Path(env).exists() else None
    cands = []
    npm = shutil.which("npm")
    if npm:
        r = subprocess.run([npm, "root", "-g"], capture_output=True, text=True)
        cands.append(Path(r.stdout.strip()) / "typescript")
    cands += [Path(p) for p in glob.glob(str(Path.home() / ".npm/_npx/*/node_modules/typescript"))]
    best = None
    for c in cands:
        pj = c / "package.json"
        if pj.exists():
            v = tuple(int(x) for x in re.findall(r"\d+", json.loads(pj.read_text())["version"])[:3])
            if best is None or v > best[0]:
                best = (v, str(c))
    return best[1] if best else None


# No checker may reach the network (a go directive newer than the local toolchain would fetch).
OFFLINE = {**os.environ, "GOTOOLCHAIN": "local", "GOPROXY": "off", "GOFLAGS": "-mod=mod"}


def run(cmd, cwd=None):
    r = subprocess.run(cmd, capture_output=True, text=True, cwd=cwd, env=OFFLINE)
    return r.returncode == 0, (r.stdout + r.stderr).strip()


class Checker:
    """One parse-only check per language: `check(paths) -> (ok, output)`, or `why` it is
    skipped. Batched; on a failure the files are re-checked one by one to name the culprit."""

    def __init__(self, work):
        self.work = work
        self.ts = typescript_module() if shutil.which("node") else None
        if shutil.which("java") and shutil.which("javac"):
            (work / "Parse.java").write_text(JAVA_CHECK)
        (work / "tscheck.js").write_text(TS_CHECK)

    def skip_reason(self, lang):
        need = {"ts": None if (shutil.which("node") and self.ts) else "node+typescript absent",
                "go": None if shutil.which("gofmt") else "gofmt absent",
                "gomod": None if shutil.which("go") else "go absent",
                "rust": None if shutil.which("rustfmt") else "rustfmt absent",
                "java": None if shutil.which("javac") else "javac absent",
                "dart": None if shutil.which("dart") else "dart absent",
                "swift": None if shutil.which("swiftc") else "swiftc absent",
                "kotlin": "no parse-only kotlin toolchain",
                "csharp": "no parse-only C# toolchain",
                "proto": None if shutil.which("protoc") else "protoc absent",
                "solidity": None if shutil.which("solc") else "solc absent",
                "gowork": None if shutil.which("go") else "go absent",
                "graphql": None if _has_module("graphql") else "graphql-core absent",
                "yaml": None if _has_module("yaml") else "PyYAML absent"}
        return need.get(lang)

    def check(self, lang, paths):
        paths = [str(p) for p in paths]
        if lang == "python":
            return self._each(paths, lambda p: ast.parse(Path(p).read_text(), p))
        if lang == "json":
            return self._each(paths, lambda p: json.loads(Path(p).read_text()))
        if lang == "toml":
            return self._each(paths, lambda p: tomllib.loads(Path(p).read_text()))
        if lang == "xml":
            return self._each(paths, lambda p: ET.fromstring(Path(p).read_text()))
        if lang == "gomod":  # `go mod edit -json` parses one go.mod, offline
            return self._each(paths, lambda p: subprocess.run(
                ["go", "mod", "edit", "-json", p], capture_output=True, check=True, env=OFFLINE))
        if lang == "gowork":  # `go work edit -json` parses one go.work, offline
            return self._each(paths, lambda p: subprocess.run(
                ["go", "work", "edit", "-json", p], capture_output=True, check=True, env=OFFLINE))
        if lang == "solidity":  # each file alone: solc parses, no imports to resolve
            return self._each(paths, lambda p: subprocess.run(
                ["solc", "--stop-after", "parsing", p], capture_output=True, check=True))
        if lang == "yaml":
            import yaml
            return self._each(paths, lambda p: yaml.safe_load(Path(p).read_text()))
        if lang == "graphql":
            import graphql
            return self._each(paths, lambda p: graphql.parse(Path(p).read_text()))
        if lang == "proto":  # each file alone: the cases' protos share one import path
            return self._each(paths, lambda p: subprocess.run(
                ["protoc", "--proto_path", str(Path(p).parent),
                 f"--descriptor_set_out={os.devnull}", str(p)],
                capture_output=True, check=True))
        cmd = {
            "ts": ["node", str(self.work / "tscheck.js"), str(self.ts or "")],
            "go": ["gofmt", "-e", "-l"],
            "rust": ["rustfmt", "--edition", "2021", "--emit", "stdout"],
            "java": ["java", str(self.work / "Parse.java")],
            "dart": ["dart", "format", "--output=none"],
            "swift": ["swiftc", "-parse"],
        }[lang]
        ok, out = run(cmd + paths)
        if ok or len(paths) == 1:
            return ok, out
        bad = [p for p in paths if not run(cmd + [p])[0]]
        return False, f"{out}\nfailing: {bad}"

    @staticmethod
    def _each(paths, fn):
        errs = []
        for p in paths:
            try:
                fn(p)
            except Exception as e:  # noqa: BLE001 - every parse error is a finding
                errs.append(f"{p}: {e}")
        return not errs, "\n".join(errs)


def _has_module(name):
    try:
        __import__(name)
    except ImportError:
        return False
    return True


def lang_of(path):
    name = Path(path).name
    if name.endswith(".gradle.kts") or name.endswith(".kt"):
        return "kotlin"
    if name == "go.work":
        return "gowork"
    ext = Path(path).suffix
    return {".ts": "ts", ".go": "go", ".rs": "rust", ".java": "java", ".dart": "dart",
            ".swift": "swift", ".py": "python", ".cs": "csharp", ".json": "json",
            ".toml": "toml", ".csproj": "xml", ".yaml": "yaml", ".yml": "yaml",
            ".mod": "gomod", ".proto": "proto", ".graphqls": "graphql", ".graphql": "graphql",
            ".gql": "graphql", ".sol": "solidity"}.get(ext)


# A file each checker must reject: the control that proves a check is not a pass-through.
BROKEN = {
    "ts": ("bad.ts", "export function f( {\n"),
    "go": ("bad.go", "package main\nfunc main() { x := }\n"),
    "gomod": ("go.mod", "module example.com/x\n\ngo 1.22 1.23\nrequire (\n"),
    "rust": ("bad.rs", "fn main() { let x = ; }\n"),
    "java": ("Bad.java", "public class Bad { void f( { }\n"),
    "dart": ("bad.dart", "void main() { final x = ; }\n"),
    "swift": ("bad.swift", "func f( {\n"),
    "python": ("bad.py", "def f(:\n"),
    "json": ("bad.json", "{\"a\": }"),
    "toml": ("bad.toml", "a = \n"),
    "xml": ("bad.csproj", "<Project><a></Project>"),
    "yaml": ("bad.yaml", "a: [1,\n"),
    "proto": ("bad.proto", 'syntax = "proto3";\nmessage M { string a = ; }\n'),
    "graphql": ("bad.graphqls", "type Query {\n  user(id: ID!: User\n}\n"),
    "gowork": ("go.work", "go 1.22\n\nuse (\n"),
    "solidity": ("Bad.sol", "pragma solidity ^0.8.24;\ncontract Bad { function f( {} }\n"),
}


class SyntaxTest(unittest.TestCase):
    def test_rendered_code_parses(self):
        work = tmpdir()
        try:
            chk = Checker(work)
            by_lang = {}
            for i, c in enumerate(every_case() + gen.cover(["evm"], quiet=True)):
                states, _ = gen.render_case(c)
                for state, by_repo in states.items():
                    for repo, files in by_repo.items():
                        root = work / "cases" / f"{i:03d}-{c['id']}" / state / repo
                        gen.write_tree(root, files)
                        for rel in files:
                            lang = lang_of(rel)
                            self.assertIsNotNone(lang, f"no checker class for {rel}")
                            by_lang.setdefault(lang, []).append(root / rel)
            checked, skipped, notes = 0, 0, []
            for lang in sorted(by_lang):
                paths = by_lang[lang]
                why = chk.skip_reason(lang)
                if why:
                    skipped += len(paths)
                    notes.append(f"{lang}:{len(paths)} {why}")
                    continue
                name, text = BROKEN[lang]
                bad = work / "control" / lang / name
                bad.parent.mkdir(parents=True, exist_ok=True)
                bad.write_text(text)
                ok, out = chk.check(lang, [bad])
                self.assertFalse(ok, f"{lang} checker accepted a broken file: {out}")
                ok, out = chk.check(lang, paths)
                self.assertTrue(ok, f"{lang}: {out}")
                checked += len(paths)
            print(f"[sibyl] syntax checked={checked} skipped={skipped} ({', '.join(notes)})",
                  file=sys.stderr)
            self.assertGreater(checked, 0)
        finally:
            shutil.rmtree(work)


class ProtoTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("protoc") and _has_module("google.protobuf"),
                         "protoc or the protobuf runtime absent")
    def test_pb2_descriptor_is_protocs(self):
        """users_pb2.py's serialized descriptor is byte for byte the one protoc writes for the
        case's .proto, in both field states."""
        from google.protobuf import descriptor_pb2

        from templates import grpc
        work = tmpdir()
        try:
            for f in (OLD, NEW):
                d = work / f.camel / "users" / "v1"
                d.mkdir(parents=True)
                (d / "users.proto").write_text(grpc.proto(f))
                out = work / f"{f.camel}.pb"
                subprocess.run(["protoc", "--proto_path", str(work / f.camel),
                                f"--descriptor_set_out={out}", str(d / "users.proto")],
                               check=True, capture_output=True)
                real = descriptor_pb2.FileDescriptorSet.FromString(out.read_bytes()).file[0]
                self.assertEqual(real.SerializeToString(), grpc._pb_bytes(f), f.camel)
        finally:
            shutil.rmtree(work)


if __name__ == "__main__":
    unittest.main(verbosity=2)
