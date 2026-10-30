#!/usr/bin/env python3
"""The outlier cohort (FH.1a): the standing corpus a glia release is judged on. Real public
repos chosen for the shape they bring (repos.json), and tasks with known answers mined from
the authors' own history (tasks/<label>.jsonl).

    python3 cohort.py build-tasks [--only <label>[,..]] [--per-kind 4] [--max-commits 3000]
                                  [--jobs 4] [--refetch]
    python3 cohort.py build-tasks --check [--only ..]
    python3 cohort.py starts [--only ..] [--compare-agent-ab]
    python3 cohort.py run --release <r> [--only ..] [--ids <regex>] [--jobs 4] [--glia <bin>]
                          [--skip-health]
    python3 cohort.py report --release <r>
    python3 cohort.py check --release <r> [--json]

`run`, `report` and `check` are the no-model runner (FH.1b); see "the runner" below.

Task kinds, newest first per repo (rows in bench/agent-ab/tasks.py's schema plus "shape"):

  drift    a commit that changed a server side and a client side together (2..10 non-test
           code files, at least one on each side; a generated path counts as the server's,
           as a regenerated SDK follows the server). The task starts at a SYNTHETIC commit S:
           the real parent P plus every non-client change of the commit C, so the checkout
           holds the server half and the truth is the authors' client half (S..C). S is never
           committed anywhere upstream, so a row holds its recipe (`start`: C, P, the client
           prefixes and the generated-path rule) and its `parent` is the ref
           refs/cohort/drift/<id> that `starts` (or build-tasks) writes into the bare clone;
           the rebuild is byte-identical (fixed identity and dates).
  ripple   a definition whose name or signature moves in one file and is edited in >= 2
           other code files (its callers); the prompt is the commit subject.
  symptom  a commit that closes a GitHub issue; the prompt is the issue alone (title + body,
           redacted), never the commit subject that states the fix.

A repo row's `picks` names commits taken first, whatever the filters say (the 13 FE.4 drift
tasks of bench/agent-ab/probe/drift.py); mined rows fill each kind up to --per-kind. A
`library` row has no boundary sides, so it gets no drift task (its role is judged by FH.2,
never on client reach).

Bare clones live under ${GLIA_COHORT_DIR:-~/.cache/glia-cohort}/<label>.git; a missing clone is
made from ${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab}/<label>.git with `git clone --local`
when that exists, else from the url. Nothing is written in the glia tree but tasks/*.jsonl.
GLIA_COHORT_OFFLINE=1: no clone, fetch or `gh` call. Issue text already in a task file is
carried over (never refetched) unless --refetch, so a rebuild is stable; --check re-derives
every row offline and exits 1 naming each differing id. Per repo it prints
`[cohort] tasks repo=<label> drift=<d> ripple=<r> symptom=<s> starts=<n>`. Stdlib only.

The runner (FH.1b) runs every no-model probe of bench/agent-ab/probe/ (health.py, oracle.py,
gate.py, names.py: their measured definitions, moved here) with one glia binary, on a thread
pool, one checkout per (task, state) under $GLIA_COHORT_DIR/work/<release>/ (a `--shared`
clone of the bare clone, deleted after), GLIA_THREADS=4:

  health   per repo at its pinned sha: `build`, `gaps`, `coverage`, the ENDPOINT / ROUTE /
           HTTP_CALLS reads, the boundary node and pair counts, unparsed single-file
           components, imported workspace packages; `flags` as health.py raised them and
           `blind` the flags the repo's role is judged on (a library never on client reach)
  locate   (every task) `find --files <prompt>` recall@10 against grep's
  rest     (every task) given the core of the change (a drift task's server half, else the
           truth file with the most changed lines applied to the work tree), the rest of the
           change from `diff-impact`, recall@10 against grep's
  gate     (drift) at S, `contract-breaks --base HEAD~1` names a client truth file in a
           schemas / models / orphaned_clients row or, since FH.1c, a `names` row (FH.7a's
           client sites still using a removed name); a shared-schema row the fold kept
           counts its changes once per entry in its `operations` (FH.7b). Each task line
           ends `cb_names=<n>`, the name rows at S (`-` when no gate ran), then
           ` (folded sample files counted)` when a folded name row (FH.7d) was read: its
           sample_files count toward truth_named beside its file (FH.1d). review is read
           with --full, so its files_named never moves with review's default caps
  clear    (drift, when the gate breaks at S) at C, fewer breaking rows than at S
  names    (drift) at S, a file `diff-impact --depth 6` reaches still holds a name the
           server change removed, and it is a client truth file
  evidence (every task, at its start) per truth file: parsed (a MODULE is located in it),
           its boundary nodes and their pair edges, through one `glia query` program

A failed check is a miss, classified: `glia_blind` (the mechanism's node or edge is absent:
the truth file unparsed, or no answer reaches it), `noisy` (it is reached, but in an answer of
more than 10x as many rows as distinct changes) or `glia_wrong` (reached, and the answer is
still wrong: a 0-breaking gate on a breaking change, a gate that does not clear, a truth file
ranked out of the top 10). A repo's health blind flag is a `glia_blind` miss of scope repo.
`run` writes results/<release>/{meta.json,health.jsonl,tasks.jsonl,misses.jsonl,summary.json,
report.md} (a run with --only / --ids replaces only its own rows) and prints
`[cohort] run release=<r> repos=<n> tasks=<t> misses=<m> (blind=<b> wrong=<w> noisy=<z>)
ms=<ms>`; `report` prints report.md; `check` re-derives every check, the miss list and the
summary from the stored probe rows and exits 1 on a difference or a missing row (FH.12's gate
compares the summary.json it validates).
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "agent-ab"))
import tasks as T  # noqa: E402

KINDS = ("drift", "ripple", "symptom")
ROLES = ("app", "library", "service-set")
PER_KIND, MAX_PER_KIND, MIN_PER_KIND, MAX_COMMITS = 4, 6, 3, 3000
# probe/drift.py's generated-path rule, verbatim: a repo's `generated` adds alternatives.
GEN_DEFAULT = (r"(openapi|generated|\.g\.dart$|\.freezed\.dart$|/gen/|typescript-sdk"
               r"|__generated__|\.pb\.)")
CODE_EXTS = T.CODE_EXTS | {".svelte"}
DRIFT_FILES = (2, 10)
MINED_FILES, MINED_LINES = (2, 12), (5, 600)
START_REF = "refs/cohort/drift/"
IDENTITY = {"GIT_AUTHOR_NAME": "glia-cohort", "GIT_AUTHOR_EMAIL": "cohort@example.invalid",
            "GIT_COMMITTER_NAME": "glia-cohort", "GIT_COMMITTER_EMAIL": "cohort@example.invalid"}
DRIFT_PROMPT = ("The latest commit on this branch (HEAD) changed the server: {subject}. The "
                "{names} were not updated with it and no longer match the server. Update the "
                "client code so it works with the server as it is now.")
SYMPTOM_PROMPT = "Resolve the GitHub issue below."
HEX40 = re.compile(r"^[0-9a-f]{40}$")
DEF_RE = re.compile(
    r"^[-+]\s*(?:export\s+)?(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?"
    r"(?:func(?:\s*\([^)]*\))?|def|function|fn|fun|class|interface|"
    r"(?:public|private|protected|static|final|override|\s)+[\w<>\[\],.? ]+?)\s+"
    r"([A-Za-z_]\w{3,})\s*[(<]")
SKIP_NAMES = {"main", "init", "test", "setup", "self", "this", "func", "function", "return"}


class CohortError(Exception):
    pass


# --- repos ------------------------------------------------------------------
def offline():
    return os.environ.get("GLIA_COHORT_OFFLINE") == "1"


def cohort_dir():
    return Path(os.environ.get("GLIA_COHORT_DIR") or Path.home() / ".cache" / "glia-cohort")


def validate(repo):
    label = repo.get("label")
    for key in ("label", "url", "sha", "lang", "role", "sides", "shape", "public"):
        if key not in repo:
            raise CohortError(f"repos.json row {label!r}: missing {key}")
    if repo["public"] is not True:
        raise CohortError(f"{label}: refused, not marked public: true")
    if not repo["url"].startswith("https://github.com/"):
        raise CohortError(f"{label}: refused, url is not a github https url")
    if not HEX40.match(repo["sha"]):
        raise CohortError(f"{label}: sha is not 40 hex")
    if repo["role"] not in ROLES:
        raise CohortError(f"{label}: role {repo['role']!r} is not one of {ROLES}")
    sides = repo["sides"]
    for key in ("server", "client"):
        if any(not p.endswith("/") for p in sides.get(key, [])):
            raise CohortError(f"{label}: a {key} prefix does not end in /")
    has = bool(sides.get("server")) and bool(sides.get("client"))
    if (repo["role"] == "library") == has:
        raise CohortError(f"{label}: a library has no sides, an app / service-set both")
    if has and not sides.get("client_names"):
        raise CohortError(f"{label}: sides need client_names (the drift prompt's words)")
    for kind, shas in repo.get("picks", {}).items():
        if kind not in KINDS or any(not HEX40.match(s) for s in shas):
            raise CohortError(f"{label}: picks.{kind} must be full shas of a known kind")


def load_repos(path=HERE / "repos.json"):
    repos = json.loads(Path(path).read_text())["repos"]
    seen = set()
    for r in repos:
        validate(r)
        if r["label"] in seen:
            raise CohortError(f"{r['label']}: label repeated")
        seen.add(r["label"])
    return repos


def has_commit(gd, rev):
    return T.git(gd, "cat-file", "-e", f"{rev}^{{commit}}", check=False).returncode == 0


def ensure_clone(repo):
    gd = cohort_dir() / f"{repo['label']}.git"
    if not gd.exists():
        if offline():
            raise CohortError(f"{repo['label']}: no bare clone at {gd} (offline)")
        gd.parent.mkdir(parents=True, exist_ok=True)
        local = T.cache_dir() / f"{repo['label']}.git"
        src = [str(local), "--local"] if local.exists() else [repo["url"]]
        subprocess.run(["git", "clone", "--bare", "--quiet", *src, str(gd)], check=True)
    if not has_commit(gd, repo["sha"]):
        if not offline():
            T.git(gd, "fetch", "--quiet", repo["url"], repo["sha"], check=False)
        if not has_commit(gd, repo["sha"]):
            raise CohortError(f"{repo['label']}: cannot resolve {repo['sha']} in {gd}")
    return gd


# --- sides and the synthetic start ----------------------------------------
def gen_rule(repo):
    extra = repo["sides"].get("generated")
    return f"(?:{GEN_DEFAULT})|(?:{extra})" if extra else GEN_DEFAULT


def is_client(prefixes, gen, path):
    return path.startswith(tuple(prefixes)) and not gen.search(path)


def is_code(path):
    return os.path.splitext(path)[1] in CODE_EXTS


def client_paths(files, prefixes, gen):
    """Every path of parent..commit the client half owns (new and, for a rename, old)."""
    out = set()
    for _, path, old, _ in files:
        out |= {p for p in (path, old) if p and is_client(prefixes, gen, p)}
    return out


def git_out(gd, *args, env=None, inp=None):
    p = subprocess.run(["git", "--git-dir", str(gd), "-c", "commit.gpgSign=false", *args],
                       capture_output=True, text=True, input=inp,
                       env={**os.environ, "LC_ALL": "C", **(env or {})})
    if p.returncode:
        raise CohortError(f"git {' '.join(args[:2])}: {p.stderr.strip()}")
    return p.stdout.strip()


def build_start(gd, row):
    """Write S (C's tree with every client path put back as it was at P) as the row's ref and
    return its sha. Deterministic: same recipe, same bytes, in any clone of the repo."""
    st = row["start"]
    commit, parent = st["commit"], st["real_parent"]
    paths = client_paths(T.changed_files(gd, parent, commit), st["client_prefixes"],
                         re.compile(st["generated"], re.I))
    subject = git_out(gd, "log", "-1", "--format=%s", commit)
    when = git_out(gd, "log", "-1", "--format=%at", commit)
    with tempfile.TemporaryDirectory() as t:
        env = {"GIT_INDEX_FILE": str(Path(t) / "index"), **IDENTITY,
               "GIT_AUTHOR_DATE": f"@{when} +0000", "GIT_COMMITTER_DATE": f"@{when} +0000"}
        git_out(gd, "read-tree", commit, env=env)
        for path in sorted(paths):
            at_p = git_out(gd, "ls-tree", parent, "--", path)
            if at_p:
                meta, _, _ = at_p.partition("\t")
                mode, _, sha = meta.split(" ")
                git_out(gd, "update-index", "--add", "--cacheinfo", f"{mode},{sha},{path}",
                        env=env)
            else:  # the commit added it: drop it (mode 0 removes, no work tree needed)
                git_out(gd, "update-index", "--index-info", env=env,
                        inp=f"0 {'0' * 40}\t{path}\n")
        tree = git_out(gd, "write-tree", env=env)
        start = git_out(gd, "commit-tree", tree, "-p", parent, "-m", f"server: {subject}",
                        env=env)
    git_out(gd, "update-ref", st["ref"], start)
    return start


def checkout_rev(gd, row):
    """The rev a task starts at: a drift row's synthetic start (rebuilt when its ref is
    missing), else the row's parent."""
    if row["shape"] != "drift":
        return row["parent"]
    if has_commit(gd, row["start"]["ref"]):
        return git_out(gd, "rev-parse", row["start"]["ref"])
    return build_start(gd, row)


# --- mining -----------------------------------------------------------------
def truth_of(gd, a, b, files):
    code = [f for f in files if is_code(f[1]) and not T.is_test(f[1])]
    truth = {"code": [{"path": p, "status": s, "old_path": o, "hunks": T.hunks(gd, a, b, p, o)}
                      for s, p, o, _ in sorted(code, key=lambda f: f[1])],
             "tests": sorted(f[1] for f in files if T.is_test(f[1])),
             "other": sorted(f[1] for f in files if f not in code and not T.is_test(f[1]))}
    stats = {"code_files": len(code), "code_lines": sum(f[3] for f in code),
             "test_files": len(truth["tests"])}
    return truth, stats


def diff_text(gd, parent, commit, path, old):
    return T.git(gd, "diff", "--no-ext-diff", "--no-textconv", "-M", "-U0", parent, commit,
                 "--", *([old] if old else []), path).stdout.decode("utf-8", errors="replace")


def ripple(gd, parent, commit, code):
    """probe/mine.py's rule: the best (name, defining file, caller files) or None."""
    texts = {f[1]: diff_text(gd, parent, commit, f[1], f[2]) for f in code}
    best = None
    for path, text in sorted(texts.items()):
        names = sorted({m.group(1) for line in text.splitlines() if line[:1] in "+-"
                        for m in [DEF_RE.match(line)]
                        if m and m.group(1).lower() not in SKIP_NAMES})
        for name in names:
            word = re.compile(rf"\b{re.escape(name)}\b")
            callers = sorted(p for p, t in texts.items() if p != path and any(
                line[:1] in "+-" and word.search(line) for line in t.splitlines()))
            if len(callers) >= 2 and (best is None or len(callers) > len(best[2])):
                best = (name, path, callers)
    return best


def base_row(repo, commit, parent, when, subject, prompt, pr, shape):
    return {"id": f"{repo['label']}-{commit[:12]}", "repo": repo["label"], "commit": commit,
            "parent": parent, "author_time": when, "subject": T.redact(subject),
            "prompt_subject": prompt, "pr": pr, "issue": None, "issue_fetch": "none",
            "after_gate_rev": None, "shape": shape}


def drift_row(repo, gd, commit, parent, when, subject, pr, files):
    """The drift row of commit, or None when it does not split into two non-empty halves."""
    sides = repo["sides"]
    gen = re.compile(gen_rule(repo), re.I)
    client = client_paths(files, sides["client"], gen)
    if not client or len(client) == len({p for f in files for p in (f[1], f[2]) if p}):
        return None
    row = base_row(repo, commit, None, when, subject, DRIFT_PROMPT.format(
        subject=T.redact(T.PREFIX_RE.sub("", subject, count=1)),
        names=sides["client_names"]), pr, "drift")
    row["start"] = {"ref": START_REF + row["id"], "commit": commit, "real_parent": parent,
                    "client_prefixes": list(sides["client"]), "generated": gen_rule(repo)}
    row["parent"] = row["start"]["ref"]
    start = build_start(gd, row)
    row["truth"], row["stats"] = truth_of(gd, start, commit, T.changed_files(gd, start, commit))
    row["probe"] = {"client_files": sorted(client), "real_parent": parent,
                    "server_files": sorted(f[1] for f in files if f[1] not in client)}
    return row


def drift_eligible(repo, files):
    sides = repo["sides"]
    if not sides.get("client"):
        return False
    gen = re.compile(gen_rule(repo), re.I)
    code = [f for f in files if is_code(f[1]) and not T.is_test(f[1])]
    if not DRIFT_FILES[0] <= len(code) <= DRIFT_FILES[1] or \
            sum(f[3] for f in code) < MINED_LINES[0]:
        return False
    client = [f for f in code if is_client(sides["client"], gen, f[1])]
    server = [f for f in code if not is_client(sides["client"], gen, f[1])
              and (f[1].startswith(tuple(sides["server"])) or gen.search(f[1]))]
    return bool(client) and bool(server)


def mined_size_ok(files):
    code = [f for f in files if is_code(f[1]) and not T.is_test(f[1])]
    lines = sum(f[3] for f in files if is_code(f[1]))
    return (MINED_FILES[0] <= len(code) <= MINED_FILES[1]
            and MINED_LINES[0] <= lines <= MINED_LINES[1]
            and any(f[0] in "MR" for f in code)), code


def entry_of(gd, commit):
    out = T.git(gd, "log", "-1", "--format=%H%x00%P%x00%an%x00%ae%x00%cn%x00%ce%x00%at%x00%B",
                commit).stdout.decode("utf-8", errors="replace")
    h, parents, an, ae, cn, ce, at, body = out.split("\x00", 7)
    return {"commit": h, "parents": parents.split(), "people": (an, ae, cn, ce),
            "author_time": int(at), "message": body.rstrip("\n").replace("\r\n", "\n")}


def pick_row(repo, gd, kind, sha):
    """A picked commit's row, whatever the mining filters say (its first parent is P)."""
    e = entry_of(gd, sha)
    if not e["parents"]:
        raise CohortError(f"{repo['label']}: pick {sha} has no parent")
    subject = e["message"].split("\n")[0].strip()
    m = T.MERGE_PR_RE.match(subject)
    pr = int(m.group(1)) if m else None
    files = T.changed_files(gd, e["parents"][0], sha)
    if kind == "drift":
        row = drift_row(repo, gd, sha, e["parents"][0], e["author_time"], subject, pr, files)
        if row is None:
            raise CohortError(f"{repo['label']}: drift pick {sha} has no client half")
        return row
    raise CohortError(f"{repo['label']}: picks.{kind} is not supported (drift only)")


def fetch_issue(url, refs):
    """tasks.py's issue fetch (`gh api`, redacted, PRs skipped); nothing when offline."""
    return (None, "unavailable") if offline() else T.fetch_issue(url, refs)


def mine(repo, gd, per_kind=PER_KIND, max_commits=MAX_COMMITS, carried=None,
         fetch=fetch_issue):
    """The repo's task rows. `carried` maps id -> (issue, issue_fetch) of rows already
    committed; a symptom candidate not in it is fetched unless carried is strict (check)."""
    rows, used = {k: [] for k in KINDS}, set()
    for kind in KINDS:
        for sha in repo.get("picks", {}).get(kind, []):
            rows[kind].append(pick_row(repo, gd, kind, sha))
            used.add(sha)
    cap = {k: max(per_kind, len(rows[k])) for k in KINDS}
    strict = carried is not None and carried.get("__strict__", False)
    for i, e in enumerate(T.log_first_parent(gd, repo["sha"])):
        if i >= max_commits or all(len(rows[k]) >= cap[k] for k in KINDS):
            break
        if e["commit"] in used:
            continue
        c = T.candidate(e)
        if not c:
            continue
        parent, subject, prompt, pr, ref_text = c
        files = T.changed_files(gd, parent, e["commit"])
        if len(rows["drift"]) < cap["drift"] and drift_eligible(repo, files):
            row = drift_row(repo, gd, e["commit"], parent, e["author_time"], subject, pr, files)
            if row:
                rows["drift"].append(row)
                used.add(e["commit"])
                continue
        ok, code = mined_size_ok(files)
        if not ok:
            continue
        rid = f"{repo['label']}-{e['commit'][:12]}"
        if len(rows["symptom"]) < cap["symptom"] and T.CLOSING_RE.search(ref_text):
            if carried and rid in carried:
                issue, fetched = carried[rid]
            elif strict:
                issue, fetched = None, "none"
            else:
                issue, fetched = fetch(repo["url"], T.issue_refs(ref_text))
            truth, stats = truth_of(gd, parent, e["commit"], files)
            if issue and not T.names_a_file(issue["title"], truth):
                row = base_row(repo, e["commit"], parent, e["author_time"], subject,
                               SYMPTOM_PROMPT, pr, "symptom")
                row.update(issue=issue, issue_fetch=fetched, truth=truth, stats=stats,
                           probe={})
                rows["symptom"].append(row)
                used.add(e["commit"])
                continue
        if len(rows["ripple"]) < cap["ripple"]:
            r = ripple(gd, parent, e["commit"], code)
            if r:
                row = base_row(repo, e["commit"], parent, e["author_time"], subject,
                               T.redact(prompt), pr, "ripple")
                row["truth"], row["stats"] = truth_of(gd, parent, e["commit"], files)
                row["probe"] = {"symbol": r[0], "defined_in": r[1], "callers": r[2]}
                rows["ripple"].append(row)
                used.add(e["commit"])
    out = []
    for kind in KINDS:
        out += sorted(rows[kind], key=lambda r: (-r["author_time"], r["id"]))
    return out


# --- files ------------------------------------------------------------------
def render(rows):
    return "".join(json.dumps(r, sort_keys=True, ensure_ascii=False) + "\n" for r in rows)


def marker(label, rows, starts):
    n = {k: sum(r["shape"] == k for r in rows) for k in KINDS}
    print(f"[cohort] tasks repo={label} drift={n['drift']} ripple={n['ripple']} "
          f"symptom={n['symptom']} starts={starts}", flush=True)


def read_rows(path):
    return [json.loads(x) for x in Path(path).read_text().splitlines()] \
        if Path(path).exists() else []


def build_one(repo, out, a):
    gd = ensure_clone(repo)
    path = out / f"{repo['label']}.jsonl"
    old = read_rows(path)
    carried = {r["id"]: (r["issue"], r["issue_fetch"]) for r in old if r.get("issue")}
    if a.check:
        carried["__strict__"] = True
        fresh = mine(repo, gd, a.per_kind, a.max_commits, carried)
        bad = [rid for rid in sorted({r["id"] for r in fresh} | {r["id"] for r in old})
               if next((r for r in fresh if r["id"] == rid), None)
               != next((r for r in old if r["id"] == rid), None)]
        if not bad and render(fresh) != (path.read_text() if path.exists() else ""):
            bad.append(f"{repo['label']} (row order or rendering)")
        rows = fresh
    else:
        rows = mine(repo, gd, a.per_kind, a.max_commits, None if a.refetch else carried)
        out.mkdir(parents=True, exist_ok=True)
        path.write_text(render(rows))
        bad = []
    return repo["label"], rows, bad


def cmd_build(a, repos):
    out, bad, total = Path(a.out), [], 0
    roles = {r["label"]: r["role"] for r in repos}
    with ThreadPoolExecutor(max_workers=max(1, a.jobs)) as pool:
        for label, rows, b in pool.map(lambda r: build_one(r, out, a), repos):
            marker(label, rows, sum(r["shape"] == "drift" for r in rows))
            kinds = [k for k in KINDS if not (k == "drift" and roles[label] == "library")]
            short = [k for k in kinds if sum(r["shape"] == k for r in rows) < MIN_PER_KIND]
            if short:
                print(f"[cohort] short repo={label} kinds={','.join(short)} "
                      f"(history holds fewer than {MIN_PER_KIND} in the first "
                      f"{a.max_commits} commits)", flush=True)
            bad += b
            total += len(rows)
    digest = hashlib.sha256("".join(
        render(read_rows(out / f"{r['label']}.jsonl")) for r in repos).encode()).hexdigest()
    print(f"[cohort] tasks total={total} repos={len(repos)} sha256={digest[:12]}")
    for rid in bad:
        print(f"[cohort] check differs: {rid}", file=sys.stderr)
    return 1 if bad else 0


def cmd_starts(a, repos):
    """Rebuild every drift row's synthetic start; with --compare-agent-ab, compare its tree to
    probe/drift.py's refs/agent-ab/drift/<id> in the agent-ab clone (FE.4's 13 tasks)."""
    bad, compared = [], 0
    for repo in repos:
        gd = ensure_clone(repo)
        rows = [r for r in read_rows(Path(a.out) / f"{repo['label']}.jsonl")
                if r["shape"] == "drift"]
        for r in rows:
            tree = git_out(gd, "rev-parse", build_start(gd, r) + "^{tree}")
            line = f"[cohort] start {r['id']} tree={tree[:12]}"
            if a.compare_agent_ab:
                ab = T.cache_dir() / f"{repo['label']}.git"
                ref = f"refs/agent-ab/drift/{r['id']}"
                if ab.exists() and has_commit(ab, ref):
                    want = git_out(ab, "rev-parse", ref + "^{tree}")
                    compared += 1
                    line += f" agent_ab={'same' if want == tree else 'DIFFERS'}"
                    if want != tree:
                        bad.append(r["id"])
            print(line, flush=True)
        marker(repo["label"], read_rows(Path(a.out) / f"{repo['label']}.jsonl"), len(rows))
    if a.compare_agent_ab:
        print(f"[cohort] starts compared={compared} differ={len(bad)}")
    return 1 if bad else 0


# --- the runner (FH.1b): the probes' measured definitions --------------------
# bench/agent-ab/probe/oracle.py's locate / rest words, verbatim
STOP = set("""this that with from into when then than they them their there these those have
has had were been being will would should could about after before under over also only
just some such each every other more most make made does done doing change changes changed
update updated updates fix fixed fixes issue issues add added adds remove removed removes
use used uses using allow allows support supports supported work works working server
client code file files function method class value values make sure when what which while
where your need needs latest commit branch head were longer match matches below github
resolve test tests version""".split())
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]{3,}")
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*[A-Z_][A-Za-z0-9_]*|[a-z][a-z0-9]+(?:_[a-z0-9]+)+")
# bench/agent-ab/probe/names.py's tokens and noise list, verbatim
TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_\-]{3,}")
NOISE = set("""this that true false null none self string number boolean return const
static class public private protected async await function import export default from
type interface extends implements readonly optional required description example
deprecated value values field fields name names data error errors message response
request result results param params query body status code item items list option
options config index count total string_ int64 int32 float uint8 json yaml omitempty
override public void final new delete update create get set post put patch""".split())
NAMES_CODE = T.CODE_EXTS | {".svelte", ".vue", ".json"}
SFC = (".svelte", ".astro", ".mdx")  # health.py's unparsed single-file components
# the boundary: the client-side node kinds and the cross-stack pair categories
BOUNDARY_KINDS = ("ENDPOINT", "GRPC_CLIENT", "GRAPHQL_OPERATION", "WS_CLIENT", "RPC_CALL",
                  "QUEUE_PRODUCER", "EVENT_EMITTER")
PAIR_CATS = ("HTTP_CALLS", "GRPC_CALLS", "GRAPHQL_CALLS", "WS_CONNECTS", "RPC_CALLS",
             "QUEUE_FLOWS", "EVENT_FLOWS")
CHECKS = ("locate", "rest", "gate", "clear", "names")
MISS_CLASSES = ("glia_blind", "glia_wrong", "noisy")
NOISE_FACTOR = 10
# the health flags health.py raised; a library is never judged on the client-reach three
REACH_FLAGS = ("routes_unreached", "clients_unpaired", "no_client_endpoints")
GLIA_THREADS = "4"
RESULTS = HERE / "results"
# the tree's release glia (`cargo build --release -p glia-cli`)
DEFAULT_GLIA = HERE.parent.parent / "target" / "release" / "glia"
TIMEOUT_BUILD, TIMEOUT_ANSWER = 3600, 1800


def sh(cmd, cwd=None, env=None, timeout=TIMEOUT_ANSWER, raw=False):
    """(rc, stdout, stderr, ms); stdout is bytes when raw."""
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, timeout=timeout)
        out = p.stdout if raw else p.stdout.decode("utf-8", "replace")
        return p.returncode, out, p.stderr.decode("utf-8", "replace"), \
            int((time.monotonic() - t0) * 1000)
    except subprocess.TimeoutExpired:
        return 124, b"" if raw else "", "timeout", int((time.monotonic() - t0) * 1000)


def checkout(gd, rev, copy):
    """A work tree at rev that shares the bare clone's objects (no copy of the history) and,
    as the probes' `git init` + fetch did, has no remote and no branch."""
    if copy.exists():
        shutil.rmtree(copy)
    copy.parent.mkdir(parents=True, exist_ok=True)
    for cmd, cwd in ((["git", "clone", "-q", "--shared", "--no-checkout", str(gd), str(copy)],
                      None),
                     (["git", "checkout", "-q", "--detach", rev], copy),
                     (["git", "remote", "remove", "origin"], copy)):
        rc, _, err, _ = sh(cmd, cwd=cwd, env={**os.environ, "LC_ALL": "C"})
        if rc:
            raise CohortError(f"{' '.join(cmd[:3])}: {err.strip()[-200:]}")


def recall(ranked, truth, k=10):
    truth = set(truth)
    if not truth:
        return None, None
    first = next((i + 1 for i, f in enumerate(ranked) if f in truth), None)
    return len(truth & set(ranked[:k])) / len(truth), first


def grep_rank(copy, words, exclude):
    score = {}
    for w in words:
        _, out, _, _ = sh(["git", "grep", "-l", "-i", "-w", "-I", w, "--", ".", ":!*.lock",
                           ":!*lock.json", ":!*.min.js"], cwd=copy, timeout=120)
        for f in out.splitlines():
            if f and f not in exclude:
                score[f] = score.get(f, 0) + 1
    return [f for f, _ in sorted(score.items(), key=lambda kv: (-kv[1], kv[0]))]


def prompt_words(text):
    seen, out = set(), []
    for w in WORD.findall(text):
        lw = w.lower()
        if lw not in STOP and lw not in seen:
            seen.add(lw)
            out.append(w)
    return out[:25]


def diff_idents(diff):
    minus, plus = set(), set()
    for line in diff.splitlines():
        if line.startswith("-") and not line.startswith("---"):
            minus |= set(IDENT.findall(line[1:]))
        elif line.startswith("+") and not line.startswith("+++"):
            plus |= set(IDENT.findall(line[1:]))
    one_side = sorted(minus ^ plus, key=lambda s: (-len(s), s))
    both = sorted(minus & plus, key=lambda s: (-len(s), s))
    return (one_side + both)[:25]


def files_in(obj, acc):
    """gate.py: every `file` string anywhere in an answer, and every string of a folded name
    row's `sample_files` (FH.7d: the row's `file` is only its first site's; FH.1d)."""
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k == "file" and isinstance(v, str):
                acc.add(v)
            elif k == "sample_files" and isinstance(v, list):
                acc.update(f for f in v if isinstance(f, str))
            else:
                files_in(v, acc)
    elif isinstance(obj, list):
        for v in obj:
            files_in(v, acc)
    return acc


def changes_in(rows, names=()):
    """gate.py: the distinct (section, field, change) triples contract-breaks rows hold.
    A row the shared-schema fold kept (FH.7b) stands for every operation in its
    `operations`, so its triples count once per operation (FH.1c); a name row (FH.7a) is
    one change per removed name, however many client sites still use it."""
    out = set()
    for r in rows:
        ops = r.get("operations") or [None]
        for op in ops:
            tag = () if op is None else (op,)
            for c in r.get("changes") or []:
                out.add((c.get("section"), c.get("field"), c.get("change")) + tag)
            if not r.get("changes"):
                out.add((r.get("kind"), r.get("key"), r.get("change")) + tag)
    for r in names:
        out.add(("name", r.get("normalised"), r.get("renamed_to")))
    return out


def norm(s):
    return s.lower().replace("_", "").replace("-", "")


def old_names(diff):
    """names.py: names on removed lines no added line keeps, case-normalised."""
    minus, plus = {}, set()
    for line in diff.splitlines():
        if line.startswith(("---", "+++")):
            continue
        toks = TOKEN.findall(line[1:])
        if line.startswith("-"):
            for t in toks:
                minus.setdefault(norm(t), t)
        elif line.startswith("+"):
            plus |= {norm(t) for t in toks}
    return {n: raw for n, raw in minus.items()
            if n not in plus and n not in NOISE and len(n) >= 4 and not n.isdigit()}


def holds(copy, path, names):
    try:
        text = (copy / path).read_text(errors="replace")
    except OSError:
        return []
    have = {norm(t) for t in TOKEN.findall(text)}
    return sorted(n for n in names if n in have)


def query_rows(d):
    """health.py's rows_of: the result rows of a `glia query --json` answer."""
    if not isinstance(d, dict):
        return []
    for k in ("rows", "results", "tables"):
        v = d.get(k)
        if isinstance(v, list):
            if v and isinstance(v[0], dict) and "rows" in v[0]:
                return [x for t in v for x in t["rows"]]
            return v
    return []


def values(row):
    return [v.get("str", v.get("int")) if isinstance(v, dict) else v
            for v in row.get("values") or []]


class Glia:
    """One glia binary run in one work tree."""

    def __init__(self, binary, copy):
        self.bin, self.copy = str(binary), copy
        self.env = {**os.environ, "GLIA_THREADS": GLIA_THREADS}
        self.env.pop("GLIA_NO_PERSIST", None)

    def run(self, *args, timeout=TIMEOUT_ANSWER):
        return sh([self.bin, *args], cwd=self.copy, env=self.env, timeout=timeout)

    def json(self, *args):
        """(answer or None, bytes, ms, error or None)."""
        rc, out, err, ms = self.run(*args)
        try:
            return json.loads(out), len(out.encode()), ms, None
        except ValueError:
            return None, len(out.encode()), ms, f"rc={rc} {err.strip()[-150:]}"

    def build(self):
        rc, _, err, ms = self.run("build", ".", timeout=TIMEOUT_BUILD)
        if rc:
            raise CohortError(f"build rc={rc} {err.strip()[-160:]}")
        return ms

    def datalog(self, program, outputs):
        path = self.copy / ".glia" / "cohort-probe.dl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(program)
        args = ["query", ".", str(path), "--json", "--limit", "1000000"]
        for o in outputs:
            args += ["--output", o]
        d, _, _, err = self.json(*args)
        if d is None:
            raise CohortError(f"query: {err}")
        out = {o: [] for o in outputs}
        for r in query_rows(d):
            out.setdefault(r.get("predicate"), []).append(values(r))
        return out


def dl_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def facts(pred, items):
    return "".join(f"{pred}({dl_str(x)}).\n" for x in items)


def evidence(glia, paths):
    """Per path: exists (in the work tree: a file the change adds does not), parsed (a
    MODULE is located in it), its boundary node kinds, the pair edges leaving or reaching
    those nodes."""
    program = (facts("truth", sorted(paths)) + facts("bkind", BOUNDARY_KINDS)
               + facts("pcat", PAIR_CATS)
               + 'parsed(F) :- truth(F), file(N, F), node(N, "MODULE").\n'
               + "cnode(F, K) :- truth(F), file(N, F), node(N, K), bkind(K).\n"
               + "paired(F, C) :- truth(F), file(N, F), node(N, K), bkind(K), "
                 "edge(N, M, C), pcat(C).\n"
               + "paired(F, C) :- truth(F), file(N, F), node(N, K), bkind(K), "
                 "edge(M, N, C), pcat(C).\n")
    out = glia.datalog(program, ["parsed", "cnode", "paired"])
    parsed = {v[0] for v in out["parsed"]}
    return {p: {"exists": (glia.copy / p).is_file(), "parsed": p in parsed,
                "boundary": sorted({v[1] for v in out["cnode"] if v[0] == p}),
                "paired": sorted({v[1] for v in out["paired"] if v[0] == p})}
            for p in sorted(paths)}


def task_line(res):
    """The runner's per-task line: each check's class or status, then the name rows at S, and
    ` (folded sample files counted)` when a folded name row was read (FH.1d)."""
    cbn = ((res.get("S") or {}).get("gate") or {}).get("cb") or {}
    return (f"[cohort] task {res['task']} {res['shape']} " + " ".join(
        f"{c['check']}={c['class'] or c['status']}" for c in res["checks"])
        + f" cb_names={cbn.get('names', '-')}"
        + (" (folded sample files counted)" if cbn.get("names_folded") else ""))


def gate_state(glia, truth):
    """gate.py's state, after the build: contract-breaks and review vs HEAD~1."""
    res = {}
    d, nb, ms, err = glia.json("contract-breaks", ".", "--base", "HEAD~1", "--json")
    if isinstance(d, dict):
        rows = [r for k in ("schemas", "models", "orphaned_clients") for r in d.get(k) or []]
        # FH.1c: a name row (FH.7a) names the client file still using a removed name;
        # an answer without the key (before FH.7a) reads as no name row
        names = d.get("names") or []
        named = files_in(rows + names, set())
        res["cb"] = {"breaking": d.get("breaking"), "advisory": d.get("advisory"),
                     "schemas": len(d.get("schemas") or []), "models": len(d.get("models") or []),
                     "orphaned_clients": len(d.get("orphaned_clients") or []),
                     "names": len(names),
                     # FH.1d: the folded name rows (FH.7d), whose sample_files count above
                     "names_folded": sum(1 for r in names if r.get("sample_files")),
                     "rows": len(rows) + len(names),
                     "distinct_changes": len(changes_in(rows, names)),
                     "files_named": len(named),
                     "truth_named": sorted(t for t in truth if t in named),
                     # oracle.py's contract_breaks row count: every list in the answer
                     "all_rows": sum(len(v) for v in d.values() if isinstance(v, list)),
                     "bytes": nb, "ms": ms}
    else:
        res["cb"] = {"error": err}
    # --full (FH.1d): every list uncapped, so files_named moves with glia's reach and never
    # with review's default caps (FH.8c / FH.8d)
    d, nb, ms, err = glia.json("review", ".", "--base", "HEAD~1", "--full", "--json")
    if isinstance(d, dict):
        named = files_in(d, set())
        res["review"] = {"truth_named": sorted(t for t in truth if t in named),
                         "files_named": len(named), "bytes": nb, "ms": ms,
                         "counts": d.get("counts")}
    else:
        res["review"] = {"error": err}
    return res


def names_state(glia, truth, changed):
    """names.py's state, after the build."""
    _, diff, _, _ = sh(["git", "diff", "HEAD~1", "HEAD"], cwd=glia.copy)
    names = old_names(diff)
    # --top-k 200: the pre-FH.8c default row cap, so the baseline's numbers stay comparable (FH.8c
    # made 25 the default; the boundary set reads every impact row's file).
    d, _, ms, err = glia.json("diff-impact", ".", "--base", "HEAD~1", "--json",
                              "--top-files", "200", "--depth", "6", "--top-k", "200")
    if not isinstance(d, dict):
        return {"error": f"diff-impact {err}"}
    imp = d.get("impact") or {}
    boundary = {f["file"] for f in imp.get("files") or [] if f.get("file")}
    boundary |= {r["file"] for r in imp.get("results") or [] if r.get("file")}
    boundary -= changed
    flagged = {f: hit for f in sorted(boundary)
               if os.path.splitext(f)[1] in NAMES_CODE
               for hit in [holds(glia.copy, f, names)] if hit}
    _, ls, _, _ = sh(["git", "ls-files"], cwd=glia.copy)
    control = {f: hit for f in ls.splitlines()
               if f not in changed and os.path.splitext(f)[1] in NAMES_CODE
               and not T.is_test(f) for hit in [holds(glia.copy, f, names)] if hit}
    t = set(truth)
    return {"names": len(names), "name_sample": sorted(names.values())[:12],
            "boundary": len(boundary), "boundary_has_truth": sorted(t & boundary),
            "flagged": len(flagged), "flagged_truth": sorted(t & set(flagged)),
            "control": len(control), "control_truth": sorted(t & set(control)), "ms": ms}


def locate(glia, row, truth):
    """oracle.py's L: find --files from the prompt (a drift task's subject) against grep."""
    issue = row.get("issue") or {}
    text = row["prompt_subject"] + (" " + (issue.get("title") or "") + " "
                                    + (issue.get("body") or "") if issue else "")
    if row["shape"] == "drift":
        text = row["subject"]
    # `--` ends the options: a prompt may start with a dash
    d, nb, ms, err = glia.json("find", "--files", "--json", "--top-k", "20", ".", "--",
                               text[:500])
    g_files = [r["file"] for r in (d or {}).get("results", []) if r.get("file")]
    w = prompt_words(text)
    t1 = time.monotonic()
    r_files = grep_rank(glia.copy, w, set())
    return {"glia": recall(g_files, truth), "grep": recall(r_files, truth), "glia_bytes": nb,
            "glia_ms": ms, "glia_err": err, "grep_ms": int((time.monotonic() - t1) * 1000),
            "words": w[:10], "glia_top": g_files[:5], "grep_top": r_files[:5],
            "truth_in_answer": sorted(set(truth) & set(g_files))}


def rest(glia, gd, row, truth):
    """oracle.py's R: given the core of the change, the rest of it."""
    if row["shape"] == "drift":
        seed = set(row["probe"]["server_files"])
        _, diff, _, _ = sh(["git", "diff", "HEAD~1", "HEAD"], cwd=glia.copy)
        base = ["--base", "HEAD~1"]
    else:
        core = max(row["truth"]["code"], key=lambda c: sum(h[1] for h in c["hunks"]) or 1)
        seed = {core["path"]}
        p = glia.copy / core["path"]
        if core["status"] == "D":
            p.unlink(missing_ok=True)
        else:
            rc, blob, err, _ = sh(["git", "--git-dir", str(gd), "show",
                                   f"{row['commit']}:{core['path']}"], raw=True)
            if rc:
                return {"error": f"seed: {err.strip()[:120]}"}
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(blob)
        _, diff, _, _ = sh(["git", "diff"], cwd=glia.copy)
        base = []
    rest_files = [f for f in truth if f not in seed]
    d, nb, ms, err = glia.json("diff-impact", ".", *base, "--json", "--top-files", "20", "--top-k", "200")
    imp = (d or {}).get("impact") or {}
    g_files = [f["file"] for f in imp.get("files") or [] if f.get("file") not in seed]
    reached = {f["file"] for f in imp.get("files") or [] if f.get("file")}
    reached |= {r["file"] for r in imp.get("results") or [] if r.get("file")}
    ids = diff_idents(diff)
    t1 = time.monotonic()
    r_files = grep_rank(glia.copy, ids, seed)
    return {"rest": len(rest_files), "glia": recall(g_files, rest_files),
            "grep": recall(r_files, rest_files), "glia_bytes": nb, "glia_ms": ms,
            "glia_err": err, "grep_ms": int((time.monotonic() - t1) * 1000),
            "idents": ids[:10], "glia_top": g_files[:5], "grep_top": r_files[:5],
            "rows": len(imp.get("results") or []), "rest_files": rest_files,
            "rest_in_answer": sorted(set(rest_files) & reached)}


def work_dir(release):
    return cohort_dir() / "work" / release


def run_task(row, repo, glia_bin, release):
    """Every probe of one task. A drift task: at S the build, the gate, the evidence, names,
    then `history sync` and locate / rest (oracle.py's order of state: the gate and names
    probes never synced history); at C the build, the gate and names. Any other task: build,
    evidence, history sync, locate, rest."""
    truth = [c["path"] for c in row["truth"]["code"]]
    res = {"task": row["id"], "repo": row["repo"], "shape": row["shape"], "truth": truth}
    t0 = time.monotonic()
    try:
        gd = ensure_clone(repo)
        start = checkout_rev(gd, row)
    except (CohortError, subprocess.CalledProcessError, OSError) as e:
        res["error"] = f"start: {str(e)[:200]}"
        return res
    states = [("S", start)] + ([("C", row["commit"])] if row["shape"] == "drift" else [])
    for name, rev in states:
        copy = work_dir(release) / f"{row['id']}-{name}"
        glia = Glia(glia_bin, copy)
        key = name if row["shape"] == "drift" else "start"
        st = res.setdefault(key, {})
        try:
            checkout(gd, rev, copy)
            st["build_ms"] = glia.build()
            if row["shape"] == "drift":
                st["gate"] = gate_state(glia, truth)
                if name == "S":
                    st["evidence"] = evidence(glia, truth)
                st["names"] = names_state(glia, truth, set(row["probe"]["server_files"]))
            else:
                st["evidence"] = evidence(glia, truth)
            if name == "S" or row["shape"] != "drift":
                rc, _, err, _ = glia.run("history", "sync", ".", timeout=TIMEOUT_BUILD)
                if rc:
                    raise CohortError(f"history sync rc={rc} {err.strip()[-160:]}")
                res["L"] = locate(glia, row, truth)
                res["R"] = rest(glia, gd, row, truth)
        except (CohortError, subprocess.CalledProcessError, OSError, ValueError,
                KeyError) as e:
            st["error"] = str(e)[:300]
        finally:
            shutil.rmtree(copy, ignore_errors=True)
    res["ms"] = int((time.monotonic() - t0) * 1000)
    res["checks"] = classify(res)
    return res


def run_health(repo, glia_bin, release):
    """health.py's measures at the repo's pinned sha, plus the boundary counts."""
    res = {"repo": repo["label"], "role": repo["role"]}
    copy = work_dir(release) / f"health-{repo['label']}"
    glia = Glia(glia_bin, copy)
    try:
        checkout(ensure_clone(repo), repo["sha"], copy)
        res["build_ms"] = glia.build()
        g = glia.json("gaps", ".", "--json")[0]
        res["gaps"] = (g or {}).get("counts")
        s = glia.json("coverage", ".", "--json")[0]
        res["coverage_rows"] = len(s.get("rows", [])) if isinstance(s, dict) else None
        lim = ["--json", "--limit", "100000"]
        e = glia.json("query", ".", "--gql", "-e", "MATCH (e:ENDPOINT) RETURN e.qname", *lim)[0]
        r = glia.json("query", ".", "--gql", "-e", "MATCH (r:ROUTE) RETURN r.qname", *lim)[0]
        p = glia.json("query", ".", "--gql", "-e", "MATCH (e:ENDPOINT)-[:HTTP_CALLS]->"
                      "(r:ROUTE) RETURN e.qname, r.qname", *lim)[0]
        ends, routes, pairs = query_rows(e), query_rows(r), query_rows(p)
        # FH.17's test-site endpoints (ORIGIN test_fixture) pair nothing by design, and
        # engine/src/health.rs leaves them out of client_reach: so does the harness
        tfe = {v[0] for v in glia.datalog('tfe(Q) :- node(N, "ENDPOINT"), '
                                          'origin(N, "test_fixture"), qname(N, Q).\n',
                                          ["tfe"])["tfe"]}
        ends = [x for x in ends if x.get("values") and values(x)[0] not in tfe]
        pairs = [x for x in pairs if x.get("values") and values(x)[0] not in tfe]
        res["test_fixture_endpoints"] = len(tfe)
        paired_e = {values(x)[0] for x in pairs if x.get("values")}
        paired_r = {values(x)[1] for x in pairs if x.get("values")}
        gp = res["gaps"] or {}
        res.update(endpoints=len(ends), routes=len(routes), http_pairs=len(pairs),
                   routes_with_caller=len(paired_r),
                   client_reach=round(len(paired_e) / len(ends), 3) if ends else None,
                   route_reach=round(1 - gp.get("unpaired_route", 0) / len(routes), 3)
                   if routes else None,
                   suspicious_endpoints=sum(1 for x in ends if re.search(
                       r"(data:|blob:|mailto:|base64)", json.dumps(x))),
                   dead_symbol=gp.get("dead_symbol", 0))
        counts = glia.datalog(
            facts("bkind", BOUNDARY_KINDS) + facts("pcat", PAIR_CATS)
            + "boundary_nodes(K, count(N)) :- node(N, K), bkind(K).\n"
            + "boundary_pairs(C, count(A)) :- edge(A, B, C), pcat(C).\n",
            ["boundary_nodes", "boundary_pairs"])
        res["boundary_nodes"] = {v[0]: v[1] for v in sorted(counts["boundary_nodes"])}
        res["boundary_pairs"] = {v[0]: v[1] for v in sorted(counts["boundary_pairs"])}
        _, ls, _, _ = sh(["git", "ls-files"], cwd=copy)
        ext = {}
        for f in ls.splitlines():
            x = os.path.splitext(f)[1]
            ext[x] = ext.get(x, 0) + 1
        res["sfc_files"] = {k: ext[k] for k in SFC if ext.get(k)}
        res["vue_files"] = ext.get(".vue", 0)
        names = set()
        for f in ls.splitlines():
            if f.endswith("package.json") and "node_modules" not in f and f != "package.json":
                try:
                    n = json.loads((copy / f).read_text()).get("name")
                except (OSError, ValueError, AttributeError):
                    n = None
                if isinstance(n, str) and n:
                    names.add(n)
        res["workspace_packages"] = len(names)
        used = 0
        for n in sorted(names)[:200]:
            _, out, _, _ = sh(["git", "grep", "-l", "-F", f"from '{n}", "--", "*.ts", "*.tsx",
                               "*.js", "*.svelte", "*.vue"], cwd=copy, timeout=60)
            used += bool(out.strip())
        res["workspace_packages_imported"] = used
        res["flags"] = health_flags(res)
        res["blind"] = [f for f in res["flags"] if not f.endswith("?")
                        and not (repo["role"] == "library" and f in REACH_FLAGS)]
    except (CohortError, subprocess.CalledProcessError, OSError, ValueError, KeyError,
            IndexError) as ex:
        res["error"] = str(ex)[:200]
    finally:
        shutil.rmtree(copy, ignore_errors=True)
    return res


def health_flags(h):
    """health.py's blind flags, verbatim (`workspace_imports?` is a question, not a flag)."""
    flags = []
    if h["routes"] and h["route_reach"] is not None and h["route_reach"] < 0.2:
        flags.append("routes_unreached")
    if h["endpoints"] and h["client_reach"] is not None and h["client_reach"] < 0.2:
        flags.append("clients_unpaired")
    if h["routes"] and h["endpoints"] < 0.1 * h["routes"]:
        flags.append("no_client_endpoints")
    if h["sfc_files"]:
        flags.append("unparsed_sfc")
    if h["workspace_packages_imported"]:
        flags.append("workspace_imports?")
    return flags


# --- the runner: classification ---------------------------------------------
def blind_why(ev, files, reached):
    """Why no answer reached these truth files: per file, unparsed / no boundary node /
    unpaired boundary nodes / no path. None when one of them is reached."""
    if set(files) & set(reached):
        return None
    out = []
    for f in sorted(files):
        e = (ev or {}).get(f)
        if e is None:
            out.append(f"{f}: no evidence")
        elif not e["exists"]:
            continue
        elif not e["parsed"]:
            out.append(f"{f}: unparsed")
        elif not e["boundary"]:
            out.append(f"{f}: no path (no boundary node in the file)")
        elif not e["paired"]:
            out.append(f"{f}: unpaired {','.join(e['boundary'])}")
        else:
            out.append(f"{f}: no path (paired {','.join(e['paired'])})")
    return "; ".join(out)


def check(name, status, cls=None, why=""):
    return {"check": name, "status": status, "class": cls, "why": why}


def existing(ev, files):
    """The files a question can still find: those in the work tree at the task's start (a
    file the change adds cannot be named before it exists). Without evidence, all of them."""
    return [f for f in files if (ev or {}).get(f, {}).get("exists", True)]


def classify(res):
    """The task's checks: status pass / miss / n/a / error, a miss's class and why. Pure: it
    reads only the stored probe rows, so `check` re-derives it."""
    if res.get("error"):
        return [check(c, "error", why=res["error"]) for c in CHECKS]
    drift = res["shape"] == "drift"
    S = res.get("S" if drift else "start") or {}
    ev = S.get("evidence")
    truth = res["truth"]
    out = []
    L = res.get("L")
    if not L or L.get("glia_err"):
        out.append(check("locate", "error", why=(L or {}).get("glia_err") or S.get("error", "")))
    elif L["glia"][0]:
        out.append(check("locate", "pass"))
    elif not existing(ev, truth):
        out.append(check("locate", "n/a", why="every truth file is added by the change"))
    elif ev and not any(ev[f]["parsed"] for f in existing(ev, truth) if f in ev):
        out.append(check("locate", "miss", "glia_blind", "every truth file unparsed"))
    else:
        out.append(check("locate", "miss", "glia_wrong", "no truth file in find's top 10 "
                          f"(first at {L['glia'][1] or '>20'})"))
    R = res.get("R")
    if not R or R.get("error") or R.get("glia_err"):
        out.append(check("rest", "error", why=(R or {}).get("error") or (R or {}).get(
            "glia_err") or S.get("error", "")))
    elif not R["rest"]:
        out.append(check("rest", "n/a", why="the core is the whole change"))
    elif R["glia"][0]:
        out.append(check("rest", "pass"))
    elif not existing(ev, R["rest_files"]):
        out.append(check("rest", "n/a", why="the rest is files the change adds"))
    else:
        why = blind_why(ev, R["rest_files"], R["rest_in_answer"])
        detail = (f"{len(R['rest_in_answer'])} of {R['rest']} rest files in {R['rows']} rows, "
                  f"first ranked file at {R['glia'][1] or '>20'}")
        if why is not None:
            out.append(check("rest", "miss", "glia_blind", why))
        elif R["rows"] > NOISE_FACTOR * R["rest"]:
            out.append(check("rest", "miss", "noisy", detail))
        else:
            out.append(check("rest", "miss", "glia_wrong", detail))
    if not drift:
        return out + [check(c, "n/a", why="not a drift task") for c in ("gate", "clear", "names")]
    names = S.get("names") or {}
    reached = names.get("boundary_has_truth") if "error" not in names else None
    cb = (S.get("gate") or {}).get("cb") or {}
    if S.get("error") or "error" in cb or "breaking" not in cb:
        out.append(check("gate", "error", why=S.get("error") or cb.get("error", "")))
    elif cb["truth_named"]:
        out.append(check("gate", "pass"))
    elif not existing(ev, truth):
        out.append(check("gate", "n/a", why="every client truth file is added by the change"))
    else:
        why = blind_why(ev, truth, reached) if reached is not None else None
        if why is not None:
            out.append(check("gate", "miss", "glia_blind", why))
        elif cb["distinct_changes"] and cb["rows"] > NOISE_FACTOR * cb["distinct_changes"]:
            out.append(check("gate", "miss", "noisy", f"{cb['rows']} rows for "
                             f"{cb['distinct_changes']} distinct changes, no client file"))
        elif not cb["breaking"]:
            out.append(check("gate", "miss", "glia_wrong", "0 breaking on a breaking change"))
        else:
            out.append(check("gate", "miss", "glia_wrong",
                             f"{cb['breaking']} breaking, no client file named"))
    C = res.get("C") or {}
    ccb = (C.get("gate") or {}).get("cb") or {}
    if "error" in cb or not cb.get("breaking"):
        out.append(check("clear", "n/a", why="the gate does not break at S"))
    elif C.get("error") or "error" in ccb or "breaking" not in ccb:
        out.append(check("clear", "error", why=C.get("error") or ccb.get("error", "")))
    elif ccb["breaking"] < cb["breaking"]:
        out.append(check("clear", "pass"))
    elif ccb["distinct_changes"] and ccb["rows"] > NOISE_FACTOR * ccb["distinct_changes"]:
        out.append(check("clear", "miss", "noisy", f"{ccb['rows']} rows for "
                         f"{ccb['distinct_changes']} distinct changes at C"))
    else:
        out.append(check("clear", "miss", "glia_wrong", f"breaking {cb['breaking']} at S, "
                         f"{ccb['breaking']} at C after the authors' fix"))
    if S.get("error") or "error" in names or "flagged" not in names:
        out.append(check("names", "error", why=S.get("error") or names.get("error", "")))
    elif names["flagged_truth"]:
        out.append(check("names", "pass"))
    elif not existing(ev, truth):
        out.append(check("names", "n/a", why="every client truth file is added by the change"))
    else:
        why = blind_why(ev, truth, names["boundary_has_truth"])
        if why is not None:
            out.append(check("names", "miss", "glia_blind", why))
        elif names["flagged"] > NOISE_FACTOR * len(truth):
            out.append(check("names", "miss", "noisy", f"{names['flagged']} files flagged for "
                             f"{len(truth)} truth files"))
        else:
            out.append(check("names", "miss", "glia_wrong",
                             "a client truth file is reached and holds no removed name"))
    return out


def misses_of(health, tasks):
    out = []
    for h in health:
        for f in h.get("blind") or []:
            out.append({"repo": h["repo"], "task": None, "scope": "repo", "check": "health",
                        "class": "glia_blind", "why": f})
    for t in tasks:
        for c in t.get("checks") or []:
            if c["status"] == "miss":
                out.append({"repo": t["repo"], "task": t["task"], "scope": t["shape"],
                            "check": c["check"], "class": c["class"], "why": c["why"]})
    return sorted(out, key=lambda m: (m["repo"], m["task"] or "", CHECKS.index(m["check"])
                                      if m["check"] in CHECKS else -1, m["why"]))


def summarize(release, glia_line, repos, health, tasks):
    """The comparable summary (no times): what FH.12's gate reads."""
    misses = misses_of(health, tasks)
    hb = {h["repo"]: h for h in health}
    per = {}
    for r in repos:
        h = hb.get(r["label"], {})
        rows = [t for t in tasks if t["repo"] == r["label"]]
        checks = {c: {s: sum(1 for t in rows for x in t.get("checks") or []
                             if x["check"] == c and x["status"] == s)
                      for s in ("pass", "miss", "n/a", "error")} for c in CHECKS}
        per[r["label"]] = {
            "role": r["role"], "tasks": len(rows), "checks": checks,
            "health": {k: h.get(k) for k in ("routes", "endpoints", "http_pairs",
                                             "client_reach", "route_reach", "dead_symbol",
                                             "blind", "error")},
            "misses": {k: sum(1 for m in misses if m["repo"] == r["label"]
                              and m["class"] == k) for k in MISS_CLASSES}}
    return {"release": release, "glia": glia_line, "repos": per,
            "tasks": {t["task"]: {c["check"]: c["status"] for c in t.get("checks") or []}
                      for t in sorted(tasks, key=lambda t: t["task"])},
            "misses": {"total": len(misses),
                       **{k: sum(m["class"] == k for m in misses) for k in MISS_CLASSES}},
            "errors": sum(1 for t in tasks for c in t.get("checks") or []
                          if c["status"] == "error") + sum(1 for h in health if h.get("error"))}


# --- the runner: files, report, commands -------------------------------------
def jsonl(rows):
    return "".join(json.dumps(r, sort_keys=True, ensure_ascii=False) + "\n" for r in rows)


def results_dir(a):
    return Path(a.results) / a.release


def load_results(d):
    meta = json.loads((d / "meta.json").read_text())
    return meta, read_rows(d / "health.jsonl"), read_rows(d / "tasks.jsonl")


def frac(rows, c):
    p = sum(1 for t in rows for x in t.get("checks") or [] if x["check"] == c
            and x["status"] == "pass")
    n = sum(1 for t in rows for x in t.get("checks") or [] if x["check"] == c
            and x["status"] in ("pass", "miss"))
    e = sum(1 for t in rows for x in t.get("checks") or [] if x["check"] == c
            and x["status"] == "error")
    return (f"{p}/{n}" if n else "-") + (f" ({e} err)" if e else "")


def cell(v):
    if v is None:
        return "-"
    if isinstance(v, dict):
        return ", ".join(f"{k} {n}" for k, n in sorted(v.items())) or "-"
    if isinstance(v, list):
        return ", ".join(v) or "-"
    return str(v).replace("|", "\\|")


def render_report(meta, repos, health, tasks):
    s = summarize(meta["release"], meta["glia"], repos, health, tasks)
    misses = misses_of(health, tasks)
    hb = {h["repo"]: h for h in health}
    m = s["misses"]
    out = [f"# Cohort results: {meta['release']}", "",
           f"glia `{meta['glia']}`; {len(repos)} repos, {len(tasks)} tasks; misses {m['total']} "
           f"(glia_blind {m['glia_blind']}, glia_wrong {m['glia_wrong']}, noisy {m['noisy']}); "
           f"errors {s['errors']}. Written by `python3 bench/cohort/cohort.py run --release "
           f"{meta['release']}` (no model); `check` re-derives it.", "",
           "## Boundary health per repo", "",
           "| repo | role | routes | endpoints | HTTP pairs | client reach | route reach | "
           "dead | boundary nodes | unparsed | blind |",
           "|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in repos:
        h = hb.get(r["label"], {})
        if h.get("error"):
            out.append(f"| {r['label']} | {r['role']} | error: {cell(h['error'])} |"
                       + " |" * 8)
            continue
        out.append("| " + " | ".join(cell(x) for x in (
            r["label"], r["role"], h.get("routes"), h.get("endpoints"), h.get("http_pairs"),
            h.get("client_reach"), h.get("route_reach"), h.get("dead_symbol"),
            h.get("boundary_nodes"), h.get("sfc_files"), h.get("blind"))) + " |")
    out += ["", "## Probes per repo", "",
            "Each cell is passed / judged (a check that does not apply is not judged).", "",
            "| repo | tasks | locate | rest | gate | clear | names | blind | wrong | noisy |",
            "|---|---|---|---|---|---|---|---|---|---|"]
    for r in repos:
        rows = [t for t in tasks if t["repo"] == r["label"]]
        pm = s["repos"][r["label"]]["misses"]
        out.append(f"| {r['label']} | {len(rows)} | " + " | ".join(
            frac(rows, c) for c in CHECKS) + f" | {pm['glia_blind']} | {pm['glia_wrong']} | "
            f"{pm['noisy']} |")
    drift = [t for t in tasks if t["shape"] == "drift"]
    out += ["", "## The drift gate", "",
            f"contract-breaks at S named a client truth file on "
            f"{sum(1 for t in drift if (t.get('S', {}).get('gate', {}).get('cb') or {}).get('truth_named'))}"
            f" of {len(drift)} drift tasks; review on "
            f"{sum(1 for t in drift if (t.get('S', {}).get('gate', {}).get('review') or {}).get('truth_named'))}.",
            "", "| task | S breaking | rows | distinct | client named | review named | "
            "C breaking | names: reached / flagged truth |",
            "|---|---|---|---|---|---|---|---|"]
    for t in drift:
        S, C = t.get("S") or {}, t.get("C") or {}
        cb = (S.get("gate") or {}).get("cb") or {}
        rv = (S.get("gate") or {}).get("review") or {}
        ccb = (C.get("gate") or {}).get("cb") or {}
        nm = S.get("names") or {}
        out.append("| " + " | ".join(cell(x) for x in (
            t["task"], cb.get("breaking", cb.get("error")), cb.get("rows"),
            cb.get("distinct_changes"), len(cb.get("truth_named") or []),
            len(rv.get("truth_named") or []), ccb.get("breaking", ccb.get("error")),
            f"{len(nm.get('boundary_has_truth') or [])} / {len(nm.get('flagged_truth') or [])}"
            f" of {len(t['truth'])}")) + " |")
    out += ["", "## Misses by class", ""]
    for k in MISS_CLASSES:
        rows = [x for x in misses if x["class"] == k]
        out += [f"### {k} ({len(rows)})", ""]
        if not rows:
            out += ["none", ""]
            continue
        out += ["| repo | task | check | why |", "|---|---|---|---|"]
        out += [f"| {x['repo']} | {x['task'] or '-'} | {x['check']} | {cell(x['why'])} |"
                for x in rows]
        out.append("")
    errs = [(t["task"], c["check"], c["why"]) for t in tasks for c in t.get("checks") or []
            if c["status"] == "error"] + [(h["repo"], "health", h["error"]) for h in health
                                          if h.get("error")]
    if errs:
        out += [f"## Errors ({len(errs)})", "", "| task | check | error |", "|---|---|---|"]
        out += [f"| {a} | {b} | {cell(c)} |" for a, b, c in errs]
        out.append("")
    return "\n".join(out)


def write_results(d, meta, repos, health, tasks):
    d.mkdir(parents=True, exist_ok=True)
    order = {r["label"]: i for i, r in enumerate(repos)}
    health = sorted(health, key=lambda h: order.get(h["repo"], len(order)))
    tasks = sorted(tasks, key=lambda t: (order.get(t["repo"], len(order)), t["task"]))
    (d / "meta.json").write_text(json.dumps(meta, indent=1, sort_keys=True) + "\n")
    (d / "health.jsonl").write_text(jsonl(health))
    (d / "tasks.jsonl").write_text(jsonl(tasks))
    (d / "misses.jsonl").write_text(jsonl(misses_of(health, tasks)))
    (d / "summary.json").write_text(json.dumps(
        summarize(meta["release"], meta["glia"], repos, health, tasks), indent=1,
        sort_keys=True) + "\n")
    (d / "report.md").write_text(render_report(meta, repos, health, tasks))
    return health, tasks


def glia_version(binary):
    rc, out, err, _ = sh([str(binary), "--version"], timeout=60)
    if rc:
        raise CohortError(f"{binary} --version: rc={rc} {err.strip()[-120:]}")
    return out.strip()


def cmd_run(a, repos, all_repos):
    t0 = time.monotonic()
    binary = Path(a.glia).expanduser()
    corpus = "".join(render(read_rows(Path(a.out) / f"{r['label']}.jsonl")) for r in all_repos)
    meta = {"release": a.release, "glia": glia_version(binary),
            "glia_threads": int(GLIA_THREADS), "noise_factor": NOISE_FACTOR,
            "tasks_sha256": hashlib.sha256(corpus.encode()).hexdigest()[:12]}
    d = results_dir(a)
    old_health, old_tasks = [], []
    if (d / "meta.json").exists():
        old_meta, old_health, old_tasks = load_results(d)
        for k in ("glia", "tasks_sha256"):
            if old_meta.get(k) != meta[k]:
                raise CohortError(f"{d} holds {k} {old_meta.get(k)!r}, this run {meta[k]!r}: "
                                  "one release's results come from one glia over one corpus")
    by = {r["label"]: r for r in repos}
    per_repo = [[("task", row, r) for row in read_rows(Path(a.out) / f"{r['label']}.jsonl")
                 if not a.ids or re.search(a.ids, row["id"])] for r in repos]
    # round robin over the repos, so one big repo's builds never fill every worker at once
    work = [w for i in range(max(map(len, per_repo), default=0)) for ws in per_repo
            if i < len(ws) for w in [ws[i]]]
    hwork = [] if a.skip_health or a.ids else [("health", None, r) for r in repos]
    print(f"[cohort] run start release={a.release} repos={len(by)} tasks={len(work)} "
          f"health={len(hwork)} jobs={a.jobs} glia={meta['glia']}", flush=True)
    new_health, new_tasks = [], []

    def one(w):
        kind, row, repo = w
        if kind == "health":
            return kind, run_health(repo, binary, a.release)
        return kind, run_task(row, repo, binary, a.release)
    with ThreadPoolExecutor(max_workers=max(1, a.jobs)) as pool:
        for kind, res in pool.map(one, hwork + work):
            if kind == "health":
                new_health.append(res)
                print(f"[cohort] health {res['repo']} routes={res.get('routes')} endpoints="
                      f"{res.get('endpoints')} pairs={res.get('http_pairs')} client_reach="
                      f"{res.get('client_reach')} dead={res.get('dead_symbol')} blind="
                      f"{','.join(res.get('blind') or []) or '-'}"
                      f"{' error=' + res['error'] if res.get('error') else ''}", flush=True)
            else:
                new_tasks.append(res)
                print(task_line(res), flush=True)
    keep_h = {h["repo"] for h in new_health}
    keep_t = {t["task"] for t in new_tasks}
    health = [h for h in old_health if h["repo"] not in keep_h] + new_health
    tasks = [t for t in old_tasks if t["task"] not in keep_t] + new_tasks
    health, tasks = write_results(d, meta, all_repos, health, tasks)
    s = summarize(a.release, meta["glia"], all_repos, health, tasks)
    m = s["misses"]
    ran = {t["repo"] for t in tasks} | {h["repo"] for h in health}
    print(f"[cohort] run release={a.release} repos={len(ran)} "
          f"tasks={len(tasks)} misses={m['total']} (blind={m['glia_blind']} "
          f"wrong={m['glia_wrong']} noisy={m['noisy']}) ms={int((time.monotonic() - t0) * 1000)}",
          flush=True)
    if s["errors"]:
        print(f"[cohort] run errors={s['errors']} (see report.md)", flush=True)
    return 0


def cmd_report(a, repos):
    meta, health, tasks = load_results(results_dir(a))
    sys.stdout.write(render_report(meta, repos, health, tasks))
    return 0


def cmd_check(a, repos):
    """Re-derive every check, the misses, the summary and the report from the probe rows;
    every repo needs a health row and every task in tasks/ a probe row."""
    d = results_dir(a)
    meta, health, tasks = load_results(d)
    bad = []
    for t in tasks:
        if classify(t) != t.get("checks"):
            bad.append(f"checks differ: {t['task']}")
    want = {row["id"] for r in repos for row in read_rows(Path(a.out) / f"{r['label']}.jsonl")}
    have = {t["task"] for t in tasks}
    bad += [f"no probe row: {i}" for i in sorted(want - have)]
    bad += [f"probe row of no task: {i}" for i in sorted(have - want)]
    bad += [f"no health row: {r['label']}" for r in repos
            if r["label"] not in {h["repo"] for h in health}]
    if (d / "misses.jsonl").read_text() != jsonl(misses_of(health, tasks)):
        bad.append("misses.jsonl differs from the probe rows")
    s = summarize(meta["release"], meta["glia"], repos, health, tasks)
    if json.loads((d / "summary.json").read_text()) != s:
        bad.append("summary.json differs from the probe rows")
    if (d / "report.md").read_text() != render_report(meta, repos, health, tasks):
        bad.append("report.md differs from the probe rows")
    for b in bad:
        print(f"[cohort] check differs: {b}", file=sys.stderr)
    if a.json:
        print(json.dumps(s, indent=1, sort_keys=True))
    m = s["misses"]
    print(f"[cohort] check release={a.release} {'ok' if not bad else 'FAILED'} - "
          f"repos={len(repos)} tasks={len(tasks)} misses={m['total']} "
          f"(blind={m['glia_blind']} wrong={m['glia_wrong']} noisy={m['noisy']}) "
          f"errors={s['errors']}{'' if not bad else f' differences={len(bad)}'}",
          file=sys.stderr if a.json else sys.stdout)
    return 1 if bad else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("cmd", choices=["build-tasks", "starts", "run", "report", "check"])
    ap.add_argument("--only", help="comma-separated labels")
    ap.add_argument("--per-kind", type=int, default=PER_KIND)
    ap.add_argument("--max-commits", type=int, default=MAX_COMMITS)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--check", action="store_true", help="re-derive offline, compare")
    ap.add_argument("--refetch", action="store_true", help="fetch every issue again")
    ap.add_argument("--compare-agent-ab", action="store_true")
    ap.add_argument("--repos", default=str(HERE / "repos.json"), help=argparse.SUPPRESS)
    ap.add_argument("--out", default=str(HERE / "tasks"), help=argparse.SUPPRESS)
    ap.add_argument("--release", help="run / report / check: the results/<release>/ dir")
    ap.add_argument("--ids", help="run: only the tasks whose id matches this regex (no health)")
    ap.add_argument("--glia", default=str(DEFAULT_GLIA), help="run: the glia binary")
    ap.add_argument("--skip-health", action="store_true", help="run: tasks only")
    ap.add_argument("--json", action="store_true", help="check: print the summary JSON")
    ap.add_argument("--results", default=str(RESULTS), help=argparse.SUPPRESS)
    a = ap.parse_args(argv)
    if not 1 <= a.per_kind <= MAX_PER_KIND:
        ap.error(f"--per-kind must be 1..{MAX_PER_KIND}")
    if a.cmd in ("run", "report", "check") and not a.release:
        ap.error(f"{a.cmd} needs --release")
    if a.release and not re.match(r"^[A-Za-z0-9][A-Za-z0-9._-]*$", a.release):
        ap.error("--release is a plain name (letters, digits, . _ -)")
    try:
        all_repos = repos = load_repos(a.repos)
        if a.only:
            want = a.only.split(",")
            repos = [r for r in repos if r["label"] in want]
            if len(repos) != len(set(want)):
                raise CohortError(f"unknown label in --only {a.only!r}")
        if a.check:
            os.environ["GLIA_COHORT_OFFLINE"] = "1"
            os.environ["GLIA_AGENT_AB_OFFLINE"] = "1"
        if a.cmd == "run":
            for r in repos:  # serially: two threads must never clone one repo at once
                ensure_clone(r)
            return cmd_run(a, repos, all_repos)
        if a.cmd == "report":
            return cmd_report(a, all_repos)
        if a.cmd == "check":
            return cmd_check(a, all_repos)
        return cmd_build(a, repos) if a.cmd == "build-tasks" else cmd_starts(a, repos)
    except (CohortError, T.TaskError, subprocess.CalledProcessError, OSError, ValueError) as e:
        print(f"[cohort] error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
