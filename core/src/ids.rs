//! Id blocks (HT.1): which numbered block a registry id belongs to.
//!
//! Every node kind, edge category and cell type is a `u32`. Its high 16 bits
//! are a **block** and its low 16 bits a **position** in that block, like
//! port ranges: 65,536 ids per block, and the owner of any id is a pure
//! function of the id ([`block_of`]). This module is a namespace table only:
//! it says which block a number belongs to and which graph types may allocate
//! there. It knows nothing of what any id means; the domains do.
//!
//! | block | name | status | |
//! |---|---|---|---|
//! | `0x0000` | `code` | registered | every id allocated through 0.5.6 (node 1..=50, edge 1..=38, cell 1..=27) sits below `0x10000`, so it stays exactly where it is: no layout, node id or consumer table moves |
//! | `0x0001` | `core` | shared | domain-free ids any domain's table may list (a future cross-domain link edge); nothing allocated in 0.5.7 |
//! | `0x0002` | `build` | registered | the build-target graph's ids (0.5.7) |
//! | `0x0003` | `goal` | reserved | a standalone goal domain; the goal layer's GOAL_UNIT / TRACES / UNIT stay in the code block, since they are written into the code graph |
//! | `0x0004..=0x7FFF` | | unassigned | the next domain takes the next number in the release that ships it |
//! | `0x8000..=0xFFFE` | `private` | private | a branch, a local domain or a contributor's prototype numbers here without asking; a shipped table never holds one, and the release that merges the idea promotes its ids into the domain's block |
//! | `0xFFFF` | | reserved | never allocated |
//!
//! Inside a block a domain allocates densely from position 1: position 0 is
//! never an id (in the code block it is id 0, which no registry holds).
//!
//! Nothing here is written to disk: a `.gmap` header names its registries as
//! `{id, name}` rows already, and an id above `0xFFFF` decodes unchanged.
//!
//! ```
//! use glia_core::ids::{block_name, block_of, id_in, BUILD, CODE};
//! const TARGET: u32 = id_in(BUILD, 1);
//! assert_eq!(TARGET, 0x0002_0001);
//! assert_eq!(block_of(TARGET), BUILD);
//! assert_eq!(block_of(44), CODE);
//! assert_eq!(block_name(TARGET), "build");
//! assert_eq!(block_name(0x8000_0001), "private");
//! assert_eq!(block_name(0x0004_0001), "0x0004?");
//! ```

use core::ops::RangeInclusive;

/// Bits an id's block is shifted by: `block = id >> BLOCK_SHIFT`.
pub const BLOCK_SHIFT: u32 = 16;

/// The block an id belongs to (its high 16 bits).
pub const fn block_of(id: u32) -> u16 {
    (id >> BLOCK_SHIFT) as u16
}

/// The id's position inside its block (its low 16 bits).
pub const fn position_of(id: u32) -> u16 {
    (id & 0xFFFF) as u16
}

/// Position `n` of `block`. `const`, so a registry writes
/// `NodeKindId(id_in(BUILD, 1))`.
pub const fn id_in(block: u16, n: u16) -> u32 {
    ((block as u32) << BLOCK_SHIFT) | n as u32
}

/// The code domain's block: every id allocated through 0.5.6.
pub const CODE: u16 = 0x0000;
/// Domain-free ids any domain's table may list.
pub const CORE: u16 = 0x0001;
/// The build-target graph's block (0.5.7).
pub const BUILD: u16 = 0x0002;
/// Reserved for a standalone goal domain.
pub const GOAL: u16 = 0x0003;
/// Private / experimental blocks: never in a shipped table.
pub const PRIVATE: RangeInclusive<u16> = 0x8000..=0xFFFE;
/// The one block no id is ever allocated in.
pub const NEVER: u16 = 0xFFFF;

/// Is `id` in a private block?
pub const fn is_private(id: u32) -> bool {
    let b = block_of(id);
    b >= 0x8000 && b <= 0xFFFE
}

/// Who may allocate in a block.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockStatus {
    /// Owned by the graph types it lists; another domain's table holding one
    /// of its ids is an error.
    Registered,
    /// Any domain's table may list its ids.
    Shared,
    /// Named but not open: no table may hold one of its ids yet.
    Reserved,
    /// The private range: allowed while a domain is unshipped, never in a
    /// shipped table.
    Private,
}

impl BlockStatus {
    /// `registered`, `shared`, `reserved` or `private`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Shared => "shared",
            Self::Reserved => "reserved",
            Self::Private => "private",
        }
    }
}

/// One numbered block of the id space.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub number: u16,
    pub name: &'static str,
    /// The graph types (`DomainTables::graph_type`) that allocate here; empty
    /// for a shared or reserved block.
    pub graph_types: &'static [&'static str],
    pub status: BlockStatus,
    /// The release the block opened in.
    pub since: &'static str,
}

/// Every named block, by number. The private range is one entry of its own
/// ([`block`] answers it for any number in [`PRIVATE`]).
pub const BLOCKS: &[Block] = &[
    Block {
        number: CODE,
        name: "code",
        graph_types: &["code"],
        status: BlockStatus::Registered,
        since: "0.4.3b",
    },
    Block {
        number: CORE,
        name: "core",
        graph_types: &[],
        status: BlockStatus::Shared,
        since: "0.5.7",
    },
    Block {
        number: BUILD,
        name: "build",
        graph_types: &["build-targets"],
        status: BlockStatus::Registered,
        since: "0.5.7",
    },
    Block {
        number: GOAL,
        name: "goal",
        graph_types: &[],
        status: BlockStatus::Reserved,
        since: "0.5.7",
    },
];

const PRIVATE_BLOCK: Block = Block {
    number: 0x8000,
    name: "private",
    graph_types: &[],
    status: BlockStatus::Private,
    since: "0.5.7",
};

/// The block numbered `number`: a [`BLOCKS`] row, the private range's row
/// (its `number` the range's first, `0x8000`), or `None` for an unassigned
/// number or `0xFFFF`.
pub fn block(number: u16) -> Option<&'static Block> {
    if PRIVATE.contains(&number) {
        return Some(&PRIVATE_BLOCK);
    }
    BLOCKS.iter().find(|b| b.number == number)
}

/// The name of `id`'s block: `code`, `build`, `private`, .., or `0x0004?`
/// for a block no row names.
pub fn block_name(id: u32) -> String {
    let b = block_of(id);
    match block(b) {
        Some(row) => row.name.to_string(),
        None => format!("0x{b:04X}?"),
    }
}

/// Every block problem with one registry `table` (`"node kind"`, `"edge
/// category"`, `"cell type"`) of the domain whose graph type is `graph_type`,
/// each `<table> 0x<id> <NAME>: <why>`: id 0 or position 0, an id in block
/// `0xFFFF`, in an unassigned block, in a reserved block, or in another
/// domain's registered block. A private id passes here; whether a table may
/// ship one is the caller's question.
pub fn check_table(graph_type: &str, table: &str, ids: &[(u32, &str)]) -> Vec<String> {
    let mut errors = Vec::new();
    for &(id, name) in ids {
        let at = format!("{table} 0x{id:08x} {name}");
        let b = block_of(id);
        if id == 0 {
            errors.push(format!("{at}: id 0 is never allocated"));
            continue;
        }
        if position_of(id) == 0 {
            errors.push(format!(
                "{at}: position 0 of a block is never allocated (a block starts at 1)"
            ));
        }
        if b == NEVER {
            errors.push(format!("{at}: block 0xFFFF is never allocated"));
            continue;
        }
        let Some(row) = block(b) else {
            errors.push(format!(
                "{at}: block 0x{b:04X} is unassigned (take your domain's block, or a private one 0x8000-0xFFFE)"
            ));
            continue;
        };
        match row.status {
            BlockStatus::Registered if !row.graph_types.contains(&graph_type) => {
                let owners = row.graph_types.join(", ");
                errors.push(format!(
                    "{at}: block {} belongs to graph_type {owners}, not {graph_type}",
                    row.name
                ));
            }
            BlockStatus::Reserved => errors.push(format!(
                "{at}: block {} is reserved; nothing is allocated there yet",
                row.name
            )),
            _ => {}
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_arithmetic() {
        assert_eq!(id_in(BUILD, 1), 0x0002_0001);
        assert_eq!(block_of(0x0002_0001), BUILD);
        assert_eq!(position_of(0x0002_0001), 1);
        assert_eq!(block_of(50), CODE);
        assert_eq!(id_in(CODE, 50), 50);
        assert!(is_private(0x8000_0001));
        assert!(is_private(0xFFFE_0001));
        assert!(!is_private(0xFFFF_0001));
        assert!(!is_private(0x7FFF_0001));
    }

    #[test]
    fn names() {
        assert_eq!(block_name(1), "code");
        assert_eq!(block_name(0x0001_0001), "core");
        assert_eq!(block_name(0x0002_0001), "build");
        assert_eq!(block_name(0x0003_0001), "goal");
        assert_eq!(block_name(0x8123_0001), "private");
        assert_eq!(block_name(0x0004_0001), "0x0004?");
        assert_eq!(block_name(0xFFFF_0001), "0xFFFF?");
        assert_eq!(block(0x9000).map(|b| b.status), Some(BlockStatus::Private));
        assert_eq!(block(0x0004), None);
    }

    #[test]
    fn blocks_are_unique_and_named() {
        for (i, b) in BLOCKS.iter().enumerate() {
            assert!(!b.name.is_empty());
            assert!(!PRIVATE.contains(&b.number) && b.number != NEVER);
            assert!(BLOCKS[..i].iter().all(|p| p.number != b.number));
            assert!(BLOCKS[..i].iter().all(|p| p.name != b.name));
            assert_eq!(
                b.status == BlockStatus::Registered,
                !b.graph_types.is_empty(),
                "{}: a registered block names its graph types, no other does",
                b.name
            );
        }
    }

    #[test]
    fn check_table_rules() {
        assert!(check_table("code", "node kind", &[(1, "ROUTE"), (50, "GOAL_UNIT")]).is_empty());
        assert_eq!(
            check_table("code", "node kind", &[(0x0002_0001, "TARGET")]),
            vec![
                "node kind 0x00020001 TARGET: block build belongs to graph_type build-targets, not code"
                    .to_string()
            ]
        );
        assert!(check_table("build-targets", "node kind", &[(0x0002_0001, "TARGET")]).is_empty());
        // a shared block is open to any domain; a private id passes here
        assert!(check_table("code", "edge category", &[(0x0001_0001, "LINK")]).is_empty());
        assert!(check_table("code", "cell type", &[(0x8000_0001, "PROTO")]).is_empty());
        let bad = check_table(
            "code",
            "node kind",
            &[
                (0, "ZERO"),
                (0x0002_0000, "POS0"),
                (0xFFFF_0001, "NEVER"),
                (0x0004_0001, "UNASSIGNED"),
                (0x0003_0001, "GOALISH"),
            ],
        );
        assert_eq!(bad.len(), 6, "{bad:#?}");
        assert!(bad[0].contains("id 0 is never allocated"));
        assert!(bad[1].contains("position 0"));
        assert!(bad[2].contains("belongs to graph_type build-targets"));
        assert!(bad[3].contains("block 0xFFFF is never allocated"));
        assert!(bad[4].contains("block 0x0004 is unassigned"));
        assert!(bad[5].contains("block goal is reserved"));
    }
}
