//! The build-target domain's id registries: its node kinds, edge categories
//! and cell types, each with an `ALL` table of `(id, name)`.
//!
//! Every id is allocated in the `build` block, `0x0002` (HT.1's id blocks:
//! `id_in(BUILD, n)`, `n` dense from 1), so no build-target id ever equals a
//! code id: a reader holding a code layout and `targets.gmap` side by side
//! (the targets answers' join to code modules, review's build-targets
//! section) can never mistake one for the other. Each row is in
//! `core/ids.lock`, which only grows.

use glia_core::ids::{BUILD, id_in};

/// Node kinds. A node's identity is its workspace and label (`node_id`):
/// the label is the build system's own, as `bazel query` prints it.
pub mod node_kind {
    use super::{BUILD, id_in};
    use glia_core::NodeKindId;

    /// A rule instance (`cc_library`, `go_test`, `alias`, ...): something
    /// the build system builds, runs or tests.
    pub const TARGET: NodeKindId = NodeKindId(id_in(BUILD, 1));
    /// A package (`//main`): the directory holding one BUILD file.
    pub const PACKAGE: NodeKindId = NodeKindId(id_in(BUILD, 2));
    /// A source or generated file a target names (`//main:hello-world.cc`).
    pub const SOURCE: NodeKindId = NodeKindId(id_in(BUILD, 3));
    /// A label outside the main workspace (`@@rules_cc+//cc:...`), or one
    /// naming nothing the snapshot holds (ATTRS `{"dangling": true}`).
    pub const EXTERNAL: NodeKindId = NodeKindId(id_in(BUILD, 4));

    pub const ALL: &[(NodeKindId, &str)] = &[
        (TARGET, "TARGET"),
        (PACKAGE, "PACKAGE"),
        (SOURCE, "SOURCE"),
        (EXTERNAL, "EXTERNAL"),
    ];
}

/// Edge categories. Each edge carries a [`VIA`](crate::registry::cell_type::VIA) cell naming the
/// attributes that declared it.
pub mod edge_category {
    use super::{BUILD, id_in};
    use glia_core::EdgeCategoryId;

    /// target -> target / external: a build-time dependency, declared by
    /// `deps`, `embed`, `actual`, `exports`, `runtime_deps` or
    /// `implementation_deps`.
    pub const DEPENDS_ON: EdgeCategoryId = EdgeCategoryId(id_in(BUILD, 1));
    /// target -> target / source / external: a runtime data dependency,
    /// declared by `data`.
    pub const RUNTIME_DEP: EdgeCategoryId = EdgeCategoryId(id_in(BUILD, 2));
    /// target -> source: a file the target compiles or exposes, declared by
    /// `srcs`, `hdrs` or `textual_hdrs`.
    pub const HAS_SRC: EdgeCategoryId = EdgeCategoryId(id_in(BUILD, 3));
    /// target / source -> package: the package that declares it.
    pub const IN_PACKAGE: EdgeCategoryId = EdgeCategoryId(id_in(BUILD, 4));

    pub const ALL: &[(EdgeCategoryId, &str)] = &[
        (DEPENDS_ON, "DEPENDS_ON"),
        (RUNTIME_DEP, "RUNTIME_DEP"),
        (HAS_SRC, "HAS_SRC"),
        (IN_PACKAGE, "IN_PACKAGE"),
    ];
}

/// Cell types.
pub mod cell_type {
    use super::{BUILD, id_in};
    use glia_core::CellTypeId;

    /// On a TARGET or SOURCE: Json `{"file":"<repo-relative path>","line":L}`,
    /// `line` 0-based. The build domain has no EVIDENCE cell: an edge's site
    /// is its source target's LOCATION.
    pub const LOCATION: CellTypeId = CellTypeId(id_in(BUILD, 1));
    /// On a TARGET: Json `{"rule_class","tags","testonly","visibility",
    /// "workspace"}`, keys and lists sorted. On a dangling EXTERNAL:
    /// `{"dangling":true}`.
    pub const ATTRS: CellTypeId = CellTypeId(id_in(BUILD, 2));
    /// On a TARGET: Text `test`, `binary`, `library` or `alias`, written by
    /// the `role_cells` pass from ATTRS' `rule_class`.
    pub const ROLE: CellTypeId = CellTypeId(id_in(BUILD, 3));
    /// On an edge: Text, the attribute names that declared it, sorted and
    /// comma-joined (`deps,embed`).
    pub const VIA: CellTypeId = CellTypeId(id_in(BUILD, 4));

    pub const ALL: &[(CellTypeId, &str)] = &[
        (LOCATION, "LOCATION"),
        (ATTRS, "ATTRS"),
        (ROLE, "ROLE"),
        (VIA, "VIA"),
    ];
}
