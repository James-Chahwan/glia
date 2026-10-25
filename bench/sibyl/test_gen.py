#!/usr/bin/env python3
"""Hermetic tests for bench/sibyl/gen.py (FH.11a): the cover holds every expressible axis pair
and no unexpressible one, the cases render deterministically with their drift by construction,
the git form has P / S / C, and every rendered file parses where a parse-only toolchain is
present (each checker first proven to reject a broken file). No glia, no network.

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
from templates.common import NEW, OLD  # noqa: E402

ALL = ",".join(gen.AXES["transport"])


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

    def test_unrendered_transport_is_refused(self):
        with self.assertRaises(SystemExit):
            gen.render_case({"id": "x", "axes": {"transport": "grpc"}})

    def test_readme_lists_axes_and_unexpressible_pairs(self):
        text = (HERE / "README.md").read_text()
        for axis, values in gen.AXES.items():
            self.assertIn(f"`{axis}`", text)
            for v in values:
                self.assertIn(f"`{v}`", text, f"README misses {axis} value {v}")
        for a, b, _ in gen.unexpressible():
            self.assertRegex(text, rf"`{re.escape(a[1])}`[^\n]*`{re.escape(b[1])}`",
                             f"README misses unexpressible {a} x {b}")


SPELL_OLD = OLD.spellings()
SPELL_NEW = NEW.spellings()


def holds(text, names):
    return any(re.search(rf"\b{re.escape(n)}\b", text) for n in names)


class RenderTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = gen.cover(["rest"], quiet=True)
        cls.renders = {c["id"]: gen.render_case(c) for c in cls.cases}

    def test_every_value_renders(self):
        seen = {k: set() for k in gen.AXES}
        for c in self.cases:
            for k, v in c["axes"].items():
                seen[k].add(v)
        for k, vs in gen.AXES.items():
            if k != "transport":
                self.assertEqual(seen[k], set(vs), k)

    def test_render_is_deterministic(self):
        c = self.cases[0]
        self.assertEqual(gen.render_case(c), self.renders[c["id"]])

    def test_expected_by_construction(self):
        for c in self.cases:
            states, exp = self.renders[c["id"]]
            cid = c["id"]
            for state in ("P", "S", "C"):
                for key in ("client", "reader"):
                    ref = exp[key] if key == "reader" else exp["pair"]["client"]
                    self.assertIn(ref["file"], states[state][ref["repo"]], f"{cid} {key}")
                srv = exp["pair"]["server"]
                for f in ("route_file", "handler_file", "model_file"):
                    self.assertIn(srv[f], states[state][srv["repo"]], f"{cid} {f}")
                self.assertIn(srv["handler"], states[state][srv["repo"]][srv["handler_file"]])
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
            # The call file names the resource, and the whole path when it is a literal.
            call = states["P"][cli_repo][exp["pair"]["client"]["file"]]
            self.assertIn("users", call.lower(), cid)
            if c["axes"]["wiring_source"] == "literal":
                self.assertIn("api/v1/users", call, cid)

    def test_layouts(self):
        for c in self.cases:
            states, exp = self.renders[c["id"]]
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
                "yaml": None if _has_yaml() else "PyYAML absent"}
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
        if lang == "yaml":
            import yaml
            return self._each(paths, lambda p: yaml.safe_load(Path(p).read_text()))
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


def _has_yaml():
    try:
        import yaml  # noqa: F401
    except ImportError:
        return False
    return True


def lang_of(path):
    name = Path(path).name
    if name.endswith(".gradle.kts") or name.endswith(".kt"):
        return "kotlin"
    ext = Path(path).suffix
    return {".ts": "ts", ".go": "go", ".rs": "rust", ".java": "java", ".dart": "dart",
            ".swift": "swift", ".py": "python", ".cs": "csharp", ".json": "json",
            ".toml": "toml", ".csproj": "xml", ".yaml": "yaml", ".mod": "gomod"}.get(ext)


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
}


class SyntaxTest(unittest.TestCase):
    def test_rendered_code_parses(self):
        work = tmpdir()
        try:
            chk = Checker(work)
            by_lang = {}
            for c in gen.cover(["rest"], quiet=True):
                states, _ = gen.render_case(c)
                for state, by_repo in states.items():
                    for repo, files in by_repo.items():
                        root = work / "cases" / c["id"] / state / repo
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
