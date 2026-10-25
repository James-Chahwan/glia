#!/usr/bin/env python3
"""Cross-stack drift tasks (FE.4 scenario probe, 2026-10-25): a real commit that changed a
server and its clients together is split in two. The task starts at a synthetic commit S
(the real parent P plus every non-client change of C: the server, migrations and any
generated client SDK, as a regenerated SDK reflects the server), and its truth is the
client half, S..C. So the checkout holds a genuine drift bug (the server moved, the
hand-written clients did not), and the answer is the client change the authors made.

    python3 probe/drift.py [--out probe/drift-tasks] [--repos-out probe/drift-repos.json]

S is written into the bare clone as refs/agent-ab/drift/<id> (its parent is P, so the
checkout holds no future history); the row's `parent` is S. Rows use tasks.py's schema
plus "shape": "drift" and "probe" {server_files, client_files, start}.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import tasks as T  # noqa: E402

REPOS = {
    "immich": {"url": "https://github.com/immich-app/immich.git", "lang": "polyglot",
               "client": ("web/src/", "mobile/lib/"), "names": "web and mobile apps"},
    "twenty": {"url": "https://github.com/twentyhq/twenty.git", "lang": "typescript",
               "client": ("packages/twenty-front/",), "names": "front-end app"},
    "photoprism": {"url": "https://github.com/photoprism/photoprism.git", "lang": "polyglot",
                   "client": ("frontend/src/",), "names": "front-end app"},
}
GEN_RE = re.compile(r"(openapi|generated|\.g\.dart$|\.freezed\.dart$|/gen/|typescript-sdk"
                    r"|__generated__|\.pb\.)", re.I)
PICKS = {
    "immich": ["3f6a071203", "47c5a3dbf6", "7b51c50a96", "8b3d6b320b", "c7f6197e75",
               "8d377bfef5"],
    "twenty": ["5f842479ec", "b682ae535e", "09bf84ea70", "65354f9e1d"],
    "photoprism": ["fd8448d6de", "c901cd78a2", "b0c23316cc"],
}
PROMPT = ("The latest commit on this branch (HEAD) changed the server: {subject}. The {names} "
          "were not updated with it and no longer match the server. Update the client code so "
          "it works with the server as it is now.")


def is_client(cfg, path):
    return path.startswith(cfg["client"]) and not GEN_RE.search(path)


def git_env(gd, *args, env=None, inp=None):
    p = subprocess.run(["git", "--git-dir", str(gd), *args], capture_output=True, text=True,
                       env={**os.environ, **(env or {})}, input=inp)
    if p.returncode:
        raise SystemExit(f"git {' '.join(args)}: {p.stderr.strip()}")
    return p.stdout.strip()


def synth_start(gd, parent, commit, client_paths, message, when):
    """S = C's tree with every client path put back as it was at P."""
    with tempfile.TemporaryDirectory() as t:
        env = {"GIT_INDEX_FILE": str(Path(t) / "index"),
               "GIT_AUTHOR_NAME": "agent-ab", "GIT_AUTHOR_EMAIL": "agent-ab@example.invalid",
               "GIT_COMMITTER_NAME": "agent-ab",
               "GIT_COMMITTER_EMAIL": "agent-ab@example.invalid",
               "GIT_AUTHOR_DATE": f"@{when} +0000", "GIT_COMMITTER_DATE": f"@{when} +0000"}
        git_env(gd, "read-tree", commit, env=env)
        for path in sorted(client_paths):
            at_p = git_env(gd, "ls-tree", parent, "--", path)
            if at_p:
                mode, _, rest = at_p.partition(" ")
                _, _, rest = rest.partition(" ")
                sha = rest.split("\t")[0]
                git_env(gd, "update-index", "--add", "--cacheinfo", f"{mode},{sha},{path}",
                        env=env)
            else:  # added by C: drop it from the index (mode 0 removes, no work tree needed)
                git_env(gd, "update-index", "--index-info", env=env,
                        inp=f"0 {'0' * 40}\t{path}\n")
        tree = git_env(gd, "write-tree", env=env)
        return git_env(gd, "commit-tree", tree, "-p", parent, "-m", message, env=env)


def main():
    here = Path(__file__).resolve().parent
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(here / "drift-tasks"))
    ap.add_argument("--repos-out", default=str(here / "drift-repos.json"))
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    repo_rows = []
    for label, cfg in REPOS.items():
        gd = T.cache_dir() / f"{label}.git"
        head = git_env(gd, "rev-parse", "HEAD")
        repo_rows.append({"label": label, "url": cfg["url"], "end_rev": head, "lang": cfg["lang"],
                          "gate_rev": None, "tuned": False, "public": True})
        rows = []
        for short in PICKS[label]:
            commit = git_env(gd, "rev-parse", f"{short}^{{commit}}")
            parent = git_env(gd, "rev-parse", f"{commit}^1")
            subject = git_env(gd, "log", "-1", "--format=%s", commit)
            files = T.changed_files(gd, parent, commit)
            client = {p for _, p, o, _ in files if is_client(cfg, p)} | \
                     {o for _, p, o, _ in files if o and is_client(cfg, o)}
            server = sorted(p for _, p, _, _ in files if not is_client(cfg, p))
            when = int(git_env(gd, "log", "-1", "--format=%at", commit))
            start = synth_start(gd, parent, commit, client, f"server: {subject}", when)
            rid = f"{label}-{commit[:12]}"
            git_env(gd, "update-ref", f"refs/agent-ab/drift/{rid}", start)
            sfiles = T.changed_files(gd, start, commit)
            code = [f for f in sfiles if T.is_code(f[1]) and not T.is_test(f[1])]
            code += [f for f in sfiles if f[1].endswith((".svelte", ".vue")) and f not in code]
            truth = {"code": [{"path": p, "status": s, "old_path": o,
                               "hunks": T.hunks(gd, start, commit, p, o)}
                              for s, p, o, _ in sorted(code, key=lambda f: f[1])],
                     "tests": sorted(f[1] for f in sfiles if T.is_test(f[1])),
                     "other": sorted(f[1] for f in sfiles if f not in code
                                     and not T.is_test(f[1]))}
            stats = {"code_files": len(code), "code_lines": sum(f[3] for f in code),
                     "test_files": len(truth["tests"])}
            prompt = PROMPT.format(subject=T.redact(T.PREFIX_RE.sub("", subject, count=1)),
                                   names=cfg["names"])
            rows.append({"id": rid, "repo": label, "commit": commit, "parent": start,
                         "author_time": when,
                         "subject": T.redact(subject), "prompt_subject": prompt, "pr": None,
                         "issue": None, "issue_fetch": "none", "after_gate_rev": None,
                         "truth": truth, "stats": stats, "shape": "drift",
                         "probe": {"server_files": server, "client_files": sorted(client),
                                   "start": start, "real_parent": parent}})
            print(f"[drift] {rid} server={len(server)} client={len(client)} "
                  f"truth_code={len(code)} | {subject[:70]}", flush=True)
        (out / f"{label}.jsonl").write_text(
            "".join(json.dumps(r, sort_keys=True) + "\n" for r in rows))
    Path(a.repos_out).write_text(json.dumps({"repos": repo_rows}, indent=1) + "\n")


if __name__ == "__main__":
    main()
