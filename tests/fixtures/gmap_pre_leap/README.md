# gmap_pre_leap — pre-0.5.0 on-disk artefacts (LG.6b)

**Do not regenerate.** These bytes are the pre-leap format by definition. Once
LC.1 bumps `FORMAT_VERSION` / `MANIFEST_VERSION`, nothing in this tree can write
them again. If a test that reads them fails, fix the reader, not the fixture.

They are what a user's disk holds today, written by released 0.4.18 code:

- the pyo3/MCP **sharded layout** at `<repo>/.ai/repo-graph/`
  (`manifest.json`, `repo-<id>-NN.gmap` shards, `cross_stack.gmap`, the
  `parse_cache.bin` incremental sidecar), and
- the **flat `glia build` layout** at `<repo>/.glia/*.gmap`.

LC.1 (version bump reports "rebuild"), LC.8 (`load_from_gmap` self-heals) and
LC.9 (directory move) test against these files. They do not capture their own.

## Pinned identity

| field | value |
|---|---|
| capture commit | `fe23c8d` (graph-shaping sources identical to `7a4883b`: `git diff fe23c8d 7a4883b -- core code-domain graph engine store parsers stamp Cargo.lock` is empty) |
| `BUILD_STAMP` | `0.4.18+p3d23e8828e7ba01a` (both the CLI and the wheel, asserted before capture) |
| `FORMAT_VERSION` | `1` (`store/src/lib.rs`) |
| `MANIFEST_VERSION` | `1` (`store/src/lib.rs`) |
| manifest `engine_version` | `0.4.18` |
| captured | 2026-09-19 |

The capture ran from a `git worktree` of `fe23c8d`, not from HEAD. At the time,
the HEAD CLI printed `0.4.18+peacf0963ef6e1503` and the shared installed wheel
printed `0.4.18+pc9d7200aa8178117`, because wave-0 and wave-1 packets had
already moved the stamped roots. The shared wheel was not touched.

## Layout

| path here | on a user's disk | written by |
|---|---|---|
| `repo/` | the repo | hand-authored sources (Go chi route + SQL, TS `fetch`, one Python function) |
| `layout-ai-repo-graph/` | `<repo>/.ai/repo-graph/` | pyo3 `generate(repo, True)` auto-persist (`write_merged_sharded`) + `ParseCache::save` |
| `layout-glia-build/` | `<repo>/.glia/` | `glia build <repo>` (`write_repo_graph` per language graph) |

The sharded layout is stored under another name because `.gitignore`
(`**/.ai/repo-graph/`) swallows that directory at any depth. To use the fixture,
materialise it into a tempdir `D` first. Copy `repo/*` to `D/`,
`layout-ai-repo-graph/*` to `D/.ai/repo-graph/` and `layout-glia-build/*` to
`D/.glia/`. This is the same trick grade.py's `materialize` uses for `.git`.

Rules for readers:

- **Shard names embed the capture path.** RepoId `4408344475944711014` is
  `RepoId::from_canonical("file://<T>")` for the capture path `T` below. Always
  discover shard file names from `manifest.json` (or by listing the flat
  directory). Never hard-code them.
- **The parse cache is relocated on materialisation.** `parse_cache.bin`
  starts with the bincode string `0.4.18+p3d23e8828e7ba01a` (its stamp),
  followed by `repo_canonical` = `file://<T>`. A materialised copy therefore
  mismatches on both the stamp and the repo identity, and LC.8's rebuild path
  must shrug it off.
- **mtimes are not part of the fixture.** A git checkout or copy gives every
  file a fresh mtime. Staleness tests must key on the manifest's
  `schema_version` / `build_stamp` / format version, never on mtime order.

Shards (all three flat-layout files are byte-identical to their sharded twins,
because both writers serialise the same `Container`):

| shard suffix | language | content |
|---|---|---|
| `-00` | Go | `api/main.go`: chi `ROUTE /users`, `listUsers`, `db.Query` data source and `data_entity:sql:users` |
| `-01` | Python | `api/worker.py`: `refresh_users` |
| `-02` | TypeScript | `web/api.ts`: `loadUsers`, `ENDPOINT` for `fetch('/users')` |
| `cross_stack` | - | the one HTTP edge pairing the TS endpoint with the Go route (`[http] ... paired=1 (exact=1 ...)`) |

## Exact capture commands

`S` = the session scratchpad and `T=$S/lg6b/capture/repo`, i.e.
`T=/tmp/claude-1000/-home-ivy-Code-glia/51fd4a3a-5bd1-4db6-8442-972763906d3f/scratchpad/lg6b/capture/repo`.
`GLIA_NO_PERSIST` was unset throughout.

```sh
git worktree add --detach $S/pre-leap fe23c8d
cd $S/pre-leap
CARGO_TARGET_DIR=$S/pre-leap-target CARGO_PROFILE_DEV_DEBUG=0 cargo build --locked -p glia-cli
# -i: an existing scratch venv's CPython 3.14.7 (/usr/bin/python3.14). The wheel is
# abi3, so the interpreter only picks the tag.
CARGO_TARGET_DIR=$S/pre-leap-target CARGO_PROFILE_DEV_DEBUG=0 \
  maturin build -m py/Cargo.toml --locked -i $S/venv/bin/python -o $S/wheels
mkdir -p $S/lg6b && mv $S/wheels $S/lg6b/wheels
python3 -m venv $S/lg6b/venv
$S/lg6b/venv/bin/pip install --no-index --no-deps \
  $S/lg6b/wheels/repo_graph_py-0.4.18-cp311-abi3-manylinux_2_34_x86_64.whl

# identity gate: both printed 0.4.18+p3d23e8828e7ba01a
$S/pre-leap-target/debug/glia --version
$S/lg6b/venv/bin/python -c 'import repo_graph_py as r; print(r.build_stamp())'

# (a) sharded MCP layout + parse cache
cp -r <glia>/tests/fixtures/gmap_pre_leap/repo $T
env -u GLIA_NO_PERSIST $S/lg6b/venv/bin/python -c \
  "import repo_graph_py as r; assert r.build_stamp() == '0.4.18+p3d23e8828e7ba01a'; r.generate('$T', True)"
cp -p $T/.ai/repo-graph/* layout-ai-repo-graph/

# (b) flat glia build layout (same T, so the RepoId matches (a))
env -u GLIA_NO_PERSIST $S/pre-leap-target/debug/glia build $T
cp -p $T/.glia/* layout-glia-build/

git worktree remove $S/pre-leap
```

Step (b) reused the parse cache from (a) (`reused 3, reparsed 0`) and then
re-saved `$T/.ai/repo-graph/parse_cache.bin` with different bytes. The
committed `parse_cache.bin` is the one pyo3 `generate` wrote in (a), copied
before (b) ran.

## Captured files

`store/tests/gmap_pre_leap.rs::pre_leap_fixture_is_intact` parses this table.
It requires the files on disk to equal the table rows exactly, each byte count
and xxhash64 (seed 0, the store's own `content_hash`) to match, every `.gmap`
to contain the archived header `GMAP 01 00 00 00`, `manifest.json`'s
`schema_version == 1` and `build_stamp == 0.4.18+p3d23e8828e7ba01a`, and each
manifest `content_hash` to match its file. The header is not at offset 0: rkyv
puts the root near the end, so the file opens with cell data. The `sha256`
column is for `sha256sum` by hand.

| file | bytes | xxhash64 | sha256 |
|---|---|---|---|
| `layout-ai-repo-graph/cross_stack.gmap` | 176 | `0cf3b01c84994afa` | `573ba6beb5b2965483b8307983898621fb677588bece90cc2e9a88ffe9f717de` |
| `layout-ai-repo-graph/manifest.json` | 681 | `0b190cd57406f507` | `4256252ac9eddf8f057b0590f32278323978d75832115aec47208c9f9218562b` |
| `layout-ai-repo-graph/parse_cache.bin` | 4672 | `78dc6437ada31023` | `7768f2969d01476e7ec1f4173d8e2f962cbf639304c0dc08c38c235e2deecf36` |
| `layout-ai-repo-graph/repo-4408344475944711014-00.gmap` | 2728 | `c83412944888110d` | `0467dc058ff8fd4f487f505325c0f7dba7b34d70e8538e3f76eb6b6da0c3101c` |
| `layout-ai-repo-graph/repo-4408344475944711014-01.gmap` | 864 | `446838830b70eda0` | `4c3a4e6cb6c43a51e3f27f7c8d6caf95d1f16e72e974af51376102a8c09f9211` |
| `layout-ai-repo-graph/repo-4408344475944711014-02.gmap` | 1216 | `7172c9f3a503185e` | `dca510387c893d4bd1ee7d6f7c4bfead0f167a86b9d088cf11ca35b023af223b` |
| `layout-glia-build/repo-4408344475944711014-00.gmap` | 2728 | `c83412944888110d` | `0467dc058ff8fd4f487f505325c0f7dba7b34d70e8538e3f76eb6b6da0c3101c` |
| `layout-glia-build/repo-4408344475944711014-01.gmap` | 864 | `446838830b70eda0` | `4c3a4e6cb6c43a51e3f27f7c8d6caf95d1f16e72e974af51376102a8c09f9211` |
| `layout-glia-build/repo-4408344475944711014-02.gmap` | 1216 | `7172c9f3a503185e` | `dca510387c893d4bd1ee7d6f7c4bfead0f167a86b9d088cf11ca35b023af223b` |
