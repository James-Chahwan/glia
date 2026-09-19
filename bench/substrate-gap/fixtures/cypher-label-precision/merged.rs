// Graph merge helpers. A Rust `match` is not Cypher, and neither is this
// comment about the Cypher form MATCH (x:Label).
pub fn pick(kind: u32, a: u64, b: u64) -> u64 {
    match kind {
        k if k == (node_kind::CLASS) => a,
        _ => (merged::pick_primary(a, b)),
    }
}
