# glia 0.5.3 - the ask to Engram (sent 2026-10-16)

**From:** the glia session, 2026-10-16. **To:** an Engram session, once James sends it.
**Status:** sent 2026-10-16 to the Engram session (engram-34) as a briefing, with an evaluation ask for 0.5.2's DE packets at the wave-5 landed commit. James: *"yeah nice work i think we got engram choice nailed down ... maybe tell the engram session now whats comming and if it works get it to evaluate the changes ?"*. Earlier: James, 2026-10-16: *"A first and get the hand off to engram ready we may need to decide what
happens to engram soon"*, then, on embeddings: *"well shouldn't we bring parts of engram in if we need the embeddings
?"* / *"...isn't it locally run in engram i swear"*. glia now embeds itself; section 4 lists what depends on Engram either way.

Scope source: `dev-notes/next-leap-0.5.3.md` §2 B (embeddings in glia; Engram, data only), §2 F.6 (concept search
over flows) and §3 (James's rulings). The boundary is unchanged: `engram-core` is the only shared crate, Engram
depends on no glia crate, and everything below crosses as **side files (data)**, never as a code dependency
(Engram's 2026-10-02 answer, question 4). glia uses fastembed directly; it copies no Engram code.

## 1. What glia 0.5.3 does itself

- **`glia embed`** (a snapshot step, never inside `glia build`): embeds each node's name, qname, leading_doc and code
  locally with fastembed-rs / bge-small - the library and model `engram-live`'s `local-embed` feature uses - and writes
  `.glia/vectors.jsonl`, cached by a hash of each node's text.
- **A semantic tier** in `find` and in the new flow search (`trace` / `flows` matching a word through what a flow
  touches), and `pack` as a fallback (seeds only when nothing matches by name). HEURISTIC, its own match tier with the
  similarity, never mixed into the lexical order. Blast-radius never seeds from vectors.
- The **VectorRow contract** and a validator, so the sidecar stays writer-agnostic.
- Rule for every learned or embedded row: it ranks, it never decides what is true.

## 2. The ask

1. **Corrections as cells** through `.glia/cells.jsonl` (`CellRow`, code-domain/src/external_inputs.rs; unknown
   fields are rejected): `{"qname": .., "kind": .., "hint": "<identity_hint>", "cell": "DECISION" | "CONV", "entry":
   {"source": "api", "id": "<stable id>", "text": ".."}}`. Only CONSTRAINT / DECISION / CONV / VECTOR are writable
   (`external_inputs::WRITABLE`). `hint` is glia's move-stable identity hint, so a correction follows a renamed symbol
   (0.5.2 DE re-key pairing). glia shows them as HEURISTIC.
2. **Optionally, read glia's vectors** instead of embedding code facts again: same model, same space. Row shape:
   `{"qname": "...", "kind": "METHOD", "hint": "...", "model": "<model id>", "dims": 384, "b64": "<base64 of dims x f32>"}`.
3. **Engram's own eval rule still governs Engram:** a new candidate source (glia's vectors, glia's flow search)
   enters its association pool as a weight-0 feature and stays only if it beats its ablation (Engram's 2026-10-02
   rule). glia's tier is judged by glia's gates and 0.5.4's agent before / after.

## 3. Contract points to settle with Engram

- **Model id and version** both sides name, so a vector written by one is read correctly by the other (`model` in
  every row; glia reports rows from more than one model).
- **Byte order** of the f32 payload: proposed little-endian, written into the contract and checked.
- **What text is embedded** for a code node. Engram's 2026-10-01 battery: full fact text beat name + qname, 78 vs 54
  of 100. glia proposes name + qname + leading_doc + code, capped; Engram's 500-byte `cap_prose` is a reference.
- **Normalisation:** glia writes unit-length vectors (cosine = dot product).
- **Correction ids and lifetime:** a stable `id` per correction (upserted by `(source, id)`), and what happens to a
  correction whose node is gone (glia reports it; who deletes it).

## 4. What depends on Engram (for James's decision on its direction)

- **Today:** the excluded `engram-export/` crate (path dependency on `../Engram`'s `engram-core`,
  `GMAP_FORMAT_VERSION` 6), built and tested by `scripts/check-engram-export.sh` in every wave close-out.
- **0.5.2's DE group (Engram anchors)** - cross-clone determinism and re-key pairing through `identity_hint` - is
  useful to glia on its own: deterministic builds across clones and stable identity for any consumer.
- **0.5.3's semantic search no longer depends on Engram:** glia embeds itself. What Engram alone brings is learning
  from use (the release gate over agent-session history, which glia has no data for) and consolidated corrections.
- **If Engram stopped,** the `engram-export` crate and its close-out gate could be retired; the DE work, `glia embed`
  and the sidecar contracts stay.

## 5. Engram's answer (engram-34, 2026-10-16)

Recorded in `next-leap-0.5.3.md` §2 B ("Engram's embedding learnings", "Corrections as cells: no source yet"):
- Contract: f32 little-endian agreed; `model` + `dims` required, naming the exact artifact (`Xenova/bge-small-en-v1.5`,
  fp32); readers skip rows of another model.
- Engram's own dense hits embed name, qname, leading doc and imports, never code, so reading glia's vectors (which
  add code) is a body change under Engram's rule; Engram keeps embedding its experience facts and every query.
- Corrections: Engram forms none today (Cell C failed its ceiling check, 2026-10-11); a cells.jsonl writer is James's
  call. Lifetime rule agreed: the writer owns its rows; glia keeps, reports and attaches nothing to an orphan; ids are
  content hashes.
- Evaluation: the baselines in the briefing were stale. Renamed is 9/9 on Kina and 21/25 on glia since Engram's X1
  (old addresses), so the re-key test is re-keys with X1 off and on. The headline is the current flat-fs body under
  Engram's per-stream rule over 10 streams; PPR enters as a registered needle test (`flat-ppr` vs `flat-fs`, Kina +
  glia first). Determinism runs first at the wave-5 commit; the replays wait for James's scope call.

James's ruling, 2026-10-16 ("sure" to the session's recommendations): determinism now at wave 5 (`3921a636`);
the Kina + glia replays once at the 0.5.2 release candidate after W14; the public streams only if those pass. No
Engram corrections writer in 0.5.3.
