//! glia-toy-domain — a test-only second domain, `toy-reel`, proving the
//! domain seam end to end without shipping a domain (0.5.0 is "prep for cross
//! domain"; dev-notes/next-leap-0.5.0.md section 3).
//!
//! The toy is deliberately unlike code: a reel of scenes, shots and objects
//! whose nodes have no names (they are addressed by kind and index), with its
//! own id registries that reuse code's numbers for other meanings. It goes
//! through every layer a real domain would, using only the domain-free crates:
//! - [`registry`] — its node kinds, edge categories and cell types;
//! - [`reel`] — the input transformer [`build`], the graph [`ToyGraph`], its
//!   container section [`ReelNav`], and the `.gmap` round trip through the
//!   store's domain-free core, header registries and named sections;
//! - [`profile`] — [`TOY_TABLES`], the build passes [`TOY_PASSES`] run by
//!   activation's `PassRegistry`, [`TOY_PROFILE`], and the activation hooks
//!   [`KindIs`] (a filter) and [`ScreenTimeSummary`] (a synth hook).
//!
//! `tests/end_to_end.rs` builds the fixture reel, runs the passes, checks
//! reachability, writes and reads the file, and activates it.
//!
//! `publish = false`, and no other crate may depend on it (the end-to-end test
//! guards both directions of that).

pub mod profile;
pub mod reel;
pub mod registry;

pub use profile::{KindIs, ScreenTimeSummary, TOY_PASSES, TOY_PROFILE, TOY_TABLES};
pub use reel::{
    GRAPH_TYPE, NAV_SECTION, REEL_REPO, ReadBack, ReelNav, ToyGraph, build, node_id, read_gmap,
};
pub use registry::{cell_type, edge_category, node_kind};
