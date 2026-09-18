#!/usr/bin/env bash
# check-neuropil.sh - compile neuropil against a glia tree WITHOUT touching neuropil (LG.6d).
#
# neuropil is glia's in-process Rust consumer: its root Cargo.toml path-depends on
# ../glia/{core,code-domain,graph,engine,activation,projection-text}, so `cargo test
# --workspace` never sees a leap break land on it. This script copies neuropil's
# Cargo.toml, Cargo.lock, rust-toolchain.toml and crates/ into an on-disk cache, rewrites
# the ../glia/ path deps to the glia tree under test, runs
# `cargo check --workspace --all-targets` in the copy (neuropil's pinned toolchain), and
# diffs the error list against the committed pre-leap baseline: NEW errors are the leap's,
# FIXED ones are listed, pre-existing ones are only counted.
#
#   bash scripts/check-neuropil.sh              diff against dev-notes/neuropil-check-baseline.txt
#   bash scripts/check-neuropil.sh --baseline   rewrite that baseline from this run
#
# Env: GLIA_ROOT (glia tree under test; default: this checkout), NEUROPIL_DIR (default
# /home/ivy/Code/neuropil), GLIA_CONSUMER_CACHE (default ~/.cache/glia-consumer-check).
# The cache holds a multi-GB check target, reused so only the first run is cold: never
# /tmp (tmpfs) and never neuropil/target. Nothing is written under neuropil: its git status
# and Cargo.lock hash are compared before and after, and a difference fails the run.
# Run at the close-out of the waves that land LD.9, LD.11a, LD.12b and LC.2, and at the
# 0.5.0 bump; LG.5b builds its fix list from the last run. The engram-export check is
# scripts/check-engram-export.sh (LG.13), not this script.
# Last line: [neuropil-check] ok|FAIL - new=N fixed=F preexisting=P in Ss against glia
#            <BUILD_STAMP> (neuropil <sha7>, lock drift +A -R)          exit 1 on FAIL.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ROOT=$(cd "${GLIA_ROOT:-$HERE}" 2>/dev/null && pwd) || { echo "[neuropil-check] FAIL - GLIA_ROOT=${GLIA_ROOT:-} does not exist"; exit 1; }
NP=${NEUROPIL_DIR:-/home/ivy/Code/neuropil}
BASELINE=$HERE/dev-notes/neuropil-check-baseline.txt
MODE=diff
case "${1:-}" in
  --baseline) MODE=baseline ;;
  "") ;;
  *) echo "usage: $0 [--baseline]" >&2; exit 2 ;;
esac
say() { echo "[neuropil-check] $*"; }

[ -f "$NP/Cargo.toml" ] || { say "SKIPPED - no neuropil checkout at $NP"; exit 0; }
[ -f "$ROOT/engine/Cargo.toml" ] || { say "FAIL - $ROOT is not a glia tree"; exit 1; }
[ "$MODE" = baseline ] || [ -f "$BASELINE" ] || { say "FAIL - no baseline at $BASELINE; record one with --baseline"; exit 1; }

CACHE=${GLIA_CONSUMER_CACHE:-$HOME/.cache/glia-consumer-check}/neuropil
SRC=$CACHE/src
TARGET=$CACHE/target
mkdir -p "$SRC" "$TARGET" || exit 1
exec 9>"$CACHE/.lock"
command -v flock >/dev/null && flock 9   # one run per cache: the copy is rewritten in place

# --no-optional-locks: a plain `git status` may refresh neuropil's .git/index, a write.
np_state() { git --no-optional-locks -C "$NP" status --porcelain 2>/dev/null; sha256sum < "$NP/Cargo.lock"; }
T0=$(date +%s.%N)
BEFORE=$(np_state)
NP_SHA=$(git -C "$NP" rev-parse HEAD 2>/dev/null || echo unknown)
NP_DIRTY=$(git --no-optional-locks -C "$NP" status --porcelain 2>/dev/null | wc -l)

# Copy (mtimes kept, so an unchanged neuropil crate is not rechecked). Every include_str!
# under crates/ points inside crates/; .cargo/ (clang + mold linker, aliases), target/ and
# scratch/ stay behind.
rsync -a --delete --exclude=target/ "$NP/crates/" "$SRC/crates/" || { say "FAIL - rsync of $NP/crates"; exit 1; }
# The copy's lock is re-seeded only when neuropil's changes: kept, cargo's resolution of the
# glia path deps is reused, so a warm run needs no crates.io index update (works offline).
NP_LOCK=$(sha256sum < "$NP/Cargo.lock")
if [ ! -f "$SRC/Cargo.lock" ] || [ "$NP_LOCK" != "$(cat "$CACHE/np-lock.sha" 2>/dev/null)" ]; then
  cp -p "$NP/Cargo.lock" "$SRC/Cargo.lock" && printf '%s\n' "$NP_LOCK" > "$CACHE/np-lock.sha" || exit 1
fi
rm -f "$SRC/rust-toolchain.toml"
[ -f "$NP/rust-toolchain.toml" ] && cp -p "$NP/rust-toolchain.toml" "$SRC/"
NP_SRC=$( (cd "$SRC" && find crates -type f -print0 | sort -z | xargs -0 sha256sum
           sha256sum < "$NP/Cargo.toml"; sha256sum < "$NP/Cargo.lock") | sha256sum | cut -c1-12)

# Rewrite the glia path deps in the ROOT manifest only (members use `.workspace = true`).
esc=$(printf '%s' "$ROOT" | sed 's/[&|\\]/\\&/g')
sed -E "s|path[[:space:]]*=[[:space:]]*\"\\.\\./glia/|path = \"$esc/|g" "$NP/Cargo.toml" > "$CACHE/Cargo.toml.new"
grep -qF "path = \"$ROOT/" "$CACHE/Cargo.toml.new" || { say "FAIL - no path = \"../glia/...\" dep in $NP/Cargo.toml to rewrite"; exit 1; }
stray=$(find "$SRC/crates" -name Cargo.toml -exec grep -lE 'path[[:space:]]*=[[:space:]]*"[^"]*glia' {} + 2>/dev/null)
[ -z "$stray" ] || { say "FAIL - member manifest path-depends on glia directly (extend the rewrite): $stray"; exit 1; }
cmp -s "$CACHE/Cargo.toml.new" "$SRC/Cargo.toml" || cp "$CACHE/Cargo.toml.new" "$SRC/Cargo.toml"

# rust-toolchain.toml in $SRC selects neuropil's pin; RUSTUP_TOOLCHAIN would override it.
(cd "$SRC" && env -u RUSTUP_TOOLCHAIN CARGO_TARGET_DIR="$TARGET" \
   cargo check --workspace --all-targets --message-format json-diagnostic-short) \
   > "$CACHE/last.jsonl" 2> "$CACHE/last.log"
RC=$?
RUSTC=$(cd "$SRC" && env -u RUSTUP_TOOLCHAIN rustc -V 2>/dev/null || echo "rustc unknown")
[ "$BEFORE" = "$(np_state)" ]; TOUCHED=$?

MODE=$MODE RC=$RC T0=$T0 ROOT=$ROOT SRC=$SRC TARGET=$TARGET CACHE=$CACHE NP=$NP \
NP_SHA=$NP_SHA NP_DIRTY=$NP_DIRTY NP_SRC=$NP_SRC BASELINE=$BASELINE RUSTC=$RUSTC TOUCHED=$TOUCHED \
GLIA_HEAD=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown) \
GLIA_DIRTY=$(git --no-optional-locks -C "$ROOT" status --porcelain 2>/dev/null | wc -l) \
python3 - <<'PY'
import collections, json, os, re, time
E = os.environ
root, src, cache, home = E["ROOT"], E["SRC"], E["CACHE"], os.path.expanduser("~")
subs = [(root + "/", "glia/"), (src + "/", ""), (E["TARGET"] + "/", "<target>/"),
        (home + "/.cargo/registry/src/", "<registry>/"), (home + "/.rustup/", "<rustup>/")]
def norm(s):
    for a, b in subs:
        s = s.replace(a, b)
    return s.strip()

# Errors: one normalised `<file>:<line>: error[<code>]: <message>` per rustc error (deduped:
# --all-targets checks a file once per target), plus cargo's own errors from stderr.
errs, stamp, rendered = set(), None, []
stamp_pkg = re.compile(r"path\+file://" + re.escape(root) + r"/stamp[#)]")
for raw in open(os.path.join(cache, "last.jsonl"), errors="replace"):
    try:
        m = json.loads(raw)
    except ValueError:
        continue
    if m.get("reason") == "build-script-executed" and stamp_pkg.search(m.get("package_id", "")):
        stamp = dict(map(tuple, m.get("env", []))).get("GLIA_PARSER_STAMP", stamp)
    if m.get("reason") != "compiler-message":
        continue
    d = m.get("message") or {}
    rendered.append((d.get("rendered") or "").rstrip())
    msg = ((d.get("message") or "").splitlines() or [""])[0]
    if not str(d.get("level", "")).startswith("error") or msg.startswith("aborting due to"):
        continue
    code = (d.get("code") or {}).get("code")
    sp = next((s for s in d.get("spans") or [] if s.get("is_primary")), None)
    where = f"{norm(sp['file_name'])}:{sp['line_start']}" if sp else f"({(m.get('target') or {}).get('name', '?')})"
    errs.add(f"{where}: error{f'[{code}]' if code else ''}: {norm(msg)}")
for line in open(os.path.join(cache, "last.log"), errors="replace"):
    if line.startswith("error") and not line.startswith("error: could not compile"):
        errs.add("cargo: " + norm(line))
if E["RC"] != "0" and not errs:
    errs.add(f"cargo: error: cargo check exited {E['RC']} with no parsed error (see {cache}/last.log)")
errs = sorted(errs)
with open(os.path.join(cache, "errors.txt"), "w") as f:
    f.write("".join(e + "\n" for e in errs))
with open(os.path.join(cache, "diagnostics.txt"), "w") as f:
    f.write("\n".join(r for r in rendered if r) + "\n")

# BUILD_STAMP = <workspace release>+p<GLIA_PARSER_STAMP>, the latter from the stamp crate's
# build-script output; `p?` when that script never ran (e.g. resolution failed).
rel = re.search(r'\[workspace\.package\][^\[]*?\nversion\s*=\s*"([^"]+)"',
                open(os.path.join(root, "Cargo.toml")).read())
build = f"{rel.group(1) if rel else '?'}+p{stamp or '?'}"

def pkgs(path):
    t = open(path, errors="replace").read()
    return set(re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', t))
lock_src, lock_np = pkgs(os.path.join(src, "Cargo.lock")), pkgs(os.path.join(E["NP"], "Cargo.lock"))
drift = f"+{len(lock_src - lock_np)} -{len(lock_np - lock_src)}"

# Diff key = the line minus its `:<line>`: neuropil's own edits shift lines; the leap does not.
key = lambda e: re.sub(r"^([^ ]+?):\d+: ", r"\1: ", e)
base, header = [], {}
if os.path.exists(E["BASELINE"]):
    for l in open(E["BASELINE"]):
        if l.startswith("# ") and " " in l[2:]:
            k, v = l[2:].rstrip("\n").split(" ", 1)
            header.setdefault(k, v)
        elif l.strip() and not l.startswith("#"):
            base.append(l.rstrip("\n"))
if E["MODE"] == "baseline":
    with open(E["BASELINE"], "w") as f:
        f.write("# LG.6d neuropil compile-check baseline, written by `bash scripts/check-neuropil.sh --baseline`\n"
                f"# glia {build} (HEAD {E['GLIA_HEAD']}, dirty={E['GLIA_DIRTY'].strip()})\n"
                f"# neuropil {E['NP_SHA']} dirty={E['NP_DIRTY'].strip()} src={E['NP_SRC']}\n"
                f"# rustc {E['RUSTC'].removeprefix('rustc ')}\n"
                f"# check `cargo check --workspace --all-targets` rc={E['RC']}, {len(errs)} error(s); a normal run diffs on the line minus its :<line>\n"
                + "".join(e + "\n" for e in errs))
    print(f"[neuropil-check] baseline written: {E['BASELINE']} ({len(errs)} error(s))")
    base, header = list(errs), {}
bc, cc = collections.Counter(map(key, base)), collections.Counter(map(key, errs))
new = sum((cc - bc).values()); fixed = sum((bc - cc).values()); pre = sum((cc & bc).values())
for label, have, other, surplus in (("NEW  ", errs, base, cc - bc), ("FIXED", base, errs, bc - cc)):
    for k, n in sorted(surplus.items()):
        lines = [e for e in have if key(e) == k]
        moved = [e for e in lines if e not in other]
        for e in (moved if len(moved) >= n else lines):
            print(f"[neuropil-check] {label} {e}")
was = header.get("neuropil", "")
if was and (was.split()[0] != E["NP_SHA"] or f"src={E['NP_SRC']}" not in was):
    print(f"[neuropil-check] note - neuropil differs from the baseline's ({header['neuropil']} -> "
          f"{E['NP_SHA']} src={E['NP_SRC']}): NEW/FIXED may be neuropil's own edits, not the leap's")
if E["TOUCHED"] != "0":
    print(f"[neuropil-check] FAIL - {E['NP']} changed during the run (git status or Cargo.lock): "
          "this check must never write there, or a neuropil session edited it concurrently")
ok = new == 0 and E["TOUCHED"] == "0"
print(f"[neuropil-check] {'ok' if ok else 'FAIL'} - new={new} fixed={fixed} preexisting={pre} "
      f"in {time.time() - float(E['T0']):.1f}s against glia {build} "
      f"(neuropil {E['NP_SHA'][:7]}, lock drift {drift})")
raise SystemExit(0 if ok else 1)
PY
