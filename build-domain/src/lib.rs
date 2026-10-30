//! glia-build-domain: the build-target domain (0.5.4, owners FA.16, FA.15).
//!
//! A second domain beside code: the node kinds, edge categories and domain
//! tables of a build-target graph, stored as `targets.gmap`. It knows nothing
//! of code and depends on no code crate (core, activation and store only). It
//! is a stamp HASHED_ROOTS entry, so a change here moves PARSER_STAMP.
//!
//! Reserved for 0.5.5 (track T9); empty in 0.5.4.
