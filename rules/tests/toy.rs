//! DA.20: the toy proof (decision Q10). A second domain with no names - a reel of
//! scenes, shots and objects - implements `FactSource` itself, inline ([`Reel`]: no
//! `MemSource`, no toy-domain), and one program runs recursion, negation, an
//! aggregate, a comparison on its `index` attribute and a `reaches` over its carry
//! edge NEXT_SHOT. The rules crate never learns a domain word: every name comes from
//! the reel's own tables, and a node prints as its kind word and index (`shot/3`).
//!
//! The reel: scenes 0-1, shots 0-5 chained by NEXT_SHOT (the hop shot/2 -> shot/3
//! HEURISTIC, the rest FACT; pushed in reverse so no answer can lean on edge order),
//! scene/0 CONTAINS_SHOT shots 0-2 and scene/1 shots 3-5, objects 0-3 of which
//! shots 1, 2 and 4 FEATURE objects 0, 1 and 2, and a LABEL cell on objects 0 and 3.
//! Its ids are test-local and mean nothing outside this file.
//!
//! [`dependency_guard`] pins the crate to the domain-free layers (glia-core,
//! glia-activation, no dev / build / target table), and [`toy_domain_untouched`]
//! holds Q10's other half: toy-domain's dependency list is unchanged.

use glia_activation::algo::GraphSource;
use glia_activation::profile::{DomainTables, EntryRule, Registries};
use glia_core::{
    Cell, CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId,
};
use glia_rules::diag::Severity;
use glia_rules::eval::{Evaluation, Options, run};
use glia_rules::explain::ExplainLimits;
use glia_rules::ir::{AttrDecl, ColType};
use glia_rules::source::{FactSource, catalog_conflicts};
use glia_rules::value::{Tier, Value};

const SCENE: NodeKindId = NodeKindId(1);
const SHOT: NodeKindId = NodeKindId(2);
const OBJECT: NodeKindId = NodeKindId(3);
const CONTAINS_SHOT: EdgeCategoryId = EdgeCategoryId(1);
const NEXT_SHOT: EdgeCategoryId = EdgeCategoryId(2);
const FEATURES: EdgeCategoryId = EdgeCategoryId(3);
const LABEL: CellTypeId = CellTypeId(2);

const TABLES: DomainTables = DomainTables {
    graph_type: "toy-reel",
    registries: Registries {
        node_kinds: &[(SCENE, "SCENE"), (SHOT, "SHOT"), (OBJECT, "OBJECT")],
        edge_categories: &[
            (CONTAINS_SHOT, "CONTAINS_SHOT"),
            (NEXT_SHOT, "NEXT_SHOT"),
            (FEATURES, "FEATURES"),
        ],
        cell_types: &[(LABEL, "LABEL")],
    },
    entry: EntryRule { kinds: &[], roles: &[], named: &[] },
    carry_edges: &[NEXT_SHOT],
    effect_sinks: &[],
    activation_weights: &[],
    activation_presets: &[],
    community_weights: &[],
    identity_rules: &[],
};

/// The reel's one attribute column: `index(Node, int)`, a node's place in the reel.
const ATTRS: &[AttrDecl] = &[AttrDecl { name: "index", ty: ColType::Int }];

/// A nameless domain's graph, scanned as plain vectors (12 nodes: no map).
struct Reel {
    /// `(id, kind, index)`, in node order.
    nodes: Vec<(NodeId, NodeKindId, u32)>,
    edges: Vec<Edge>,
    /// By edge ordinal.
    tiers: Vec<Tier>,
    labels: Vec<(NodeId, Vec<Cell>)>,
}

fn word(kind: NodeKindId) -> &'static str {
    match kind {
        SCENE => "scene",
        SHOT => "shot",
        _ => "object",
    }
}

/// toy-domain's identity scheme: a hash of `<kind word>/<index>`, never displayed.
fn id(kind: NodeKindId, index: u32) -> NodeId {
    NodeId::from_parts("toy-reel", RepoId(1), kind, &format!("{}/{index}", word(kind)))
}

fn reel() -> Reel {
    let mut r =
        Reel { nodes: Vec::new(), edges: Vec::new(), tiers: Vec::new(), labels: Vec::new() };
    for (kind, n) in [(SCENE, 2), (SHOT, 6), (OBJECT, 4)] {
        r.nodes.extend((0..n).map(|i| (id(kind, i), kind, i)));
    }
    let mut edge = |from: NodeId, to: NodeId, cat: EdgeCategoryId, tier: Tier| {
        let conf = if tier == Tier::Fact { Confidence::Strong } else { Confidence::Weak };
        r.edges.push(Edge::new(from, to, cat, conf));
        r.tiers.push(tier);
    };
    for i in (0..5).rev() {
        let tier = if i == 2 { Tier::Heuristic } else { Tier::Fact };
        edge(id(SHOT, i), id(SHOT, i + 1), NEXT_SHOT, tier);
    }
    for sh in 0..6 {
        edge(id(SCENE, sh / 3), id(SHOT, sh), CONTAINS_SHOT, Tier::Fact);
    }
    for (sh, o) in [(1, 0), (2, 1), (4, 2)] {
        edge(id(SHOT, sh), id(OBJECT, o), FEATURES, Tier::Fact);
    }
    for (o, text) in [(0, "cat"), (3, "lamp")] {
        let cell = Cell { kind: LABEL, payload: CellPayload::Text(text.into()) };
        r.labels.push((id(OBJECT, o), vec![cell]));
    }
    r
}

impl GraphSource for Reel {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|n| n.0).collect()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

impl FactSource for Reel {
    fn tables(&self) -> &DomainTables {
        &TABLES
    }

    fn node_kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.nodes.iter().find(|n| n.0 == id).map(|n| n.1)
    }

    fn node_cells(&self, id: NodeId) -> &[Cell] {
        self.labels.iter().find(|l| l.0 == id).map_or(&[], |l| l.1.as_slice())
    }

    fn render_node(&self, id: NodeId) -> String {
        match self.nodes.iter().find(|n| n.0 == id) {
            Some(&(_, kind, index)) => format!("{}/{index}", word(kind)),
            None => format!("#{:016x}", id.0),
        }
    }

    fn attributes(&self) -> &[AttrDecl] {
        ATTRS
    }

    fn attr_rows(&self, attr: usize, out: &mut Vec<(NodeId, Value)>) {
        if attr == 0 {
            out.extend(self.nodes.iter().map(|n| (n.0, Value::Int(i64::from(n.2)))));
        }
    }

    fn edge_tier(&self, ordinal: usize, _e: &Edge) -> Tier {
        self.tiers.get(ordinal).copied().unwrap_or(Tier::Derived)
    }
}

const PROGRAM: &str = r#"
later(a, b) :- edge(a, b, "NEXT_SHOT").
later(a, c) :- later(a, b), edge(b, c, "NEXT_SHOT").
in_scene(o, s) :- edge(s, sh, "CONTAINS_SHOT"), edge(sh, o, "FEATURES").
featured(o) :- edge(_, o, "FEATURES").
unseen(o) :- node(o, "OBJECT"), !featured(o).
shots(s, count(sh)) :- edge(s, sh, "CONTAINS_SHOT").
early(sh) :- node(sh, "SHOT"), index(sh, i), i < 3.
to_last(a) :- node(a, "SHOT"), index(z, 5), node(z, "SHOT"), reaches(a, z).
labelled(o) :- cell(o, "LABEL", _).
"#;

fn evaluate(r: &Reel) -> Evaluation<'_, Reel> {
    let mut opts = Options::default();
    opts.max_rows = 10_000;
    match run("toy.dl", PROGRAM, r, &opts) {
        Ok(ev) => ev,
        Err(e) => panic!("the toy program evaluates: {e}"),
    }
}

/// Output `pred`'s rows, rendered, with their tiers.
fn table(ev: &Evaluation<'_, Reel>, pred: &str) -> Vec<(Vec<String>, Tier)> {
    let rows = ev.rows(pred).unwrap_or_else(|e| panic!("{pred}: {e}"));
    rows.iter().map(|r| (r.values.iter().map(|v| ev.render(v)).collect(), r.tier)).collect()
}

fn row(cols: &[&str], tier: Tier) -> (Vec<String>, Tier) {
    (cols.iter().map(|c| c.to_string()).collect(), tier)
}

fn shot(i: u32) -> String {
    format!("shot/{i}")
}

#[test]
fn toy_program_rows() {
    let r = reel();
    assert!(catalog_conflicts(&r).is_empty(), "{:?}", catalog_conflicts(&r));
    let ev = evaluate(&r);
    // `later` is read unbound and is an output (magic never rewrites one), so its
    // recursion runs from every NEXT_SHOT edge, which analysis says, and only that.
    let warned: Vec<_> = ev.warnings().iter().map(|d| (d.severity, d.predicate.clone())).collect();
    assert_eq!(warned, [(Severity::Warning, Some("later/2".to_string()))]);
    assert!(ev.warnings()[0].message.starts_with("recursive predicate `later/2` is unseeded"));
    let outputs =
        ["early/1", "in_scene/2", "labelled/1", "later/2", "shots/2", "to_last/1", "unseen/1"];
    assert_eq!(ev.outputs(), outputs);

    // Recursion: every ordered shot pair, HEURISTIC exactly when it crosses shot/2 -> shot/3.
    let mut later = Vec::new();
    for a in 0..6u32 {
        for b in a + 1..6 {
            let tier = if a <= 2 && b >= 3 { Tier::Heuristic } else { Tier::Fact };
            later.push((vec![shot(a), shot(b)], tier));
        }
    }
    assert_eq!(later.len(), 15);
    assert_eq!(table(&ev, "later"), later);
    let in_scene = [["object/0", "scene/0"], ["object/1", "scene/0"], ["object/2", "scene/1"]];
    assert_eq!(table(&ev, "in_scene"), in_scene.map(|c| row(&c, Tier::Fact)));
    assert_eq!(
        table(&ev, "shots"),
        [row(&["scene/0", "3"], Tier::Fact), row(&["scene/1", "3"], Tier::Fact)]
    );
    let early: Vec<_> = (0..3).map(|i| (vec![shot(i)], Tier::Fact)).collect();
    assert_eq!(table(&ev, "early"), early);
    let to_last: Vec<_> = (0..5)
        .map(|i| (vec![shot(i)], if i <= 2 { Tier::Heuristic } else { Tier::Fact }))
        .collect();
    assert_eq!(table(&ev, "to_last"), to_last);
    assert_eq!(
        table(&ev, "labelled"),
        [row(&["object/0"], Tier::Fact), row(&["object/3"], Tier::Fact)]
    );
    for pred in outputs.iter().filter(|p| **p != "unseen/1") {
        assert!(ev.rows(pred).unwrap().iter().all(|r| r.absence.is_empty()), "{pred} has absence");
    }

    // Negation: the one object no shot features, with the absence it rests on.
    assert_eq!(table(&ev, "unseen"), [row(&["object/3"], Tier::Fact)]);
    let unseen = ev.rows("unseen").unwrap();
    let caveats = &unseen[0].absence;
    assert_eq!(caveats.len(), 1);
    assert_eq!(caveats[0].negated, "featured/1");
    assert_eq!(caveats[0].categories, ["FEATURES"]);
    assert!(!caveats[0].any_category);
}

#[test]
fn toy_witness() {
    let r = reel();
    let mut ev = evaluate(&r);
    let rows = ev.rows("later").unwrap();
    let target = rows
        .iter()
        .find(|x| x.values == [Value::Node(id(SHOT, 0)), Value::Node(id(SHOT, 5))])
        .expect("later(shot/0, shot/5) holds");
    assert_eq!(target.tier, Tier::Heuristic);
    let mut limits = ExplainLimits::default();
    limits.max_candidates = 10_000;
    let w = ev.explain(target, &limits).expect("later(shot/0, shot/5) explains");
    let head = (w.predicate.as_str(), w.tier, w.height, w.truncated);
    assert_eq!(head, ("later/2", Tier::Heuristic, 5, false));
    let hops: Vec<_> = w
        .leaf_edges()
        .iter()
        .map(|h| {
            let e = &r.edges[h.ordinal];
            assert_eq!((e.from, e.to, e.category), (h.from, h.to, h.category), "ordinal is global");
            (r.render_node(h.from), r.render_node(h.to), h.category, h.tier)
        })
        .collect();
    let want: Vec<_> = (0..5)
        .map(|i| {
            let tier = if i == 2 { Tier::Heuristic } else { Tier::Fact };
            (shot(i), shot(i + 1), NEXT_SHOT, tier)
        })
        .collect();
    assert_eq!(hops, want);
}

/// The keys of `[name]` in a Cargo manifest, sorted (toy-domain's guard reads it so).
fn table_keys(manifest: &str, name: &str) -> Vec<String> {
    let header = format!("[{name}]");
    let mut keys: Vec<String> = manifest
        .lines()
        .map(str::trim)
        .skip_while(|l| *l != header)
        .skip(1)
        .take_while(|l| !l.starts_with('['))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim().to_string()))
        .collect();
    keys.sort();
    keys
}

#[test]
fn dependency_guard() {
    let manifest = include_str!("../Cargo.toml");
    assert_eq!(table_keys(manifest, "dependencies"), ["glia-activation", "glia-core"]);
    // No [dev-dependencies], [build-dependencies], [target.*.dependencies] or
    // [dependencies.<crate>] table: nothing else can reach the crate, even in a test.
    let dep_tables: Vec<&str> = manifest
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('[') && l.contains("dependencies"))
        .collect();
    assert_eq!(dep_tables, ["[dependencies]"]);
    assert!(!manifest.contains("toy-domain"), "rules/Cargo.toml names toy-domain");
}

#[test]
fn toy_domain_untouched() {
    let manifest = include_str!("../../toy-domain/Cargo.toml");
    assert_eq!(
        table_keys(manifest, "dependencies"),
        ["glia-activation", "glia-core", "glia-store"]
    );
}
