//! The reflexion model as rules (DB.8a): [`REFLEXION`] (`reflexion.dl`) states check's
//! CC-2 model (engine/src/check.rs module doc `# Reflexion model (check v2)`) as one
//! stratified program, and [`derive`] evaluates it over check's EDB, reading the
//! relations back into check's `ReflexionFacts`, the struct check's native derivation
//! fills and `render_model` reads. The native derivation is the oracle: the parity
//! tests (`rules::reflexion_parity`) hold the two to byte-identical `CheckReport` JSON.
//!
//! **EDB.** Seven domain builtins beside the [`CodeFacts`] (whose `under` the program
//! reads), every column bound or free, each FACT but `edge_site`:
//!
//! | relation | rows |
//! |---|---|
//! | `component_path(C, P)` | `ModelEdb::component_path`: each normalised path, its one owner |
//! | `layer_of(C, Rank, S)` | `ModelEdb::layer_of`, `S` = `"strict"` or `"lax"` |
//! | `next_rank(R, R2)` | `R2 = R + 1` for each rank a layer holds; no row past `u32::MAX`, the native `checked_add(1)` |
//! | `allow(C1, C2, Id)` | each admitted allow's id |
//! | `located(N, F)` | `CodeFacts::files`, the raw file string, in node order |
//! | `project_path(N, P)` | `CodeFacts::projects`, in node order |
//! | `edge_site(E, A, B)` | `E` the index of each distinct `check::edge_site` among the checked categories' edges, in `all_edges` order; tiered as `why::tier_of` tiers its first edge |
//!
//! A component is its `ModelEdb::components` index as an Int, so every relation keys
//! as the native maps do; `E` keeps its first edge, evidence, tier and note beside the
//! program, so a `dep` row reads back as the native `DepEdge`.
//!
//! **Read back.** `in_component`, `unmapped`, `dep` (each pair's sites in `E` order,
//! the native `all_edges` order), `allowed` (named as the native names it: the
//! smallest allow id, else `layer:<upper>><lower>`; naming is rendering), `closed`,
//! `convergence`, `divergence`, `absence`, and `unmapped_edges` = the `unmapped_edge`
//! rows. [`derive_tiered`] also returns the rules layer's tier of each
//! `dependent(C1, C2)`: the min over its sites of the site's tier, which the parity
//! tests compare with the strongest-edge tier the matrix renders.
//!
//! The program is fixed and non-recursive, so it runs without a row budget: its
//! largest relation, `owns`, is at most (placed nodes) x (declared paths), its EDB's
//! product, never a closure the budget exists to stop. No magic rewrite either (it
//! applies to recursion only).

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use glia_activation::algo::GraphSource;
use glia_activation::profile::DomainTables;
use glia_code_domain::evidence::Evidence;
use glia_core::{Cell, Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_rules::diag::RulesError;
use glia_rules::eval::{Evaluation, Options, Row, evaluate};
use glia_rules::ir::{AttrDecl, BindReq, BuiltinDecl, BuiltinKind, ColType, Program};
use glia_rules::parser::parse_program;
use glia_rules::source::{Builtin, BuiltinRow, FactSource};
use glia_rules::value::{Tier, Value, ValueRef};

use super::facts::CodeFacts;
use crate::check::{Allows, DepEdge, EdgeSite, LayerFact, ModelEdb, ReflexionFacts, edge_site};
use crate::why;

/// The model's rule text (`engine/src/rules/reflexion.dl`).
pub(crate) const REFLEXION: &str = include_str!("reflexion.dl");

/// The source name the program's spans carry.
const REFLEXION_SOURCE: &str = "reflexion";

/// The relations read back, as `name/arity` (the evaluator's outputs).
const OUTPUTS: [&str; 10] = [
    "in_component/2",
    "unmapped/2",
    "dep/3",
    "dependent/2",
    "allowed/2",
    "closed/1",
    "convergence/2",
    "divergence/2",
    "absence/2",
    "unmapped_edge/1",
];

/// The rules layer's tier of each `dependent(C1, C2)` row.
pub(crate) type DependentTiers = BTreeMap<(usize, usize), Tier>;

/// The program, parsed once.
fn parsed() -> Result<&'static Program, String> {
    static PARSED: OnceLock<Result<Program, String>> = OnceLock::new();
    match PARSED
        .get_or_init(|| parse_program(REFLEXION_SOURCE, REFLEXION).map_err(|e| e.to_string()))
    {
        Ok(p) => Ok(p),
        Err(e) => Err(e.clone()),
    }
}

/// Derive the model's relations from `edb` over the `checked` categories through
/// [`REFLEXION`] (module doc): check's `DeriveModel` for the rules layer. `facts` must
/// be facts over `merged`. An `Err` is the evaluation's rendered error.
pub(crate) fn derive<'m>(
    facts: &CodeFacts<'m>,
    merged: &'m MergedGraph,
    edb: ModelEdb,
    checked: Vec<EdgeCategoryId>,
) -> Result<ReflexionFacts<'m>, String> {
    derive_tiered(facts, merged, edb, checked).map(|(f, _)| f)
}

/// [`derive`], with the tier the rules layer gives each `dependent(C1, C2)`.
pub(crate) fn derive_tiered<'m>(
    facts: &CodeFacts<'m>,
    merged: &'m MergedGraph,
    edb: ModelEdb,
    checked: Vec<EdgeCategoryId>,
) -> Result<(ReflexionFacts<'m>, DependentTiers), String> {
    let program = parsed()?;
    let sites = edge_sites(merged, &checked);
    let src = ModelFacts {
        facts,
        relations: model_relations(facts, &edb, &sites),
    };
    let mut options = Options::default();
    options.outputs = OUTPUTS.iter().map(|s| (*s).to_string()).collect();
    options.magic = false;
    options.max_rows = usize::MAX;
    let ev = evaluate(program, &src, &options).map_err(|e| e.to_string())?;
    for w in ev.warnings() {
        eprintln!("{w}");
    }
    let read = ReadBack { ev: &ev };

    let mut in_component: HashMap<NodeId, usize> = HashMap::new();
    for r in read.rows("in_component/2")? {
        let (n, c) = (node(&r, 0)?, index(&r, 1)?);
        if let Some(held) = in_component.insert(n, c)
            && held != c
        {
            return Err(format!(
                "reflexion: node {} sits in components {held} and {c}",
                facts.render_node(n)
            ));
        }
    }
    let mut unmapped: HashMap<NodeId, String> = HashMap::new();
    for r in read.rows("unmapped/2")? {
        unmapped.insert(node(&r, 0)?, text(&r, 1)?);
    }

    // Each pair's sites in E order: the native all_edges order.
    let mut deps: Vec<(usize, (usize, usize))> = Vec::new();
    for r in read.rows("dep/3")? {
        deps.push((index(&r, 2)?, (index(&r, 0)?, index(&r, 1)?)));
    }
    deps.sort_unstable();
    let mut slots: Vec<Option<DepEdge<'m>>> = sites.into_iter().map(Some).collect();
    let mut dep: BTreeMap<(usize, usize), Vec<DepEdge<'m>>> = BTreeMap::new();
    for (e, pair) in deps {
        let Some(site) = slots.get_mut(e).and_then(Option::take) else {
            return Err(format!(
                "reflexion: dep row names edge site {e}, which is no site or is already in a pair"
            ));
        };
        dep.entry(pair).or_default().push(site);
    }
    let mut dependent: DependentTiers = BTreeMap::new();
    for r in read.rows("dependent/2")? {
        dependent.insert((index(&r, 0)?, index(&r, 1)?), r.tier);
    }

    let mut allowed: BTreeMap<(usize, usize), String> = BTreeMap::new();
    for r in read.rows("allowed/2")? {
        let pair = (index(&r, 0)?, index(&r, 1)?);
        let Some(label) = allowed_label(&edb.allow, &edb.layer_of, pair) else {
            return Err(format!(
                "reflexion: allowed({}, {}) has neither an allow nor a layer pair",
                pair.0, pair.1
            ));
        };
        allowed.insert(pair, label);
    }
    let closed = !read.rows("closed/1")?.is_empty();
    let convergence = read.pairs("convergence/2")?;
    let divergence = read.pairs("divergence/2")?;
    let absence = read.pairs("absence/2")?;
    let unmapped_edges = read.rows("unmapped_edge/1")?.len();
    drop(ev);

    let out = ReflexionFacts {
        components: edb.components,
        component_path: edb.component_path,
        layer_of: edb.layer_of,
        allow: edb.allow,
        checked,
        in_component,
        unmapped,
        dep,
        unmapped_edges,
        allowed,
        closed,
        convergence,
        divergence,
        absence,
    };
    Ok((out, dependent))
}

/// What `allowed(C1, C2)` is named as in the matrix: the pair's smallest allow id
/// (an explicit allow names the decision better), else `layer:<upper>><lower>`;
/// `None` when neither holds the pair.
fn allowed_label(
    allow: &BTreeMap<(usize, usize), Allows>,
    layer_of: &BTreeMap<usize, LayerFact>,
    pair: (usize, usize),
) -> Option<String> {
    let smallest = allow
        .get(&pair)
        .and_then(|ids| ids.iter().min_by(|a, b| a.0.cmp(&b.0)));
    if let Some((id, _)) = smallest {
        return Some(id.clone());
    }
    let (upper, lower) = (layer_of.get(&pair.0)?, layer_of.get(&pair.1)?);
    Some(format!("layer:{}>{}", upper.name, lower.name))
}

/// Every distinct checked edge site, in `all_edges` order (`E` is the position):
/// check's dedup key, the first edge holding it, its evidence and `why` tier.
fn edge_sites<'m>(merged: &'m MergedGraph, checked: &[EdgeCategoryId]) -> Vec<DepEdge<'m>> {
    let mut seen: HashSet<EdgeSite> = HashSet::new();
    let mut out = Vec::new();
    for e in merged.all_edges() {
        if !checked.contains(&e.category) {
            continue;
        }
        let ev = Evidence::of(e);
        if !seen.insert(edge_site(e, ev.as_ref())) {
            continue;
        }
        let (tier, note) = why::tier_of(ev.as_ref(), e);
        out.push(DepEdge {
            edge: e,
            ev,
            tier,
            note,
        });
    }
    out
}

/// A row index or component index as an Int. A `Vec` holds at most `isize::MAX`
/// elements, so on 64-bit targets every index converts; the saturation is never hit.
fn int(i: usize) -> Value {
    Value::Int(i64::try_from(i).unwrap_or(i64::MAX))
}

fn string(s: &str) -> Value {
    Value::Str(s.to_string())
}

/// A row of FACT.
fn fact(values: Vec<Value>) -> BuiltinRow {
    BuiltinRow {
        values,
        tier: Tier::Fact,
    }
}

/// The seven EDB relations over `edb`, `facts` and `sites` (module doc).
fn model_relations(
    facts: &CodeFacts<'_>,
    edb: &ModelEdb,
    sites: &[DepEdge<'_>],
) -> Vec<ModelRelation> {
    let component_path = edb
        .component_path
        .iter()
        .map(|(p, c)| fact(vec![int(*c), string(p)]))
        .collect();
    let layer_of = edb
        .layer_of
        .iter()
        .map(|(c, l)| {
            let strict = if l.strict { "strict" } else { "lax" };
            fact(vec![int(*c), Value::Int(i64::from(l.rank)), string(strict)])
        })
        .collect();
    let ranks: BTreeSet<u32> = edb.layer_of.values().map(|l| l.rank).collect();
    let next_rank = ranks
        .into_iter()
        .filter_map(|r| {
            r.checked_add(1)
                .map(|n| fact(vec![Value::Int(i64::from(r)), Value::Int(i64::from(n))]))
        })
        .collect();
    let allow = edb
        .allow
        .iter()
        .flat_map(|((c1, c2), ids)| {
            ids.iter()
                .map(move |(id, _)| fact(vec![int(*c1), int(*c2), string(id)]))
        })
        .collect();
    let order = facts.node_ids();
    let files = facts.files();
    let located = order
        .iter()
        .filter_map(|id| match files.get(id) {
            Some(Some(f)) => Some(fact(vec![Value::Node(*id), string(f)])),
            _ => None,
        })
        .collect();
    let projects = facts.projects();
    let project_path = order
        .iter()
        .filter_map(|id| {
            projects
                .get(id)
                .map(|p| fact(vec![Value::Node(*id), string(p)]))
        })
        .collect();
    let edge_site = sites
        .iter()
        .enumerate()
        .map(|(i, s)| BuiltinRow {
            values: vec![int(i), Value::Node(s.edge.from), Value::Node(s.edge.to)],
            tier: Tier::parse(s.tier).unwrap_or(Tier::Derived),
        })
        .collect();
    use ColType::{Int, Node, Str};
    vec![
        ModelRelation::new("component_path", &[Int, Str], false, component_path),
        ModelRelation::new("layer_of", &[Int, Int, Str], false, layer_of),
        ModelRelation::new("next_rank", &[Int, Int], false, next_rank),
        ModelRelation::new("allow", &[Int, Int, Str], false, allow),
        ModelRelation::new("located", &[Node, Str], false, located),
        ModelRelation::new("project_path", &[Node, Str], false, project_path),
        ModelRelation::new("edge_site", &[Int, Node, Node], true, edge_site),
    ]
}

/// One EDB relation as a domain builtin, every column bound or free: its rows and a
/// per-column index, each built on its first bound lookup (looked up only, never
/// iterated).
struct ModelRelation {
    name: &'static str,
    cols: &'static [ColType],
    /// Its rows carry their own tier; an untiered relation's rows read FACT.
    tiered: bool,
    rows: Vec<BuiltinRow>,
    index: Vec<OnceCell<HashMap<Value, Vec<usize>>>>,
}

impl ModelRelation {
    fn new(
        name: &'static str,
        cols: &'static [ColType],
        tiered: bool,
        rows: Vec<BuiltinRow>,
    ) -> Self {
        ModelRelation {
            name,
            cols,
            tiered,
            rows,
            index: cols.iter().map(|_| OnceCell::new()).collect(),
        }
    }
}

impl Builtin for ModelRelation {
    fn decl(&self) -> BuiltinDecl {
        BuiltinDecl {
            name: self.name.to_string(),
            cols: self.cols.to_vec(),
            bind: vec![BindReq::May; self.cols.len()],
            tiered: self.tiered,
            kind: BuiltinKind::Domain,
        }
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        if args.len() != self.cols.len() {
            return Err(format!(
                "takes {} arguments, got {}",
                self.cols.len(),
                args.len()
            ));
        }
        let agrees = |r: &BuiltinRow| {
            args.iter()
                .zip(&r.values)
                .all(|(a, v)| a.is_none_or(|a| v.as_ref() == a))
        };
        let Some((c, key)) = args.iter().enumerate().find_map(|(c, a)| a.map(|a| (c, a))) else {
            out.extend(self.rows.iter().cloned());
            return Ok(());
        };
        let Some(slot) = self.index.get(c) else {
            return Ok(());
        };
        let index = slot.get_or_init(|| {
            let mut m: HashMap<Value, Vec<usize>> = HashMap::new();
            for (i, r) in self.rows.iter().enumerate() {
                if let Some(v) = r.values.get(c) {
                    m.entry(v.clone()).or_default().push(i);
                }
            }
            m
        });
        if let Some(hits) = index.get(&key.to_owned()) {
            out.extend(
                hits.iter()
                    .filter_map(|&i| self.rows.get(i))
                    .filter(|r| agrees(r))
                    .cloned(),
            );
        }
        Ok(())
    }
}

/// The [`CodeFacts`] with the model's EDB as domain builtins: the source
/// [`REFLEXION`] evaluates over.
struct ModelFacts<'f, 'a> {
    facts: &'f CodeFacts<'a>,
    relations: Vec<ModelRelation>,
}

impl GraphSource for ModelFacts<'_, '_> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.facts.node_ids()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        self.facts.edges()
    }
}

impl FactSource for ModelFacts<'_, '_> {
    fn tables(&self) -> &DomainTables {
        self.facts.tables()
    }

    fn node_kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.facts.node_kind(id)
    }

    fn node_cells(&self, id: NodeId) -> &[Cell] {
        self.facts.node_cells(id)
    }

    fn render_node(&self, id: NodeId) -> String {
        self.facts.render_node(id)
    }

    fn attributes(&self) -> &[AttrDecl] {
        self.facts.attributes()
    }

    fn attr_rows(&self, attr: usize, out: &mut Vec<(NodeId, Value)>) {
        self.facts.attr_rows(attr, out);
    }

    fn edge_tier(&self, ordinal: usize, e: &Edge) -> Tier {
        self.facts.edge_tier(ordinal, e)
    }

    fn builtins(&self) -> Vec<&dyn Builtin> {
        let mut out = self.facts.builtins();
        out.extend(self.relations.iter().map(|r| r as &dyn Builtin));
        out
    }
}

/// The relations of one evaluation, read by signature.
struct ReadBack<'e, 's, 'f, 'a> {
    ev: &'e Evaluation<'s, ModelFacts<'f, 'a>>,
}

impl ReadBack<'_, '_, '_, '_> {
    fn rows(&self, sig: &str) -> Result<Vec<Row>, String> {
        self.ev.rows(sig).map_err(|e: RulesError| e.to_string())
    }

    /// A `(C1, C2)` relation as a set.
    fn pairs(&self, sig: &str) -> Result<BTreeSet<(usize, usize)>, String> {
        self.rows(sig)?
            .iter()
            .map(|r| Ok((index(r, 0)?, index(r, 1)?)))
            .collect()
    }
}

/// Column `c` of `r`, or the error naming the shape the program broke.
fn column(r: &Row, c: usize) -> Result<&Value, String> {
    r.values.get(c).ok_or_else(|| {
        format!(
            "reflexion: a row of {} columns has no column {c}",
            r.values.len()
        )
    })
}

fn node(r: &Row, c: usize) -> Result<NodeId, String> {
    match column(r, c)? {
        Value::Node(id) => Ok(*id),
        v => Err(format!("reflexion: column {c} is {v:?}, not a node")),
    }
}

fn index(r: &Row, c: usize) -> Result<usize, String> {
    match column(r, c)? {
        Value::Int(i) => {
            usize::try_from(*i).map_err(|_| format!("reflexion: column {c} is {i}, not an index"))
        }
        v => Err(format!("reflexion: column {c} is {v:?}, not an index")),
    }
}

fn text(r: &Row, c: usize) -> Result<String, String> {
    match column(r, c)? {
        Value::Str(s) => Ok(s.clone()),
        v => Err(format!("reflexion: column {c} is {v:?}, not a string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_rules::analysis::analyse;
    use glia_rules::source::{catalog_conflicts, catalog_of};

    fn empty() -> MergedGraph {
        MergedGraph::new(Vec::new())
    }

    fn no_model() -> ModelEdb {
        ModelEdb::declare(&[])
    }

    #[test]
    fn program_parses_and_analyses_clean() {
        let p = parsed().expect("reflexion.dl parses");
        let g = empty();
        let facts = CodeFacts::new(&g);
        let edb = no_model();
        let src = ModelFacts {
            facts: &facts,
            relations: model_relations(&facts, &edb, &[]),
        };
        assert!(
            catalog_conflicts(&src).is_empty(),
            "{:?}",
            catalog_conflicts(&src)
        );
        let an = analyse(p, &catalog_of(&src)).expect("reflexion.dl analyses");
        assert!(an.warnings.is_empty(), "{:?}", an.warnings);
        // Fifteen relations, every output among them, a comment line above each rule.
        let mut heads: Vec<String> = Vec::new();
        for r in &p.rules {
            let sig = r.head.signature();
            if !heads.contains(&sig) {
                heads.push(sig);
            }
        }
        assert_eq!(heads.len(), 15, "{heads:?}");
        for o in OUTPUTS {
            assert!(heads.iter().any(|h| h == o), "{o} is not defined");
        }
        let lines: Vec<&str> = REFLEXION.lines().collect();
        let mut rules = 0;
        for (i, l) in lines.iter().enumerate() {
            if l.trim().is_empty() || l.starts_with("//") {
                continue;
            }
            rules += 1;
            assert!(
                i > 0 && lines[i - 1].starts_with("//"),
                "no comment above `{l}`"
            );
        }
        assert_eq!(rules, p.rules.len());
    }

    #[test]
    fn next_rank_stops_at_the_last_rank() {
        let mut edb = no_model();
        for (c, rank) in [(0, 0), (1, 1), (2, u32::MAX)] {
            edb.layer_of.insert(
                c,
                LayerFact {
                    name: format!("l{c}"),
                    rank,
                    strict: true,
                },
            );
        }
        let g = empty();
        let facts = CodeFacts::new(&g);
        let rels = model_relations(&facts, &edb, &[]);
        let next = rels
            .iter()
            .find(|r| r.name == "next_rank")
            .expect("next_rank");
        let rows: Vec<&[Value]> = next.rows.iter().map(|r| r.values.as_slice()).collect();
        assert_eq!(
            rows,
            [
                &[Value::Int(0), Value::Int(1)][..],
                &[Value::Int(1), Value::Int(2)][..],
            ],
            "no row past u32::MAX"
        );
        let layers = rels
            .iter()
            .find(|r| r.name == "layer_of")
            .expect("layer_of");
        assert_eq!(
            layers.rows[2].values,
            [
                Value::Int(2),
                Value::Int(i64::from(u32::MAX)),
                string("strict")
            ]
        );
    }

    #[test]
    fn a_relation_answers_bound_and_free_calls() {
        let rel = ModelRelation::new(
            "allow",
            &[ColType::Int, ColType::Int, ColType::Str],
            false,
            vec![
                fact(vec![int(0), int(1), string("b")]),
                fact(vec![int(0), int(1), string("a")]),
                fact(vec![int(2), int(1), string("c")]),
            ],
        );
        let call = |args: &[Option<ValueRef<'_>>]| {
            let mut out = Vec::new();
            rel.call(args, &mut out).expect("a call");
            out.into_iter()
                .map(|r| r.values)
                .collect::<Vec<Vec<Value>>>()
        };
        assert_eq!(call(&[None, None, None]).len(), 3);
        assert_eq!(
            call(&[Some(ValueRef::Int(0)), None, None]),
            [
                vec![int(0), int(1), string("b")],
                vec![int(0), int(1), string("a")]
            ]
        );
        assert_eq!(
            call(&[None, Some(ValueRef::Int(1)), Some(ValueRef::Str("c"))]),
            [vec![int(2), int(1), string("c")]]
        );
        assert!(call(&[Some(ValueRef::Int(9)), None, None]).is_empty());
        let mut out = Vec::new();
        assert!(rel.call(&[None], &mut out).is_err(), "arity is checked");
    }

    #[test]
    fn allowed_label_prefers_the_smallest_allow_id() {
        let mut layer_of = BTreeMap::new();
        for (c, name, rank) in [(0, "ui", 0), (1, "core", 1)] {
            layer_of.insert(
                c,
                LayerFact {
                    name: name.to_string(),
                    rank,
                    strict: false,
                },
            );
        }
        let mut allow: BTreeMap<(usize, usize), Allows> = BTreeMap::new();
        allow.insert((0, 1), vec![("z".into(), None), ("a".into(), None)]);
        assert_eq!(
            allowed_label(&allow, &layer_of, (0, 1)).as_deref(),
            Some("a")
        );
        assert_eq!(
            allowed_label(&BTreeMap::new(), &layer_of, (0, 1)).as_deref(),
            Some("layer:ui>core")
        );
        assert_eq!(allowed_label(&allow, &layer_of, (1, 2)), None);
    }
}
