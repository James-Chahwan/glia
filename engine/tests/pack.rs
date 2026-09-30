//! CC.4b — `pack::pack` / `pack::pack_ids`: context packed to a token budget.
//!
//! The fixture is the packet's acceptance tree, written to a tempdir and built
//! with `generate_one`: `shop/a.py` holds a 12-line documented `price(o)` and
//! `place(o)`, which calls it; `shop/b.py` imports `place` and `checkout(o)`
//! calls it; `shop/util.py` is an unrelated 20-line `format_report()`;
//! `pyproject.toml` sits at the root. From the seed `price`, the undirected
//! PPR neighbourhood is `place` (CALLS, weight 5), the module `shop::a`
//! (DEFINES, weight 1), then `checkout` and `shop::b` beyond them; nothing
//! links `shop::util` to the rest.

use std::path::Path;

use glia_engine::generate_one;
use glia_engine::pack::{
    DEFAULT_BUDGET_TOKENS, DEFAULT_BYTES_PER_TOKEN_X10, Pack, PackArgs, PackedNode,
    estimate_tokens, pack, pack_ids,
};
use glia_graph::MergedGraph;
use glia_projection_text::ladder::{Fidelity, Pick, render_pack, rendered_as};

const A_PY: &str = "def price(o):
    \"\"\"Price an order.

    Sums the line totals, then takes the discount off.
    \"\"\"
    total = 0
    for line in o.lines:
        total = total + line.qty * line.unit
    if o.discount:
        total = total - o.discount
    total = round(total, 2)
    return total


def place(o):
    return price(o)
";

const B_PY: &str = "from shop.a import place


def checkout(o):
    return place(o)
";

const UTIL_PY: &str = "def format_report():
    rows = []
    rows.append(\"report\")
    rows.append(\"======\")
    rows.append(\"\")
    rows.append(\"orders: see the ledger\")
    rows.append(\"refunds: see the ledger\")
    rows.append(\"\")
    rows.append(\"totals\")
    rows.append(\"------\")
    rows.append(\"gross\")
    rows.append(\"net\")
    rows.append(\"tax\")
    rows.append(\"\")
    rows.append(\"notes\")
    rows.append(\"-----\")
    rows.append(\"none\")
    rows.append(\"\")
    rows.append(\"end\")
    return \"\\n\".join(rows)
";

/// `price`'s last body line, which only its Full block shows.
const PRICE_LAST_LINE: &str = "    return total";

fn write_shop(root: &Path) {
    std::fs::create_dir_all(root.join("shop")).unwrap();
    std::fs::write(root.join("shop/a.py"), A_PY).unwrap();
    std::fs::write(root.join("shop/b.py"), B_PY).unwrap();
    std::fs::write(root.join("shop/util.py"), UTIL_PY).unwrap();
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy())
        .expect("generate_one")
        .merged
}

fn shop() -> (tempfile::TempDir, MergedGraph) {
    let dir = tempfile::tempdir().unwrap();
    write_shop(dir.path());
    let m = build(dir.path());
    (dir, m)
}

fn budget(tokens: usize) -> PackArgs {
    let mut a = PackArgs::default();
    a.budget_tokens = tokens;
    a
}

fn id_of(m: &MergedGraph, qname: &str) -> glia_core::NodeId {
    m.node_id_by_qname(qname)
        .unwrap_or_else(|| panic!("no node {qname}"))
}

fn node<'p>(p: &'p Pack, qname: &str) -> Option<&'p PackedNode> {
    p.nodes.iter().find(|n| n.qname == qname)
}

fn rung(name: &str) -> Fidelity {
    match name {
        "qname" => Fidelity::Qname,
        "outline" => Fidelity::Outline,
        "preview" => Fidelity::Preview,
        "full" => Fidelity::Full,
        other => panic!("unknown fidelity {other}"),
    }
}

/// The pack's manifest agrees with itself and with its text: ranks strictly
/// increase, every tier / reason is a named one, the counts add up, and the
/// ladder rendering the manifest's picks over the WHOLE graph gives exactly
/// `text` (the pack renders over a candidate subset; this pins that the two
/// agree byte for byte).
fn assert_manifest_shape(m: &MergedGraph, p: &Pack) {
    for w in p.nodes.windows(2) {
        assert!(
            w[0].rank < w[1].rank,
            "manifest not in rank order: {:?}",
            p.nodes
        );
    }
    for n in &p.nodes {
        assert!(["fact", "derived", "heuristic"].contains(&n.tier), "{n:?}");
        assert!(["seed", "neighbour"].contains(&n.reason), "{n:?}");
        assert!(n.tokens >= 1, "{n:?}");
        if n.reason == "neighbour" {
            assert_eq!(n.tier, "derived", "{n:?}");
            assert_eq!(n.matched, None, "{n:?}");
        }
    }
    assert_eq!(p.dropped, p.candidates - p.nodes.len());
    assert_eq!(p.bytes, p.text.len());
    assert_eq!(
        p.used_tokens,
        estimate_tokens(p.bytes, DEFAULT_BYTES_PER_TOKEN_X10)
    );
    if !p.nodes.is_empty() {
        let picks: Vec<Pick> = p
            .nodes
            .iter()
            .map(|n| Pick {
                id: glia_core::NodeId(n.id),
                fidelity: rung(n.fidelity),
            })
            .collect();
        assert_eq!(
            render_pack(m, &format!("context for {}", p.query), &picks),
            p.text
        );
    }
}

#[test]
fn generous_budget_packs_everything_full() {
    let (_dir, m) = shop();
    let p = pack(&m, "price", &budget(100_000));
    assert_manifest_shape(&m, &p);

    let seed = &p.nodes[0];
    assert_eq!(seed.qname, "shop::a::price");
    assert_eq!(seed.reason, "seed");
    assert_eq!(seed.tier, "fact");
    assert_eq!(seed.matched, Some("exact_name"));
    assert_eq!(seed.fidelity, "full");
    assert_eq!(seed.kind, "FUNCTION");
    assert_eq!(seed.file.as_deref(), Some("shop/a.py"));
    assert_eq!(seed.line, Some(1), "1-based (LD.1)");

    // "Everything full": every node at the highest rung the ladder renders it
    // at. A node with no CODE text renders its Full as Outline and the
    // manifest says so (CC.4a's handoff: report `rendered_as`); here the
    // Python MODULEs carry their file's CODE, so every node is `full`.
    for n in &p.nodes {
        let top = rendered_as(&m, glia_core::NodeId(n.id), Fidelity::Full).unwrap();
        assert_eq!(n.fidelity, top.name(), "{n:?} is below its top rung");
        if n.kind == "FUNCTION" {
            assert_eq!(n.fidelity, "full", "{n:?}");
        }
    }
    assert!(p.used_tokens <= 100_000, "{}", p.used_tokens);
    assert_eq!(p.rerenders, 0);
    assert!(p.text.starts_with("# context for price\n"), "{}", p.text);
    assert!(p.text.contains(PRICE_LAST_LINE), "{}", p.text);
    assert!(p.absence.is_none());

    assert_eq!(p.nodes[1].qname, "shop::a::place", "{:#?}", p.nodes);
    assert_eq!(p.nodes[1].reason, "neighbour");
    assert_eq!(p.nodes[1].tier, "derived");
    assert_eq!(p.nodes[1].matched, None);
    let checkout = node(&p, "shop::b::checkout").expect("checkout packed at 100k");
    if let Some(report) = node(&p, "shop::util::format_report") {
        assert!(report.rank > checkout.rank, "{:#?}", p.nodes);
    }
    assert_eq!(p.bytes_per_token, "3.7");
    assert_eq!(p.budget_tokens, 100_000);
}

#[test]
fn budget_sweep_respects_the_budget_and_the_seed() {
    let (_dir, m) = shop();
    let price = id_of(&m, "shop::a::price");
    let mut packed_any = false;
    for b in [8usize, 16, 32, 64, 128, 256, 512] {
        let p = pack(&m, "price", &budget(b));
        assert_manifest_shape(&m, &p);
        if p.nodes.is_empty() {
            let a = p
                .absence
                .as_ref()
                .unwrap_or_else(|| panic!("budget {b}: empty pack, no absence"));
            assert_eq!(a.reason, "budget_too_small", "budget {b}");
            assert!(
                p.text.is_empty(),
                "budget {b}: an empty pack renders nothing"
            );
            assert_eq!(p.used_tokens, 0);
            continue;
        }
        packed_any = true;
        assert!(p.used_tokens <= b, "budget {b}: used {}", p.used_tokens);
        let seed = node(&p, "shop::a::price")
            .unwrap_or_else(|| panic!("budget {b}: price not packed: {:#?}", p.nodes));
        assert_eq!(p.nodes[0].id, price.0, "budget {b}: the seed ranks first");
        let top = p.nodes.iter().map(|n| rung(n.fidelity)).max().unwrap();
        assert_eq!(rung(seed.fidelity), top, "budget {b}: {:#?}", p.nodes);
        if b == 512 {
            let place = node(&p, "shop::a::place").expect("place packed at 512");
            assert!(rung(place.fidelity) >= Fidelity::Outline, "{place:?}");
        }
        // A larger budget never packs the seed lower.
        let bigger = pack(&m, "price", &budget(b * 2));
        let seed2 = node(&bigger, "shop::a::price").unwrap();
        assert!(
            rung(seed2.fidelity) >= rung(seed.fidelity),
            "budget {b} -> {}",
            b * 2
        );
    }
    assert!(packed_any, "no budget in the sweep packed anything");
}

#[test]
fn estimate_tokens_rounds_up() {
    assert_eq!(estimate_tokens(37, 37), 10);
    assert_eq!(estimate_tokens(38, 37), 11);
    assert_eq!(estimate_tokens(0, 37), 0);
    assert_eq!(estimate_tokens(40, 10), 40);
    // Below one byte per token clamps to one byte per token.
    assert_eq!(estimate_tokens(40, 0), 40);
    assert_eq!(DEFAULT_BUDGET_TOKENS, 8000);
}

#[test]
fn unknown_query_has_find_absence() {
    let (_dir, m) = shop();
    let p = pack(&m, "zzz_nothing", &budget(4000));
    assert!(p.nodes.is_empty());
    assert_eq!(p.candidates, 0);
    assert_eq!(p.used_tokens, 0);
    assert!(p.text.is_empty(), "{}", p.text);
    let a = p.absence.expect("absence");
    assert_eq!(a.reason, "no_match");
    assert_eq!(a.tier, "FACT");
}

#[test]
fn pack_ids_takes_explicit_seeds() {
    let (_dir, m) = shop();
    let checkout = id_of(&m, "shop::b::checkout");
    let p = pack_ids(&m, &[checkout], &budget(4000));
    assert_manifest_shape(&m, &p);
    assert_eq!(p.nodes[0].qname, "shop::b::checkout");
    assert_eq!(p.nodes[0].reason, "seed");
    assert_eq!(p.nodes[0].tier, "fact");
    assert_eq!(p.nodes[0].matched, None);
    assert_eq!(p.query, "shop::b::checkout");
    assert!(node(&p, "shop::a::place").is_some(), "{:#?}", p.nodes);

    // An id no graph holds seeds nothing: an empty pack that says why.
    let none = pack_ids(&m, &[glia_core::NodeId(1)], &budget(4000));
    assert!(none.nodes.is_empty());
    assert_eq!(none.absence.expect("absence").reason, "no_match");
}

#[test]
fn deterministic() {
    let (dir, m) = shop();
    let args = budget(256);
    let a = serde_json::to_string(&pack(&m, "price", &args)).unwrap();
    let b = serde_json::to_string(&pack(&m, "price", &args)).unwrap();
    assert_eq!(a, b);

    let again = build(dir.path());
    let t1 = pack(&m, "price", &budget(100_000)).text;
    let t2 = pack(&again, "price", &budget(100_000)).text;
    assert_eq!(t1, t2);
    let t1 = pack(&m, "price", &args).text;
    let t2 = pack(&again, "price", &args).text;
    assert_eq!(t1, t2);
}
