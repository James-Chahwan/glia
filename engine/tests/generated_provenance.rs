//! DE.7 fired_on proof: ORIGIN `generated` from a generator header or a
//! codegen file name, on a REAL build of
//! `bench/substrate-gap/fixtures/generated-provenance`.
//!
//! The positives: `db/queries_gen.go` (an sqlc header, a plain name),
//! `lib/user.g.dart` (a codegen name, no header) and
//! `Data/Settings.Designer.cs` (a designer name and a C# header). The
//! negatives grade.py's frozen `forbid` vocabulary cannot state (it has no
//! cell form): the hand-written `db/store.go`, and `db/limits.go`, whose
//! edit warning says nothing of a generator, carry no ORIGIN at all. Run
//! with `-- --nocapture` to see the `[provenance]` marker.

use glia_code_domain::{cell_type, node_kind};
use glia_core::{CellPayload, NodeKindId};
use glia_engine::{GenerateResult, generate_one};

const GENERATED: &str = r#"{"provenance":"generated"}"#;

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/generated-provenance"
    )
    .to_string()
}

/// The ORIGIN payloads of every entry of the `kind` node named `qname`;
/// panics unless the build holds exactly one such node id.
fn origins(r: &GenerateResult, kind: NodeKindId, qname: &str) -> Vec<String> {
    let mut ids: Vec<_> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter(move |n| {
                g.nav.kind_by_id.get(&n.id) == Some(&kind)
                    && g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)
            })
        })
        .map(|n| n.id)
        .collect();
    ids.sort_by_key(|id| id.0);
    ids.dedup();
    let qnames: Vec<&String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter_map(move |n| g.nav.qname_by_id.get(&n.id)))
        .collect();
    assert_eq!(ids.len(), 1, "one {qname}; the build holds {qnames:?}");
    r.merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| n.id == ids[0])
        .flat_map(|n| &n.cells)
        .filter(|c| c.kind == cell_type::ORIGIN)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) | CellPayload::Text(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn generated_files_carry_origin_generated_and_hand_written_ones_none() {
    let r = generate_one(&fixture()).expect("fixture builds");
    for (kind, qname, why) in [
        (node_kind::FUNCTION, "db::queries_gen::GetUser", "an sqlc header"),
        (node_kind::FUNCTION, "lib::user.g::userToJson", "a `.g.dart` name"),
        (node_kind::METHOD, "Gen::Data::Settings::Get", "a `.Designer.cs` name"),
    ] {
        let o = origins(&r, kind, qname);
        assert!(!o.is_empty(), "{qname} ({why}) carries an ORIGIN");
        assert!(o.iter().all(|p| p == GENERATED), "{qname} ({why}): {o:?}");
    }
    for (kind, qname, why) in [
        (node_kind::FUNCTION, "db::store::NewStore", "hand-written"),
        (node_kind::FUNCTION, "db::limits::Limit", "an edit warning that names no generator"),
        (node_kind::METHOD, "lib::user::User::toJson", "the hand-written half of the part"),
    ] {
        let o = origins(&r, kind, qname);
        assert!(o.is_empty(), "{qname} ({why}) carries no ORIGIN, got {o:?}");
    }
}
