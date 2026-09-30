//! Domain profile (LD.14a): the per-domain dials the domain-agnostic layers
//! read, as data a domain declares in a `const` / `static`.
//!
//! Split in two so the query side never depends on the build side:
//! - [`DomainTables`] is the data half — no generics, const-constructible:
//!   the id registries, which nodes are entrypoints ([`EntryRule`]), which
//!   edges carry reachability / blast radius, the effect sinks
//!   ([`EffectSink`]), the activation weights with their named presets, and
//!   the integer community weights (CD.1c). Query consumers (blast radius,
//!   liveness, PPR seeding, communities) need only this, so a crate that
//!   cannot name the domain's graph type still reads it.
//! - [`DomainProfile<G, C>`] is the tables plus the domain's build passes
//!   ([`PassRegistry<G, C>`], LD.13), over the domain's graph `G` and build
//!   context `C`.
//!
//! Home: this crate depends only on `core`, so a profile's SHAPE cannot name
//! a domain id — each domain fills it from its own crate (the code domain's
//! tables are `code_domain::profile::CODE_TABLES`, its profile the engine's
//! `profile::CODE_PROFILE`).
//!
//! Every field is a `&'static` slice, `&str` or plain value — never a map — so
//! a domain builds the tables with a struct literal in a `const`. Neither
//! struct is `#[non_exhaustive]` on purpose: a new profile dimension must
//! force every domain to fill it.

use std::fmt;

use glia_core::{CellTypeId, EdgeCategoryId, NodeKindId};

use crate::ActivationConfig;
use crate::passes::{PassRegistry, PassReport};

// ============================================================================
// Registries
// ============================================================================

/// A domain's id registries: every node kind, edge category and cell type it
/// allocates, with its display name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registries {
    pub node_kinds: &'static [(NodeKindId, &'static str)],
    pub edge_categories: &'static [(EdgeCategoryId, &'static str)],
    pub cell_types: &'static [(CellTypeId, &'static str)],
}

fn name_of<T: PartialEq + Copy>(
    table: &'static [(T, &'static str)],
    id: T,
) -> Option<&'static str> {
    table.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
}

impl Registries {
    /// The node kind's name, or `None` when the domain does not register it.
    pub fn kind_name(&self, k: NodeKindId) -> Option<&'static str> {
        name_of(self.node_kinds, k)
    }

    /// The edge category's name, or `None` when the domain does not register it.
    pub fn category_name(&self, c: EdgeCategoryId) -> Option<&'static str> {
        name_of(self.edge_categories, c)
    }

    /// The cell type's name, or `None` when the domain does not register it.
    pub fn cell_name(&self, t: CellTypeId) -> Option<&'static str> {
        name_of(self.cell_types, t)
    }

    /// Ids and names unique within each registry; names non-empty.
    fn check(&self, errors: &mut Vec<String>) {
        check_registry("node kind", self.node_kinds.iter().map(|(i, n)| (i.0, *n)), errors);
        check_registry(
            "edge category",
            self.edge_categories.iter().map(|(i, n)| (i.0, *n)),
            errors,
        );
        check_registry("cell type", self.cell_types.iter().map(|(i, n)| (i.0, *n)), errors);
    }
}

fn check_registry(
    what: &str,
    rows: impl Iterator<Item = (u32, &'static str)>,
    errors: &mut Vec<String>,
) {
    let rows: Vec<(u32, &str)> = rows.collect();
    for (i, (id, name)) in rows.iter().enumerate() {
        if name.is_empty() {
            errors.push(format!("{what} {id} is registered with an empty name"));
        }
        if rows[..i].iter().any(|(prior, _)| prior == id) {
            errors.push(format!("{what} {id} is registered more than once"));
        }
        if !name.is_empty() && rows[..i].iter().any(|(_, prior)| prior == name) {
            errors.push(format!("{what} name {name:?} is registered more than once"));
        }
    }
}

// ============================================================================
// Entrypoints
// ============================================================================

/// Nodes of `kinds` that are entrypoints by name: the name equals one of
/// `exact`, or starts with one of `prefixes` (case-sensitive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedEntry {
    pub kinds: &'static [NodeKindId],
    pub exact: &'static [&'static str],
    pub prefixes: &'static [&'static str],
}

impl NamedEntry {
    /// Does this rule make a `kind` node called `name` an entrypoint?
    pub fn matches(&self, kind: NodeKindId, name: &str) -> bool {
        self.kinds.contains(&kind)
            && (self.exact.contains(&name) || self.prefixes.iter().any(|p| name.starts_with(p)))
    }
}

/// Which nodes are entrypoints: externally-triggered roots that liveness
/// seeds from.
///
/// A node is an entry when its kind is one of `kinds` (whatever its name), one
/// of its roles is one of `roles`, or a [`NamedEntry`] matches its kind and
/// name. Roles are kinds a node carries beside its own (the code domain's
/// ROLE cells, LB.3a): an Angular `@Component` is a CLASS with the COMPONENT
/// role, and must be as live as a COMPONENT node (LB.3b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryRule {
    pub kinds: &'static [NodeKindId],
    pub roles: &'static [NodeKindId],
    pub named: &'static [NamedEntry],
}

impl EntryRule {
    /// Is a node of `kind` called `name`, carrying `roles`, an entrypoint?
    /// Pass `&[]` for `roles` to ask whether kind and name alone make it one.
    /// A node with no known kind is an entry only through a role.
    pub fn is_entry(&self, kind: Option<NodeKindId>, name: &str, roles: &[NodeKindId]) -> bool {
        if roles.iter().any(|r| self.roles.contains(r)) {
            return true;
        }
        let Some(kind) = kind else {
            return false;
        };
        self.kinds.contains(&kind) || self.named.iter().any(|n| n.matches(kind, name))
    }
}

// ============================================================================
// Effect sinks
// ============================================================================

/// One class of external effect (LE.4d): a walk that reaches a node of one of
/// `kinds` over an edge of one of `via` has that effect (`db`, `queue_produce`,
/// `http_call`, ...), and stops there.
///
/// A sink is the PAIR, never a category alone: a category is also emitted for
/// uses that are not effects (the code domain's USES links a function to any
/// symbol it names, not only the queue producer it sends through), and a
/// producer with no consumer in the build has no flow edge at all, so only the
/// target kind together with the reaching category names the effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectSink {
    /// The effect's name, unique within the table.
    pub class: &'static str,
    pub kinds: &'static [NodeKindId],
    pub via: &'static [EdgeCategoryId],
}

impl EffectSink {
    /// Is reaching a `kind` node over a `via` edge this effect?
    pub fn matches(&self, kind: NodeKindId, via: EdgeCategoryId) -> bool {
        self.kinds.contains(&kind) && self.via.contains(&via)
    }
}

// ============================================================================
// Activation presets
// ============================================================================

/// A named, task-tuned lens over the base activation weights: each override
/// replaces (or adds) one category's weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActivationPreset {
    pub name: &'static str,
    pub overrides: &'static [(EdgeCategoryId, f64)],
}

// ============================================================================
// Tables
// ============================================================================

/// The data half of a domain profile: everything a query needs, no generics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomainTables {
    /// The graph-type tag of the domain's node ids (`"code"`); also the
    /// `domain=` value of the `[passes]` marker.
    pub graph_type: &'static str,
    pub registries: Registries,
    pub entry: EntryRule,
    /// The semantic edges reachability and blast radius follow, in declared
    /// order (structural containment / import fan-out stays out).
    pub carry_edges: &'static [EdgeCategoryId],
    /// The external effects a node can have (data access, a queue, an
    /// outbound call, an event, ...), each a (target kind, reaching category)
    /// class; the first match in table order classifies. Empty is legal: a
    /// domain with no effects.
    pub effect_sinks: &'static [EffectSink],
    /// Base PPR weight per edge category; a category not listed weighs 1.0
    /// (the [`ActivationConfig`] default).
    pub activation_weights: &'static [(EdgeCategoryId, f64)],
    pub activation_presets: &'static [ActivationPreset],
    /// Integer weight per edge category for grouping: communities and split
    /// cuts build their [`WeightedGraph`](crate::algo::community::WeightedGraph)
    /// from it (CD.1c). A category not listed weighs 0 and is left out of
    /// every grouping, so a listed weight is `>= 1`. Apart from
    /// `activation_weights` on purpose: those are PPR ranking dials, and a
    /// ranking retune must not move community structure.
    pub community_weights: &'static [(EdgeCategoryId, u32)],
}

impl DomainTables {
    /// The preset called `name`, if the domain declares one.
    pub fn preset(&self, name: &str) -> Option<&'static ActivationPreset> {
        self.activation_presets.iter().find(|p| p.name == name)
    }

    /// [`ActivationConfig::default`] with the base weights, then the named
    /// preset's overrides. `None`, or a name no preset has, is the base.
    pub fn activation_config(&self, preset: Option<&str>) -> ActivationConfig {
        let mut config = ActivationConfig::default();
        config.edge_weights.extend(self.activation_weights.iter().copied());
        if let Some(p) = preset.and_then(|name| self.preset(name)) {
            config.edge_weights.extend(p.overrides.iter().copied());
        }
        config
    }

    /// Does reachability / blast radius follow edges of category `c`?
    pub fn carries(&self, c: EdgeCategoryId) -> bool {
        self.carry_edges.contains(&c)
    }

    /// Category `c`'s community weight: its `community_weights` row (the
    /// first, as `WeightedGraph::from_source` reads it), or 0 - left out of
    /// every grouping - when it has none. A linear find: the table holds at
    /// most one row per registered category.
    pub fn community_weight(&self, c: EdgeCategoryId) -> u32 {
        self.community_weights.iter().find(|(k, _)| *k == c).map_or(0, |(_, w)| *w)
    }

    /// The effect sink (with its table index) that reaching a `kind` node over
    /// a `via` edge is, first match in table order; `None` when it is no
    /// effect.
    pub fn effect_sink(
        &self,
        kind: NodeKindId,
        via: EdgeCategoryId,
    ) -> Option<(usize, &'static EffectSink)> {
        self.effect_sinks.iter().enumerate().find(|(_, s)| s.matches(kind, via))
    }

    /// Every problem with the tables, or `Ok`: the registries hold unique ids
    /// and names; every id the entry rule, carry edges, effect sinks, weights
    /// and presets name is registered, and none is listed twice in one list;
    /// every weight is finite and `>= 0`; preset names are non-empty and
    /// unique; a named entry can match something; effect sink classes are
    /// non-empty and unique, each sink names a kind and a category, and no
    /// (kind, category) pair belongs to two sinks (the first would hide the
    /// second); every `community_weights` row names a registered category
    /// no earlier row names, with a weight `>= 1` (a 0 row is refused as
    /// ambiguous: leaving a category out already weighs it 0). Each
    /// `community_weights` error names its row, `community_weights[i]`.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.graph_type.is_empty() {
            errors.push("graph_type is empty".to_string());
        }
        let r = &self.registries;
        r.check(&mut errors);

        let kinds = |field: &str, ids: &[NodeKindId], errors: &mut Vec<String>| {
            for (i, k) in ids.iter().enumerate() {
                if r.kind_name(*k).is_none() {
                    errors
                        .push(format!("{field} holds node kind {}, which is not registered", k.0));
                }
                if ids[..i].contains(k) {
                    errors.push(format!("{field} lists node kind {} more than once", k.0));
                }
            }
        };
        kinds("entry.kinds", self.entry.kinds, &mut errors);
        kinds("entry.roles", self.entry.roles, &mut errors);
        for (i, n) in self.entry.named.iter().enumerate() {
            kinds(&format!("entry.named[{i}].kinds"), n.kinds, &mut errors);
            if n.kinds.is_empty() || (n.exact.is_empty() && n.prefixes.is_empty()) {
                errors.push(format!("entry.named[{i}] matches no node"));
            }
            if n.prefixes.contains(&"") {
                errors.push(format!(
                    "entry.named[{i}] has an empty prefix, which matches every name: list its kinds in entry.kinds"
                ));
            }
        }

        let categories = |field: &str,
                          ids: &mut dyn Iterator<Item = EdgeCategoryId>,
                          errors: &mut Vec<String>| {
            let mut seen: Vec<EdgeCategoryId> = Vec::new();
            for c in ids {
                if r.category_name(c).is_none() {
                    errors.push(format!(
                        "{field} holds edge category {}, which is not registered",
                        c.0
                    ));
                }
                if seen.contains(&c) {
                    errors.push(format!("{field} lists edge category {} more than once", c.0));
                }
                seen.push(c);
            }
        };
        categories("carry_edges", &mut self.carry_edges.iter().copied(), &mut errors);
        for (i, sink) in self.effect_sinks.iter().enumerate() {
            let field = format!("effect_sinks[{i}]");
            if sink.class.is_empty() {
                errors.push(format!("{field} has an empty class"));
            }
            if self.effect_sinks[..i].iter().any(|s| s.class == sink.class) {
                errors.push(format!("effect sink class {:?} is declared more than once", sink.class));
            }
            if sink.kinds.is_empty() || sink.via.is_empty() {
                errors.push(format!("{field} matches no node: it needs a kind and a category"));
            }
            kinds(&format!("{field}.kinds"), sink.kinds, &mut errors);
            categories(&format!("{field}.via"), &mut sink.via.iter().copied(), &mut errors);
            for (j, prior) in self.effect_sinks[..i].iter().enumerate() {
                let shared = sink.kinds.iter().find_map(|k| {
                    sink.via.iter().find(|c| prior.matches(*k, **c)).map(|c| (k, c))
                });
                if let Some((k, c)) = shared {
                    errors.push(format!(
                        "{field} ({:?}) overlaps effect_sinks[{j}] ({:?}) on node kind {} over edge category {}: the first would hide the second",
                        sink.class, prior.class, k.0, c.0
                    ));
                }
            }
        }

        let weights = |field: &str, rows: &[(EdgeCategoryId, f64)], errors: &mut Vec<String>| {
            categories(field, &mut rows.iter().map(|(c, _)| *c), errors);
            for (c, w) in rows {
                if !w.is_finite() || *w < 0.0 {
                    errors.push(format!(
                        "{field} weighs edge category {} {w}: a weight is finite and >= 0",
                        c.0
                    ));
                }
            }
        };
        weights("activation_weights", self.activation_weights, &mut errors);
        for (i, p) in self.activation_presets.iter().enumerate() {
            if p.name.is_empty() {
                errors.push(format!("activation_presets[{i}] has an empty name"));
            }
            if self.activation_presets[..i].iter().any(|q| q.name == p.name) {
                errors.push(format!("activation preset {:?} is declared more than once", p.name));
            }
            weights(&format!("activation preset {:?}", p.name), p.overrides, &mut errors);
        }

        for (i, (c, w)) in self.community_weights.iter().enumerate() {
            let field = format!("community_weights[{i}]");
            if r.category_name(*c).is_none() {
                errors
                    .push(format!("{field} holds edge category {}, which is not registered", c.0));
            }
            if self.community_weights[..i].iter().any(|(prior, _)| prior == c) {
                errors.push(format!("{field} lists edge category {} more than once", c.0));
            }
            if *w == 0 {
                errors.push(format!(
                    "{field} weighs edge category {} 0: a community weight is >= 1, and a category left out weighs 0",
                    c.0
                ));
            }
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

// ============================================================================
// Profile
// ============================================================================

/// A domain's whole profile: its [`DomainTables`] and the build passes that
/// turn its assembled graph `G` into the stored one, given build context `C`.
pub struct DomainProfile<G: 'static, C: 'static = ()> {
    pub tables: DomainTables,
    pub passes: PassRegistry<G, C>,
}

// Manual impls, as for `PassRegistry`: a derive would demand `G` / `C` bounds.
impl<G: 'static, C: 'static> Clone for DomainProfile<G, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<G: 'static, C: 'static> Copy for DomainProfile<G, C> {}

impl<G: 'static, C: 'static> fmt::Debug for DomainProfile<G, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainProfile")
            .field("tables", &self.tables)
            .field("passes", &self.passes)
            .finish()
    }
}

impl<G: 'static, C: 'static> DomainProfile<G, C> {
    /// The passes that write node cells, with the cell types each writes, in
    /// run order.
    pub fn cell_populators(&self) -> Vec<(&'static str, &'static [CellTypeId])> {
        self.passes
            .order()
            .into_iter()
            .filter(|s| !s.populates.is_empty())
            .map(|s| (s.name, s.populates))
            .collect()
    }

    /// Run the domain's passes over `g` (see [`PassRegistry::run`]).
    ///
    /// fired_on marker, once per call (grep token `[passes] domain=`):
    ///   `[passes] domain=<graph_type> resolve=<r> post=<p> finalize=<f>`
    pub fn run_passes(&self, g: &mut G, ctx: &C) -> PassReport {
        let report = self.passes.run(g, ctx);
        eprintln!(
            "[passes] domain={} resolve={} post={} finalize={}",
            self.tables.graph_type, report.resolve, report.post, report.finalize
        );
        report
    }

    /// Every problem with the profile, or `Ok`: [`DomainTables::validate`],
    /// [`PassRegistry::validate`], and every cell type a pass populates is
    /// registered.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = self.tables.validate().err().unwrap_or_default();
        if let Err(e) = self.passes.validate() {
            errors.extend(e);
        }
        for spec in self.passes.specs() {
            for t in spec.populates {
                if self.tables.registries.cell_name(*t).is_none() {
                    errors.push(format!(
                        "pass {:?} populates cell type {}, which is not registered",
                        spec.name, t.0
                    ));
                }
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::{PassSpec, Stage};
    use crate::{Direction, Specificity};

    const ROUTE: NodeKindId = NodeKindId(1);
    const FUNCTION: NodeKindId = NodeKindId(2);
    const METHOD: NodeKindId = NodeKindId(3);
    const CLASS: NodeKindId = NodeKindId(4);
    const COMPONENT: NodeKindId = NodeKindId(5);
    const CALLS: EdgeCategoryId = EdgeCategoryId(1);
    const IMPORTS: EdgeCategoryId = EdgeCategoryId(2);
    const CONTAINS: EdgeCategoryId = EdgeCategoryId(3);
    const ORIGIN: CellTypeId = CellTypeId(1);
    const ROLE: CellTypeId = CellTypeId(2);

    const TABLES: DomainTables = DomainTables {
        graph_type: "toy",
        registries: Registries {
            node_kinds: &[
                (ROUTE, "ROUTE"),
                (FUNCTION, "FUNCTION"),
                (METHOD, "METHOD"),
                (CLASS, "CLASS"),
                (COMPONENT, "COMPONENT"),
            ],
            edge_categories: &[(CALLS, "CALLS"), (IMPORTS, "IMPORTS"), (CONTAINS, "CONTAINS")],
            cell_types: &[(ORIGIN, "ORIGIN"), (ROLE, "ROLE")],
        },
        entry: EntryRule {
            kinds: &[ROUTE],
            roles: &[COMPONENT],
            named: &[NamedEntry {
                kinds: &[FUNCTION, METHOD],
                exact: &["main"],
                prefixes: &["test"],
            }],
        },
        carry_edges: &[CALLS],
        effect_sinks: &[],
        activation_weights: &[(CALLS, 5.0), (IMPORTS, 3.0)],
        activation_presets: &[
            ActivationPreset { name: "repair", overrides: &[(CALLS, 8.0), (CONTAINS, 2.0)] },
            ActivationPreset { name: "wide", overrides: &[(IMPORTS, 0.0)] },
        ],
        community_weights: &[(CALLS, 3), (IMPORTS, 1)],
    };

    #[test]
    fn entry_rule_kind_exact_and_prefix() {
        let e = TABLES.entry;
        // Any name on an entry kind.
        assert!(e.is_entry(Some(ROUTE), "", &[]));
        assert!(e.is_entry(Some(ROUTE), "GET /users", &[]));
        // Named kinds: exact or prefix, case-sensitive.
        assert!(e.is_entry(Some(FUNCTION), "main", &[]));
        assert!(e.is_entry(Some(METHOD), "test_login", &[]));
        assert!(e.is_entry(Some(METHOD), "tester", &[]));
        assert!(!e.is_entry(Some(FUNCTION), "Main", &[]));
        assert!(!e.is_entry(Some(FUNCTION), "mainly", &[]));
        assert!(!e.is_entry(Some(FUNCTION), "Test", &[]));
        assert!(!e.is_entry(Some(FUNCTION), "", &[]));
        // The name rule belongs to its kinds only.
        assert!(!e.is_entry(Some(CLASS), "main", &[]));
        // A role makes an entry whatever the kind, even an unknown one.
        assert!(e.is_entry(Some(CLASS), "Page", &[COMPONENT]));
        assert!(e.is_entry(None, "Page", &[CLASS, COMPONENT]));
        assert!(!e.is_entry(Some(CLASS), "Page", &[CLASS]));
        // No kind, no role: never an entry.
        assert!(!e.is_entry(None, "main", &[]));
    }

    #[test]
    fn activation_config_applies_weights_then_preset() {
        let base = TABLES.activation_config(None);
        let d = ActivationConfig::default();
        assert_eq!(base.edge_weights.len(), 2);
        assert_eq!(base.edge_weights.get(&CALLS), Some(&5.0));
        assert_eq!(base.edge_weights.get(&IMPORTS), Some(&3.0));
        assert_eq!(
            (base.damping, base.direction, base.node_specificity),
            (d.damping, Direction::Forward, Specificity::None)
        );
        assert_eq!(
            (base.top_k, base.max_iterations, base.epsilon),
            (d.top_k, d.max_iterations, d.epsilon)
        );

        let repair = TABLES.activation_config(Some("repair"));
        assert_eq!(repair.edge_weights.len(), 3);
        assert_eq!(repair.edge_weights.get(&CALLS), Some(&8.0), "an override replaces");
        assert_eq!(repair.edge_weights.get(&IMPORTS), Some(&3.0), "the rest stays base");
        assert_eq!(repair.edge_weights.get(&CONTAINS), Some(&2.0), "an override adds");
        assert_eq!(TABLES.activation_config(Some("wide")).edge_weights.get(&IMPORTS), Some(&0.0));

        // An unknown preset is the base, as None is.
        for p in [Some("nonsense"), Some(""), Some("Repair")] {
            assert_eq!(TABLES.activation_config(p).edge_weights, base.edge_weights, "{p:?}");
        }
        assert!(TABLES.preset("repair").is_some() && TABLES.preset("nonsense").is_none());
        assert!(TABLES.carries(CALLS) && !TABLES.carries(IMPORTS));
    }

    #[test]
    fn validate_flags_unregistered_ids() {
        assert_eq!(TABLES.validate(), Ok(()));

        let bad = DomainTables { carry_edges: &[CALLS, EdgeCategoryId(999)], ..TABLES };
        let errors = bad.validate().unwrap_err();
        assert_eq!(errors, ["carry_edges holds edge category 999, which is not registered"]);

        let bad = DomainTables {
            graph_type: "",
            entry: EntryRule {
                kinds: &[ROUTE, NodeKindId(77), ROUTE],
                roles: &[NodeKindId(78)],
                named: &[NamedEntry { kinds: &[FUNCTION], exact: &[], prefixes: &[""] }],
            },
            effect_sinks: &[EffectSink { class: "db", kinds: &[ROUTE], via: &[EdgeCategoryId(55)] }],
            activation_weights: &[(CALLS, -1.0), (IMPORTS, f64::NAN), (CALLS, 2.0)],
            activation_presets: &[
                ActivationPreset { name: "p", overrides: &[(EdgeCategoryId(66), 1.0)] },
                ActivationPreset { name: "p", overrides: &[] },
            ],
            ..TABLES
        };
        let errors = bad.validate().unwrap_err();
        let expect = [
            "graph_type is empty",
            "entry.kinds holds node kind 77, which is not registered",
            "entry.kinds lists node kind 1 more than once",
            "entry.roles holds node kind 78, which is not registered",
            "entry.named[0] has an empty prefix, which matches every name: list its kinds in entry.kinds",
            "effect_sinks[0].via holds edge category 55, which is not registered",
            "activation_weights lists edge category 1 more than once",
            "activation_weights weighs edge category 1 -1: a weight is finite and >= 0",
            "activation_weights weighs edge category 2 NaN: a weight is finite and >= 0",
            "activation preset \"p\" holds edge category 66, which is not registered",
            "activation preset \"p\" is declared more than once",
        ];
        assert_eq!(errors, expect);

        let dup = DomainTables {
            registries: Registries {
                node_kinds: &[(ROUTE, "ROUTE"), (ROUTE, "ROUTE2"), (FUNCTION, "ROUTE")],
                ..TABLES.registries
            },
            entry: EntryRule { kinds: &[], roles: &[], named: &[] },
            ..TABLES
        };
        assert_eq!(
            dup.validate().unwrap_err(),
            [
                "node kind 1 is registered more than once",
                "node kind name \"ROUTE\" is registered more than once"
            ]
        );
    }

    #[test]
    fn effect_sinks_classify_and_validate() {
        const SINKS: &[EffectSink] = &[
            EffectSink { class: "call", kinds: &[ROUTE], via: &[CALLS] },
            EffectSink { class: "import", kinds: &[ROUTE, CLASS], via: &[IMPORTS] },
        ];
        let tables = DomainTables { effect_sinks: SINKS, ..TABLES };
        assert_eq!(tables.validate(), Ok(()));
        // The pair classifies: the kind alone or the category alone does not.
        assert_eq!(tables.effect_sink(ROUTE, CALLS).map(|(i, s)| (i, s.class)), Some((0, "call")));
        assert_eq!(tables.effect_sink(CLASS, IMPORTS).map(|(i, s)| (i, s.class)), Some((1, "import")));
        assert!(tables.effect_sink(CLASS, CALLS).is_none());
        assert!(tables.effect_sink(FUNCTION, IMPORTS).is_none());
        assert!(TABLES.effect_sink(ROUTE, CALLS).is_none(), "an empty table has no effects");

        // An unregistered kind is rejected, as an unregistered category is.
        let bad = DomainTables {
            effect_sinks: &[EffectSink { class: "db", kinds: &[NodeKindId(77), ROUTE], via: &[CALLS] }],
            ..TABLES
        };
        assert_eq!(
            bad.validate().unwrap_err(),
            ["effect_sinks[0].kinds holds node kind 77, which is not registered"]
        );

        let bad = DomainTables {
            effect_sinks: &[
                EffectSink { class: "", kinds: &[], via: &[CALLS] },
                EffectSink { class: "db", kinds: &[ROUTE, ROUTE], via: &[CALLS, CALLS] },
                EffectSink { class: "db", kinds: &[ROUTE], via: &[CALLS] },
            ],
            ..TABLES
        };
        assert_eq!(
            bad.validate().unwrap_err(),
            [
                "effect_sinks[0] has an empty class",
                "effect_sinks[0] matches no node: it needs a kind and a category",
                "effect_sinks[1].kinds lists node kind 1 more than once",
                "effect_sinks[1].via lists edge category 1 more than once",
                "effect sink class \"db\" is declared more than once",
                "effect_sinks[2] (\"db\") overlaps effect_sinks[1] (\"db\") on node kind 1 over edge category 1: the first would hide the second",
            ]
        );
    }

    #[test]
    fn validate_rejects_bad_community_weights() {
        let rows: [(&[(EdgeCategoryId, u32)], &str); 3] = [
            (
                &[(CALLS, 2), (EdgeCategoryId(999), 1)],
                "community_weights[1] holds edge category 999, which is not registered",
            ),
            (
                &[(CALLS, 2), (IMPORTS, 1), (CALLS, 3)],
                "community_weights[2] lists edge category 1 more than once",
            ),
            (
                &[(IMPORTS, 0), (CALLS, 2)],
                "community_weights[0] weighs edge category 2 0: a community weight is >= 1, and a category left out weighs 0",
            ),
        ];
        for (weights, error) in rows {
            let bad = DomainTables { community_weights: weights, ..TABLES };
            assert_eq!(bad.validate().unwrap_err(), [error], "{weights:?}");
        }
        // Each problem is reported against its own row, in row order.
        let bad = DomainTables {
            community_weights: &[(CALLS, 0), (EdgeCategoryId(7), 2), (CALLS, 1)],
            ..TABLES
        };
        assert_eq!(
            bad.validate().unwrap_err(),
            [
                "community_weights[0] weighs edge category 1 0: a community weight is >= 1, and a category left out weighs 0",
                "community_weights[1] holds edge category 7, which is not registered",
                "community_weights[2] lists edge category 1 more than once",
            ]
        );
        // No rows is a domain that groups nothing: legal.
        assert_eq!(DomainTables { community_weights: &[], ..TABLES }.validate(), Ok(()));
    }

    #[test]
    fn community_weight_defaults_to_zero() {
        assert_eq!(TABLES.community_weight(CALLS), 3);
        assert_eq!(TABLES.community_weight(IMPORTS), 1);
        assert_eq!(TABLES.community_weight(CONTAINS), 0, "registered, not listed");
        assert_eq!(TABLES.community_weight(EdgeCategoryId(999)), 0, "not registered");
        let none = DomainTables { community_weights: &[], ..TABLES };
        assert_eq!(none.community_weight(CALLS), 0);
        // Independent of the PPR weights: IMPORTS weighs 3.0 there and 1
        // here, and `repair` adds CONTAINS at 2.0 there while it stays 0 here.
        assert_eq!(TABLES.activation_config(None).edge_weights.get(&IMPORTS), Some(&3.0));
        assert_eq!(
            TABLES.activation_config(Some("repair")).edge_weights.get(&CONTAINS),
            Some(&2.0)
        );
        assert_eq!(TABLES.community_weight(CONTAINS), 0);
    }

    /// A toy graph: the passes that touched it, in order.
    type Log = Vec<&'static str>;

    const fn pass(
        name: &'static str,
        stage: Stage,
        populates: &'static [CellTypeId],
    ) -> PassSpec<Log> {
        PassSpec { name, stage, after: &[], populates, run: |g, _| g.push("ran") }
    }

    #[test]
    fn cell_populators_lists_populating_passes_in_run_order() {
        const PROFILE: DomainProfile<Log> = DomainProfile {
            tables: TABLES,
            passes: PassRegistry::new(&[
                pass("sort", Stage::Finalize, &[]),
                pass("tag_roles", Stage::Post, &[ROLE]),
                pass("pair", Stage::Resolve, &[]),
                pass("tag_origin", Stage::Resolve, &[ORIGIN, ROLE]),
            ]),
        };
        assert_eq!(PROFILE.validate(), Ok(()));
        assert_eq!(
            PROFILE.cell_populators(),
            [("tag_origin", &[ORIGIN, ROLE][..]), ("tag_roles", &[ROLE][..])]
        );
        let mut g: Log = Vec::new();
        let report = PROFILE.run_passes(&mut g, &());
        assert_eq!(report.ran, ["pair", "tag_origin", "tag_roles", "sort"]);
        assert_eq!((report.resolve, report.post, report.finalize), (2, 1, 1));
        assert_eq!(g.len(), 4);

        let none: DomainProfile<Log> =
            DomainProfile { tables: TABLES, passes: PassRegistry::empty() };
        assert!(none.cell_populators().is_empty());
    }

    #[test]
    fn profile_validate_flags_passes_and_unregistered_populates() {
        const BAD: DomainProfile<Log> = DomainProfile {
            tables: DomainTables { carry_edges: &[EdgeCategoryId(999)], ..TABLES },
            passes: PassRegistry::new(&[
                pass("tag", Stage::Post, &[CellTypeId(42)]),
                pass("tag", Stage::Post, &[]),
            ]),
        };
        assert_eq!(
            BAD.validate().unwrap_err(),
            [
                "carry_edges holds edge category 999, which is not registered",
                "pass name \"tag\" is declared more than once",
                "pass \"tag\" populates cell type 42, which is not registered",
            ]
        );
    }
}
