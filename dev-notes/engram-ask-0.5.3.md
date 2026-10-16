# glia 0.5.3 - the ask to Engram (DRAFT, not sent)

**From:** the glia session, 2026-10-16. **To:** an Engram session, once James sends it.
**Status:** draft. James, 2026-10-16: *"A first and get the hand off to engram ready we may need to decide what
happens to engram soon"*. glia builds its half first (section 2) against fixtures and a written contract; this ask
goes out when James has decided Engram's direction (section 5 lists what depends on Engram either way).

Scope source: `dev-notes/next-leap-0.5.3.md` §2 B (Engram, data only), §2 F.6 (concept search over flows) and §3
(James's rulings on questions 2-4). The boundary is unchanged: `engram-core` is the only shared crate, Engram
depends on no glia crate, and everything below crosses as **side files (data)**, never as a code dependency
(Engram's 2026-10-02 answer, question 4).

## 1. What glia 0.5.3 will do with Engram's data

- **A semantic tier in `find`** and in the new flow search (`trace` / `flows` matching a word through what a flow
  touches): nodes whose VECTOR is near the query's vector, reported as their own HEURISTIC match tier with the
  similarity, never mixed into the lexical order.
- **`pack` fallback:** semantic matches become pack's seeds only when nothing matches by name (the rule pack's fuzzy
  seeds follow today). Blast-radius never seeds from vectors.
- **Corrections as cells:** Engram's consolidated corrections land as DECISION / CONV cells on the node they name.
- Rule for every row from Engram: learning ranks, it never decides what is true. A FACT / DERIVED row is never
  promoted or hidden by a learned score.

glia takes **no embedding model**. It ranks stored vectors; it never computes one.

## 2. What glia builds first, without Engram (James's option A)

- Fixture `.glia/vectors.jsonl` files in `bench/substrate-gap/fixtures/` and a deterministic, test-only embedding,
  so the semantic tier is gated in glia's own suite.
- The written contract below, and a check (`glia gaps` rows, plus a `[vectors]` marker) that reports rows whose qname
  matches no node, rows from more than one `model`, and rows whose payload length disagrees with `dims`.
- The query-vector path: a caller passes a query vector with its `model` name; when the model differs from the
  sidecar's, or no vector is given, the semantic tier is skipped and the answer's absence says so.

## 3. The ask

1. **Write `.glia/vectors.jsonl`** at the repo root (`external_inputs::VECTORS_FILE`), one row per node, the existing
   `VectorRow` (code-domain/src/external_inputs.rs; unknown fields are rejected):
   `{"qname": "...", "kind": "METHOD", "hint": "<identity_hint>", "model": "<model id>", "dims": 384, "b64": "<base64 of dims x f32>"}`.
   - `kind` disambiguates nodes sharing a qname; `hint` is glia's move-stable identity hint, so a renamed symbol's
     vector follows it (0.5.2 DE re-key pairing).
   - The payload is at most 65,536 bytes (`MAX_VECTOR_BYTES`).
2. **Supply query vectors.** For a text query, the caller (repo-graph's MCP, or Engram) embeds the query with the
   same model and passes the vector to glia. Which side does it is a cross-session call; Engram already embeds
   (fastembed, bge-small).
3. **Corrections as cells** through `.glia/cells.jsonl` (`CellRow`): `{"qname": .., "kind": .., "hint": ..,
   "cell": "DECISION" | "CONV", "entry": {"source": "api", "id": "<stable id>", "text": ".."}}`. Only CONSTRAINT /
   DECISION / CONV / VECTOR are writable (`external_inputs::WRITABLE`).
4. **Engram's own eval rule still governs Engram:** a new candidate source enters its association pool as a weight-0
   feature and stays only if it beats its ablation (Engram's 2026-10-02 rule). glia's tier is judged by glia's gates
   and 0.5.4's agent before / after.

## 4. Contract points to settle with Engram

- **Byte order** of the f32 payload: proposed little-endian, written into the contract and checked.
- **`model` and `dims` required** for rows the semantic tier uses (both are optional in `VectorRow` today); a row
  without them is reported, not used.
- **What text is embedded.** Engram's 2026-10-01 battery: full fact text beat name + qname, 78 vs 54 of 100, and
  glia's leading_doc capture was the load-bearing signal. The sidecar should say which (`model` could name it).
- **Staleness.** Vectors are written against one commit; after a change, rows whose qname or hint no longer
  resolves are reported by the check, and the writer refreshes them. Whether the row carries the commit it was
  written at is open.
- **Normalisation:** whether vectors arrive unit-length (cosine = dot product) or glia normalises on load.

## 5. What depends on Engram (for James's decision on its direction)

- **Today:** the excluded `engram-export/` crate (path dependency on `../Engram`'s `engram-core`,
  `GMAP_FORMAT_VERSION` 6), built and tested by `scripts/check-engram-export.sh` in every wave close-out.
- **0.5.2's DE group (Engram anchors)** - cross-clone determinism and re-key pairing through `identity_hint` - is
  useful to glia on its own: deterministic builds across clones and stable identity for any consumer.
- **0.5.3 §B is writer-agnostic.** glia reads a sidecar file against a written contract; any producer can write it
  (Engram, repo-graph with fastembed, or another tool). Nothing in glia depends on Engram's code.
- **If Engram stopped,** the `engram-export` crate and its close-out gate could be retired; the DE work and the §B
  sidecar contract stay.
