//! IdentityResolver (DD.5): the last Resolve pass. SAME_AS stars for one package across
//! repos, a package and the project that publishes it, proto / Avro / JSON Schema messages
//! (with a diverged qualifier), contract operations (the op key shared with CC.8) and SCIP
//! symbols (DD.7); vendored copies as HEURISTIC candidates, never collapsed (DD.6). Filled
//! by DD.5.
