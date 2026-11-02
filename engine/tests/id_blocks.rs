//! HT.1: the id blocks and the id lock.
//!
//! Every shipped domain's registries obey `glia_core::ids` (each id in its own
//! domain's block, a shared block, never a reserved, unassigned or private
//! one), no id is in two shipped domains' tables, every block a domain holds
//! is gap-free from position 1, and the union of the shipped tables equals
//! `core/ids.lock` exactly. The lock is line text, so a PR diff shows exactly
//! the ids it allocates; it is append-only: `GLIA_UPDATE_IDS=1` adds the new
//! rows and still fails when a locked row would change or disappear
//! ("allocated ids never move").
//!
//! fired_on: `[ids] blocks domains=<d> ids=<n> lock=<rows> next=(<block>:<table>=<id> ..)`
//! on stderr, once per run.

use glia_activation::profile::{DomainTables, Registries};
use glia_code_domain::profile::CODE_TABLES;
use glia_core::NodeKindId;
use glia_core::ids::{self, BlockStatus};

/// Every domain a released glia ships. toy-domain is never here: it numbers
/// from 1 on purpose (its overlap with code proves decoding is header-driven)
/// and is never published.
const SHIPPED: &[&DomainTables] = &[&CODE_TABLES];

/// The three registries, in the order the lock's `next=` reads them: the
/// lock's table key and the display word `check_table` names.
const TABLES: [(&str, &str); 3] = [
    ("node_kind", "node kind"),
    ("edge_category", "edge category"),
    ("cell_type", "cell type"),
];

const LOCK_HEADER: &str = "\
# glia id lock (HT.1): every id a shipped domain allocates, one per line:
#   <table> 0x<id, 8 hex> <block> <graph_type> <NAME>, sorted by (table, id).
# Append-only: an allocated id never moves or disappears. Regenerate with
#   GLIA_UPDATE_IDS=1 cargo test -p glia-engine --test engine_tests id_blocks::
# (it only adds rows). The blocks are core/src/ids.rs's.
";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    table: &'static str,
    id: u32,
    graph_type: &'static str,
    name: &'static str,
}

impl Row {
    fn line(&self) -> String {
        format!(
            "{} 0x{:08x} {} {} {}",
            self.table,
            self.id,
            ids::block_name(self.id),
            self.graph_type,
            self.name
        )
    }
}

fn registry_rows(r: &Registries, table: &str) -> Vec<(u32, &'static str)> {
    match table {
        "node_kind" => r.node_kinds.iter().map(|(i, n)| (i.0, *n)).collect(),
        "edge_category" => r.edge_categories.iter().map(|(i, n)| (i.0, *n)).collect(),
        _ => r.cell_types.iter().map(|(i, n)| (i.0, *n)).collect(),
    }
}

fn rows(domains: &[&DomainTables]) -> Vec<Row> {
    let mut out = Vec::new();
    for d in domains {
        for (table, _) in TABLES {
            for (id, name) in registry_rows(&d.registries, table) {
                out.push(Row {
                    table,
                    id,
                    graph_type: d.graph_type,
                    name,
                });
            }
        }
    }
    out.sort();
    out
}

fn lock_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/ids.lock")
}

/// The lock's id rows (comments and blank lines left out).
fn lock_rows(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// `(table, id)` of a lock row.
fn key(line: &str) -> Option<(&str, &str)> {
    let mut f = line.split_whitespace();
    Some((f.next()?, f.next()?))
}

/// The lock after adding `rendered`'s new rows to `existing`, or why it may
/// not be written: a locked row that `rendered` changes or drops.
fn merge_lock(existing: &[String], rendered: &[String]) -> Result<Vec<String>, Vec<String>> {
    let mut errors = Vec::new();
    for old in existing {
        if rendered.contains(old) {
            continue;
        }
        match rendered.iter().find(|r| key(r) == key(old)) {
            Some(new) => errors.push(format!(
                "locked id changed: `{old}` -> `{new}` (allocated ids never move)"
            )),
            None => errors.push(format!(
                "locked id disappeared: `{old}` (allocated ids never move)"
            )),
        }
    }
    if errors.is_empty() {
        let mut out = rendered.to_vec();
        out.sort_by(|a, b| {
            let (ta, ia) = key(a).unwrap_or_default();
            let (tb, ib) = key(b).unwrap_or_default();
            (ta, ia).cmp(&(tb, ib))
        });
        Ok(out)
    } else {
        Err(errors)
    }
}

/// The next free id of each (block, table) a shipped domain allocates in:
/// one past the highest position held. A code id prints in decimal (it is its
/// position), any other block's in hex.
fn next_free(domains: &[&DomainTables], rows: &[Row]) -> Vec<String> {
    let mut out = Vec::new();
    for b in ids::BLOCKS {
        let owned = b.status == BlockStatus::Registered
            && domains
                .iter()
                .any(|d| b.graph_types.contains(&d.graph_type));
        let held = rows.iter().any(|r| ids::block_of(r.id) == b.number);
        if !owned && !held {
            continue;
        }
        for (table, _) in TABLES {
            let top = rows
                .iter()
                .filter(|r| r.table == table && ids::block_of(r.id) == b.number)
                .map(|r| ids::position_of(r.id))
                .max()
                .unwrap_or(0);
            let next = ids::id_in(b.number, top + 1);
            let shown = if b.number == ids::CODE {
                next.to_string()
            } else {
                format!("0x{next:08x}")
            };
            out.push(format!("{}:{table}={shown}", b.name));
        }
    }
    out
}

/// (a) every shipped domain passes the block rules as shipped, and (d) each
/// block a domain holds is gap-free from position 1 in each table.
#[test]
fn shipped_tables_pass_the_block_rules() {
    for d in SHIPPED {
        assert_eq!(d.validate_blocks(true), Ok(()), "{}", d.graph_type);
        for (table, _) in TABLES {
            let rows = registry_rows(&d.registries, table);
            let mut blocks: Vec<u16> = rows.iter().map(|(id, _)| ids::block_of(*id)).collect();
            blocks.sort_unstable();
            blocks.dedup();
            for b in blocks {
                let mut held: Vec<u16> = rows
                    .iter()
                    .filter(|(id, _)| ids::block_of(*id) == b)
                    .map(|(id, _)| ids::position_of(*id))
                    .collect();
                held.sort_unstable();
                let dense: Vec<u16> = (1..=held.len() as u16).collect();
                assert_eq!(
                    held,
                    dense,
                    "{} {table}: block {} must be gap-free from position 1",
                    d.graph_type,
                    ids::block_name(ids::id_in(b, 1))
                );
            }
        }
    }
}

/// (b) no id is in two shipped domains' tables (per table).
#[test]
fn no_id_in_two_shipped_domains() {
    let all = rows(SHIPPED);
    let mut clashes = Vec::new();
    for (i, r) in all.iter().enumerate() {
        for prior in &all[..i] {
            if prior.table == r.table && prior.id == r.id && prior.graph_type != r.graph_type {
                clashes.push(format!("{} and {}", prior.line(), r.line()));
            }
        }
    }
    assert!(clashes.is_empty(), "ids in two domains: {clashes:#?}");
}

/// (c) the shipped tables equal `core/ids.lock` exactly; under
/// `GLIA_UPDATE_IDS=1` the lock gains the new rows only.
#[test]
fn lock_matches_the_shipped_tables() {
    let all = rows(SHIPPED);
    let rendered: Vec<String> = all.iter().map(Row::line).collect();
    let next = next_free(SHIPPED, &all);
    let path = lock_path();
    let existing = std::fs::read_to_string(&path)
        .map(|t| lock_rows(&t))
        .unwrap_or_default();

    let update = std::env::var("GLIA_UPDATE_IDS").is_ok_and(|v| v == "1");
    let locked = if update {
        let merged = match merge_lock(&existing, &rendered) {
            Ok(m) => m,
            Err(e) => panic!(
                "refusing to rewrite {}: {e:#?}\nnext free: {}",
                path.display(),
                next.join(" ")
            ),
        };
        let mut text = LOCK_HEADER.to_string();
        for l in &merged {
            text.push_str(l);
            text.push('\n');
        }
        if let Err(e) = std::fs::write(&path, text) {
            panic!("writing {}: {e}", path.display());
        }
        merged
    } else {
        existing
    };

    eprintln!(
        "[ids] blocks domains={} ids={} lock={} next=({})",
        SHIPPED.len(),
        rendered.len(),
        locked.len(),
        next.join(" ")
    );

    if locked != rendered {
        let added: Vec<&String> = rendered.iter().filter(|r| !locked.contains(r)).collect();
        let gone: Vec<&String> = locked.iter().filter(|l| !rendered.contains(l)).collect();
        panic!(
            "{} differs from the shipped tables.\n  not locked yet (allocate, then run with \
             GLIA_UPDATE_IDS=1): {added:#?}\n  locked but changed or gone (allocated ids never \
             move): {gone:#?}\nnext free: {}",
            path.display(),
            next.join(" ")
        );
    }
}

/// The update path never rewrites history: a new row is added, a changed or
/// dropped one refuses.
#[test]
fn lock_update_is_append_only() {
    let lock = vec![
        "node_kind 0x00000001 code code ROUTE".to_string(),
        "node_kind 0x00000002 code code CLASS".to_string(),
    ];
    let added = vec![
        lock[0].clone(),
        lock[1].clone(),
        "node_kind 0x00000003 code code METHOD".to_string(),
    ];
    assert_eq!(merge_lock(&lock, &added), Ok(added.clone()));

    let renamed = vec![
        lock[0].clone(),
        "node_kind 0x00000002 code code KLASS".to_string(),
    ];
    let e = merge_lock(&lock, &renamed).unwrap_err();
    assert_eq!(e.len(), 1);
    assert!(e[0].starts_with("locked id changed"), "{e:?}");

    // CLASS moved from 2 to 3: its locked row disappears.
    let moved = vec![
        lock[0].clone(),
        "node_kind 0x00000003 code code CLASS".to_string(),
    ];
    let e = merge_lock(&lock, &moved).unwrap_err();
    assert!(e[0].starts_with("locked id disappeared"), "{e:?}");
}

/// (e) a code table holding a build-block id fails with the block's owner
/// named; a private id passes unshipped and fails shipped.
#[test]
fn a_foreign_or_private_row_fails() {
    const TARGET: NodeKindId = NodeKindId(ids::id_in(ids::BUILD, 1));
    let foreign = DomainTables {
        registries: Registries {
            node_kinds: &[(TARGET, "TARGET")],
            ..CODE_TABLES.registries
        },
        ..CODE_TABLES
    };
    assert_eq!(
        foreign.validate_blocks(false),
        Err(vec![
            "node kind 0x00020001 TARGET: block build belongs to graph_type build-targets, not code"
                .to_string()
        ])
    );

    const PROTO: NodeKindId = NodeKindId(ids::id_in(0x8000, 1));
    let private = DomainTables {
        registries: Registries {
            node_kinds: &[(PROTO, "PROTO")],
            ..CODE_TABLES.registries
        },
        ..CODE_TABLES
    };
    assert_eq!(private.validate_blocks(false), Ok(()));
    let e = private.validate_blocks(true).unwrap_err();
    assert_eq!(e.len(), 1);
    assert!(
        e[0].starts_with("node kind 0x80000001 PROTO: block private is never shipped"),
        "{e:?}"
    );

    // the next-free line names the block's next position, in hex past code
    let build = DomainTables {
        graph_type: "build-targets",
        ..foreign
    };
    let next = next_free(&[&build], &rows(&[&build]));
    assert!(
        next.contains(&"build:node_kind=0x00020002".to_string()),
        "{next:?}"
    );
    assert!(
        next.contains(&"build:cell_type=0x00020001".to_string()),
        "{next:?}"
    );
}
