//! The toy-reel domain's id registries: its own node kinds, edge categories
//! and cell types, each with an `ALL` table of `(id, name)`.
//!
//! Test-only ids, NOT code-domain ids, and deliberately overlapping code's
//! numbers: `SHOT` = 2 (code's CLASS), `FEATURES` = 3 (code's IMPORTS),
//! `TIMECODE` = 1 (code's CODE). A reader that names a toy file's ids from
//! code-domain instead of the file's own header gets every one of those
//! wrong, so the end-to-end test fails unless decoding is header-driven.
//! Nothing here is a code-domain allocation: this is a separate table in a
//! crate no shipped crate depends on.

/// Node kinds. Nodes carry no names: a node is addressed by its kind and its
/// index in the reel (`shot 2`), kept in the domain's `ReelNav` section.
pub mod node_kind {
    use glia_core::NodeKindId;

    pub const SCENE: NodeKindId = NodeKindId(1);
    pub const SHOT: NodeKindId = NodeKindId(2);
    pub const OBJECT: NodeKindId = NodeKindId(3);

    pub const ALL: &[(NodeKindId, &str)] = &[(SCENE, "SCENE"), (SHOT, "SHOT"), (OBJECT, "OBJECT")];
}

/// Edge categories.
pub mod edge_category {
    use glia_core::EdgeCategoryId;

    /// scene -> shot: the shot belongs to the scene.
    pub const CONTAINS_SHOT: EdgeCategoryId = EdgeCategoryId(1);
    /// shot -> shot: the next shot of the same scene.
    pub const NEXT_SHOT: EdgeCategoryId = EdgeCategoryId(2);
    /// shot -> object: the object is on screen in the shot.
    pub const FEATURES: EdgeCategoryId = EdgeCategoryId(3);
    /// object -> object: the same real-world object seen twice (lower id to
    /// higher), paired by the `reidentify_objects` pass.
    pub const SAME_OBJECT: EdgeCategoryId = EdgeCategoryId(4);

    pub const ALL: &[(EdgeCategoryId, &str)] = &[
        (CONTAINS_SHOT, "CONTAINS_SHOT"),
        (NEXT_SHOT, "NEXT_SHOT"),
        (FEATURES, "FEATURES"),
        (SAME_OBJECT, "SAME_OBJECT"),
    ];
}

/// Cell types.
pub mod cell_type {
    use glia_core::CellTypeId;

    /// On a shot: Json `{"start_ms":N,"end_ms":M}`.
    pub const TIMECODE: CellTypeId = CellTypeId(1);
    /// On an object: Text, what the object is (`cat`).
    pub const LABEL: CellTypeId = CellTypeId(2);
    /// On an object: Json `{"ms":N,"shots":M}`, the total duration of the
    /// shots featuring the object or any object re-identified as it. Written
    /// by the `screen_time` pass.
    pub const SCREEN_TIME: CellTypeId = CellTypeId(3);

    pub const ALL: &[(CellTypeId, &str)] = &[
        (TIMECODE, "TIMECODE"),
        (LABEL, "LABEL"),
        (SCREEN_TIME, "SCREEN_TIME"),
    ];
}
