# Shared parse cache (`glia cache`)

A build keeps the parse of every source file in `<repo>/.glia/graph/parse_cache.bin`
and reparses only the files whose content changed. A fresh checkout (a new clone,
a CI runner, a second worktree) starts with no sidecar and parses everything.
The shared cache lets such a checkout fetch the parses another machine already
made for the same bytes.

```text
glia cache push <REPO> <STORE> [--unsigned] [--key-file <FILE>] [--layout] [--json]
glia cache pull <REPO> <STORE> [--unsigned] [--key-file <FILE>] [--verify <N|all>] [--jobs <N>] [--layout] [--json]
glia cache gc <STORE> [--keep-stamps <N>] [--max-bytes <BYTES>] [--json]
```

`--layout` also moves the whole finished layout of a clean checkout (see
[Whole-layout objects](#whole-layout-objects)), so a fresh clone of a commit CI
already built skips the build entirely, not just the parse.

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

Only `.gpc` objects count toward the byte totals. Whole-layout parts
(`layout/*.gla`) go with their stamp and are outside `--max-bytes`.

## Whole-layout objects

Sharing parses saves only the parse phase: the walk, the per-language graph
builds, the 15 resolvers and the post-passes still run. A clean checkout at a
commit another machine already built can skip all of them. `push --layout`
uploads the finished `.glia/graph/` layout, and `pull --layout` installs it
before the per-file pull runs (so a later edit still starts warm).

```sh
# CI, after building the commit
glia build "$REPO" && glia cache push "$REPO" /mnt/team/glia-cache --layout

# a fresh clone of the same commit
glia cache pull ~/work/shop /mnt/team/glia-cache --layout
# [cache] layout pull repo=shop key=<12 hex> tree=<12 hex> result=hit
```

**Clean work trees only.** A layout is keyed, pushed and installed only when
`git status` (tracked and untracked files, `.glia` excluded) prints nothing, so
the tracked files are exactly `HEAD^{tree}`. Otherwise the result is `dirty` and
the step is skipped; the per-file push or pull still runs.

**What the key covers.** A blake3 `derive_key("glia layout object v1")` over:

- the build stamp;
- the repo identity key (two clones of one remote agree);
- the `HEAD` tree id;
- the `.glia` input fingerprint (the map the manifest records: overlay, docs,
  history and test snapshots, by content);
- the target: CPU architecture, pointer width, endianness and path separator
  (shards are rkyv archives, reused only on the same target);
- `overlay=on` (the default layout dir always holds the overlay-applied graph);
- a digest of what the build sees beyond the tracked tree: the REGION nodes of
  every gated directory on disk (a gitignored `target/`, an empty `dist/`, a
  checked-out submodule), the project roots, and every `compile_commands.json`
  in the repo root, a project root or a child directory of either (the C/C++
  build reads it from gitignored build dirs). A source or doc file the walk
  reads but git does not track (hidden from `git status` by `.git/info/exclude`
  or a global excludes file) makes the checkout dirty.

Not covered: other reads of gitignored files outside the walk (a tsconfig
`extends` resolved into `node_modules`). Submodule content is not walked.

**What travels.** `manifest.json` and every file it names (the shards,
`cross_stack.gmap`, foreign shards), each checked against its manifest hash
when exported. The parse cache travels per file, the timeline sidecar names
commits rather than a tree and stays home, and `.gitignore` is written by the
install. `push` refuses (`result=stale`) a layout that is missing, older than
the checkout (`is_gmap_stale`), mid-write, or not this repo's alone (a
multi-repo or merged layout).

**Install.** The engine re-derives the key from the checkout and refuses
another one (`key mismatch`), accepts only plain `manifest.json` / `*.gmap`
names the manifest names (no path can leave the layout dir), and rewrites the
manifest's `repos` to what a local build writes: this checkout's label (its
directory name, which differs between machines), its root relative to the
layout dir, and its `HEAD` commit. The files are staged in
`.glia/graph.pull.<pid>.tmp/` and moved in shards first and `manifest.json`
last; every file they replace or orphan is kept aside there, so the disk holds
two layouts until the check. The check is a full load with rebuilding off
(`load_or_rebuild(dir, repo, false)`): build stamp, `.glia` inputs, source
mtimes, every shard's hash, every CODE span. If it fails, the new files are
removed, the old ones restored byte for byte, and the result is `rejected`.

**Object.** The layout is one body split into parts, each an ordinary signed
cache object, so a layout larger than the 64 MiB object bound still moves:

```text
body   = b"GLIALY01" | entry_count u32 LE | per entry: name_len u16 LE | name | data_len u64 LE | data
part i = part_count u32 LE | body_len u64 LE | the i-th 48 MiB of the body,
         stored as the object of key blake3 derive_key("glia layout object part v1", key | i u32 LE)
store  = <store>/v1/<stamp>/layout/<key>.gla (part 0), <key>.<i>.gla (i >= 1)
```

A part's key binds it to its layout and index, so the MAC refuses a part
served for another layout or index; every part repeats the part count and body
length, so a dropped or truncated part is refused; a push writes part 0 last.
Caps are checked before any length is used: at most 4096 files, 128-byte
names, 512 MiB per file and 1 GiB in all. Without a key, `--layout` needs
`--unsigned` like any pull, and then the store is trusted as-is: there is no
re-parse sample for a layout, only the install's checks.

Markers (stderr), before the per-file line:

```text
[cache] layout push repo=<label> key=<12 hex|-> tree=<12 hex|-> result=<pushed|present|stale|dirty>
[cache] layout pull repo=<label> key=<12 hex|-> tree=<12 hex|-> result=<hit|miss|rejected|dirty>
[cache] rejected v1/<stamp>/layout/<key>.gla: <reason>
```

`--json` adds a `layout` object (`result`, `key`, `tree`, `reason`, `files`,
`bytes`, `parts`). A layout that is `stale`, `dirty`, `miss` or `rejected` is not
a failure: the exit code is the per-file step's. A store error, or an install
that could not put the previous layout back, exits 1. The two steps are
independent: a layout installed before a per-file pull fails stays installed
(it was verified on its own).

## Not included

There is no HTTP(S) store yet. A `http://` / `https://` STORE exits 2 with
`unsupported store <s>` and makes no network request. It will be added as a
separate step (CE.2e), behind the same store interface and the same object
format.
