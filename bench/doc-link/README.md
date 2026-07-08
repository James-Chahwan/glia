# bench/doc-link — Tier-4 doc-ingestion fixture + smoke test

A self-contained, offline proof that the **Confluence → graph** ingest chain
works end to end (see `dev-notes/tier4-doc-ingestion-plan.md`). No network — it
exercises the *deterministic* half of `glia docs sync`, which is exactly the
half the byte-identical build gate cares about.

## Pieces

- `fixture-repo/` — a tiny Python "code repo" (`ordering.py`, `users.py`) whose
  symbols the docs reference: `OrderService`, `PaymentGateway`, `charge`,
  `place_order`, `get_user`.
- `pages/*.xhtml` — two pages in Confluence **storage format** (the XHTML the
  REST API returns), with code marked up the way Confluence does it: inline
  `<code>` spans, an `<ac:structured-macro ac:name="code">` block, and an
  `<a href>` source link.
- `pages/pages.jsonl` — one line per page: space key, title, url, version,
  body file. This is the shape the future REST fetch produces.

## Run

```sh
# 1. sync: storage-format pages → deterministic snapshot the engine ingests
cargo run -p repo-graph-doc-sources --bin docsync -- \
    bench/doc-link/fixture-repo bench/doc-link/pages/pages.jsonl
# → writes bench/doc-link/fixture-repo/.glia/docs-snapshot/manifest.jsonl (gitignored)

# 2. ingest + link: build the graph (SnapshotDocSource reads the snapshot,
#    link_doc_sections wires doc→code edges)
cargo run -p glia-cli -- analyze bench/doc-link/fixture-repo --format json
```

## Expected (asserted by `./check.sh`)

- **1 DOC_SPACE** — `docspace::confluence::DEV`
- **5 DOC_SECTION** — chunked by heading across the two pages, each `CONTAINS`-ed
  by the space
- **5 DOCUMENTS** cross-edges — every code-formatted mention resolves to the real
  symbol, and nothing else does (precision-first, the Tier-4 linker policy):
  - `OrderService`, `PaymentGateway` (CLASS)
  - `PaymentGateway.charge` → `charge`, `OrderService.place_order` → `place_order` (METHOD)
  - `get_user` (FUNCTION)

## What is NOT covered here

The live REST fetch (`/wiki/rest/api/content` with the `CONFLUENCE_TOKEN` env
token) bolts on top of `docsync` once the Confluence account has product access
— it only has to emit the same `pages.jsonl` + body files. Keeping fetch out of
this fixture is deliberate: the network step is non-deterministic and must stay
outside the build's byte-identical path.
