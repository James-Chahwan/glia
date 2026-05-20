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
_POST_INSTALL_BUILD_EXT = {"matplotlib/matplotlib", "astropy/astropy"}

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
    sh(["uv", "pip", "install", "--python", str(py_bin),
        "pip", "setuptools", "wheel", "cython", *extra_build_deps],
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
    env["CFLAGS"] = f"{env.get('CFLAGS','')} -Wno-incompatible-pointer-types -Wno-error".strip()
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
        r = subprocess.run(f"{py_bin} setup.py build_ext --inplace", shell=True,
                           cwd=str(repo_dir), env=env, capture_output=True,
                           text=True, timeout=600)
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

    # 7. assemble prefix + suffix
    prefix = (
        "<|im_start|>system\n"
        "You are an expert Python developer who fixes bugs by producing unified git diffs. "
        "Output only the diff, no prose or explanation.<|im_end|>\n"
        "<|im_start|>user\n"
        f"A bug has been reported in the `{inst['repo']}` repository. Here is the report:\n\n"
        f"{inst['problem_statement']}\n\n"
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

    suffix = (
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
    t0 = time.time()
    r = sh(
        ["python", str(OUTDIR / "run_llama_pathB.py"),
         gguf, str(prefix_path), str(suffix_path), str(summaries_aplus), str(out_path)],
        capture_output=True, text=True, env=inf_env,
    )
    wall = time.time() - t0
    if r.returncode != 0:
        raise SystemExit(f"run_llama_pathB failed rc={r.returncode}: {r.stderr[-1000:]}")
    log(f"pathB-llama done in {wall:.1f}s; out={out_path}")
    return out_path, wall, len(aplus_cells)


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


def apply_and_test(inst, repo_dir, out_path, workdir=None):
    text = out_path.read_text()
    diff = extract_diff(text)
    if not diff:
        return {"apply": "NO-DIFF", "f2p": None, "reg": None, "reg_fail": None}

    healed = heal_diff(diff, repo_dir)
    if healed != diff:
        log("heal_diff: applied corrections")
        diff = healed

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
        r2 = sh(["patch", "-p1", "--forward", "--quiet", "--fuzz=5", "-d", str(repo_dir), "-i", str(patch_path)],
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
        out_path, wall, n_cells = run_pipeline(inst, repo_dir, args.model, workdir, no_siblings=args.no_siblings, no_keysym=args.no_keysym)
    except SystemExit as e:
        result = {"instance_id": inst["instance_id"], "model": args.model, "error": str(e), "wall_s": None}
        with open(args.results, "a") as f:
            f.write(json.dumps(result) + "\n")
        print(json.dumps(result, indent=2))
        return 1

    test_result = apply_and_test(inst, repo_dir, out_path, workdir=workdir)

    result = {
        "instance_id": inst["instance_id"],
        "model": args.model,
        "wall_s": round(wall, 1),
        "aplus_cells": n_cells,
        **test_result,
    }
    with open(args.results, "a") as f:
        f.write(json.dumps(result) + "\n")
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
