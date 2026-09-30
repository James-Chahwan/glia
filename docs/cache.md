# Shared parse cache (`glia cache`)

A build keeps the parse of every source file in `<repo>/.glia/graph/parse_cache.bin`
and reparses only the files whose content changed. A fresh checkout (a new clone,
a CI runner, a second worktree) starts with no sidecar and parses everything.
The shared cache lets such a checkout fetch the parses another machine already
made for the same bytes.

```text
glia cache push <REPO> <STORE> [--unsigned] [--key-file <FILE>] [--json]
glia cache pull <REPO> <STORE> [--unsigned] [--key-file <FILE>] [--verify <N|all>] [--jobs <N>] [--json]
glia cache gc <STORE> [--keep-stamps <N>] [--max-bytes <BYTES>] [--json]
```

The build stays offline. `glia build` never reads a store. `pull` is a separate step:
it writes the sidecar, and the next build reuses it through the same checks it
applies to its own cache (build stamp, repo identity, go.mod module set, content
hash, MODULE form). The engine and the glia-py wheel contain no transport. They
only produce keys and payloads and run the verified import
(`glia_engine::shared_cache`). Moving and authenticating bytes is done by the
`glia` binary alone.

## What a key covers

Each cached parse is stored under a 32-byte blake3 content address
(`shared_cache::file_key`). The address hashes every input the parse depends on,
and nothing else:

- the build stamp (`<release>+p<parser stamp>`), which moves with any parser or
  extractor change;
- the repo identity key, which is part of every node id in the parse;
- the language tag;
- the repo-relative path as the walk spells it;
- the MODULE qname the build's plan gives the file (a same-stem sibling names it
  by file name);
- the go.mod module-set key, for Go files only;
- the blake3 of the file's content.

Two checkouts share an object only when all of these match. A checkout with
different content, another identity or another release asks for different keys.
The key is an address, not a secret and not a signature.

## Store layout

```text
<store>/v1/<stamp>/LAST                  mtime = the stamp's last upload (gc's recency)
<store>/v1/<stamp>/<aa>/<key>.gpc        one object per content address; <aa> = the key's first two hex chars
<store>/v1/<stamp>/<aa>/<key>.gpc.<pid>.tmp   a push's staging file, renamed into place
```

Each stamp gets its own directory, so gc can drop a whole release at once. An
object is:

```text
b"GLIAPC01" | flags u8 (bit 0 = signed) | key [32] | payload_len u64 LE | payload | mac [32] (signed only)
```

The payload is the sidecar entry's own bytes: the file's content hash, its
language and its parse, in canonical order. The MAC is
`blake3::keyed_hash(signing key, every byte before the MAC)`.

## Trust modes

A store holds objects written by other machines, so `pull` trusts nothing it can
check.

**Keyed store (the default).** Every object carries a MAC under a 32-byte key
shared by the machines allowed to push. The key comes from `--key-file <FILE>`
(64 hex characters, surrounding whitespace ignored) or `GLIA_CACHE_KEY`. A pull
refuses an object that:

- is larger than 64 MiB (it is read at most one byte past the limit);
- is malformed (bad magic, unknown flags, or a length that does not match the
  object size; every length is checked before it is used);
- names a key other than the one requested (a store cannot answer one key with
  another key's object);
- has no MAC, or a MAC that does not verify (a constant-time compare).

This defends against a store that anyone else can write to: a forged, altered,
swapped or replayed-under-another-key object is refused before import. The
engine's import then checks every payload against the checkout (decodes it
within bounds; checks content hash, language and MODULE form) and writes only
what passes. With a key the default re-parse sample is 0, because the MAC
already authenticates the writer. `--verify N|all` adds a sample anyway. A key
holder can still push a wrong parse. The key is the trust boundary.

**Unsigned store (`--unsigned`).** Objects are written without a MAC and read
without checking one. Use this only for a store no one else can write to (a
directory on your own machine). The import's checks still run, and a pull
re-parses a random sample of the accepted entries with the build's own parse
code (default 32, `--verify N|all`) and compares the bytes. One mismatch exits 1,
and nothing is written. A sample catches bulk poisoning with high probability.
It catches a single targeted entry only with probability sample / files, so
`--verify all` is the full check.

Without a key, `push` and `pull` need `--unsigned` explicitly and otherwise exit 2
with `no cache key: set GLIA_CACHE_KEY or pass --key-file (or --unsigned to
trust the store as-is)`. When a key is set, it takes precedence over `--unsigned`:
objects are signed and checked, and one note line says so.

Key hygiene: the key is never printed, and no error message contains it. On
unix, a key file that other users can read triggers a warning (`chmod 600` it).
Generate a key with, for example, `openssl rand -hex 32 > ~/.config/glia/cache.key`.

## Directory-store recipe

A directory store works on a local disk or on any shared filesystem mount
(NFS, SMB). A push stages each object as `<file>.<pid>.tmp` and renames it into
place, so a reader never sees a partly written object. Two pushers writing the
same key write identical bytes, because the payload is canonical.

```sh
export GLIA_CACHE_KEY=$(cat ~/.config/glia/cache.key)

# on a machine that has built the repo
glia build ~/src/shop
glia cache push ~/src/shop /mnt/team/glia-cache

# on a fresh checkout of the same commit
glia cache pull ~/work/shop /mnt/team/glia-cache --jobs 16
glia build ~/work/shop        # [incremental] shop: reused N, reparsed 0, evicted 0
```

`push` uploads only the entries that a build of the checkout as it is now would
reuse. Stale entries are counted (`stale=`) and skipped, and objects already in
the store are counted as `present=`. `pull` fetches only the files the
checkout's cache lacks (`local_hits=` counts the rest). It runs `--jobs` fetches
in parallel (default 8), and the result does not depend on how they are
scheduled.

Exit codes: 0 ok; 1 a store or verification failure (a failed pull writes
nothing); 2 a usage error (a bad store, no key or a bad key, a repo that is not
a directory). `--json` prints the summary as one JSON object.

Markers (stderr), after the engine's `[cache] export` / `[cache] import` lines:

```text
[cache] push store=<dir> repo=<label> entries=<n> uploaded=<u> present=<p> stale=<s> signed=<yes|no>
[cache] pull store=<dir> repo=<label> files=<n> local_hits=<h> fetched=<f> missing=<m> rejected=<r> verified=<v> signed=<yes|no>
[cache] rejected <v1/...>: <reason>          (first 20 per pull)
[cache] gc store=<dir> stamps_kept=<k> stamps_removed=<r> objects_removed=<o> bytes=<before>-><after>
```

In the pull marker, `rejected` covers both transport rejections (size, format,
key, MAC) and the import's own rejections.

## GC

`glia cache gc <STORE>` prunes a directory store:

1. It orders the stamps newest first by the mtime of their `LAST` file (a push
   touches it on every upload; a stamp without `LAST` uses the directory's
   mtime; ties are broken by name). It keeps the newest `--keep-stamps`
   (default 2: this release and the previous one) and removes the rest whole.
   A release never reads another stamp's objects.
2. With `--max-bytes <BYTES>` (a count, or with a `K` / `M` / `G` suffix in
   powers of 1024), it deletes the kept objects oldest first (by mtime, then by
   path) until the total is at most the budget.
3. It removes staging files (`*.tmp`) older than an hour, left by a crashed
   push. A newer one belongs to a push still in progress and is left alone.

Only `.gpc` objects count toward the byte totals.

## Not included

There is no HTTP(S) store yet. A `http://` / `https://` STORE exits 2 with
`unsupported store <s>` and makes no network request. It will be added as a
separate step (CE.2e), behind the same store interface and the same object
format.
