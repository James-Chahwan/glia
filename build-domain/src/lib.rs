//! glia-build-domain: the build-target domain (0.5.7, HT.4), the first
//! shipped domain beside code.
//!
//! A build-target graph holds what a build system says about a repo: its
//! targets (rules: libraries, binaries, tests, aliases), their packages, the
//! source files they compile and the labels outside the workspace they name,
//! joined by the attributes that declared each link (`deps`, `data`, `srcs`,
//! ...). It is read from a snapshot of `bazel query` output, never by running
//! the build system inside a glia build: the query needs the build system,
//! its toolchains and network, so a snapshot step takes it once and the graph
//! is rebuilt from that file alone, the way the history snapshot feeds
//! co-change.
//!
//! Nothing here knows code. The crate depends on the domain-free layers only
//! (core, activation, store) and goes through the same seam toy-domain proves:
//! - [`registry`]: its node kinds, edge categories and cell types, allocated
//!   in the `build` id block (`0x0002`), so no id equals a code id;
//! - [`profile`]: [`BUILD_TABLES`] (entries, carry edges, weights),
//!   [`BUILD_PASSES`] and [`BUILD_PROFILE`];
//! - [`graph`]: [`BuildGraph`] (a `GraphSource`), its node ids, and the
//!   header-described `targets.gmap` round trip;
//! - [`snapshot`]: [`TargetRow`] and the `.glia/targets-snapshot/` the
//!   snapshot step writes ([`write_targets`]) and a build reads
//!   ([`read_targets`]) (HT.3).
//!
//! It is a stamp HASHED_ROOTS entry, so a change here moves PARSER_STAMP.

pub mod graph;
pub mod profile;
pub mod registry;
pub mod snapshot;

pub use graph::{
    BuildGraph, GRAPH_TYPE, LABELS_SECTION, NodeLabel, ReadBack, node_id, node_key, read_gmap,
};
pub use profile::{BUILD_PASSES, BUILD_PROFILE, BUILD_TABLES, ROLES, role_of};
pub use registry::{cell_type, edge_category, node_kind};
pub use snapshot::{
    TARGETS_DIR, TARGETS_FILE, TARGETS_VERSION, TargetRow, TargetsMeta, TargetsSnapshot,
    WorkspaceMeta, read_targets, targets_dir, write_targets,
};
