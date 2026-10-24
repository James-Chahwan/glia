//! glia-embed: query-time text embeddings (0.5.4, owner FB.1).
//!
//! The crate turns text into vectors for the semantic tiers: fastembed's
//! bge-small behind the `fastembed` feature, and a deterministic `test-hash`
//! model without it, which every test uses. It is never a build input: no
//! build pass calls it, it is not a stamp HASHED_ROOTS entry, and an embedding
//! change never moves PARSER_STAMP. Vectors are written only by the `glia embed`
//! snapshot step, CPU only, never concurrently with a build's rayon pool.
