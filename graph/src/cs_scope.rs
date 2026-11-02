//! C# name lookup for a heritage base (HQ.13). A C# base clause names its
//! base by a simple name (`class TeamDomainObject : DomainObject<TeamState>`;
//! the parser keeps `DomainObject`), which C# resolves by simple-name lookup
//! (ECMA-334, "Namespace and type names") in the scope ENCLOSING the declaring
//! type, never through the type's own members:
//!
//! 1. the nested types of each enclosing type, innermost first
//!    (`MongoFieldTests.Inherited : Entity` reads the private nested
//!    `MongoFieldTests.Entity`), rule `nested_type`;
//! 2. the members of the declaring type's namespace, then of each enclosing
//!    namespace (`Shop::Teams::Domain`, `Shop::Teams`, `Shop`): a namespace's
//!    members hide every using, rule `enclosing_namespace`;
//! 3. the file's `using` directives plus every `global using` of the build
//!    group ([`NavFact::CsUsing`], recorded by the C# parser): a namespace
//!    using's members (rule `using_namespace`) and a static using's nested
//!    types (rule `using_static`). Two distinct hits bind nothing (C#'s
//!    CS0104). An alias using is the import binding's: `decide_ref`'s
//!    ImportBinding arm reads it first.
//!
//! When none of them holds the name, the HQ.1 repo-unique fallback still
//! runs, except in a file that opens a namespace the repo does not declare
//! while the repo-unique candidate sits in a declared namespace: that base is
//! the external namespace's type as far as the file says (`using
//! Vendor.ClientLibrary;` + `: Content<T>` beside an in-repo
//! `Shop.Core.Model.Content`), so no name guess binds it
//! ([`CsLookup::RefusedExternal`]).
//!
//! Crate-private: [`crate::calls`]' `decide_ref` asks [`CsScopes::lookup`]
//! for every Bare INHERITS_FROM / IMPLEMENTS ref read in a `.cs` file that
//! neither the file's import bindings nor its own symbols bound. Built once
//! per `resolve_refs` call, on the first C# heritage ref, beside the pass's
//! repo-unique name indexes. Everything read is a BTree or a looked-up map,
//! and every answer is one node or a refusal, so no HashMap order shows.
//!
//! Not read: MSBuild `<Using Include>` / `<ImplicitUsings>` (a project-level
//! using); generic arity (`Content<T>` vs a non-generic `Content`: the parser
//! drops type arguments from a Bare base). Block-namespace usings are the
//! file's (LA.40a), and global usings apply to the whole build group: both
//! can only widen the scope, never refuse.
//!
//! fired_on, once per graph build with a C# heritage ref that reached the
//! lookup: `[cs-scope] heritage refs=<n> nested=<k> enclosing=<e> using=<u>
//! static=<s> ambiguous=<a> refused_external=<x> fallback=<f>`.

use glia_code_domain::{CsUsingForm, NavFact, UnresolvedRef, node_kind};
use glia_core::NodeId;

use crate::imports::{NamespaceIndex, NsHit};
use crate::types::RepoGraph;

/// Which C# lookup step bound a base ([`CsLookup::Bound`]); `decide_ref`
/// writes it as the edge's EVIDENCE rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CsRule {
    /// A nested type of an enclosing type (`nested_type`).
    NestedType,
    /// A member of the declaring type's own or an enclosing namespace
    /// (`enclosing_namespace`).
    EnclosingNamespace,
    /// A member of a namespace the file (or a global using) opens
    /// (`using_namespace`).
    UsingNamespace,
    /// A nested type of a `using static` type (`using_static`).
    UsingStatic,
}

/// What [`CsScopes::lookup`] found for one base name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CsLookup {
    /// The one type the name denotes in the base clause's scope.
    Bound(NodeId, CsRule),
    /// Two distinct types at the first scope level that has the name (two
    /// nested types, two members of one namespace, or members of two opened
    /// namespaces): nothing binds.
    Ambiguous,
    /// No scope level holds the name, the file opens a namespace the repo
    /// does not declare, and the repo-unique candidate sits in a declared
    /// namespace: nothing binds, no name guess.
    RefusedExternal,
    /// No scope level holds the name: the repo-unique fallback runs.
    NotFound,
}

/// The `[cs-scope] heritage ..` counters, merged across the pooled chunks of
/// one `resolve_refs` call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CsScopeCounts {
    /// C# Bare heritage refs that reached the lookup.
    pub(crate) refs: usize,
    pub(crate) nested: usize,
    pub(crate) enclosing: usize,
    pub(crate) using: usize,
    pub(crate) static_: usize,
    pub(crate) ambiguous: usize,
    pub(crate) refused_external: usize,
    /// [`CsLookup::NotFound`] refs handed to the repo-unique fallback.
    pub(crate) fallback: usize,
}

impl CsScopeCounts {
    /// Count one lookup's answer.
    pub(crate) fn note(&mut self, hit: CsLookup) {
        self.refs += 1;
        let slot = match hit {
            CsLookup::Bound(_, CsRule::NestedType) => &mut self.nested,
            CsLookup::Bound(_, CsRule::EnclosingNamespace) => &mut self.enclosing,
            CsLookup::Bound(_, CsRule::UsingNamespace) => &mut self.using,
            CsLookup::Bound(_, CsRule::UsingStatic) => &mut self.static_,
            CsLookup::Ambiguous => &mut self.ambiguous,
            CsLookup::RefusedExternal => &mut self.refused_external,
            CsLookup::NotFound => &mut self.fallback,
        };
        *slot += 1;
    }

    /// Add a later chunk's counters.
    pub(crate) fn merge(&mut self, other: CsScopeCounts) {
        self.refs += other.refs;
        self.nested += other.nested;
        self.enclosing += other.enclosing;
        self.using += other.using;
        self.static_ += other.static_;
        self.ambiguous += other.ambiguous;
        self.refused_external += other.refused_external;
        self.fallback += other.fallback;
    }

    /// The fired_on line, `None` when no C# heritage ref reached the lookup.
    pub(crate) fn marker(&self) -> Option<String> {
        (self.refs > 0).then(|| {
            format!(
                "[cs-scope] heritage refs={} nested={} enclosing={} using={} static={} \
                 ambiguous={} refused_external={} fallback={}",
                self.refs,
                self.nested,
                self.enclosing,
                self.using,
                self.static_,
                self.ambiguous,
                self.refused_external,
                self.fallback
            )
        })
    }
}

/// The C# scopes of one graph: its namespaces and the group's global usings.
/// The file's own usings are read from its MODULE's nav facts at lookup.
pub(crate) struct CsScopes {
    index: NamespaceIndex,
    /// Every `global using` of any C# MODULE of the graph, deduped and
    /// sorted: C# applies one to every file of its project.
    globals: Vec<(CsUsingForm, String)>,
}

/// The kinds a C# base can be.
fn is_type(g: &RepoGraph, id: &NodeId) -> bool {
    matches!(
        g.nav.kind_by_id.get(id).copied(),
        Some(k) if k == node_kind::CLASS
            || k == node_kind::STRUCT
            || k == node_kind::INTERFACE
            || k == node_kind::ENUM
    )
}

/// The outermost type enclosing `id` (itself when it is no nested type).
fn outermost_type(g: &RepoGraph, id: NodeId) -> NodeId {
    let mut at = id;
    while let Some(parent) = g.nav.parent_of.get(&at).copied() {
        if !is_type(g, &parent) {
            break;
        }
        at = parent;
    }
    at
}

/// The namespace scope of type `id`: its outermost type's qname without the
/// last segment (`Shop::Teams` for `Shop::Teams::Team::Inner`); `None` for a
/// type with a one-segment qname.
fn namespace_of(g: &RepoGraph, id: NodeId) -> Option<&str> {
    let outer = outermost_type(g, id);
    g.nav
        .qname_by_id
        .get(&outer)
        .and_then(|q| q.rsplit_once("::"))
        .map(|(ns, _)| ns)
}

impl CsScopes {
    pub(crate) fn build(g: &RepoGraph) -> Self {
        let mut globals: Vec<(CsUsingForm, String)> = g
            .nav
            .nav_facts
            .values()
            .flatten()
            .filter_map(|f| match f {
                NavFact::CsUsing {
                    form, path, global, ..
                } if *global && *form != CsUsingForm::Alias => Some((*form, path.clone())),
                _ => None,
            })
            .collect();
        globals.sort();
        globals.dedup();
        CsScopes {
            index: NamespaceIndex::build(g),
            globals,
        }
    }

    /// The type children of `parent` named `name`, other than `skip`, each
    /// once.
    fn nested_named(g: &RepoGraph, parent: NodeId, name: &str, skip: NodeId) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = Vec::new();
        for child in g.nav.children_of.get(&parent).into_iter().flatten() {
            if *child != skip
                && is_type(g, child)
                && g.nav.name_by_id.get(child).map(String::as_str) == Some(name)
                && !out.contains(child)
            {
                out.push(*child);
            }
        }
        out
    }

    /// The type `path` names (`A::B::T`): the one type member `T` of
    /// namespace `A::B`.
    fn type_at(&self, g: &RepoGraph, path: &str) -> Option<NodeId> {
        let (ns, last) = path.rsplit_once("::")?;
        match self.index.member(ns, last) {
            NsHit::Unique(id) if is_type(g, &id) => Some(id),
            _ => None,
        }
    }

    /// C# simple-name lookup of base `name` for heritage ref `r` (its
    /// `from` the declaring type, its `from_module` the file MODULE);
    /// `heritage_base` is the HQ.1 repo-unique candidate of a name, read only
    /// when no scope level holds it.
    pub(crate) fn lookup(
        &self,
        g: &RepoGraph,
        r: &UnresolvedRef,
        name: &str,
        heritage_base: impl Fn(&str) -> Option<NodeId>,
    ) -> CsLookup {
        // (1) Nested types of each enclosing type, innermost first. The
        // declaring type's own members are not in scope of its base clause.
        let mut at = g.nav.parent_of.get(&r.from).copied();
        while let Some(owner) = at.filter(|p| is_type(g, p)) {
            match Self::nested_named(g, owner, name, r.from).as_slice() {
                [one] => return CsLookup::Bound(*one, CsRule::NestedType),
                [] => {}
                _ => return CsLookup::Ambiguous,
            }
            at = g.nav.parent_of.get(&owner).copied();
        }
        // (2) The declaring type's namespace, then each enclosing one.
        let mut ns = namespace_of(g, r.from);
        while let Some(level) = ns {
            if self.index.is_package(level) {
                match self.index.member(level, name) {
                    NsHit::Unique(id) if is_type(g, &id) => {
                        return CsLookup::Bound(id, CsRule::EnclosingNamespace);
                    }
                    NsHit::Ambiguous => return CsLookup::Ambiguous,
                    _ => {}
                }
            }
            ns = level.rsplit_once("::").map(|(outer, _)| outer);
        }
        // (3) The file's usings and the group's global usings.
        let own: Vec<(CsUsingForm, &str)> = g
            .nav
            .nav_facts
            .get(&r.from_module)
            .into_iter()
            .flatten()
            .filter_map(|f| match f {
                NavFact::CsUsing { form, path, .. } if *form != CsUsingForm::Alias => {
                    Some((*form, path.as_str()))
                }
                _ => None,
            })
            .collect();
        let mut hits: Vec<(NodeId, CsRule)> = Vec::new();
        let mut ambiguous = false;
        let globals = self.globals.iter().map(|(f, p)| (*f, p.as_str()));
        for (form, path) in own.iter().copied().chain(globals) {
            let found =
                match form {
                    CsUsingForm::Namespace => match self.index.member(path, name) {
                        NsHit::Unique(id) if is_type(g, &id) => Some((id, CsRule::UsingNamespace)),
                        NsHit::Ambiguous => {
                            ambiguous = true;
                            None
                        }
                        _ => None,
                    },
                    CsUsingForm::Static => self.type_at(g, path).and_then(|t| {
                        match Self::nested_named(g, t, name, r.from).as_slice() {
                            [one] => Some((*one, CsRule::UsingStatic)),
                            [] => None,
                            _ => {
                                ambiguous = true;
                                None
                            }
                        }
                    }),
                    CsUsingForm::Alias => None,
                };
            if let Some(hit) = found
                && !hits.iter().any(|(id, _)| *id == hit.0)
            {
                hits.push(hit);
            }
        }
        match hits.as_slice() {
            [(id, rule)] if !ambiguous => return CsLookup::Bound(*id, *rule),
            [] if !ambiguous => {}
            _ => return CsLookup::Ambiguous,
        }
        // (4) Nothing in scope: refuse the name guess when the file opens an
        // undeclared namespace and the candidate is a declared namespace's.
        let opens_external = own.iter().any(|(form, path)| match form {
            CsUsingForm::Namespace => !self.index.declares(path),
            CsUsingForm::Static => !self
                .index
                .declares(path.rsplit_once("::").map_or(*path, |(ns, _)| ns)),
            CsUsingForm::Alias => false,
        });
        let namespaced_candidate = || {
            heritage_base(name)
                .and_then(|c| namespace_of(g, c))
                .is_some_and(|ns| self.index.is_package(ns))
        };
        if opens_external && namespaced_candidate() {
            CsLookup::RefusedExternal
        } else {
            CsLookup::NotFound
        }
    }
}

#[cfg(test)]
mod tests {
    use glia_code_domain::{
        CallQualifier, FileParse, GRAPH_TYPE, NavFact, UnresolvedRef, cell_type, edge_category,
        node_kind,
    };
    use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, NodeKindId, RepoId};

    use super::{CsLookup, CsRule, CsScopes};
    use crate::types::RepoGraph;
    use glia_code_domain::CsUsingForm::{self, Namespace, Static};

    const REPO: RepoId = RepoId(7);

    fn id(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, REPO, kind, qname)
    }

    /// One C# file: its MODULE (`module`, at `file`), the namespace PACKAGE
    /// it opens (CONTAINS), its top-level types under that namespace (nav
    /// parent the MODULE, as the parser records a file-scoped namespace's
    /// types), each `(name, nested children)`, and its usings.
    struct File<'a> {
        module: &'a str,
        ns: &'a str,
        types: &'a [(&'a str, &'a [&'a str])],
        usings: &'a [(CsUsingForm, &'a str, bool)],
    }

    fn file_parse(f: &File) -> FileParse {
        let mut fp = FileParse::default();
        let module = id(node_kind::MODULE, f.module);
        let pos = |file: String| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                "{{\"file\":\"{file}\",\"start_line\":0,\"end_line\":1}}"
            )),
        };
        let file = format!("{}.cs", f.module.replace("::", "/"));
        let push = |fp: &mut FileParse, nid: NodeId, name: &str, qname: &str, kind, parent| {
            fp.nodes.push(Node {
                id: nid,
                repo: REPO,
                confidence: Confidence::Strong,
                cells: vec![pos(file.clone())],
            });
            fp.nav.record(nid, name, qname, kind, parent);
        };
        let simple = |q: &str| q.rsplit("::").next().unwrap_or(q).to_string();
        push(
            &mut fp,
            module,
            &simple(f.module),
            f.module,
            node_kind::MODULE,
            None,
        );
        let pkg = id(node_kind::PACKAGE, f.ns);
        push(
            &mut fp,
            pkg,
            &simple(f.ns),
            f.ns,
            node_kind::PACKAGE,
            Some(module),
        );
        fp.edges.push(Edge {
            from: module,
            to: pkg,
            category: edge_category::CONTAINS,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        for (name, nested) in f.types {
            let q = format!("{}::{name}", f.ns);
            let t = id(node_kind::CLASS, &q);
            push(&mut fp, t, name, &q, node_kind::CLASS, Some(module));
            for inner in *nested {
                let iq = format!("{q}::{inner}");
                push(
                    &mut fp,
                    id(node_kind::CLASS, &iq),
                    inner,
                    &iq,
                    node_kind::CLASS,
                    Some(t),
                );
            }
        }
        for (line, (form, path, global)) in f.usings.iter().enumerate() {
            fp.nav.record_fact(
                module,
                NavFact::CsUsing {
                    form: *form,
                    path: path.to_string(),
                    alias: None,
                    global: *global,
                    line: line as u32,
                },
            );
        }
        fp
    }

    fn graph(files: &[File]) -> RepoGraph {
        let parses: Vec<FileParse> = files.iter().map(file_parse).collect();
        crate::build::build_dotted(REPO, parses).expect("graph")
    }

    /// The heritage ref of type `from` (a qname) in file `module` naming
    /// `base`.
    fn base_ref(module: &str, from: &str, base: &str) -> UnresolvedRef {
        UnresolvedRef {
            from: id(node_kind::CLASS, from),
            from_module: id(node_kind::MODULE, module),
            qualifier: CallQualifier::Bare(base.to_string()),
            category: edge_category::INHERITS_FROM,
            line: 0,
        }
    }

    /// The lookup with the repo-unique fallback the graph's names give.
    fn look(g: &RepoGraph, r: &UnresolvedRef) -> CsLookup {
        let CallQualifier::Bare(name) = &r.qualifier else {
            return CsLookup::NotFound;
        };
        let names = crate::calls::GlobalNames::new(g);
        CsScopes::build(g).lookup(g, r, name, |n| names.heritage_base(n))
    }

    const CONTENT: File = File {
        module: "Core::Model::Content",
        ns: "Shop::Core::Model",
        types: &[("Content", &[])],
        usings: &[],
    };
    const DOMAIN_OBJECT: File = File {
        module: "Core::Commands::DomainObject",
        ns: "Shop::Core::Commands",
        types: &[("DomainObject", &[]), ("Entity", &[])],
        usings: &[],
    };

    #[test]
    fn a_using_names_the_base() {
        let g = graph(&[
            CONTENT,
            File {
                module: "Domain::Enriched",
                ns: "Shop::Domain::Contents",
                types: &[("EnrichedContent", &[])],
                usings: &[(Namespace, "Shop::Core::Model", false)],
            },
        ]);
        let r = base_ref(
            "Domain::Enriched",
            "Shop::Domain::Contents::EnrichedContent",
            "Content",
        );
        assert_eq!(
            look(&g, &r),
            CsLookup::Bound(
                id(node_kind::CLASS, "Shop::Core::Model::Content"),
                CsRule::UsingNamespace
            )
        );
    }

    #[test]
    fn the_enclosing_namespace_names_the_base_and_hides_a_using() {
        let g = graph(&[
            CONTENT,
            File {
                module: "Domain::Local",
                ns: "Shop::Domain",
                types: &[("Content", &[])],
                usings: &[],
            },
            File {
                module: "Domain::Contents::Enriched",
                ns: "Shop::Domain::Contents",
                types: &[("EnrichedContent", &[])],
                usings: &[(Namespace, "Shop::Core::Model", false)],
            },
        ]);
        let r = base_ref(
            "Domain::Contents::Enriched",
            "Shop::Domain::Contents::EnrichedContent",
            "Content",
        );
        assert_eq!(
            look(&g, &r),
            CsLookup::Bound(
                id(node_kind::CLASS, "Shop::Domain::Content"),
                CsRule::EnclosingNamespace
            )
        );
    }

    #[test]
    fn a_nested_sibling_beats_a_top_level_type() {
        let g = graph(&[
            DOMAIN_OBJECT,
            File {
                module: "Tests::FieldTests",
                ns: "Shop::Tests",
                types: &[("FieldTests", &["Entity", "Inherited"])],
                usings: &[(Namespace, "Shop::Core::Commands", false)],
            },
        ]);
        let r = base_ref(
            "Tests::FieldTests",
            "Shop::Tests::FieldTests::Inherited",
            "Entity",
        );
        assert_eq!(
            look(&g, &r),
            CsLookup::Bound(
                id(node_kind::CLASS, "Shop::Tests::FieldTests::Entity"),
                CsRule::NestedType
            )
        );
    }

    #[test]
    fn an_external_using_refuses_the_name_guess() {
        let g = graph(&[
            CONTENT,
            File {
                module: "Api::News",
                ns: "Shop::Api::News",
                types: &[("NewsEntity", &[])],
                usings: &[(Namespace, "Vendor::ClientLibrary", false)],
            },
        ]);
        let r = base_ref("Api::News", "Shop::Api::News::NewsEntity", "Content");
        assert_eq!(look(&g, &r), CsLookup::RefusedExternal);
    }

    #[test]
    fn only_in_repo_usings_keep_the_fallback() {
        let g = graph(&[
            CONTENT,
            DOMAIN_OBJECT,
            File {
                module: "Api::News",
                ns: "Shop::Api::News",
                types: &[("NewsEntity", &[])],
                usings: &[(Namespace, "Shop::Core::Commands", false)],
            },
        ]);
        let r = base_ref("Api::News", "Shop::Api::News::NewsEntity", "Content");
        assert_eq!(look(&g, &r), CsLookup::NotFound);
    }

    #[test]
    fn two_opened_namespaces_declaring_the_name_bind_nothing() {
        let g = graph(&[
            File {
                module: "Data::Store",
                ns: "Shop::Data::Store",
                types: &[("Repository", &[])],
                usings: &[],
            },
            File {
                module: "Data::Cache",
                ns: "Shop::Data::Cache",
                types: &[("Repository", &[])],
                usings: &[],
            },
            File {
                module: "Domain::Repo",
                ns: "Shop::Domain",
                types: &[("ContentRepository", &[])],
                usings: &[
                    (Namespace, "Shop::Data::Store", false),
                    (Namespace, "Shop::Data::Cache", false),
                ],
            },
        ]);
        let r = base_ref(
            "Domain::Repo",
            "Shop::Domain::ContentRepository",
            "Repository",
        );
        assert_eq!(look(&g, &r), CsLookup::Ambiguous);
    }

    #[test]
    fn a_global_using_in_another_file_opens_the_namespace() {
        let g = graph(&[
            CONTENT,
            File {
                module: "GlobalUsings",
                ns: "Shop",
                types: &[],
                usings: &[(Namespace, "Shop::Core::Model", true)],
            },
            File {
                module: "Domain::Enriched",
                ns: "Shop::Domain::Contents",
                types: &[("EnrichedContent", &[])],
                usings: &[(Namespace, "Vendor::ClientLibrary", false)],
            },
        ]);
        let r = base_ref(
            "Domain::Enriched",
            "Shop::Domain::Contents::EnrichedContent",
            "Content",
        );
        assert_eq!(
            look(&g, &r),
            CsLookup::Bound(
                id(node_kind::CLASS, "Shop::Core::Model::Content"),
                CsRule::UsingNamespace
            )
        );
    }

    #[test]
    fn a_static_using_opens_its_nested_types() {
        let g = graph(&[
            File {
                module: "Core::Bases",
                ns: "Shop::Core",
                types: &[("Bases", &["Handler"])],
                usings: &[],
            },
            File {
                module: "Api::Thing",
                ns: "Shop::Api",
                types: &[("Thing", &[])],
                usings: &[(Static, "Shop::Core::Bases", false)],
            },
        ]);
        let r = base_ref("Api::Thing", "Shop::Api::Thing", "Handler");
        assert_eq!(
            look(&g, &r),
            CsLookup::Bound(
                id(node_kind::CLASS, "Shop::Core::Bases::Handler"),
                CsRule::UsingStatic
            )
        );
    }

    #[test]
    fn counts_and_marker() {
        let mut c = super::CsScopeCounts::default();
        assert_eq!(c.marker(), None);
        c.note(CsLookup::NotFound);
        c.note(CsLookup::RefusedExternal);
        let mut d = super::CsScopeCounts::default();
        d.note(CsLookup::Bound(
            id(node_kind::CLASS, "X"),
            CsRule::NestedType,
        ));
        c.merge(d);
        assert_eq!(
            c.marker().as_deref(),
            Some(
                "[cs-scope] heritage refs=3 nested=1 enclosing=0 using=0 static=0 ambiguous=0 \
                 refused_external=1 fallback=1"
            )
        );
    }
}
