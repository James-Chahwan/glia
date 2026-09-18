#!/usr/bin/env bash
# check-engram-export.sh - build + test engram-export OUT of the workspace, writing nothing tracked (LG.13).
#
# engram-export/ is excluded from the workspace (it path-depends on the sibling Engram checkout's
# engram-core), so `cargo test --workspace` never compiles it - yet it is an in-repo consumer of
# glia's crates, names and struct literals. An in-place `cargo test --manifest-path
# engram-export/Cargo.toml` rewrites the TRACKED engram-export/Cargo.lock whenever resolution moves
# (any crate added under engine), which dirties the tree. So: copy the crate into its own gitignored
# target (engram-export/target/check-src), rewrite its relative path deps to absolute ones, and run
# `cargo test --offline` there with engram-export/target as the build cache - separate from the
# workspace target, so it never queues on agents' builds. Engram is only read.
#
#   bash scripts/check-engram-export.sh                 build + test; report Cargo.lock drift
#   bash scripts/check-engram-export.sh --update-lock   ...then copy the fresh lock over the tracked one
#   bash scripts/check-engram-export.sh --canary        negative control: injects a compile error
#
# Env: ENGRAM_DIR (default: <this checkout>/../Engram); GLIA_ROOT (glia tree under test, default this
# checkout - mid-wave, siblings' half-written crates break the live tree, so point it at a coherent
# one, e.g. `git archive HEAD` plus your change; --update-lock then writes THAT tree's lock).
# Last line, gated on by dev-notes/wave-runner/closeout.py:
#   [engram-export] check: ok - build + N tests passed against engram-core GMAP_FORMAT_VERSION=V
#   [engram-export] check: FAILED (<why>) against engram-core GMAP_FORMAT_VERSION=V        exit 1
#   [engram-export] check: SKIPPED - no Engram checkout at <dir>                            exit 0
# A green run leaves the binary at engram-export/target/debug/glia-export-engram; run that for
# real-repo acceptance instead of `cargo run` in place. Full cargo output: target/check-last.log.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ROOT=$(cd "${GLIA_ROOT:-$HERE}" 2>/dev/null && pwd) || { echo "[engram-export] check: FAILED (GLIA_ROOT=${GLIA_ROOT:-} does not exist)"; exit 1; }
ENGRAM=${ENGRAM_DIR:-$(cd "$HERE/.." && pwd)/Engram}
CRATE=$ROOT/engram-export
TARGET=$HERE/engram-export/target
SRC=$TARGET/check-src
CANARY=0 UPDATE=0
for a in "$@"; do
  case "$a" in
    --canary) CANARY=1 ;;
    --update-lock) UPDATE=1 ;;
    *) echo "usage: $0 [--canary] [--update-lock]" >&2; exit 2 ;;
  esac
done
say() { echo "[engram-export] $*"; }

[ -f "$ENGRAM/crates/engram-core/Cargo.toml" ] || { say "check: SKIPPED - no Engram checkout at $ENGRAM"; exit 0; }
VER=$(grep -oE 'GMAP_FORMAT_VERSION: u32 = [0-9]+' "$ENGRAM/crates/engram-core/src/lib.rs" | grep -oE '[0-9]+$')
fail() { say "check: FAILED ($1) against engram-core GMAP_FORMAT_VERSION=${VER:-?}"; exit 1; }
[ -f "$CRATE/Cargo.toml" ] || fail "no engram-export crate in $ROOT"

mkdir -p "$TARGET" || fail "mkdir $TARGET"
exec 9>"$TARGET/.check.lock"
command -v flock >/dev/null && flock 9   # one run at a time: the copy is rebuilt in place

# Copy everything but target/ (src, examples, tests, Cargo.lock); tar keeps mtimes, so an
# unchanged crate is not recompiled.
rm -rf "$SRC" && mkdir -p "$SRC" || fail "reset $SRC"
tar --exclude=./target -C "$CRATE" -cf - . | tar -C "$SRC" -xf - || fail "copy of $CRATE"

# ../../Engram/ first: the generic ../ rule would otherwise eat it. Both are crate-name-agnostic.
esc() { printf '%s' "$1" | sed 's/[&|\\]/\\&/g'; }
sed -E -e "s|(path[[:space:]]*=[[:space:]]*\")\.\./\.\./Engram/|\1$(esc "$ENGRAM")/|" \
       -e "s|(path[[:space:]]*=[[:space:]]*\")\.\./|\1$(esc "$ROOT")/|" "$CRATE/Cargo.toml" > "$SRC/Cargo.toml" \
  || fail "rewrite of Cargo.toml"
stray=$(grep -nE 'path[[:space:]]*=[[:space:]]*"[^/"]' "$SRC/Cargo.toml")
[ -z "$stray" ] || fail "relative path dep left unrewritten: $stray"
grep -q '^\[workspace\]' "$SRC/Cargo.toml" || printf '\n[workspace]\n' >> "$SRC/Cargo.toml"
[ "$CANARY" = 1 ] && printf '\nconst _: () = assert!(false, "engram-export check canary");\n' >> "$SRC/src/lib.rs"

eng_lock() { stat -c %Y "$ENGRAM/Cargo.lock" 2>/dev/null; sha256sum < "$ENGRAM/Cargo.lock" 2>/dev/null; }
E0=$(eng_lock)
# cd: cargo config and rustup toolchain discovery walk up from the cwd, so every caller gets
# the same ones (glia's), wherever it was started.
cargo_in() { (cd "$SRC" && CARGO_TARGET_DIR="$TARGET" cargo "$@" --offline --manifest-path "$SRC/Cargo.toml" 2>&1); }
out=$(cargo_in test); rc=$? step="cargo test"
# `cargo test` links no plain bin unless tests/ exists: build it for the real-repo runs.
[ "$rc" = 0 ] && { out+=$'\n'$(cargo_in build --bins); rc=$? step="cargo build --bins"; }
printf '%s\n' "$out" > "$TARGET/check-last.log"
[ "$(eng_lock)" = "$E0" ] || fail "$ENGRAM/Cargo.lock changed during the run - this check must only read Engram"

if [ "$rc" != 0 ]; then
  printf '%s\n' "$out" | grep -E '^error' | head -20
  say "full cargo output: $TARGET/check-last.log"
  fail "$step rc=$rc"
fi
passed=$(printf '%s\n' "$out" | grep -oE 'test result: ok\. [0-9]+ passed' | awk '{s += $4} END {print s + 0}')

# Drift: packages whose [[package]] entry (version, source, dependencies) differs between the
# fresh resolution and the tracked lock. Cargo.lock records no path for path deps, so the copy's
# lock is valid in place.
pkgs() { awk 'BEGIN { RS = "" } /^\[\[package\]\]/ { gsub(/\n/, " "); print }' "$1" | LC_ALL=C sort; }
drift=$(LC_ALL=C comm -3 <(pkgs "$SRC/Cargo.lock") <(pkgs "$CRATE/Cargo.lock") | grep -oE 'name = "[^"]+"' | sort -u | wc -l)
note=""
if [ "$UPDATE" = 1 ]; then
  cmp -s "$SRC/Cargo.lock" "$CRATE/Cargo.lock" || cp "$SRC/Cargo.lock" "$CRATE/Cargo.lock" || fail "copy of the lock"
  say "lock refreshed ($drift package entries)"
elif [ "$drift" -gt 0 ]; then
  note=" (Cargo.lock drift: $drift entries; rerun with --update-lock)"
fi
say "check: ok - build + $passed tests passed against engram-core GMAP_FORMAT_VERSION=${VER:-?}$note"
