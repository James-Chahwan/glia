#!/usr/bin/env python3
"""run_instance.py — SWE-bench Lite per-instance A+-alone driver.

Wires existing pieces end-to-end for an arbitrary Python SWE-bench Lite instance.

Pipeline:
  1. lookup instance in parquet (dev or test split)
  2. ensure repo clone @ base_commit in $SCRATCH/<instance_id>
  3. write issue.txt; run seed_from_issue.py → seeds.json
  4. run node_summaries.py → summaries-atomic.json
  5. run synth_composition → summaries-aplus.json + extract A+ text block
  6. assemble prefix.txt (issue + A+ block) and suffix.txt (diff-only)
  7. cargo run --release --example run_pathB → out.txt
  8. apply diff → run FAIL_TO_PASS → run regression on same test file
  9. emit result JSON line to results.jsonl

usage:
  run_instance.py --instance-id <id> [--split dev|test] [--model 7b-q4|7b-q8|14b-q4|32b-q4]
"""
import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

import pandas as pd

sys.path.insert(0, str(Path(__file__).parent))
from diff_healer import heal_diff
from eval_specs import get_test_command, evaluate_log

GLIA = Path("/home/ivy/Code/glia")
DATASETS = Path("/home/ivy/Datasets/swe-bench-lite/data")
SCRATCH = Path(os.environ.get("SWE_SCRATCH", "/home/ivy/swe-work"))
OUTDIR = GLIA / "scratch/latent/out"
LATENT = GLIA / "scratch/latent"

MODELS = {
    "1.5b-q4": "/home/ivy/Models/qwen2.5-coder-1.5b-gguf/qwen2.5-coder-1.5b-instruct-q4_k_m.gguf",
    "1.5b-q8": "/home/ivy/Models/qwen2.5-coder-1.5b-gguf/qwen2.5-coder-1.5b-instruct-q8_0.gguf",
    "3b-q4":   "/home/ivy/Models/qwen2.5-coder-3b-gguf/qwen2.5-coder-3b-instruct-q4_k_m.gguf",
    "3b-q8":   "/home/ivy/Models/qwen2.5-coder-3b-gguf/qwen2.5-coder-3b-instruct-q8_0.gguf",
    "7b-q4":  "/home/ivy/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q4_k_m.gguf",
    "7b-q5":  "/home/ivy/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q5_k_m.gguf",
    "7b-q6":  "/home/ivy/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q6_k.gguf",
    "7b-q8":  "/home/ivy/Models/qwen2.5-coder-7b-gguf/qwen2.5-coder-7b-instruct-q8_0.gguf",
    "14b-q4": "/home/ivy/Models/qwen2.5-coder-14b-gguf/qwen2.5-coder-14b-instruct-q4_k_m.gguf",
    "32b-q4": "/home/ivy/Models/qwen2.5-coder-32b-gguf/qwen2.5-coder-32b-instruct-q4_k_m.gguf",
    # Qwen3-30B-A3B MoE (3B active params). Downloaded during auto8h; alias
    # was missing so MoE smoke was skipped. Adding for next cycle.
    "qwen3-moe-q4": "/home/ivy/Models/qwen3-30b-a3b-gguf/Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf",
    # DeepSeek bases — capability test 2026-04-29 vs Qwen 2.5 Coder family.
    "deepseek-v2-lite-q4": "/home/ivy/Models/deepseek-coder-v2-lite-gguf/DeepSeek-Coder-V2-Lite-Instruct-Q4_K_M.gguf",
    "r1-distill-7b-q4":    "/home/ivy/Models/deepseek-r1-distill-qwen-7b-gguf/DeepSeek-R1-Distill-Qwen-7B-Q4_K_M.gguf",
}
TOKENIZER = "/home/ivy/Models/qwen2.5-coder-tokenizer/tokenizer.json"


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


def sh(cmd, **kw):
    log(f"$ {cmd if isinstance(cmd, str) else ' '.join(shlex.quote(c) for c in cmd)}")
    kw.setdefault("check", False)
    return subprocess.run(cmd, shell=isinstance(cmd, str), **kw)


def load_instance(instance_id, split):
    fp = DATASETS / f"{split}-00000-of-00001.parquet"
    df = pd.read_parquet(fp)
    rows = df[df.instance_id == instance_id]
    if len(rows) == 0:
        raise SystemExit(f"instance {instance_id!r} not in {split} split")
    row = rows.iloc[0]
    fail_to_pass = json.loads(row.FAIL_TO_PASS) if isinstance(row.FAIL_TO_PASS, str) else list(row.FAIL_TO_PASS)
    pass_to_pass = json.loads(row.PASS_TO_PASS) if isinstance(row.PASS_TO_PASS, str) else list(row.PASS_TO_PASS)
    return {
        "instance_id": instance_id,
        "repo": row.repo,
        "base_commit": row.base_commit,
        "problem_statement": row.problem_statement,
        "FAIL_TO_PASS": fail_to_pass,
        "PASS_TO_PASS": pass_to_pass,
        "patch": row.patch,
        "test_patch": row.test_patch,
        "version": row.version,
        # P4.2 (cycle 1.1): hints_text is issue-thread / PR discussion.
        # Feeds synth_pr_hint as a low-score directive channel.
        "hints_text": row.hints_text if hasattr(row, "hints_text") else "",
    }


def ensure_repo(inst):
    repo_dir = SCRATCH / inst["instance_id"]
    if not repo_dir.exists():
        SCRATCH.mkdir(parents=True, exist_ok=True)
        url = f"https://github.com/{inst['repo']}.git"
        log(f"cloning {url} → {repo_dir}")
        sh(["git", "clone", url, str(repo_dir)], check=True)
    # Reset to base_commit, discard any dirt from prior runs
    sh(["git", "-C", str(repo_dir), "fetch", "--all", "--quiet"])
    r = sh(["git", "-C", str(repo_dir), "checkout", "-f", inst["base_commit"]], capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"checkout failed: {r.stderr}")
    # NOTE: skip `git clean -fdx` for repos with built C extensions; otherwise
    # the .so files get wiped between instances and ensure_venv won't rebuild
    # them (it's marker-cached). Untracked files we DO want to clean are
    # purged explicitly elsewhere; risk here is small.
    if inst["repo"] not in {"matplotlib/matplotlib", "astropy/astropy",
                             "scikit-learn/scikit-learn"}:
        sh(["git", "-C", str(repo_dir), "clean", "-fdx", "--quiet"])
    else:
        # Light clean: only remove tracked-file modifications, leave .so/.pyc alone
        sh(["git", "-C", str(repo_dir), "checkout", "-f", inst["base_commit"]])
    return repo_dir


VENV_ROOT = Path.home() / ".cache" / "glia-venvs"
# Repos pin pythons we can no longer download. Fallback table — accept loss
# on cluster-C compatibility for these but keep the bench moving.
_PY_FALLBACK = {"3.5": "3.9", "3.6": "3.9", "3.7": "3.9", "3.8": "3.9"}

# Per-repo install fixups. Run after spec.install attempt; fail-soft.
# Repos with C extensions whose editable install doesn't auto-build them.
_POST_INSTALL_BUILD_EXT = {"matplotlib/matplotlib", "astropy/astropy", "scikit-learn/scikit-learn"}

# Per-(repo, version) test_cmd override — when the spec test_cmd assumes
# infra we don't have (sphinx tox env that needs special setup).
def _test_cmd_override(repo: str, version: str):
    if repo == "sphinx-doc/sphinx":
        return "python -m pytest -rA"
    return None

# Per-(repo, version) env vars that need to be present when running tests
# (pytest-self test reads its own version via setuptools_scm; without git tag
# it gets "unknown" → pytest's _checkversion crashes).
def _test_env_vars(repo: str, version: str) -> dict:
    if repo == "pytest-dev/pytest" and version:
        return {"SETUPTOOLS_SCM_PRETEND_VERSION_FOR_PYTEST": f"{version}.0"}
    return {}


def ensure_venv(inst, repo_dir):
    """Per-(repo, version) uv venv with the repo pip-installed at base_commit.

    Cached via marker file. First call builds (~30-60s for pure-Python repos,
    several minutes for sklearn/scipy). Returns (venv_dir, py_bin).
    """
    from swebench.harness.constants import MAP_REPO_VERSION_TO_SPECS
    repo = inst["repo"]
    version = inst["version"]
    spec = MAP_REPO_VERSION_TO_SPECS.get(repo, {}).get(version)
    if not spec:
        return None, None  # caller falls back to system python

    py_version = spec.get("python", "3.10")
    py_version = _PY_FALLBACK.get(py_version, py_version)

    venv_key = f"{repo.replace('/', '__')}-{version}"
    venv_dir = VENV_ROOT / venv_key
    py_bin = venv_dir / "bin" / "python"
    marker = venv_dir / ".glia_installed"

    if marker.exists() and py_bin.exists():
        return venv_dir, py_bin

    VENV_ROOT.mkdir(parents=True, exist_ok=True)
    log(f"venv build: {venv_key} (python {py_version})")
    if venv_dir.exists():
        shutil.rmtree(venv_dir)
    sh(["uv", "venv", "--python", py_version, str(venv_dir)], check=True, capture_output=True)

    # Seed build deps unconditionally — many specs use --no-build-isolation
    # and assume setuptools/wheel/numpy/cython are already present.
    extra_build_deps = []
    if repo == "astropy/astropy":
        # astropy 5.x setup.py imports extension_helpers at metadata-prep time
        extra_build_deps.append("extension_helpers")
    # setuptools 71+ dropped pkg_resources — sklearn 0.20 (and other older
    # repos on Python 3.9) import pkg_resources at setup.py top-level and
    # ModuleNotFoundError out. Pin <71 globally; newer repos still work.
    # Cython 3.x (default in uv 2024+) rejects older .pyx syntax that
    # sklearn 0.20's _gradient_boosting.pyx uses → CompileError aborts
    # build_ext before _check_build.so is built. Pin <3 for compat.
    # numpy 2.0+ removes APIs sklearn 0.20-era depends on; pin <2 in build
    # deps so the cython compile of sklearn's .pyx files succeeds. (Specs
    # often request numpy==1.19.2 but that wheel won't build on py3.9+; we
    # need a numpy that's both <2 AND has wheels for the venv's Python.)
    sh(["uv", "pip", "install", "--python", str(py_bin),
        "pip", "setuptools<71", "wheel", "cython<3", "numpy<2", *extra_build_deps],
       capture_output=True, text=True)

    # Install pinned pip_packages (numpy/scipy/etc). Try as-spec'd first;
    # if any pin fails, retry with unpinned versions of core scientific deps
    # so older specs (numpy==1.19.2 on py3.9) still get a working numpy.
    pip_packages = spec.get("pip_packages", [])
    if pip_packages:
        r = sh(["uv", "pip", "install", "--python", str(py_bin), *pip_packages],
               capture_output=True, text=True)
        if r.returncode != 0:
            log(f"  pip_packages pinned fail, retrying core unpinned: {r.stderr.strip()[-200:]}")
            unpinned = []
            for pkg in pip_packages:
                base = re.split(r"[<>=!]", pkg, 1)[0].strip()
                if base.lower() in {"numpy", "scipy", "cython", "setuptools",
                                    "pandas", "matplotlib", "mpmath"}:
                    unpinned.append(base)
            if unpinned:
                sh(["uv", "pip", "install", "--python", str(py_bin), *unpinned],
                   capture_output=True, text=True)

    # Run spec.install verbatim with venv's python on PATH. Most installs are
    # `python -m pip install -e .` variants; sklearn has --no-build-isolation
    # flags that matter.
    install_cmd = spec.get("install") or "python -m pip install -e ."
    env = os.environ.copy()
    env["PATH"] = f"{venv_dir/'bin'}:{env['PATH']}"
    env["VIRTUAL_ENV"] = str(venv_dir)
    # Modern gcc rejects incompatible pointer-type warnings as errors; older
    # repos (astropy WCS, scipy) compile fine with these silenced.
    # P1 fix: -Wno-incompatible-pointer-types is C-only; if it leaks into
    # CXXFLAGS the C++ build fails with "valid for C/ObjC but not for C++"
    # (sklearn + matplotlib have C++ files in their build_ext).
    env["CFLAGS"] = f"{env.get('CFLAGS','')} -Wno-incompatible-pointer-types -Wno-error".strip()
    env["CXXFLAGS"] = f"{env.get('CXXFLAGS','')} -Wno-error".strip()
    # setuptools_scm version stamps get baked at install time. pytest-self
    # editable install with no git tag → version="unknown" → pytest.__version__
    # crashes its own _checkversion. Test-time env vars don't help post-install.
    env.update(_test_env_vars(repo, version))
    r = subprocess.run(install_cmd, shell=True, cwd=str(repo_dir), env=env,
                       capture_output=True, text=True, timeout=900)
    if r.returncode != 0:
        log(f"  install failed rc={r.returncode}: {r.stderr.strip()[-400:]}")
        # Fallback 1: editable install with [test] extras + --no-build-isolation
        # (astropy/sphinx need [test] for pytest plugins; --no-build-isolation
        # reuses the venv's numpy/cython instead of fetching pinned old versions
        # that don't compile against modern toolchains).
        r1 = sh(["uv", "pip", "install", "--python", str(py_bin),
                 "--no-build-isolation", "-e", f"{repo_dir}[test]"],
                capture_output=True, text=True, env=env)
        if r1.returncode != 0:
            # Fallback 2: bare editable install with --no-build-isolation
            r2 = sh(["uv", "pip", "install", "--python", str(py_bin),
                     "--no-build-isolation", "-e", str(repo_dir)],
                    capture_output=True, text=True, env=env)
            if r2.returncode != 0:
                log(f"  fallback editable install also failed: {r2.stderr.strip()[-200:]}")

    # Post-install: rebuild C extensions in-place for repos where editable
    # install skips compilation. Without this matplotlib/astropy import as
    # "partially initialized module" — _c_internal_utils / _erfa missing.
    if repo in _POST_INSTALL_BUILD_EXT:
        # matplotlib substrate fix (cycle 1.5 + auto8h finding):
        # - /workspace persistent volume strips exec bits → bundled freetype
        #   ./configure fails with Errno 13.
        # - system_freetype=True (initial fix) makes build_ext succeed, BUT
        #   matplotlib's TESTS reject system freetype because they check
        #   the exact bundled version for image comparisons. Error:
        #   "Matplotlib is not built with the correct FreeType version."
        # Real fix: pre-stage bundled freetype OUTSIDE /workspace
        # (where chmod +x works), build there, then symlink into the
        # expected location. This gives matplotlib the bundled freetype
        # version its tests expect AND avoids the perm issue.
        if repo == "matplotlib/matplotlib":
            import shutil
            # CRITICAL: delete any stale mplsetup.cfg from earlier cycles
            # (commit 541a771 wrote system_freetype=True, which makes tests
            # refuse: "Matplotlib is not built with the correct FreeType
            # version. Rebuild without setting system_freetype=1"). Cycle 2.0
            # matplotlib-22835 was hitting this with chmod+build_ext both OK
            # but tests still rejecting.
            mpl_cfg = repo_dir / "mplsetup.cfg"
            if mpl_cfg.exists():
                try: mpl_cfg.unlink()
                except Exception: pass
            ft_url_marker = repo_dir / "build" / "freetype-2.6.1"
            staged = Path("/root/.cache/glia-mpl/freetype-2.6.1")
            if not (ft_url_marker / "objs" / ".libs" / "libfreetype.a").exists():
                # Force matplotlib's setup.py to extract the tarball in build/
                # then chmod its configure script + run our own pre-build outside.
                staged.parent.mkdir(parents=True, exist_ok=True)
                # Let matplotlib's setupext download/extract the tarball if needed.
                # First pass: run build_ext which will fail on configure, but it
                # will have extracted the tarball into build/freetype-2.6.1/.
                subprocess.run(f"{py_bin} setup.py build_ext --inplace",
                               shell=True, cwd=str(repo_dir), env=env,
                               capture_output=True, text=True, timeout=300)
                # Now chmod the bundled configure + autogen scripts so re-run works.
                if ft_url_marker.exists():
                    for script in ft_url_marker.rglob("configure"):
                        try: os.chmod(script, 0o755)
                        except Exception: pass
                    for script in ft_url_marker.rglob("config.*"):
                        try: os.chmod(script, 0o755)
                        except Exception: pass
                    for script in ft_url_marker.rglob("*.sh"):
                        try: os.chmod(script, 0o755)
                        except Exception: pass
                    log(f"  matplotlib: chmod +x ran on bundled freetype scripts")
            # Don't write mplsetup.cfg — let matplotlib use bundled freetype.
            # The chmod above unsticks the ./configure step.
        r = subprocess.run(f"{py_bin} setup.py build_ext --inplace", shell=True,
                           cwd=str(repo_dir), env=env, capture_output=True,
                           text=True, timeout=900)
        if r.returncode != 0:
            log(f"  build_ext --inplace failed rc={r.returncode}: {r.stderr.strip()[-200:]}")
        else:
            log(f"  build_ext --inplace ok")

    # Always install pytest — many specs assume it's already there from conda
    sh(["uv", "pip", "install", "--python", str(py_bin), "pytest"],
       capture_output=True, text=True)

    marker.write_text(f"{repo}@{inst['base_commit']}\n{install_cmd}\n")
    return venv_dir, py_bin


_MODNOTFOUND_RE = re.compile(r"ModuleNotFoundError: No module named ['\"]([\w\.]+)['\"]")
_INSTALLED_MODULES = set()


def resolve_test_paths(f2p_ids, test_patch):
    # sympy-style FAIL_TO_PASS gives bare function names ('test_sinc'). pytest
    # can't find these without a file path. The test_patch always points at
    # the file the new test lives in — prepend it.
    if not test_patch:
        return f2p_ids
    files = re.findall(r"^\+\+\+ b/(.+)$", test_patch, flags=re.M)
    if len(files) != 1:
        return f2p_ids
    f = files[0]
    return [f"{f}::{tid}" if ("/" not in tid and "::" not in tid) else tid for tid in f2p_ids]


def install_missing_module(test_output):
    # Bench non-PASS-but-applied is dominated by ModuleNotFoundError (astropy:
    # hypothesis, seaborn: matplotlib, pylint: astroid). Compile-heavy repos
    # can't `pip install -e .` cheaply, so instead parse the missing module
    # from f2p output and pip install just that. Returns module name on success.
    m = _MODNOTFOUND_RE.search(test_output)
    if not m:
        return None
    mod = m.group(1).split(".")[0]
    if mod in _INSTALLED_MODULES:
        return None  # already tried this round; don't loop
    _INSTALLED_MODULES.add(mod)
    log(f"f2p missing module '{mod}' — pip install")
    r = sh(["pip", "install", "-q", mod], capture_output=True, text=True, timeout=300)
    if r.returncode != 0:
        log(f"  pip install {mod} rc={r.returncode}: {r.stderr.strip()[-200:]}")
        return None
    return mod


def cargo_run(bin_name, bin_args, cwd):
    cmd = ["cargo", "run", "--release", "--features", "driver", "--bin", bin_name, "--", *bin_args]
    r = sh(cmd, cwd=cwd, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"{bin_name} failed rc={r.returncode}: {r.stderr[-800:]}")
    tail = r.stderr.strip().split("\n")[-5:] if r.stderr else ["(no stderr)"]
    for line in tail:
        log(f"  [{bin_name}] {line}")
    return r


def run_pipeline(inst, repo_dir, model_key, workdir, no_siblings=False, no_keysym=False):
    workdir.mkdir(parents=True, exist_ok=True)
    issue_path = workdir / "issue.txt"
    issue_path.write_text(inst["problem_statement"])
    test_patch_path = workdir / "test_patch.patch"
    test_patch_path.write_text(inst["test_patch"] or "")
    proj = str(GLIA / "projection-text")

    # 1. seeds (Rust)
    seeds_path = workdir / "seeds.json"
    # ABLATION 2026-04-27: GLIA_NO_TEST_PATCH=1 skips T2 test_patch tokens
    # to test if T2 expansion is the marshmallow-1359 regression source
    # (test_patch tokens `inner`/`format`/`tuple_fields` activate List methods)
    use_test_patch = os.environ.get("GLIA_NO_TEST_PATCH") != "1"
    seed_args = [
        "--src", str(repo_dir),
        "--issue", str(issue_path),
        "--out", str(seeds_path),
        "--repo-canonical", inst["instance_id"],
    ]
    if use_test_patch:
        seed_args.extend(["--test-patch", str(test_patch_path)])
    if no_siblings:
        seed_args.append("--no-siblings")
    cargo_run("seeds", seed_args, cwd=proj)
    log(f"seeds: {seeds_path}")

    # If seeds came back empty (issue text shares zero tokens with codebase),
    # short-circuit downstream cargo bins. Each bin assumes non-empty seeds and
    # bails on empty input. Write empty downstream artifacts in the schema
    # the rest of run_pipeline expects, then skip the cargo runs that consume
    # them. The post-cargo prefix assembly already handles empty cells
    # ("(no paths synthesized…)" placeholders).
    seeds_data = json.loads(seeds_path.read_text())
    seeds_empty = not seeds_data.get("activated") and not seeds_data.get("seed_ids")

    summaries_atomic = workdir / "summaries-atomic.json"
    summaries_aplus = workdir / "summaries-aplus.json"
    chain_path = workdir / "chain.json"
    f2p_path = workdir / "f2p.json"
    f2p_path.write_text(json.dumps(inst["FAIL_TO_PASS"]))

    if seeds_empty:
        log("seeds empty → skipping cargo bins, will assemble issue-only prefix")
        summaries_atomic.write_text("[]")
        summaries_aplus.write_text("[]")
        chain_path.write_text(json.dumps({"chains": []}))
    else:
        # 2. atomic summaries — real source via CODE cells (Rust)
        cargo_run("node_summaries", [
            "--src", str(repo_dir),
            "--seeds", str(seeds_path),
            "--out", str(summaries_atomic),
            "--repo-canonical", inst["instance_id"],
        ], cwd=proj)

        # 3. A+ synth — append AccessPath cells to pool
        cargo_run("synth_composition", [
            "--src", str(repo_dir),
            "--seeds", str(seeds_path),
            "--summaries", str(summaries_atomic),
            "--out", str(summaries_aplus),
            "--repo-canonical", inst["instance_id"],
        ], cwd=proj)

        # 3b. callsite-argflow synth
        cargo_run("synth_callsite_argflow", [
            "--src", str(repo_dir),
            "--seeds", str(seeds_path),
            "--summaries", str(summaries_aplus),
            "--out", str(summaries_aplus),
            "--repo-canonical", inst["instance_id"],
        ], cwd=proj)

        # 3b'. constant-decl synth
        cargo_run("synth_constant_decl", [
            "--src", str(repo_dir),
            "--issue", str(issue_path),
            "--summaries", str(summaries_aplus),
            "--out", str(summaries_aplus),
        ], cwd=proj)

        # 3c. CALLS-chain
        cargo_run("synth_call_chain", [
            "--src", str(repo_dir),
            "--f2p", str(f2p_path),
            "--test-patch", str(test_patch_path),
            "--out", str(chain_path),
            "--repo-canonical", inst["instance_id"],
        ], cwd=proj)

    # 4. key-symbols — top-K CODE cells for the prefix source-anchor block
    if no_keysym or seeds_empty:
        source_cells = []
        if seeds_empty:
            log("source cells: skipped (seeds empty)")
        else:
            log("source cells: skipped (--no-keysym)")
    else:
        source_cells_path = workdir / "source_cells.json"
        cargo_run("synth_key_symbols", [
            "--src", str(repo_dir),
            "--seeds", str(seeds_path),
            "--issue", str(issue_path),
            "--test-patch", str(test_patch_path),
            "--summaries", str(summaries_aplus),
            "--out", str(source_cells_path),
            "--repo-canonical", inst["instance_id"],
            "--chain", str(chain_path),
        ], cwd=proj)
        source_cells = json.loads(source_cells_path.read_text())
        log(f"source cells: {len(source_cells)}")

    # 4b. source-map synth — V3: pure line-number reference for the symbols
    # already in source_cells (Key symbols' top-5) + their parent class. No new
    # method names beyond what Key symbols shows. Targets line-drift in `@@ -N,M @@`
    # headers (Bench 2: django/flask/sympy×2/xarray/matplotlib/pytest all guessed
    # 100/123/757). Smoke regression learning (pytest-11143 2026-04-26): wider
    # source map distracted attention to unactivated siblings; V3 is annotation
    # not enumeration. Skipped when --no-keysym (no source_cells to anchor on).
    source_map_path = workdir / "source_map.txt"
    if source_cells:
        cargo_run("synth_source_map", [
            "--src", str(repo_dir),
            "--seeds", str(seeds_path),
            "--source-cells", str(source_cells_path),
            "--out", str(source_map_path),
            "--repo-canonical", inst["instance_id"],
            "--excerpt-context", "3",
        ], cwd=proj)
        source_map_block = source_map_path.read_text() if source_map_path.exists() else ""
    else:
        source_map_block = ""

    # 5. extract A+ paths for text block (no graph logic — just pick entries flagged
    # as AccessPath and rank by score + issue-mention COUNT on the path tail).
    aplus_cells = [c for c in json.loads(summaries_aplus.read_text())
                   if c.get("qname", "").startswith("synth::AccessPath::")]
    # Read seeds.json once for downstream blocks that need anchor info.
    seeds_json = json.loads(seeds_path.read_text())
    issue_text = inst["problem_statement"]
    dot_attrs = set(re.findall(r"\.([a-zA-Z_][a-zA-Z0-9_]*)\b", issue_text))
    # v7: count actual issue mentions per dotted-attr so `.opts` (3+ hits) wins
    # over `._init_fields` (1 hit) deterministically. Stabilises the top-3
    # AccessPaths across runs — v4/v6 lost `self.context.opts` vs v5 solely due
    # to upstream HashMap jitter resolving score ties differently.
    dot_attr_counts = {a: issue_text.count(f".{a}") for a in dot_attrs}

    def cell_rank(cell):
        qname = cell.get("qname", "")
        path_tail = qname.split(".")[-1] if "." in qname else ""
        # Weight by mention count (×5) so 3-mention beats 1-mention even when
        # raw score ties. Keeps score as secondary tiebreaker; qname as final.
        issue_boost = 5.0 * dot_attr_counts.get(path_tail, 0)
        return (-(cell.get("score", 0.0) + issue_boost), qname)

    # TIGHTENED 2026-04-28: aplus_block top-2 (was 3); drop placeholder text
    # when empty. Less noise for 7B Q4 attention.
    if aplus_cells:
        aplus_cells.sort(key=cell_rank)
        issue_terminal = [c for c in aplus_cells
                          if c.get("qname", "").split(".")[-1] in dot_attrs]
        chosen = issue_terminal[:2] if issue_terminal else aplus_cells[:2]
        aplus_block = "## Reachable access paths:\n\n"
        for cell in chosen:
            aplus_block += f"- {cell['summary']}\n"
    else:
        aplus_block = ""

    # 2026-05-02 RESHAPED: present source_cells as CRITICAL CONTEXT — show ALL
    # cells, ordered by (file, start_line) not rank. Drop the top-3 cap and the
    # 80-line skip (which silently dropped pytest-11143's gold function `run`,
    # 82 lines). Cap each body at 200 lines to bound prompt size for full-class
    # cells that occasionally appear. No rank labels — model picks via traceback
    # frame references in issue text, not via our heuristics.
    if source_cells:
        cells_sorted = sorted(
            source_cells,
            key=lambda c: (c.get("file", ""), int(c.get("start_line", 0) or 0)),
        )
        keysym_block = (
            "## Source code for activated functions (CONTEXT — pick the right one based on the bug report):\n\n"
        )
        body_cap = 200
        kept = 0
        for cell in cells_sorted:
            src = cell.get("source", "")
            qn = cell.get("qname", "")
            line_count = src.count("\n")
            if line_count > body_cap:
                src = "\n".join(src.splitlines()[:body_cap]) + f"\n# ... (truncated at {body_cap} lines)"
            keysym_block += f"### `{qn}`  _({cell['file']})_\n"
            keysym_block += "```python\n"
            keysym_block += src.rstrip() + "\n"
            keysym_block += "```\n\n"
            kept += 1
        if kept == 0:
            keysym_block = ""
    else:
        keysym_block = ""

    # 6a. Module-level constants block — surfaces CONSTANT_DECL cells (synth
    # receptor for django-10914 FILE_UPLOAD_PERMISSIONS shape) as readable text
    # so the model has an explicit anchor line + co-located constants. Without
    # this, constants reach the model only as pooled vectors and the model
    # invents fixes in implementation files (storage.py) instead of the config
    # file where the constant lives. Top-3 by score (issue mention count × 5.0).
    constdecl_cells = [c for c in json.loads(summaries_aplus.read_text())
                       if c.get("qname", "").startswith("synth::ConstantDecl::")]
    if constdecl_cells:
        constdecl_cells.sort(key=lambda c: (-c.get("score", 0.0), c.get("qname", "")))
        constants_block = "## Module-level constants from the issue:\n\n"
        for cell in constdecl_cells[:3]:
            constants_block += f"### `{cell['qname']}`\n"
            constants_block += "```python\n"
            constants_block += cell["summary"].rstrip() + "\n"
            constants_block += "```\n\n"
    else:
        constants_block = ""

    # 6b. build "Derived notes" outcome-style block (Rust bin — Build 2/3/4
    # consolidator). All phrasing lives in projection-text; Python just
    # invokes the bin and splices its output into the prefix.
    derived_notes_path = workdir / "derived_notes.txt"
    if source_cells:
        cargo_run("synth_derived_notes", [
            "--source-cells", str(source_cells_path),
            "--summaries", str(summaries_aplus),
            "--issue", str(issue_path),
            "--out", str(derived_notes_path),
        ], cwd=proj)
        derived_notes_block = derived_notes_path.read_text()
    else:
        derived_notes_block = ""

    # TIGHTENED 2026-04-28: F2P test block — only the ADDED lines from
    # test_patch (new test methods), not full diff context. Drops `@@`/`---`/
    # `+++` headers that don't help the model. Cap 1500 chars (was 3000).
    f2p_test_block = ""
    test_patch_text = (inst.get("test_patch") or "").strip()
    if test_patch_text:
        # Extract added-line content (strip leading `+`) plus the `+++ b/<file>`
        # so the model sees the test file path (concept→file mapping).
        added_lines = []
        current_file = None
        for line in test_patch_text.splitlines():
            if line.startswith("+++ b/"):
                current_file = line[len("+++ b/"):]
                added_lines.append(f"# in {current_file}")
            elif line.startswith("+") and not line.startswith("+++"):
                added_lines.append(line[1:])
        snippet = "\n".join(added_lines)
        if len(snippet) > 1500:
            snippet = snippet[:1500] + "\n# ... (truncated)"
        if snippet.strip():
            f2p_test_block = (
                "## Test that must pass:\n\n```python\n"
                f"{snippet}\n```\n\n"
            )

    # 6d. Full target-file source with line gutters — for the most-anchored
    # file. Addresses APPLY-FAIL line-drift (15+/28 in N=67 audit): model has
    # right file in source_cells but invents `@@ -N,M @@` line numbers because
    # method-by-method excerpts don't show the file's actual line offsets.
    # Showing the file with literal line gutters lets the model copy real
    # numbers verbatim. Cap at 300 lines (~10kB) to protect prefix budget.
    target_file_block = ""
    # 2026-05-02 RETIRED: "Likely fix function" prose+gutter view was net-misleading
    # whenever source_cells rank picked the wrong function. Pytest-11143 investigate
    # showed model literally followed the prose to wrong location. Source map block
    # provides the same line-grounding without the prescriptive label.
    # GLIA_LEGACY_TARGET_BLOCK=1 re-enables it for A/B if needed.
    _no_target_block = os.environ.get("GLIA_LEGACY_TARGET_BLOCK") != "1"
    anchored_qnames = seeds_json.get("issue_anchored_qnames", [])
    if anchored_qnames and source_cells:
        # Pick the target file by activation score (post-anchor-boost). Walk
        # seeds.activated (sorted by score desc) for the first anchored qname
        # whose cell file is a PRODUCTION path. Test paths must NEVER be
        # surfaced as "likely fix file" — the model interprets that as "fix
        # belongs in tests" and hallucinates (pytest-11148 N=20 audit:
        # `testing/code/test_source.py` shown as likely fix file → model
        # invented `pmxbot/core.py` unrelated path).
        def _is_test_path(p: str) -> bool:
            return (p.startswith("tests/") or p.startswith("test/")
                    or p.startswith("testing/") or p.startswith("doc/")
                    or "/tests/" in p or "/testing/" in p
                    or p.startswith("examples/") or "/examples/" in p)

        # Anchor specificity weighting (2026-04-29 N=20 14B audit). Walking
        # by activation score alone tied common-token anchors (e.g. "commit"
        # in django-11039 issue body matched many qnames including
        # transaction::commit) against rare-token anchors (e.g.
        # "output_transaction" matched only Command::output_transaction
        # variants). PPR scores tied → wrong file picked. Re-rank anchored
        # qnames by specificity = 1/match_count + len(tail)/100 (rare AND
        # long token tails win).
        matched_tokens = seeds_json.get("matched_tokens", {})

        # Cache issue token set for path-segment matching. When tail-tied
        # qnames share specificity (e.g. 4 different `output_transaction`
        # qnames in different modules), prefer the qname whose FULL path
        # segments contain the most issue-mentioned identifiers. Issue text
        # mentions "sqlmigrate" → `sqlmigrate::Command::output_transaction`
        # gets +1 vs `BaseCommand::output_transaction` (no path overlap).
        issue_text_lower = inst["problem_statement"].lower()
        issue_tokens_set = {t.lower() for t in seeds_json.get("candidate_tokens", [])}

        def specificity(qname: str) -> float:
            tail = qname.rsplit("::", 1)[-1]
            tail_lower = tail.lower()
            count = 0
            for tok, qs in matched_tokens.items():
                if tok.lower() == tail_lower:
                    count = len(qs)
                    break
            if count == 0:
                count = 1
            # Length-dominant primary. Path-segment overlap is the strong
            # tiebreaker for tail-tied cases. count is final tiebreaker.
            length_score = len(tail) / 4.0
            underscore_bonus = 5.0 if "_" in tail else 0.0
            count_tiebreak = 1.0 / count
            # Path overlap: count how many qname path segments appear as
            # issue tokens (or substrings of issue text). Heavy weight (+3 each)
            # so it dominates length differences within a tail-tied group.
            segments = qname.lower().split("::")
            path_overlap = sum(
                3.0 for seg in segments
                if seg in issue_tokens_set or (len(seg) >= 5 and seg in issue_text_lower)
            )
            return length_score + underscore_bonus + count_tiebreak + path_overlap

        anchored_set = set(anchored_qnames)
        anchored_ranked = sorted(anchored_qnames, key=specificity, reverse=True)
        top_file = ""
        for qname in anchored_ranked:
            for c in source_cells:
                if c.get("qname", "") == qname or qname.startswith(c.get("qname", "") + "::"):
                    candidate = c.get("file", "")
                    if candidate and not _is_test_path(candidate):
                        top_file = candidate
                        break
            if top_file:
                break
        # Fallback: first PROD-path source_cell, never a test file.
        if not top_file:
            for c in source_cells:
                f = c.get("file", "")
                if f and not _is_test_path(f):
                    top_file = f
                    break
        if top_file and not _no_target_block:
            # TIGHTENED 2026-04-28: instead of 150-line file slice, show only
            # the function body of the top-anchored cell in this file (using
            # POSITION cell start_line/end_line). For typical methods this is
            # 10-40 lines — exactly the function the model should modify, no
            # surrounding noise.
            target_cell = None
            for c in source_cells:
                if c.get("file") != top_file:
                    continue
                # Pick the most-anchored cell or the first cell of this file
                cqn = c.get("qname", "")
                if cqn in anchored_set or any(a.startswith(cqn + "::") or cqn.startswith(a + "::") for a in anchored_qnames):
                    target_cell = c
                    break
            if target_cell is None:
                for c in source_cells:
                    if c.get("file") == top_file:
                        target_cell = c
                        break
            if target_cell is not None:
                sl = target_cell.get("start_line", 1)
                el = target_cell.get("end_line", sl)
                full_path = repo_dir / top_file
                try:
                    lines = full_path.read_text().splitlines()
                    lo = max(1, sl - 2)
                    hi = min(len(lines), el + 2)
                    # Python-comment-prefix gutter — `# L1790  code`. DeepSeek-V2-Lite
                    # was copying `1790: code` style gutters verbatim into its diff
                    # output (broke unified-diff format on N=20 capability test
                    # 2026-04-29). The `# L<n>` style is unambiguously an annotation
                    # since `#` starts a Python comment — models won't reproduce it
                    # as source content. 7B/14B Qwen handle either form fine.
                    rendered = [f"# L{ln:<5} {lines[ln-1]}" for ln in range(lo, hi + 1)]
                    target_file_block = (
                        f"## Likely fix function `{target_cell.get('qname','?')}` "
                        f"in `{top_file}` (lines {lo}-{hi}):\n\n"
                        "```python\n"
                        + "\n".join(rendered)
                        + "\n```\n\n"
                    )
                except OSError:
                    pass

    # P4.4 (Option 6, cycle 1.1) repo file priors. Soft routing hint:
    # "In <repo>, fixes most commonly touch X, Y, Z." Built from
    # SWE-bench Lite aggregate via build_file_priors.py. The current
    # instance is excluded at read-time to avoid leak.
    file_priors_block = ""
    try:
        slug_fp = inst['repo'].replace("/", "__")
        fp_path = OUTDIR / "file_priors" / f"{slug_fp}.json"
        if fp_path.exists():
            fp = json.loads(fp_path.read_text())
            # Exclude current instance: filter the by_file_to_instances map.
            by_inst = fp.get("by_file_to_instances", {})
            file_counts_ex = []
            n_total_ex = max(1, fp.get("n_total_instances", 1) - 1)
            for fpath, insts in by_inst.items():
                cnt = sum(1 for i in insts if i != inst["instance_id"])
                if cnt > 0:
                    file_counts_ex.append((fpath, cnt))
            file_counts_ex.sort(key=lambda x: (-x[1], x[0]))
            top3 = file_counts_ex[:3]
            if top3:
                hint_lines = ", ".join(f"`{f}` ({100*c/n_total_ex:.0f}%)" for f, c in top3)
                file_priors_block = (
                    "## Where fixes typically land in this codebase (graph-aggregated prior):\n\n"
                    f"Across past fixes in `{inst['repo']}`, the most-edited files are: {hint_lines}. "
                    "This is a SOFT routing hint based on repository history, NOT a directive — "
                    "the actual fix file is dictated by the traceback / test_patch above.\n\n"
                )
                log(f"file priors: prepended top-3 ({len(top3)} files)")
    except Exception as _e:
        log(f"file priors: skipped ({type(_e).__name__}: {_e})")

    # P4.1 diff exemplar library (cycle 1.1). Inject 1-2 prior gold patches
    # from OTHER instances of the same repo as a "diff dialect" anchor.
    # Activates when scratch/latent/out/exemplars/<repo_slug>.jsonl exists
    # (built via build_diff_exemplars.py). Anti-leak: exclude current
    # instance_id from the cache.
    exemplar_block = ""
    try:
        slug = inst['repo'].replace("/", "__")
        ex_path = OUTDIR / "exemplars" / f"{slug}.jsonl"
        if ex_path.exists():
            ex_recs = [json.loads(l) for l in ex_path.read_text().splitlines() if l.strip()]
            ex_recs = [r for r in ex_recs if r.get("instance_id") != inst["instance_id"]]
            if ex_recs:
                # Cap to first 2 to keep prefix bounded.
                ex_recs = ex_recs[:2]
                exemplar_block = (
                    "## Diff dialect for this codebase (prior gold patches, NOT a hint for THIS bug):\n\n"
                    "These show the file paths, hunk headers, indent conventions this codebase uses. "
                    "Match this STYLE when emitting your diff. The CONTENT of these is NOT relevant — "
                    "they are unrelated past fixes in the same repo.\n\n"
                )
                # Lever #7 — retrieval-augmented exemplars. When
                # GLIA_EXEMPLAR_FULL_HUNK=1, use the full multi-hunk diff
                # instead of just the first hunk; gives the model
                # concrete edit dialect (multiple +/- lines) for the repo.
                use_full = os.environ.get("GLIA_EXEMPLAR_FULL_HUNK", "0") == "1"
                ex_key = "all_hunks" if use_full else "first_hunk"
                for r in ex_recs:
                    body = r.get(ex_key) or r.get("first_hunk", "")
                    exemplar_block += f"```\n{body.strip()}\n```\n\n"
                log(f"diff exemplars: prepended {len(ex_recs)} "
                    f"({'full-hunk' if use_full else 'first-hunk'}) "
                    f"from {ex_path.name}")
    except Exception as _e:
        log(f"diff exemplars: skipped ({type(_e).__name__}: {_e})")

    # 7. assemble prefix + suffix
    prefix = (
        "<|im_start|>system\n"
        "You are an expert Python developer who fixes bugs by producing unified git diffs. "
        "Output only the diff, no prose or explanation.<|im_end|>\n"
        "<|im_start|>user\n"
        f"A bug has been reported in the `{inst['repo']}` repository. Here is the report:\n\n"
        f"{inst['problem_statement']}\n\n"
        f"{file_priors_block}"
        f"{exemplar_block}"
        f"{f2p_test_block}"
        f"{target_file_block}"
        f"{constants_block}"
        f"{source_map_block}"
        f"{keysym_block}"
        f"{derived_notes_block}"
        f"{aplus_block}\n"
        "## Additional activated context (pooled):\n\n"
    )
    prefix_path = workdir / "prefix.txt"
    prefix_path.write_text(prefix)

    # Graph-derived composed directive (cycle 0.6+, Bundle B4).
    # Calls projection-text/src/bin/synth_directive which orchestrates three
    # named-content channels:
    #   1. synth_traceback_target — Python traceback frames (high precision
    #      when present; ~1/7 of cycle_loop_set hits)
    #   2. synth_test_expectation — test_patch identifiers (broad — 7/7)
    #   3. synth_prose_mention    — backtick + CamelCase issue prose
    # Channels score themselves; primary block + supporting blocks are
    # composed into one directive at workdir/directive.txt.
    #
    # Cycle 0.6 reframe: cycle 0.5 A + 0.5 D established that the lever is
    # structure × graph-derived NAMED content (multiplied; either alone
    # FAILs). Widening the named-content funnel from 1 channel (traceback)
    # to 3 channels is the cycle 0.7 test.
    #
    # GLIA_DIRECTIVE_LEGACY=1 reverts to the single-channel traceback bin
    # (cycle 0.2/0.3 behaviour) for comparison runs.
    directive_path = workdir / "directive.txt"
    legacy_only = os.environ.get("GLIA_DIRECTIVE_LEGACY") == "1"
    synth_composer = GLIA / "target/release/synth_directive"
    synth_target_bin = GLIA / "target/release/synth_traceback_target"
    if synth_composer.exists() and not legacy_only:
        cmd = [
            str(synth_composer),
            "--src", str(repo_dir),
            "--issue", str(issue_path),
            "--text-out", str(directive_path),
        ]
        if test_patch_path.exists() and test_patch_path.stat().st_size > 0:
            cmd += ["--test-patch", str(test_patch_path)]
        # P4.2: write hints_text to a tmp file + pass to composer if non-empty.
        hints_text = inst.get("hints_text") or ""
        if hints_text.strip():
            hints_path = workdir / "hints_text.txt"
            hints_path.write_text(hints_text)
            cmd += ["--hints-text", str(hints_path)]
        sh(cmd, capture_output=True, text=True)
    elif synth_target_bin.exists():
        sh(
            [str(synth_target_bin),
             "--src", str(repo_dir),
             "--issue", str(issue_path),
             "--text-out", str(directive_path)],
            capture_output=True, text=True,
        )
    directive_text = ""
    if directive_path.exists():
        body = directive_path.read_text().strip()
        # Inert when no channel produced content. Composer writes a stub
        # "(no channel produced..." message; legacy bin writes "no graph
        # node matched..." / "no targeting available".
        inert_markers = (
            "no graph node matched",
            "no targeting available",
            "no channel produced",
        )
        if body and not any(m in body for m in inert_markers):
            directive_text = directive_path.read_text() + "\n\n"
            log(f"composed directive: {len(directive_text)} chars")
        else:
            log("composed directive: (inert; no channel surfaced graph-derived content)")

    # Lever 1 (test-runtime-evidence pre-flight): apply the test_patch to the
    # repo at base_commit and run the F2P test to capture the ACTUAL failing
    # traceback the test produces *before* any model edit. The traceback names
    # the exact runtime types, line numbers, and access paths the bug hits —
    # information the issue text often paraphrases or omits. Surface as a
    # directive prefix so the model sees concrete runtime evidence.
    #
    # Gated on GLIA_RUNTIME_EVIDENCE=1 (default off; adds ~10-30s per instance
    # for test setup + run). Always reverts the test_patch after capture so the
    # subsequent inference + apply_and_test flow is unaffected.
    if os.environ.get("GLIA_RUNTIME_EVIDENCE") == "1":
        try:
            runtime_evidence = _capture_runtime_evidence(inst, repo_dir, workdir)
            if runtime_evidence:
                evidence_block = (
                    "\n## Runtime evidence (F2P failure trace at base_commit)\n\n"
                    "The failing-to-pass tests were applied and run before any "
                    "model fix; the traceback below is what they produce. It "
                    "names the exact runtime types and line numbers the bug "
                    "exercises:\n\n"
                    f"```\n{runtime_evidence.strip()}\n```\n\n"
                )
                # Lever #2 — behavioral-target directive (GLIA_BEHAVIORAL_TARGET=1).
                # Parse the raw F2P trace into assertion-level structured bullets
                # so the model sees "what value does the test expect vs see"
                # rather than just the raw traceback. Concrete behavioral target
                # directly informs RIGHT-LINE-WRONG-CONTENT class failures.
                if os.environ.get("GLIA_BEHAVIORAL_TARGET", "0") == "1":
                    behavioral_bullets = _extract_behavioral_target(runtime_evidence)
                    if behavioral_bullets:
                        evidence_block += (
                            "## Behavioral target (extracted from trace)\n\n"
                            "The test's assertion(s) failed with the following "
                            "expected-vs-actual contract. Your fix must make each "
                            "assertion's actual value equal the expected:\n\n"
                            f"{behavioral_bullets}\n\n"
                        )
                        log(f"behavioral target: extracted {behavioral_bullets.count(chr(10))} assertion bullets")
                directive_text = evidence_block + directive_text
                log(f"runtime evidence: prepended {len(runtime_evidence)}c F2P traceback")
        except Exception as e:
            log(f"runtime evidence: skipped ({type(e).__name__}: {e})")

    suffix = (
        f"{directive_text}"
        "Produce a minimal unified git diff that fixes the bug. Output rules:\n"
        "- First line must be `diff --git a/... b/...`.\n"
        "- Do NOT wrap the diff in code fences (no triple backticks).\n"
        "- Do NOT emit an `index <sha>..<sha>` line.\n"
        "- `@@` hunk headers must use the real file's line numbers; do not fabricate them.\n"
        "- Emit only the diff. No prose before or after.<|im_end|>\n"
        "<|im_start|>assistant"
    )
    suffix_path = workdir / "suffix.txt"
    suffix_path.write_text(suffix)

    # 7. inference (Path B embed-injection via llama.cpp; ~7× faster than candle, parity SOLVE confirmed)
    out_path = workdir / "out.txt"
    gguf = MODELS[model_key]
    log(f"running pathB-llama: model={model_key}")
    # Reasoning-distilled models emit long internal reasoning before the diff.
    # Default MAX_NEW=400 exhausts before the diff is reached. Bump for them.
    inf_env = os.environ.copy()
    if "r1-distill" in model_key or "deepseek-r1" in model_key.lower():
        inf_env["MAX_NEW"] = inf_env.get("MAX_NEW", "1800")
    # C9 wider context: when GLIA_PER_TOKEN_POOL=1, pass PER_TOKEN_POOL=1 +
    # POOL_MARKERS=1 to run_llama_pathB. Each summary becomes a multi-token
    # attendable segment instead of a mean-pooled vector — preserves per-
    # entry information at the cost of more prompt tokens. The default
    # Qwen2.5-Coder 7B ctx is 32K; bump N_CTX to fit larger prefixes.
    if os.environ.get("GLIA_PER_TOKEN_POOL") == "1":
        inf_env["PER_TOKEN_POOL"] = "1"
        inf_env["POOL_MARKERS"] = "1"
        # P2 fix (cycle 1.1-gpu follow-up): size N_CTX based on actual pool
        # size. Per-token-pool token count tracks roughly linearly with the
        # number of summary entries; matplotlib-22711 with 1155 entries
        # hit ~60K tokens. Use entry count to pick a safe N_CTX bucket:
        #   ≤300 entries  → 32K  (marshmallow, most django/sphinx)
        #   ≤700 entries  → 65K  (matplotlib-22835 / 323 entries)
        #   >700 entries  → 131K (matplotlib-22711 / 1155 entries)
        # Qwen 2.5 Coder supports 131K natively; A40 48GB fits 131K KV
        # cache (~15GB additional) comfortably.
        if "N_CTX" not in inf_env:
            try:
                n_entries = len(json.loads(summaries_aplus.read_text()))
            except Exception:
                n_entries = 0
            if n_entries > 700:
                inf_env["N_CTX"] = "131072"
                log(f"N_CTX=131072 ({n_entries} pool entries — heavy)")
            elif n_entries > 300:
                inf_env["N_CTX"] = "65536"
                log(f"N_CTX=65536 ({n_entries} pool entries)")
            else:
                inf_env["N_CTX"] = "32768"
                log(f"N_CTX=32768 ({n_entries} pool entries)")

    # Lever #1 — plan-then-edit (GLIA_PLAN_THEN_EDIT=1).
    # 14B+ models target correctly but write wrong edit content
    # (RIGHT-TARGET-WRONG-EDIT). Force an explicit prose plan before the
    # diff to surface the model's reasoning, then prepend the plan into the
    # edit suffix so the model's diff is informed by its own analysis.
    # Doubles inference wall for the planning pass (~50% of beam=3 wall);
    # should improve edit-content correctness on the 5 non-PASS instances.
    plan_then_edit = os.environ.get("GLIA_PLAN_THEN_EDIT", "0") == "1"
    if plan_then_edit:
        planning_suffix = (
            f"{directive_text}"
            "Before emitting the diff, write a SHORT plan (4-8 sentences):\n"
            "1. What is the bug? (one sentence)\n"
            "2. Which symbol (function/method/class) carries the bug? Name it.\n"
            "3. What does the failing test assert? What value does it expect vs see?\n"
            "4. What is the MINIMAL change to make the test pass? Describe in words.\n\n"
            "Plan first, no code, no diff syntax. Emit ONLY the plan now.<|im_end|>\n"
            "<|im_start|>assistant"
        )
        planning_suffix_path = workdir / "suffix_plan.txt"
        planning_suffix_path.write_text(planning_suffix)
        plan_path = workdir / "plan.md"
        plan_env = inf_env.copy()
        plan_env["MAX_NEW"] = "300"
        log(f"plan-then-edit: planning pass")
        try:
            ri_plan = sh(["python", str(OUTDIR / "run_llama_pathB.py"),
                          gguf, str(prefix_path), str(planning_suffix_path),
                          str(summaries_aplus), str(plan_path)],
                         capture_output=True, text=True, env=plan_env)
            if ri_plan.returncode == 0 and plan_path.exists():
                plan_text = plan_path.read_text().strip()
                if plan_text:
                    log(f"plan-then-edit: plan {len(plan_text)}c — prepending to edit suffix")
                    plan_aware_suffix = (
                        f"{directive_text}"
                        "## Plan (your own analysis)\n\n"
                        f"{plan_text}\n\n"
                        "## Diff\n\n"
                        "Now emit a minimal unified git diff implementing the plan above.\n"
                        "- First line must be `diff --git a/... b/...`.\n"
                        "- Do NOT wrap in code fences.\n"
                        "- `@@` hunks must use real file line numbers.\n"
                        "- Emit only the diff.<|im_end|>\n"
                        "<|im_start|>assistant"
                    )
                    suffix_path.write_text(plan_aware_suffix)
            else:
                log(f"plan-then-edit: planning rc={ri_plan.returncode}; skipping plan integration")
        except Exception as e:
            log(f"plan-then-edit: exception ({type(e).__name__}: {e}); fallback to direct")

    # A2 — pass directive's PRIMARY target qnames through to pathB as
    # GLIA_PROTECTED_QNAMES so the pool-cap path NEVER drops gold-relevant
    # entries even when T_total > n_ctx. Builds a comma-separated list of
    # `pkg::cls::method` style qnames found in backticks in the directive
    # (capped at top-10 to keep env value bounded).
    if directive_path.exists():
        try:
            dt = directive_path.read_text()
            qnames = list(dict.fromkeys(
                re.findall(r"`([a-zA-Z_][\w]*(?:::[\w:]+)+)`", dt)
            ))[:10]
            if qnames:
                inf_env["GLIA_PROTECTED_QNAMES"] = ",".join(qnames)
                log(f"protected qnames: {len(qnames)} from directive (pool-cap will keep)")
        except Exception:
            pass

    # B5 beam-sampling. When GLIA_SAMPLES>1, run N inference passes with
    # SAMPLE_TEMP>0 + per-sample seed, write each candidate diff to
    # out_sample_<i>.txt, dedup by exact-text equality, score each by a
    # cheap heuristic (diff present + applies cleanly via dry-run), and
    # promote the highest-scoring candidate to out.txt. The full
    # apply+test scoring happens later in apply_and_test, but the cheap
    # apply check up-front filters obvious malformed candidates so we
    # don't waste the post-test pass on them.
    samples = int(os.environ.get("GLIA_SAMPLES", "1"))
    sample_temp_str = os.environ.get("GLIA_SAMPLE_TEMP", "0.6") if samples > 1 else "0"
    sample_top_k = os.environ.get("GLIA_SAMPLE_TOP_K", "5")

    t0 = time.time()
    if samples <= 1:
        r = sh(
            ["python", str(OUTDIR / "run_llama_pathB.py"),
             gguf, str(prefix_path), str(suffix_path), str(summaries_aplus), str(out_path)],
            capture_output=True, text=True, env=inf_env,
        )
        wall = time.time() - t0
        if r.returncode != 0:
            raise SystemExit(f"run_llama_pathB failed rc={r.returncode}: {r.stderr[-1000:]}")
        log(f"pathB-llama done in {wall:.1f}s; out={out_path}")
    else:
        # Build the candidate set.
        candidates = []
        for i in range(samples):
            cand_env = dict(inf_env)
            cand_env["SAMPLE_TEMP"] = sample_temp_str
            cand_env["SAMPLE_TOP_K"] = sample_top_k
            cand_env["SAMPLE_SEED"] = str(0xC0DE + i)
            cand_path = workdir / f"out_sample_{i}.txt"
            ti = time.time()
            ri = sh(
                ["python", str(OUTDIR / "run_llama_pathB.py"),
                 gguf, str(prefix_path), str(suffix_path), str(summaries_aplus), str(cand_path)],
                capture_output=True, text=True, env=cand_env,
            )
            ci = time.time() - ti
            if ri.returncode != 0:
                # P5 fix: surface pathB's stderr tail so future debugging
                # doesn't require a re-run. Common causes of rc=3: T_total
                # exceeds n_ctx (need N_CTX bump). Common rc=4: llama_decode
                # prefill failure (out-of-memory or batch shape mismatch).
                err_tail = (ri.stderr or "")[-300:].strip().replace("\n", " | ")
                log(f"  sample {i}: rc={ri.returncode} ({ci:.1f}s); skipped — stderr: {err_tail}")
                continue
            text = cand_path.read_text() if cand_path.exists() else ""
            candidates.append((i, cand_path, text, ci))
            log(f"  sample {i}: {len(text)}c in {ci:.1f}s")

            # Lever #8 — lens-attention-bias as a parallel beam channel.
            # GLIA_ATTN_INJECTION=1 runs lens-attention-bias bin with the
            # same prefix/suffix, injecting target embed at L25-27 via
            # cb_eval write path (Rust runtime, not Python). Adds a 2nd
            # candidate per sample = doubles beam diversity, lets matrix
            # Option C compare baseline vs latent-steered side-by-side.
            # Target qname resolved from directive PRIMARY block heuristic
            # (first `::`-containing backticked symbol).
            if os.environ.get("GLIA_ATTN_INJECTION", "0") == "1":
                try:
                    target_qname = os.environ.get("GLIA_ATTN_TARGET_QNAME") or \
                        _extract_primary_target_qname(directive_path)
                    if target_qname:
                        lens_bin = (GLIA / "scratch/lens/target/release"
                                    / "lens-attention-bias")
                        attn_alpha = os.environ.get("GLIA_ATTN_ALPHA", "0.3")
                        attn_json = workdir / f"attn_sample_{i}.json"
                        attn_text_path = workdir / f"out_sample_{i}_attnbias.txt"
                        if lens_bin.exists():
                            t_attn = time.time()
                            ri_attn = sh([
                                str(lens_bin),
                                "--weights", gguf,
                                "--tokenizer", "/home/ivy/Models/qwen2.5-coder-tokenizer/tokenizer.json",
                                "--prefix", str(prefix_path),
                                "--suffix", str(suffix_path),
                                "--target-qname", target_qname,
                                "--inject-layers", "25:28",
                                "--inject-positions", "23:26",
                                "--alpha", attn_alpha,
                                "--max-new", "400",
                                "--out", str(attn_json),
                            ], capture_output=True, text=True, env=cand_env)
                            ai = time.time() - t_attn
                            if ri_attn.returncode == 0 and attn_json.exists():
                                try:
                                    attn_data = json.loads(attn_json.read_text())
                                    injected = attn_data.get("injected_output", "")
                                    if injected.strip():
                                        attn_text_path.write_text(injected)
                                        # Use a high-bit index to avoid colliding
                                        # with regular sample indices.
                                        attn_idx = i + 1000
                                        candidates.append(
                                            (attn_idx, attn_text_path, injected, ai))
                                        log(f"  sample {attn_idx} (attn-bias α={attn_alpha} "
                                            f"target={target_qname.split('::')[-1]}): "
                                            f"{len(injected)}c in {ai:.1f}s")
                                except Exception as ex:
                                    log(f"  attn-bias sample {i}: parse failed ({ex})")
                            else:
                                err_tail = (ri_attn.stderr or "")[-200:].strip()
                                log(f"  attn-bias sample {i}: rc={ri_attn.returncode} — {err_tail}")
                except Exception as ex:
                    log(f"  attn-bias sample {i}: skipped ({type(ex).__name__}: {ex})")
        wall = time.time() - t0
        # Dedup by text (greedy + low-temp can collide).
        seen = set()
        uniq = []
        for c in candidates:
            key = c[2].strip()
            if key and key not in seen:
                seen.add(key)
                uniq.append(c)
        # Cheap pre-score: diff present (has `diff --git`) +1, applies
        # cleanly via `git apply --check` +2 (else 0).
        def _cheap_score(cand):
            (idx, path, text, _ci) = cand
            s = 0
            if "diff --git" in text:
                s += 1
            patch_path = Path("/tmp") / f"sample_check_{idx}.patch"
            patch_path.write_text(text)
            r = sh(["git", "-C", str(repo_dir), "apply", "--check", str(patch_path)],
                   capture_output=True, text=True)
            if r.returncode == 0:
                s += 2
            else:
                # Try fuzz-tolerant patch as the model-diff apply path does.
                r2 = sh(["patch", "-p1", "--forward", "--quiet", "--fuzz=5", "-l",
                         "--dry-run", "-d", str(repo_dir), "-i", str(patch_path)],
                        capture_output=True, text=True)
                if r2.returncode == 0:
                    s += 1
            return s
        scored = [(c, _cheap_score(c)) for c in uniq]
        scored.sort(key=lambda x: (-x[1], x[0][0]))
        if scored:
            # Matrix Option C: top-K candidates by cheap_score → full
            # apply+test each → pick by REAL test score. Records per-
            # (instance, beam) outcomes to workdir/beam_matrix.jsonl for
            # the solution_curve.py analyzer. K defaults to 2 (top-2 of N
            # samples) to keep wall-cost bounded; tunable via env.
            # P5.1 (Option 7, cycle 1.1): GLIA_BEAM_TEST=1 promotes from
            # "test top-2 by cheap score" (matrix Option C default) to
            # "test ALL candidates" (full test-execution-guided beam).
            # Costs N× test wall-clock vs 2× — heavy, only worth it when
            # we want maximum candidate-selection accuracy.
            if os.environ.get("GLIA_BEAM_TEST") == "1":
                top_k = samples
            else:
                top_k = int(os.environ.get("GLIA_BEAM_TOP_K", "2"))
            test_top = scored[:top_k]
            log(f"beam: {len(uniq)} unique of {samples} candidates; "
                f"full-testing top-{len(test_top)} by cheap score")
            beam_matrix_path = workdir / "beam_matrix.jsonl"
            matrix_records = []
            full_results = []
            for cand, cheap_s in test_top:
                idx, _path, text, ci = cand
                # apply_and_test mutates + resets the repo (git checkout
                # -- . at end); safe to call multiple times in serial.
                # P7 fix: but it can leave the test_patch.patch applied if
                # mid-flow exception fires. Force-clean before each
                # candidate's apply_and_test so TESTPATCH-FAIL doesn't
                # silently propagate from a prior candidate's leftover state.
                sh(["git", "-C", str(repo_dir), "checkout", "--", "."],
                   capture_output=True, text=True)
                cand_workdir = workdir / f"sample_{idx}_test"
                cand_workdir.mkdir(exist_ok=True)
                cand_out = cand_workdir / "out.txt"
                cand_out.write_text(text)
                try:
                    t_apt = time.time()
                    res = apply_and_test(inst, repo_dir, cand_out,
                                         workdir=cand_workdir)
                    apt_wall = time.time() - t_apt
                except Exception as e:
                    log(f"  sample {idx}: apply_and_test raised "
                        f"{type(e).__name__}: {e}")
                    res = {"apply": "ERROR", "f2p": None,
                           "reg": None, "reg_fail": None}
                    apt_wall = 0.0
                rec = {
                    "instance_id": inst["instance_id"],
                    "sample_idx": idx,
                    "cheap_score": cheap_s,
                    "infer_wall_s": ci,
                    "test_wall_s": apt_wall,
                    "apply": res.get("apply"),
                    "f2p": res.get("f2p"),
                    "p2p_pass": res.get("reg"),
                    "p2p_fail": res.get("reg_fail"),
                    "diff_chars": len(text),
                }
                matrix_records.append(rec)
                full_results.append((cand, res))
                log(f"  sample {idx}: cheap={cheap_s} apply={res.get('apply')} "
                    f"f2p={res.get('f2p')} reg_fail={res.get('reg_fail')}")
            # Persist per-candidate matrix data.
            with open(beam_matrix_path, "w") as f:
                for r in matrix_records:
                    f.write(json.dumps(r) + "\n")
            # Pick best by full result, hierarchy: clean PASS > PASS-with-
            # regressions > FAIL(more F2P pass) > NO-RUN > APPLY-FAIL.
            def _full_score(item):
                _cand, res = item
                f = (res.get("f2p") or "")
                a = (res.get("apply") or "").lower()
                if a == "apply-fail":
                    return -100
                if a == "error":
                    return -110
                if f.startswith("NO-RUN") or f.startswith("TIMEOUT"):
                    return -80
                if f == "PASS" and (res.get("reg_fail") or 0) == 0:
                    return 1000
                if "regressions" in f:
                    return 900
                if f.startswith("FAIL"):
                    m = re.match(r"FAIL \((\d+)/", f)
                    return int(m.group(1)) * 10 if m else 0
                return 0
            best_cand, best_res = max(full_results, key=_full_score)
            best_idx, _bp, best_text, _bci = best_cand
            log(f"beam: matrix tested {len(full_results)} candidates; "
                f"promoting sample_{best_idx} "
                f"(apply={best_res.get('apply')} f2p={best_res.get('f2p')})")
            out_path.write_text(best_text)
        else:
            log(f"beam: all {samples} samples produced empty/failed candidates")
            out_path.write_text("")

    # P2.1 D2 pool reshape (cycle 1.1, full 2-pass variant). When pass-1
    # produced a non-empty diff that didn't trivially apply, capture D2
    # per-position attention via lens-attention, aggregate to per-pool-entry
    # scores using pool_positions.json (emitted by run_llama_pathB.py),
    # drop bottom-30% by attention from summaries-aplus, re-render the
    # prompt, re-infer. The reshaped output competes with pass-1 via
    # _is_better_result.
    #
    # Gated on GLIA_POOL_RESHAPE=full. Costs roughly +2× inference per
    # instance (one capture pass + one generation pass with reshaped pool).
    if os.environ.get("GLIA_POOL_RESHAPE") == "full" and out_path.exists():
        try:
            pos_path = workdir / "out.pool_positions.json"
            if not pos_path.exists():
                log("pool-reshape: pool_positions.json missing; skipping")
            else:
                lens_attn_bin = GLIA / "scratch/lens/target/release/lens-attention"
                lens_tokenizer = Path("/home/ivy/Models/qwen2.5-coder-tokenizer/tokenizer.json")
                if not (lens_attn_bin.exists() and lens_tokenizer.exists()):
                    log(f"pool-reshape: prereq missing (bin={lens_attn_bin.exists()}, tok={lens_tokenizer.exists()}); skipping")
                else:
                    log("pool-reshape: capturing D2 attention norms via lens-attention")
                    attn_out = workdir / "pool_reshape_attn.jsonl"
                    sh(
                        [str(lens_attn_bin),
                         "--weights", gguf,
                         "--tokenizer", str(lens_tokenizer),
                         "--prefix", str(prefix_path),
                         "--suffix", str(suffix_path),
                         "--max-new", "8",
                         "--out", str(attn_out)],
                        capture_output=True, text=True,
                    )
                    if attn_out.exists():
                        # Aggregate attention norms per pool entry at
                        # decision layers (25-27 per cycle 0.4 lens).
                        pool_pos = json.loads(pos_path.read_text())
                        entries = pool_pos.get("entries", [])
                        # Build per-position score by summing decision-band layers.
                        pos_score: dict[int, float] = {}
                        with open(attn_out) as f:
                            for line in f:
                                rec = json.loads(line)
                                layer = rec.get("layer")
                                pos = rec.get("position_idx")
                                norm = rec.get("norm")
                                if layer is None or pos is None or norm is None:
                                    continue
                                if 25 <= layer <= 27:
                                    pos_score[pos] = pos_score.get(pos, 0.0) + norm
                        # Aggregate per pool entry: mean norm over its token range.
                        entry_scores = []
                        for e in entries:
                            ts, te = e["token_start"], e["token_end"]
                            scores = [pos_score.get(p, 0.0) for p in range(ts, te)]
                            mean = sum(scores) / max(1, len(scores))
                            entry_scores.append((e["idx"], mean, e.get("qname")))
                        entry_scores.sort(key=lambda x: x[1])
                        n_drop = max(1, len(entry_scores) * 30 // 100)
                        drop_idx = {e[0] for e in entry_scores[:n_drop]}
                        log(f"pool-reshape: scored {len(entry_scores)} entries; "
                            f"dropping bottom {n_drop} (idx={sorted(drop_idx)})")
                        # Filter summaries-aplus.json.
                        all_summaries = json.loads(summaries_aplus.read_text())
                        kept = [s for i, s in enumerate(all_summaries) if i not in drop_idx]
                        reshaped_path = workdir / "summaries-aplus-reshaped.json"
                        reshaped_path.write_text(json.dumps(kept))
                        # Re-run inference with reshaped pool.
                        reshape_out_path = workdir / "out_reshape.txt"
                        log(f"pool-reshape: re-inferring with {len(kept)} entries (was {len(all_summaries)})")
                        t_rs = time.time()
                        r_rs = sh(
                            ["python", str(OUTDIR / "run_llama_pathB.py"),
                             gguf, str(prefix_path), str(suffix_path),
                             str(reshaped_path), str(reshape_out_path)],
                            capture_output=True, text=True, env=inf_env,
                        )
                        wall_rs = time.time() - t_rs
                        if r_rs.returncode == 0 and reshape_out_path.exists() and reshape_out_path.read_text().strip():
                            # Score: did reshape produce a different diff with
                            # better chance of applying? Cheap-apply check.
                            cand_patch = Path("/tmp/_pool_reshape_check.patch")
                            cand_patch.write_text(reshape_out_path.read_text())
                            r_chk = sh(
                                ["git", "-C", str(repo_dir), "apply", "--check", str(cand_patch)],
                                capture_output=True, text=True,
                            )
                            orig_patch = Path("/tmp/_pool_reshape_orig.patch")
                            orig_patch.write_text(out_path.read_text())
                            r_chk_orig = sh(
                                ["git", "-C", str(repo_dir), "apply", "--check", str(orig_patch)],
                                capture_output=True, text=True,
                            )
                            if r_chk.returncode == 0 and r_chk_orig.returncode != 0:
                                log(f"pool-reshape: promoted (reshape applies, original didn't; +{wall_rs:.1f}s)")
                                out_path = reshape_out_path
                                wall += wall_rs
                            elif r_chk.returncode == 0 and r_chk_orig.returncode == 0:
                                # Both apply — keep original (no signal to flip).
                                log(f"pool-reshape: both apply; keeping pass-1 (+{wall_rs:.1f}s wasted)")
                            else:
                                log(f"pool-reshape: reshape doesn't apply either; keeping pass-1")
                        else:
                            log(f"pool-reshape: re-inference failed rc={r_rs.returncode}; keeping pass-1")
                    else:
                        log("pool-reshape: lens-attention produced no output; skipping reshape")
        except Exception as e:
            log(f"pool-reshape: raised {type(e).__name__}: {e}; keeping pass-1")

    # Lever #3 — when GLIA_SKIP_VALIDATOR_PASS=1, skip the validator-driven
    # pass-2 entirely and rely on sage-runtime (Lever 3) for recovery.
    # Cycle 1.2 django evidence: validator pass-2 promoted a 1042c diff that
    # made things WORSE (compositional dropped all hunks); only sage-runtime
    # pass-2 (informed by actual test trace) fixed it. Validator's text-only
    # critique can mislead; runtime trace is gold signal.
    skip_validator = os.environ.get("GLIA_SKIP_VALIDATOR_PASS", "0") == "1"

    # C2 (cycle 0.6 spitball sage loop): two-pass refinement. After pass-1
    # emits a diff, synth_validator critiques it against the directive.
    # When critique non-empty (not the inert sentinel), re-prompt with the
    # critique prepended and the first-pass diff as "previous attempt" for
    # the model to revise. Pass-2 output replaces pass-1 if non-empty.
    #
    # Gated on GLIA_TWO_PASS=1 (default off so cycle 0.7 baseline isn't
    # disturbed). Doubles inference time per instance; only worth it after
    # cycle 0.7 failure-mode classifier shows enough APPLY-FAIL /
    # TARGET-MISMATCH cases that the orchestration cost is justified.
    if (os.environ.get("GLIA_TWO_PASS") == "1" and out_path.exists()
            and not skip_validator):
        synth_validator_bin = GLIA / "target/release/synth_validator"
        if synth_validator_bin.exists() and directive_path.exists():
            critique_path = workdir / "critique.md"
            sh(
                [str(synth_validator_bin),
                 "--diff", str(out_path),
                 "--directive", str(directive_path),
                 "--text-out", str(critique_path)],
                capture_output=True, text=True,
            )
            crit = critique_path.read_text() if critique_path.exists() else ""
            if crit.strip() and "no critique" not in crit:
                log(f"sage loop: pass-1 critique non-empty ({len(crit)}c); running pass-2")
                pass1_diff = out_path.read_text()
                pass2_suffix = (
                    f"{crit}\n\n"
                    f"## Previous attempt (pass-1, REJECTED — do NOT repeat)\n\n"
                    f"```\n{pass1_diff.strip()}\n```\n\n"
                    f"## Corrected diff\n\n"
                    "Produce a minimal unified git diff per the critique above. Output rules:\n"
                    "- First line must be `diff --git a/... b/...`.\n"
                    "- Do NOT wrap the diff in code fences (no triple backticks).\n"
                    "- Do NOT emit an `index <sha>..<sha>` line.\n"
                    "- `@@` hunk headers must use the real file's line numbers; do not fabricate them.\n"
                    "- Emit only the diff. No prose before or after.<|im_end|>\n"
                    "<|im_start|>assistant"
                )
                pass2_suffix_path = workdir / "suffix_pass2.txt"
                pass2_suffix_path.write_text(pass2_suffix)
                pass2_out_path = workdir / "out_pass2.txt"
                t1 = time.time()
                r2 = sh(
                    ["python", str(OUTDIR / "run_llama_pathB.py"),
                     gguf, str(prefix_path), str(pass2_suffix_path),
                     str(summaries_aplus), str(pass2_out_path)],
                    capture_output=True, text=True, env=inf_env,
                )
                wall2 = time.time() - t1
                if r2.returncode == 0 and pass2_out_path.exists():
                    pass2_diff = pass2_out_path.read_text().strip()
                    if pass2_diff:
                        log(f"sage loop: pass-2 promoted ({len(pass2_diff)}c in {wall2:.1f}s)")
                        out_path = pass2_out_path
                        wall += wall2
                    else:
                        log(f"sage loop: pass-2 produced empty diff in {wall2:.1f}s; keeping pass-1")
                else:
                    log(f"sage loop: pass-2 failed (rc={r2.returncode}); keeping pass-1")
            else:
                log("sage loop: pass-1 critique inert (diff aligns with directive); skipping pass-2")
        else:
            log("sage loop: synth_validator or directive missing; skipping pass-2")

    # P4 fix: report BOTH the AccessPath cell count (aplus_cells, what this
    # function has always returned) AND the total pool size, so downstream
    # JSONL doesn't read "aplus_cells=0" and conclude the pool is empty.
    total_pool = len(json.loads(summaries_aplus.read_text())) if summaries_aplus.exists() else 0
    return out_path, wall, len(aplus_cells), total_pool


_HUNK_HDR_RE = re.compile(r'^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@', re.M)


def _split_hunks(block):
    m = re.search(r'^@@ ', block, flags=re.M)
    if m is None:
        return block, []
    header = block[:m.start()]
    rest = block[m.start():]
    hunks = [h for h in re.split(r'(?=^@@ )', rest, flags=re.M) if h.strip()]
    return header, hunks


def _is_hunk_complete(hunk):
    # "Complete" = the model didn't run out of tokens mid-hunk. Off-by-one
    # count errors (one side over/short) are common at 7B Q4 and recoverable
    # via --recount or patch-fuzz, so they don't count as truncation.
    # Truncation = both sides short (or malformed last line).
    first_line = hunk.split("\n", 1)[0]
    m = re.match(r'^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@', first_line)
    if not m:
        return False
    declared_minus = int(m.group(2)) if m.group(2) else 1
    declared_plus = int(m.group(4)) if m.group(4) else 1
    body = hunk.split("\n", 1)[1] if "\n" in hunk else ""
    body_lines = body.split("\n")
    while body_lines and not body_lines[-1]:
        body_lines.pop()
    actual_minus = 0
    actual_plus = 0
    for ln in body_lines:
        if not ln:
            actual_minus += 1
            actual_plus += 1
            continue
        c = ln[0]
        if c == ' ':
            actual_minus += 1
            actual_plus += 1
        elif c == '-':
            actual_minus += 1
        elif c == '+':
            actual_plus += 1
        elif c == '\\':
            pass
        else:
            return False
    if actual_minus < declared_minus and actual_plus < declared_plus:
        return False
    return True


def _clean_block(block):
    header, hunks = _split_hunks(block)
    if not hunks:
        return None
    while hunks and not _is_hunk_complete(hunks[-1]):
        hunks.pop()
    if not hunks:
        return None
    out = header if header.endswith("\n") else header + "\n"
    for h in hunks:
        out += h if h.endswith("\n") else h + "\n"
    return out


def split_and_clean_multifile(diff):
    # Weak models truncate the last file or last hunk when they hit the token
    # cap. git apply rejects the whole patch on any malformed block. Drop
    # trailing invalid hunks/blocks so the surviving prefix can still apply.
    blocks = [b for b in re.split(r'(?=^diff --git )', diff, flags=re.M) if b.strip()]
    if not blocks:
        return diff
    all_valid = True
    for b in blocks:
        _, hunks = _split_hunks(b)
        if not hunks or not all(_is_hunk_complete(h) for h in hunks):
            all_valid = False
            break
    if all_valid:
        return diff
    cleaned = []
    for b in blocks:
        c = _clean_block(b)
        if c is None:
            break
        cleaned.append(c)
    if not cleaned:
        return diff
    return "".join(cleaned)


def extract_diff(out_text):
    text = out_text.split("<|im_end|>")[0]
    m = re.search(r"(diff --git .*|--- a/.*|--- [^\n/][^\n]*)", text, re.DOTALL)
    if not m:
        return None
    diff = m.group(0)
    diff = re.sub(r"\n```.*$", "", diff, flags=re.DOTALL)
    # Strip weak-model format noise: fake `index ...` lines (any garbage after
    # "index "), `mode <oct>` standalone lines, and any stray code fences. 14B
    # emits malformed mode/index ("10, 100644 (mode 100644)") that breaks the
    # narrow regex.
    diff = re.sub(r"^index [^\n]*\n", "", diff, flags=re.M)
    diff = re.sub(r"^mode [^\n]*\n", "", diff, flags=re.M)
    diff = re.sub(r"^```(diff)?\s*\n", "", diff, flags=re.M)
    # 14B drops the a/ b/ prefix on diff --git / --- / +++ lines. git apply -p1
    # then strips the leading dir component instead, breaking path resolution.
    # Re-add prefixes when missing.
    diff = re.sub(r"^diff --git (?!a/)(\S+)(?:\s+(?!b/)(\S+))?\s*$",
                  lambda m: f"diff --git a/{m.group(1)} b/{m.group(2) or m.group(1)}",
                  diff, flags=re.M)
    diff = re.sub(r"^--- (?!a/)(?!/dev/null)(\S.*)$", r"--- a/\1", diff, flags=re.M)
    diff = re.sub(r"^\+\+\+ (?!b/)(?!/dev/null)(\S.*)$", r"+++ b/\1", diff, flags=re.M)
    diff = split_and_clean_multifile(diff)
    if not diff.endswith("\n"):
        diff += "\n"
    return diff


def compositional_apply_and_test(inst, repo_dir, out_path, workdir):
    """P3.2 (Option 5, cycle 1.1). Hunk-by-hunk apply + test. Splits a
    multi-hunk diff, applies each hunk INDIVIDUALLY, runs F2P+P2P after
    each, keeps the hunk only if F2P pass count strictly increases AND
    P2P fail count does not increase. Reverts otherwise.

    Returns the combined-kept-hunks test_result dict (same shape as
    apply_and_test). When the diff has 1 hunk or fewer, just defers to
    apply_and_test directly. Addresses the django PASS-but-1-regressions
    failure mode by isolating the regression-causing hunk.
    """
    diff_text = out_path.read_text() if out_path.exists() else ""
    if not diff_text.strip():
        return apply_and_test(inst, repo_dir, out_path, workdir=workdir)

    header, hunks = _split_hunks(diff_text)
    if len(hunks) <= 1:
        # Single hunk → no compositional gain; defer.
        return apply_and_test(inst, repo_dir, out_path, workdir=workdir)

    log(f"compositional: {len(hunks)} hunks; testing hunk-by-hunk")
    kept_hunks: list[str] = []
    per_hunk_records: list[dict] = []
    best_result: dict = {"apply": "INIT", "f2p": None}

    def _f2p_pass_count(res: dict) -> int:
        return int(res.get("f2p_pass") or 0)
    def _p2p_fail_count(res: dict) -> int:
        return int(res.get("reg_fail") or 0)

    baseline_pass = 0
    baseline_p2p_fail = 0
    # Reset repo before each attempt; build combined diff = header + kept + this hunk.
    for i, h in enumerate(hunks):
        candidate_diff = header + "".join(kept_hunks) + h
        candidate_path = workdir / f"compositional_hunk_{i}.diff"
        candidate_path.write_text(candidate_diff)
        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
        res = apply_and_test(inst, repo_dir, candidate_path, workdir=workdir)
        f2p_pass = _f2p_pass_count(res)
        p2p_fail = _p2p_fail_count(res)
        keep = (f2p_pass > baseline_pass) and (p2p_fail <= baseline_p2p_fail)
        # Special case: if the candidate broke applying (APPLY-FAIL), don't keep.
        if (res.get("apply") or "").lower() == "apply-fail":
            keep = False
        per_hunk_records.append({
            "hunk_idx": i,
            "kept": keep,
            "apply": res.get("apply"),
            "f2p": res.get("f2p"),
            "f2p_pass_after": f2p_pass,
            "p2p_fail_after": p2p_fail,
        })
        if keep:
            kept_hunks.append(h)
            baseline_pass = f2p_pass
            baseline_p2p_fail = p2p_fail
            best_result = res
            log(f"compositional: hunk {i} kept (f2p_pass={f2p_pass}, p2p_fail={p2p_fail})")
        else:
            log(f"compositional: hunk {i} dropped (f2p_pass={f2p_pass}, "
                f"p2p_fail={p2p_fail}, apply={res.get('apply')})")

    # Final combined diff = kept hunks only.
    final_diff = header + "".join(kept_hunks)
    final_path = workdir / "compositional_final.diff"
    final_path.write_text(final_diff)
    # Persist per-hunk record for the matrix analyzer.
    hunk_log = workdir / "hunk_attribution.jsonl"
    with open(hunk_log, "w") as f:
        for r in per_hunk_records:
            f.write(json.dumps(r) + "\n")
    log(f"compositional: kept {len(kept_hunks)}/{len(hunks)} hunks; final result f2p={best_result.get('f2p')}")
    # P3 fix (cycle 1.1-gpu follow-up): if NO hunks survived the keep-test,
    # don't return INIT — fall back to plain apply_and_test on the FULL
    # diff. The compositional path is supposed to *improve* over plain apply
    # by isolating the regression-causing hunk; if every hunk individually
    # was rejected, plain apply might still produce useful F2P signal
    # (single-hunk apply could even apply different mechanics than per-hunk
    # serial). Worst case it's redundant with the per-hunk test, same wall.
    if not kept_hunks:
        log("compositional: 0 hunks survived; falling back to plain apply_and_test on full diff")
        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
        return apply_and_test(inst, repo_dir, out_path, workdir=workdir)
    # Promote final composite to out_path so the rest of run_instance sees it.
    out_path.write_text(final_diff)
    return best_result


def _capture_runtime_evidence(inst, repo_dir, workdir) -> str:
    """Lever 1 helper. Apply test_patch, run the F2P test once at base_commit
    (NO model fix applied), capture the failing test's traceback / assertion,
    revert the test_patch. Returns the failure block (trimmed to ~3KB) or
    empty string if anything fails.

    Side-effects on repo_dir are reverted via `git checkout -- .` before
    returning so the subsequent inference pipeline starts from a clean tree.
    """
    test_patch = inst.get("test_patch") or ""
    if not test_patch.strip():
        return ""
    f2p_ids = inst.get("FAIL_TO_PASS") or []
    if not f2p_ids:
        return ""
    # Apply test_patch with the same fuzz tolerance the model-diff path uses.
    patch_path = workdir / "_runtime_evidence_test_patch.patch"
    patch_path.write_text(test_patch)
    r = sh(["git", "-C", str(repo_dir), "apply", "--recount", "--whitespace=fix",
            str(patch_path)], capture_output=True, text=True)
    if r.returncode != 0:
        r2 = sh(["patch", "-p1", "--forward", "--quiet", "--fuzz=5", "-l",
                 "-d", str(repo_dir), "-i", str(patch_path)],
                capture_output=True, text=True)
        if r2.returncode != 0:
            log("runtime evidence: test_patch failed to apply at base_commit; skipping")
            sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
            return ""
    # Build test command from spec + ensure venv.
    try:
        test_cmd, directives, _ = get_test_command(inst)
    except KeyError:
        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
        return ""
    venv_dir, py_bin = ensure_venv(inst, repo_dir)
    test_env = os.environ.copy()
    if venv_dir:
        test_env["PATH"] = f"{venv_dir/'bin'}:{test_env['PATH']}"
        test_env["VIRTUAL_ENV"] = str(venv_dir)
    test_env["CFLAGS"] = (
        f"{test_env.get('CFLAGS','')} -Wno-incompatible-pointer-types -Wno-error"
    ).strip()
    test_env["CXXFLAGS"] = f"{test_env.get('CXXFLAGS','')} -Wno-error".strip()
    test_env.update(_test_env_vars(inst["repo"], inst.get("version", "")))
    full_cmd = f"{test_cmd} {' '.join(directives)}"
    log(f"runtime evidence: running F2P at base_commit ({full_cmd})")
    try:
        rt = subprocess.run(full_cmd, shell=True, cwd=str(repo_dir),
                            capture_output=True, text=True, timeout=120, env=test_env)
        log_text = (rt.stdout or "") + "\n" + (rt.stderr or "")
    except subprocess.TimeoutExpired:
        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
        return ""
    finally:
        # Always revert before returning so subsequent inference sees a clean tree.
        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
    return _extract_failure_block(log_text)


def _extract_primary_target_qname(directive_path) -> str:
    """Lever #8 helper. Scan the composed directive for the first
    `pkg::cls::method` style qname inside a backtick — that's the
    PRIMARY target the directive surfaced. Returns empty string if
    nothing matched.

    Used to feed lens-attention-bias's --target-qname arg without
    requiring the caller to know the synth_directive internals.
    """
    if not directive_path or not directive_path.exists():
        return ""
    try:
        text = directive_path.read_text()
    except Exception:
        return ""
    # Prefer the first qname inside the PRIMARY block — synth_directive
    # always puts it under "Implementation-side targets:" or
    # "Required fix target". Fall back to any `::`-containing backtick.
    m = re.search(r"`([a-zA-Z_][\w]*::[\w:]+)`", text)
    if m:
        return m.group(1)
    return ""


def _extract_behavioral_target(trace: str) -> str:
    """Lever #2 — distill a failing-test traceback into structured
    "expected vs actual" bullets for the directive prefix.

    Patterns matched (best-effort, single-pass over the trace):
      - pytest AssertionError: ``assert 0 == 1`` → "Expression evaluates 0; must equal 1"
      - pytest assert with operator: ``assert obj.attr == "foo"`` → likewise
      - django AssertionError: ``AssertionError: 'foo' != 'bar' : ...`` → "got 'foo'; expected 'bar'"
      - generic raised exception: ``ValueError: explanation`` → "Test triggers ValueError: ..."
    Returns up to 4 bullets joined by newlines. Empty string if nothing matched.
    """
    if not trace:
        return ""
    bullets = []
    # Pattern 1: pytest 'assert LEFT == RIGHT' (most common form)
    for m in re.finditer(r"^\s*(?:E\s+)?assert\s+(.+?)\s*==\s*(.+?)$",
                          trace, flags=re.M):
        actual = m.group(1).strip().rstrip(",")
        expected = m.group(2).strip().rstrip(",")
        if len(actual) < 200 and len(expected) < 200:
            bullets.append(f"- Expression `{actual}` must equal `{expected}` "
                            f"(currently fails the equality).")
        if len(bullets) >= 4:
            break
    # Pattern 2: django/unittest 'X != Y' AssertionError with optional msg
    for m in re.finditer(r"AssertionError:\s*(.+?)\s*!=\s*(.+?)(?:\s+:\s+.+)?$",
                          trace, flags=re.M):
        actual = m.group(1).strip()
        expected = m.group(2).strip()
        if len(actual) < 200 and len(expected) < 200:
            bullets.append(f"- Got `{actual}`; expected `{expected}`.")
        if len(bullets) >= 4:
            break
    # Pattern 3: top-level exception with class + message
    for m in re.finditer(r"^\s*(?:E\s+)?([A-Z][A-Za-z_]+(?:Error|Exception)):\s*(.+?)$",
                          trace, flags=re.M):
        exc = m.group(1).strip()
        msg = m.group(2).strip()
        if exc in ("AssertionError",):
            continue  # already covered above
        if len(msg) < 300:
            bullets.append(f"- Bug raises `{exc}`: {msg}")
        if len(bullets) >= 4:
            break
    # Dedupe + cap
    seen = set()
    deduped = []
    for b in bullets:
        if b not in seen:
            seen.add(b)
            deduped.append(b)
    return "\n".join(deduped[:4])


def _extract_failure_block(test_log: str) -> str:
    """Lever 3 helper. Pull the failure-detail section out of a pytest /
    django runtests / sympy bin/test log. Pytest emits "=== FAILURES ===" +
    per-test traceback; django emits "FAIL: ..." + traceback; sympy emits
    "Error" + similar. Return a trimmed chunk small enough to embed in the
    pass-2 prompt without blowing context (~3KB cap).
    """
    if not test_log:
        return ""
    # Pytest: "=========== FAILURES ===========" up to next "=" banner / EOF.
    m = re.search(r"=+\s*FAILURES\s*=+", test_log)
    if m:
        start = m.end()
        # Stop at the next "=== short test summary" / "=== passes" / EOF.
        end_m = re.search(r"\n=+\s*(short test summary|PASSES|warnings summary|FAILURES)\s*=+",
                          test_log[start:])
        end = (start + end_m.start()) if end_m else len(test_log)
        block = test_log[start:end].strip()
        return block[-3000:]
    # Django: "FAIL: <test_id>" or "ERROR: <test_id>" line; take up to 50 lines.
    m = re.search(r"^(FAIL|ERROR):\s+\S+", test_log, flags=re.M)
    if m:
        start = m.start()
        lines = test_log[start:].split("\n")
        block = "\n".join(lines[:50])
        return block[-3000:]
    # Fallback: last 3KB of log (test runner emitted unstructured failure).
    return test_log.strip()[-3000:]


def _is_better_result(new_result: dict, old_result: dict) -> bool:
    """Lever 3 helper. Decide whether pass-2's test_result improved on pass-1.
    Hierarchy (best→worst): PASS > PASS-but-regressions > FAIL(some pass)
    > FAIL(0 pass) > NO-RUN > APPLY-FAIL > TIMEOUT.
    """
    def score(r):
        apply = (r.get("apply") or "").lower()
        f2p = (r.get("f2p") or "")
        if apply == "apply-fail":
            return -100
        if f2p.startswith("TIMEOUT"):
            return -90
        if f2p.startswith("NO-RUN"):
            return -80
        if f2p == "PASS":
            return 1000
        if "regressions" in f2p:
            return 900
        if f2p.startswith("FAIL"):
            # Heuristic: more F2P passes = better. Parse "FAIL (X/Y pass, ..."
            mm = re.match(r"FAIL \((\d+)/", f2p)
            return int(mm.group(1)) * 10 if mm else 0
        return 0
    return score(new_result) > score(old_result)


def apply_and_test(inst, repo_dir, out_path, workdir=None):
    text = out_path.read_text()
    diff = extract_diff(text)
    if not diff:
        return {"apply": "NO-DIFF", "f2p": None, "reg": None, "reg_fail": None}

    healed = heal_diff(diff, repo_dir)
    if healed != diff:
        log("heal_diff: applied corrections")
        diff = healed

    # Post-auto8h edit-content lever: when GLIA_NORMALIZE_DIFF=1, run the
    # diff through normalize_diff_via_apply (patch with fuzz → git diff).
    # Compresses bloated diffs (sphinx 1607c) and re-anchors line numbers
    # to current source, removing APPLY-FAIL caused by line drift. Opt-in
    # for now to A/B against the baseline; default off until validated.
    #
    # B1+B3 polish (cycle 2.0 observation): on diffs that already apply
    # cleanly at correct line numbers, the round-trip produces output
    # ≥ original because git's --unified=3 may emit more context than the
    # model. Log message now says "no compression available" instead of
    # the misleading "produced N≥M". Behavioral semantics unchanged:
    # original is kept when normalize doesn't shrink it.
    if os.environ.get("GLIA_NORMALIZE_DIFF") == "1":
        from diff_healer import normalize_diff_via_apply
        normalized = normalize_diff_via_apply(diff, repo_dir)
        if normalized and normalized.strip() and len(normalized) <= len(diff):
            log(f"normalize_diff: {len(diff)}c → {len(normalized)}c (compressed)")
            diff = normalized
        elif normalized:
            log(f"normalize_diff: no compression available ({len(normalized)}c ≥ {len(diff)}c); keeping original")
        else:
            log(f"normalize_diff: round-trip failed (apply or git-diff returned empty); keeping original")

    # Apply test_patch first (adds the FAIL_TO_PASS tests which don't exist at base_commit).
    testpatch_path = Path("/tmp/run_instance.testpatch")
    testpatch_path.write_text(inst.get("test_patch") or "")
    if inst.get("test_patch"):
        r = sh(["git", "-C", str(repo_dir), "apply", "--recount", "--whitespace=nowarn", str(testpatch_path)],
               capture_output=True, text=True)
        if r.returncode != 0:
            sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
            return {"apply": "TESTPATCH-FAIL", "f2p": None, "reg": None, "reg_fail": None,
                    "apply_err": r.stderr[-400:]}

    patch_path = Path("/tmp/run_instance.patch")
    patch_path.write_text(diff)

    apply_status = "FAIL"
    r = sh(["git", "-C", str(repo_dir), "apply", "--recount", "--whitespace=fix", str(patch_path)],
           capture_output=True, text=True)
    if r.returncode == 0:
        apply_status = "applied"
    else:
        # GNU patch 2.8 default fuzz tolerance rejects fuzz>=3. The marshmallow
        # OG SOLVE diff hits fuzz=3 (context lines don't match exactly — the
        # model often hallucinates the before-context). Bump fuzz to 5 so the
        # generated diff with shifted/mismatched context still applies via the
        # fuzz fallback path. Confirmed 2026-05-21 with `patch --fuzz=10 --dry-run`
        # succeeding at fuzz=3 on marshmallow-1359 with offset -2.
        #
        # `-l` = --ignore-whitespace. Models often output 4-space body indent
        # for methods inside classes when the actual file uses 8 spaces.
        # Without `-l` even the fuzz fallback rejects. Confirmed 2026-05-21
        # cycle 0.3 integration smoke: target line correctly identified
        # (fields.py:1114) but indent mismatch blocked apply.
        r2 = sh(["patch", "-p1", "--forward", "--quiet", "--fuzz=5", "-l", "-d", str(repo_dir), "-i", str(patch_path)],
                capture_output=True, text=True)
        if r2.returncode == 0:
            apply_status = "applied(fuzz)"
        else:
            sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
            return {"apply": "APPLY-FAIL", "f2p": None, "reg": None, "reg_fail": None,
                    "apply_err": (r.stderr + r2.stderr)[-400:]}

    # Per-repo test command from swebench spec — django runtests.py / sympy bin/test
    # / pytest -rA depending on repo. Directives = test files extracted from
    # test_patch (django dotted, others paths). Parser → {test_id: status}.
    try:
        test_cmd, directives, _ = get_test_command(inst)
    except KeyError as e:
        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
        return {"apply": apply_status, "f2p": f"NO-SPEC ({e})", "reg": None, "reg_fail": None}

    override = _test_cmd_override(inst["repo"], inst.get("version", ""))
    if override:
        log(f"test cmd override: {override} (was: {test_cmd})")
        test_cmd = override
    full_cmd = f"{test_cmd} {' '.join(directives)}"
    log(f"test cmd: {full_cmd}")
    test_env = os.environ.copy()
    venv_dir, py_bin = ensure_venv(inst, repo_dir)
    if venv_dir:
        test_env["PATH"] = f"{venv_dir/'bin'}:{test_env['PATH']}"
        test_env["VIRTUAL_ENV"] = str(venv_dir)
    test_env["CFLAGS"] = f"{test_env.get('CFLAGS','')} -Wno-incompatible-pointer-types -Wno-error".strip()
    test_env["CXXFLAGS"] = f"{test_env.get('CXXFLAGS','')} -Wno-error".strip()
    test_env.update(_test_env_vars(inst["repo"], inst.get("version", "")))
    # ensure_repo's `git clean -fdx` between instances wipes the in-tree
    # `.so` files built during venv install (mpl `_c_internal_utils`,
    # astropy `_erfa`, etc). Rebuild in-place before tests run.
    if inst["repo"] in _POST_INSTALL_BUILD_EXT and py_bin:
        marker_so = repo_dir / ".glia_built_ext"
        if not marker_so.exists():
            r = subprocess.run(f"{py_bin} setup.py build_ext --inplace", shell=True,
                               cwd=str(repo_dir), env=test_env,
                               capture_output=True, text=True, timeout=600)
            if r.returncode == 0:
                marker_so.touch()
                log("  build_ext --inplace (post-clean) ok")
            else:
                log(f"  build_ext --inplace (post-clean) failed: {r.stderr.strip()[-200:]}")
    # Same shape as the build_ext fix above, but for pytest's setuptools-scm
    # generated _version.py: install creates `src/_pytest/_version.py` with
    # `version = "<spec>.0"`, then `git clean -fdx` between instances wipes it
    # because it's untracked. Without this, `from _pytest._version import
    # version` raises ModuleNotFoundError at first pytest import → NO-RUN.
    if inst["repo"] == "pytest-dev/pytest":
        version_py = repo_dir / "src" / "_pytest" / "_version.py"
        if not version_py.exists():
            v = inst.get("version", "0.0")
            version_py.parent.mkdir(parents=True, exist_ok=True)
            version_py.write_text(
                f'# Regenerated post-`git clean -fdx` (setuptools-scm output).\n'
                f'__version__ = version = "{v}.0"\n'
                f'__version_tuple__ = version_tuple = ({v.replace(".", ", ")}, 0)\n'
            )
            log(f"  rewrote {version_py.name} = {v}.0 (post-clean)")
    # 300s ceiling: sympy bin/test on a whole file can take 12+ min, way past
    # any signal point. If F2P doesn't run inside 5 min, mark as TIMEOUT and
    # move on. Real fixes finish in <30s; long runtimes are sympy-test-harness
    # noise, not model-quality signal.
    try:
        r = subprocess.run(full_cmd, shell=True, cwd=str(repo_dir),
                           capture_output=True, text=True, timeout=300, env=test_env)
        log_text = (r.stdout or "") + "\n" + (r.stderr or "")
    except subprocess.TimeoutExpired as te:
        log_text = (te.stdout.decode() if te.stdout else "") + "\n" + \
                   (te.stderr.decode() if te.stderr else "") + \
                   "\n*** TIMEOUT after 300s ***\n"
        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
        return {"apply": apply_status, "f2p": "TIMEOUT (300s)",
                "reg": None, "reg_fail": None, "timeout": True}

    # Persist full log for offline diagnosis (next bench shouldn't lose info on /tmp wipe).
    if workdir is not None:
        try:
            (workdir / "test_log.txt").write_text(log_text)
        except OSError:
            pass

    f2p_ids = inst["FAIL_TO_PASS"]
    p2p_ids = inst.get("PASS_TO_PASS") or []
    ev = evaluate_log(inst["repo"], log_text, f2p_ids, p2p_ids)

    if ev["all_pass"]:
        f2p_result = "PASS"
    elif ev["f2p_pass"] == len(f2p_ids) and ev["p2p_fail"] > 0:
        f2p_result = f"PASS-but-{ev['p2p_fail']}-regressions"
    elif ev["f2p_missing"] and ev["f2p_pass"] == 0 and ev["f2p_fail"] == 0:
        # Parser couldn't find any F2P id → likely runner crashed before tests
        tail = log_text.strip().split("\n")[-1][:200]
        f2p_result = f"NO-RUN ({tail})"
    else:
        f2p_result = f"FAIL ({ev['f2p_pass']}/{len(f2p_ids)} pass, {ev['f2p_fail']} fail, {len(ev['f2p_missing'])} missing)"

    sh(["git", "-C", str(repo_dir), "checkout", "--", "."])

    return {
        "apply": apply_status,
        "f2p": f2p_result,
        "reg": ev["p2p_pass"],
        "reg_fail": ev["p2p_fail"],
        "f2p_pass": ev["f2p_pass"],
        "f2p_fail": ev["f2p_fail"],
        "f2p_missing_n": len(ev["f2p_missing"]),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--instance-id", required=True)
    ap.add_argument("--split", default="dev", choices=["dev", "test"])
    ap.add_argument("--model", default="7b-q4", choices=list(MODELS.keys()))
    ap.add_argument("--results", default=str(OUTDIR / "instance_results.jsonl"))
    ap.add_argument("--no-siblings", action="store_true", help="drop sibling expansion (seeds 60→120)")
    ap.add_argument("--no-keysym", action="store_true", help="drop Key symbols text-prefix block")
    ap.add_argument("--tag", default="", help="suffix for workdir (e.g. 'a1') to avoid clobbering prior runs")
    args = ap.parse_args()

    inst = load_instance(args.instance_id, args.split)
    log(f"instance: {inst['instance_id']} | repo: {inst['repo']} @ {inst['base_commit'][:8]}")
    log(f"F2P: {len(inst['FAIL_TO_PASS'])} tests; P2P: {len(inst['PASS_TO_PASS'])} tests")

    repo_dir = ensure_repo(inst)
    tag_suffix = f"-{args.tag}" if args.tag else ""
    workdir = OUTDIR / f"inst-{inst['instance_id']}-{args.model}{tag_suffix}"

    try:
        out_path, wall, n_cells, n_pool = run_pipeline(inst, repo_dir, args.model, workdir, no_siblings=args.no_siblings, no_keysym=args.no_keysym)
    except SystemExit as e:
        result = {"instance_id": inst["instance_id"], "model": args.model, "error": str(e), "wall_s": None}
        with open(args.results, "a") as f:
            f.write(json.dumps(result) + "\n")
        print(json.dumps(result, indent=2))
        return 1

    # P4.3 (cycle 1.1) post-edit static check. Runs synth_check on out.txt
    # before venv test execution. STATIC-FAIL reasons are stored at
    # workdir/static_check.json so Lever 3 can include them in the sage
    # runtime-trace critique block. Doesn't gate apply_and_test on its
    # own — that's apply's job — but enriches downstream diagnostics.
    try:
        if out_path.exists() and out_path.read_text().strip():
            static_check_bin = GLIA / "target/release/synth_check"
            if static_check_bin.exists():
                check_out = workdir / "static_check.json"
                sh(
                    [str(static_check_bin),
                     "--src", str(repo_dir),
                     "--diff", str(out_path),
                     "--check-out", str(check_out)],
                    capture_output=True, text=True,
                )
                if check_out.exists():
                    sc = json.loads(check_out.read_text())
                    log(f"static-check: {sc.get('status')} ({len(sc.get('reasons', []))} reasons)")
    except Exception as _e:
        log(f"static-check: skipped ({type(_e).__name__}: {_e})")

    # P3.2 (cycle 1.1): when GLIA_COMPOSITIONAL=1, run hunk-by-hunk
    # apply+test that keeps only hunks which strictly improve F2P without
    # regressing P2P. Falls through to plain apply_and_test for single-
    # hunk diffs (no compositional gain available there). Note the default
    # call to apply_and_test still happens via compositional_apply_and_test
    # for single-hunk cases.
    if os.environ.get("GLIA_COMPOSITIONAL") == "1":
        test_result = compositional_apply_and_test(inst, repo_dir, out_path, workdir)
    else:
        test_result = apply_and_test(inst, repo_dir, out_path, workdir=workdir)

    # Lever 3 (sage pass-2 with runtime trace): when pass-1 applied cleanly but
    # F2P still failed (RIGHT-LINE-WRONG-CONTENT / RIGHT-TARGET-WRONG-EDIT) OR
    # APPLY-FAIL'd, the new test_log carries the post-edit traceback — concrete
    # runtime evidence the directive-side critique cannot synthesize. Re-prompt
    # with the new traceback as a "your previous attempt produced THIS failure"
    # block and re-test. Gated on GLIA_SAGE_RUNTIME=1 (defaults to GLIA_TWO_PASS).
    sage_runtime = os.environ.get("GLIA_SAGE_RUNTIME",
                                  os.environ.get("GLIA_TWO_PASS", "0")) == "1"
    pass1_f2p = test_result.get("f2p") or ""
    pass1_apply = test_result.get("apply") or ""
    needs_runtime_pass2 = sage_runtime and (
        pass1_f2p.startswith("FAIL") or pass1_apply == "APPLY-FAIL"
        or pass1_f2p.startswith("NO-RUN")
    )
    if needs_runtime_pass2:
        try:
            test_log_path = workdir / "test_log.txt"
            test_log = test_log_path.read_text() if test_log_path.exists() else ""
            failure_block = _extract_failure_block(test_log)
            if failure_block:
                log(f"sage runtime: pass-1 {pass1_f2p}/{pass1_apply}; "
                    f"building pass-2 directive from {len(failure_block)}c runtime trace")
                pass1_diff = out_path.read_text() if out_path.exists() else ""
                # P4.3 integration: pull static-check reasons (if any) so
                # the model sees both runtime AND static failure signals.
                static_block = ""
                try:
                    sc_path = workdir / "static_check.json"
                    if sc_path.exists():
                        sc = json.loads(sc_path.read_text())
                        if sc.get("status") == "STATIC-FAIL" and sc.get("reasons"):
                            static_block = (
                                "Additionally, a static check on your diff found:\n"
                                + "\n".join(f"- {r}" for r in sc["reasons"])
                                + "\n\n"
                            )
                except Exception:
                    pass
                runtime_critique = (
                    f"## Runtime evidence from your previous attempt\n\n"
                    f"Your previous diff was applied (or attempted) and the F2P test "
                    f"produced this failure:\n\n"
                    f"```\n{failure_block.strip()}\n```\n\n"
                    f"{static_block}"
                    f"## Previous diff (REJECTED — must produce a different result)\n\n"
                    f"```\n{pass1_diff.strip()[:4000]}\n```\n\n"
                    f"## Corrected diff\n\n"
                    f"Read the failure above. The error type, object type, "
                    f"attribute name, and the exact line numbers in the traceback "
                    f"tell you precisely what the fix must do differently. Output "
                    f"rules:\n"
                    f"- First line must be `diff --git a/... b/...`.\n"
                    f"- Do NOT wrap the diff in code fences (no triple backticks).\n"
                    f"- Do NOT emit an `index <sha>..<sha>` line.\n"
                    f"- `@@` hunk headers must use the real file's line numbers.\n"
                    f"- Emit only the diff. No prose before or after.<|im_end|>\n"
                    f"<|im_start|>assistant"
                )
                pass2_suffix_path = workdir / "suffix_pass2_runtime.txt"
                pass2_suffix_path.write_text(runtime_critique)
                pass2_out_path = workdir / "out_pass2_runtime.txt"
                t2 = time.time()
                gguf = MODELS[args.model]
                prefix_path = workdir / "prefix.txt"
                summaries_aplus = workdir / "summaries-aplus.json"
                inf_env = os.environ.copy()
                r3 = sh(
                    ["python", str(OUTDIR / "run_llama_pathB.py"),
                     gguf, str(prefix_path), str(pass2_suffix_path),
                     str(summaries_aplus), str(pass2_out_path)],
                    capture_output=True, text=True, env=inf_env,
                )
                wall3 = time.time() - t2
                if r3.returncode == 0 and pass2_out_path.exists():
                    pass2_diff = pass2_out_path.read_text().strip()
                    if pass2_diff and pass2_diff != pass1_diff.strip():
                        log(f"sage runtime: pass-2 diff produced "
                            f"({len(pass2_diff)}c in {wall3:.1f}s); re-testing")
                        sh(["git", "-C", str(repo_dir), "checkout", "--", "."])
                        test_result_2 = apply_and_test(
                            inst, repo_dir, pass2_out_path, workdir=workdir,
                        )
                        if _is_better_result(test_result_2, test_result):
                            log(f"sage runtime: pass-2 promoted "
                                f"(p1={pass1_f2p} → p2={test_result_2.get('f2p')})")
                            out_path = pass2_out_path
                            test_result = test_result_2
                            wall += wall3
                        else:
                            log(f"sage runtime: pass-2 not better; keeping pass-1")
                    else:
                        log("sage runtime: pass-2 empty or unchanged; keeping pass-1")
                else:
                    log(f"sage runtime: pass-2 inference failed rc={r3.returncode}; "
                        f"keeping pass-1")
            else:
                log("sage runtime: no FAILURES block in test_log; skipping pass-2")
        except Exception as e:
            log(f"sage runtime: pass-2 raised {type(e).__name__}: {e}; keeping pass-1")

    # Tier 3 #8 (cycle 1.1): per-instance lens trajectory recording. For
    # FAIL instances, run the cycle 0.4 logit-lens once with the SAME
    # prefix+suffix the inference used. Captures per-(position, layer)
    # top-K logits at the decision band (positions 23-25 × layers 25-27).
    # Output lands at scratch/lens/cycle/cycle-<tag>-lens-<id>.jsonl so the
    # matrix analyzer (solution_curve.py) can overlay "at which layer did
    # the model commit wrong" alongside pass@k.
    #
    # Gated on GLIA_LENS_TRACE=1 (default off — lens forward pass roughly
    # doubles per-instance wall when active). Recommended for cycle 1.1+.
    if os.environ.get("GLIA_LENS_TRACE") == "1":
        try:
            f2p_str = test_result.get("f2p") or ""
            apply_str = test_result.get("apply") or ""
            should_trace = (
                f2p_str.startswith("FAIL")
                or f2p_str.startswith("NO-RUN")
                or apply_str == "APPLY-FAIL"
            )
            if should_trace:
                lens_bin = GLIA / "scratch/lens/target/release/lens"
                lens_prefix = workdir / "prefix.txt"
                lens_suffix = workdir / "suffix.txt"
                tokenizer_path = Path("/home/ivy/Models/qwen2.5-coder-tokenizer/tokenizer.json")
                lens_gguf = MODELS.get(args.model)
                if (lens_bin.exists() and lens_gguf and tokenizer_path.exists()
                        and lens_prefix.exists() and lens_suffix.exists()):
                    cycle_tag = args.tag or "untagged"
                    lens_out = (GLIA / f"scratch/lens/cycle/lens-{cycle_tag}-{inst['instance_id']}.jsonl")
                    lens_out.parent.mkdir(parents=True, exist_ok=True)
                    log(f"lens-trace: capturing decision-band trajectory for {inst['instance_id']}")
                    sh(
                        [str(lens_bin),
                         "--weights", lens_gguf,
                         "--tokenizer", str(tokenizer_path),
                         "--prefix", str(lens_prefix),
                         "--suffix", str(lens_suffix),
                         "--output-positions", "23..=27",
                         "--mode", "generate",
                         "--max-new", "32",
                         "--out", str(lens_out)],
                        capture_output=True, text=True,
                    )
                    log(f"lens-trace: wrote {lens_out}")
                else:
                    log(f"lens-trace: prereq missing (bin={lens_bin.exists()}, "
                        f"gguf={bool(lens_gguf)}, tok={tokenizer_path.exists()}, "
                        f"prefix={lens_prefix.exists()}, suffix={lens_suffix.exists()}); skipping")
        except Exception as e:
            log(f"lens-trace: raised {type(e).__name__}: {e}; continuing")

    result = {
        "instance_id": inst["instance_id"],
        "model": args.model,
        "wall_s": round(wall, 1),
        "aplus_cells": n_cells,         # AccessPath cells only
        "pool_size": n_pool,             # total summary entries in pool
        **test_result,
    }
    with open(args.results, "a") as f:
        f.write(json.dumps(result) + "\n")
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
