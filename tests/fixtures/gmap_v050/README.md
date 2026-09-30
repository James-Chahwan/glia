# gmap_v050 - a 0.5.0 on-disk layout (CD.7b)

**Do not regenerate.** These bytes are the 0.5.0 format (`FORMAT_VERSION` 2)
by definition. Once CD.7b bumps `FORMAT_VERSION` to 3, nothing in this tree can
write them again. If a test that reads them fails, fix the reader, not the
fixture.

They are what a 0.5.0 user's disk holds: the one layout `glia build`, the
install-hooks hooks and pyo3 all write at `<repo>/.glia/graph/` (LC.9),
written by the released v0.5.0 code. CD.7b proves that every file here reports
`OldFormat { found: Some(2) }` to a format-3 reader, and that LC.8's
`load_or_rebuild` rebuilds the layout once from its recorded root.

## Pinned identity

| field | value |
|---|---|
| capture commit | `2170ff8` = tag `v0.5.0` (`git log -1 v0.5.0`) |
| `BUILD_STAMP` | `0.5.0+p5fa8bd06e59848d5` (`glia --version` printed `glia 0.5.0 (build 0.5.0+p5fa8bd06e59848d5)` before the capture) |
| `FORMAT_VERSION` | `2` (`store/src/container.rs`) |
| `MANIFEST_VERSION` | `2` (`store/src/layout.rs`) |
| manifest `engine_version` | `0.5.0` |
| captured | 2026-10-01 |

## Layout

| path here | on a user's disk | written by |
|---|---|---|
| `../gmap_pre_leap/repo/` | the repo | hand-authored sources (Go chi route + SQL, TS `fetch`, one Python function), shared with LG.6b's pre-leap capture |
| `layout/` | `<repo>/.glia/graph/` | `glia build <repo>` at v0.5.0 (`persist_result`, writer `cli`) |

Not captured: `parse_cache.bin` (a sidecar the layout does not need; its own
stamp already makes a later build discard it) and the layout's self-ignoring
`.gitignore` (its `*` would hide this fixture from git).

To use the fixture, materialise it into a tempdir `D` first: copy
`../gmap_pre_leap/repo/*` to `D/` and `layout/*` to `D/.glia/graph/`. The
manifest records the repo root as `../..`, so the materialised layout resolves
its root to `D`, and `load_or_rebuild(<D>/.glia/graph, None, true)` rebuilds
it with no repo path.

Rules for readers:

- **Shard names are the RepoId of the directory name.** The capture directory
  was named `repo` and lies outside git, so its RepoId is keyed `dir:repo`
  (`[repo-id] source=dir key=dir:repo`), id `12576015503297116104`. Read
  shard names from `manifest.json`; never hard-code them.
- **Staleness keys on the manifest.** A copy gives every file a fresh mtime.
  The manifest's `schema_version` 2 still matches a 0.5.1 reader, so the first
  thing a 0.5.1 load finds wrong is the `build_stamp`
  (`written by another glia build (0.5.0+p5fa8bd06e59848d5)`); a direct shard
  open reports `old format v2 (this build reads v3)`.

What the build printed: `[evidence] edges=9 missing=0 ...
emitters=extractor:data_entities=1,extractor:data_sources=1,graph:calls=1,parser:go=2,parser:python=1,parser:typescript=2,resolver:http=1`,
`[edge-cells] intra=8 cross=1 with_cells=9` and
`[gmap] layout .../repo/.glia/graph: shards=4 format=2 sections=code:3`.

| shard suffix | language | content |
|---|---|---|
| `-00` | Go | `api/main.go`: chi `ROUTE /users`, `listUsers`, `db.Query` data source and its data entity; 5 EVIDENCE JSON payloads |
| `-01` | Python | `api/worker.py`: `refresh_users`; 1 EVIDENCE JSON payload |
| `-02` | TypeScript | `web/api.ts`: `loadUsers`, `ENDPOINT` for `fetch('/users')`; 2 EVIDENCE JSON payloads |
| `cross_stack` | - | the one HTTP edge pairing the TS endpoint with the Go route; 1 `resolver:http` EVIDENCE JSON payload |

## Exact capture commands

`C=/home/ivy/.cache/glia-CD.7b`. `GLIA_NO_PERSIST` was unset throughout. The
tag's tree came from `git archive` (the build stamp is a content hash, so it
equals a worktree of the tag's).

```sh
mkdir -p $C/src050 && git archive v0.5.0 | tar -x -C $C/src050
cd $C/src050
CARGO_TARGET_DIR=$C/target050 CARGO_PROFILE_DEV_DEBUG=0 cargo build --locked -p glia-cli
$C/target050/debug/glia --version     # glia 0.5.0 (build 0.5.0+p5fa8bd06e59848d5)

mkdir -p $C/capture
cp -r <glia>/tests/fixtures/gmap_pre_leap/repo $C/capture/repo
env -u GLIA_NO_PERSIST $C/target050/debug/glia build $C/capture/repo
cp -p $C/capture/repo/.glia/graph/{manifest.json,cross_stack.gmap,repo-*.gmap} layout/
```

## Captured files

`store/tests/gmap_v050.rs::v050_fixture_is_intact` parses this table. It
requires the files under `layout/` to equal the table rows exactly, each byte
count and xxhash64 (seed 0, the store's own `content_hash`) to match, every
`.gmap` to open with the preamble `GLIAGMAP` + format `2` and contain the
archived header `GMAP 02 00 00 00`, `manifest.json`'s `schema_version == 2`
and `build_stamp == 0.5.0+p5fa8bd06e59848d5`, and each manifest `content_hash`
to match its file. The `sha256` column is for `sha256sum` by hand.

| file | bytes | xxhash64 | sha256 |
|---|---|---|---|
| `layout/cross_stack.gmap` | 2384 | `bebb6db618d0aeea` | `19c3fc1b93b16f2c1ed90b32f70893e0c11975ee896a5507ed7d3d7f79048a40` |
| `layout/manifest.json` | 794 | `54b53ed73849cc78` | `a11721777e785fd715944d37b2993c862ce14918435c464edaced0e05cf16a1c` |
| `layout/repo-12576015503297116104-00.gmap` | 5568 | `ca7e4fbc9cf726a7` | `480b5ecbbafd7274b3bed17fbaeb0b076c5b5872dd723ddf1aeb567a36cf0537` |
| `layout/repo-12576015503297116104-01.gmap` | 3168 | `6837f6aadd5bcfac` | `cd904a3dc6520c78b127f0f627a261e0fd240604d3c698386452723ff81eba84` |
| `layout/repo-12576015503297116104-02.gmap` | 3632 | `c019cce501e1cece` | `07d39d3c8630ddf02216f03c8db3c577452414df0b8b9e7ebaeec858317e0da1` |
