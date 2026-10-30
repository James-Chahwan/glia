//! The Lua build (0.5.5): `require` resolution and call binding over what
//! the Lua parser read. Built from Brian Ealdwine's design in glia PR #3
//! (its `resolve_lua_requires` / `resolve_lua_calls`), without the plugin
//! families' lib roots, plus the review's gaps: `<spec>/init.lua` and the
//! conventional `lua/`, `lib/` and `src/` roots.
//!
//! **Requires** ([`resolve_requires`]). A `require("a.b")` from file `p`
//! tries, in this total order, and binds the first `.lua` MODULE that exists:
//!
//! 1. beside the requirer: `<dir(p)>/<spec>` (the spec verbatim as one file
//!    name, a plugin host's sibling require), then `<dir(p)>/a/b`;
//! 2. per ancestor directory of `p`, nearest first, ending at the repo root,
//!    under each root `""`, `lua`, `lib`, `src`: `<A>/<root>/a/b`;
//!
//! each candidate as the file `a/b.lua`, then the directory module
//! `a/b/init.lua` (Lua's `?/init.lua`). A dotted spec maps `.` (and `/`) to
//! the path separator. None found: the require stays unbound, never guessed.
//! IMPORTS rules `require_sibling`, `require_path` and `require_root`
//! (emitter `graph:imports`); `local x = require(..)` binds `x` to the target.
//!
//! **Calls** ([`resolve_lua_calls`]), file-local only:
//! - `Bare(key)`: the parser already chose the visible local or the file's
//!   global, so the key binds exactly in the caller's file, rule `local_call`;
//! - `Attribute { T, f }`: the caller's file minted `T.f`, rule
//!   `module_table`; or `T` is a bound require alias and the target file has
//!   exactly one symbol whose last segment is `f`, rule `require_alias`.
//!
//! **Classes** ([`resolve_heritage`]): an `INHERITS_FROM` ref `Bare(Base)`
//! from a CLASS binds the caller's file's CLASS `Base` (rule `metatable`), else
//! `Base` is a bound require alias and the required file has exactly one CLASS
//! (rule `require_alias`). A call `T:m()` / `T.m()` no rule above bound, `T`
//! a CLASS of the caller's file, binds the nearest ancestor's `<A>.m` through
//! those edges (rule `inherited_method`, at most 8 levels).
//!
//! Everything else stays unresolved: no repo-wide name match, no globals of
//! other files. fired_on, once per Lua graph:
//! `[imports] lua requires bound=<b> (sibling=<s> path=<p> root=<r> init=<i>) unbound=<u>` and
//! `[lua-calls] bound=<b> (local=<l> module_table=<m> require_alias=<a> inherited=<i>) unbound=<u> ambiguous=<x>` and
//! `[lua-classes] heritage bound=<b> (metatable=<m> require_alias=<a>) unbound=<u>`.

use std::collections::{HashMap, HashSet};

use glia_code_domain::{
    CallQualifier, CallSite, ImportStmt, ImportTarget, UnresolvedRef, edge_category, node_kind,
};
use glia_core::NodeId;

use crate::calls::{enclosing_module, graph_evidence, push_edge};
use crate::types::RepoGraph;

/// The conventional source roots a require is tried under, after the plain
/// path: Neovim's runtimepath `lua/`, then `lib/` and `src/`.
const ROOTS: [&str; 4] = ["", "lua", "lib", "src"];

/// Requirer module -> alias -> required module.
pub(crate) type Aliases = HashMap<NodeId, HashMap<String, NodeId>>;

#[derive(Default)]
pub(crate) struct RequireStats {
    sibling: usize,
    path: usize,
    root: usize,
    init: usize,
    unbound: usize,
}

impl RequireStats {
    pub(crate) fn marker(&self) -> Option<String> {
        let bound = self.sibling + self.path + self.root;
        (bound + self.unbound > 0).then(|| {
            format!(
                "[imports] lua requires bound={bound} (sibling={} path={} root={} init={}) unbound={}",
                self.sibling, self.path, self.root, self.init, self.unbound
            )
        })
    }
}

/// One candidate module qname and the rule that would bind it.
struct Candidate {
    qname: String,
    rule: &'static str,
    init: bool,
}

/// The candidates for `spec` required from `from_module`, in try order.
fn candidates(from_module: &str, spec: &str) -> Vec<Candidate> {
    let segs: Vec<&str> = from_module.split("::").collect();
    let dirs = &segs[..segs.len().saturating_sub(1)];
    let path: Vec<&str> = spec.split(['.', '/']).filter(|s| !s.is_empty()).collect();
    let mut out = Vec::new();
    let mut push = |parts: Vec<&str>, rule: &'static str| {
        if parts.is_empty() {
            return;
        }
        // The file `a/b.lua` before the directory module `a/b/init.lua`.
        let qname = parts.join("::");
        out.push(Candidate {
            qname: format!("{qname}::init"),
            rule,
            init: true,
        });
        let init = out.len() - 1;
        out.insert(
            init,
            Candidate {
                qname,
                rule,
                init: false,
            },
        );
    };
    let mut sibling = dirs.to_vec();
    sibling.push(spec);
    push(sibling, "require_sibling");
    let mut relative = dirs.to_vec();
    relative.extend(&path);
    push(relative, "require_sibling");
    for k in (0..=dirs.len()).rev() {
        for root in ROOTS {
            let mut parts = dirs[..k].to_vec();
            if !root.is_empty() {
                parts.push(root);
            }
            parts.extend(&path);
            push(
                parts,
                if root.is_empty() {
                    "require_path"
                } else {
                    "require_root"
                },
            );
        }
    }
    out
}

/// Bind every `require` ([`candidates`]); returns the stats and the aliases
/// `local x = require(..)` bound.
pub(crate) fn resolve_requires(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
) -> (RequireStats, Aliases) {
    let mut stats = RequireStats::default();
    let mut aliases: Aliases = HashMap::new();
    for stmt in imports {
        let ImportTarget::Module { path, alias } = &stmt.target else {
            continue;
        };
        let Some(from) = g.symbols.module_by_qname.get(&stmt.from_module).copied() else {
            continue;
        };
        let hit = candidates(&stmt.from_module, path)
            .into_iter()
            .find_map(|c| {
                g.symbols
                    .module_by_qname
                    .get(&c.qname)
                    .copied()
                    .filter(|id| *id != from)
                    .map(|id| (id, c))
            });
        let Some((target, c)) = hit else {
            stats.unbound += 1;
            continue;
        };
        match c.rule {
            "require_sibling" => stats.sibling += 1,
            "require_path" => stats.path += 1,
            _ => stats.root += 1,
        }
        if c.init {
            stats.init += 1;
        }
        let ev = graph_evidence("graph:imports", c.rule).line(stmt.line);
        push_edge(g, from, target, edge_category::IMPORTS, ev);
        if let Some(alias) = alias {
            aliases
                .entry(from)
                .or_default()
                .insert(alias.clone(), target);
        }
    }
    (stats, aliases)
}

/// Child CLASS -> its bound bases (the INHERITS_FROM edges [`resolve_heritage`] wrote).
pub(crate) type Bases = HashMap<NodeId, Vec<NodeId>>;

#[derive(Default)]
pub(crate) struct HeritageStats {
    metatable: usize,
    require_alias: usize,
    unbound: usize,
}

impl HeritageStats {
    pub(crate) fn marker(&self) -> Option<String> {
        let bound = self.metatable + self.require_alias;
        (bound + self.unbound > 0).then(|| {
            format!(
                "[lua-classes] heritage bound={bound} (metatable={} require_alias={}) unbound={}",
                self.metatable, self.require_alias, self.unbound
            )
        })
    }
}

fn is_class(g: &RepoGraph, id: NodeId) -> bool {
    g.nav.kind_by_id.get(&id) == Some(&node_kind::CLASS)
}

/// Bind the parser's INHERITS_FROM refs (module doc); the rest are returned.
pub(crate) fn resolve_heritage(
    g: &mut RepoGraph,
    refs: Vec<UnresolvedRef>,
    aliases: &Aliases,
) -> (HeritageStats, Bases, Vec<UnresolvedRef>) {
    let mut stats = HeritageStats::default();
    let mut bases: Bases = HashMap::new();
    let mut rest = Vec::new();
    for r in refs {
        let (true, CallQualifier::Bare(base)) =
            (r.category == edge_category::INHERITS_FROM, &r.qualifier)
        else {
            rest.push(r);
            continue;
        };
        let in_file = g
            .symbols
            .module_symbols
            .get(&r.from_module)
            .and_then(|s| s.get(base).copied())
            .filter(|id| is_class(g, *id))
            .map(|id| (id, "metatable"));
        let bound = in_file.or_else(|| {
            let target = aliases.get(&r.from_module)?.get(base)?;
            let classes: Vec<NodeId> = g
                .symbols
                .module_symbols
                .get(target)?
                .values()
                .copied()
                .filter(|id| is_class(g, *id))
                .collect::<HashSet<NodeId>>()
                .into_iter()
                .collect();
            match classes.as_slice() {
                [only] => Some((*only, "require_alias")),
                _ => None,
            }
        });
        match bound {
            Some((to, rule)) => {
                if rule == "metatable" {
                    stats.metatable += 1;
                } else {
                    stats.require_alias += 1;
                }
                let ev = graph_evidence("graph:lua", rule).line(r.line);
                push_edge(g, r.from, to, edge_category::INHERITS_FROM, ev);
                bases.entry(r.from).or_default().push(to);
            }
            None => {
                stats.unbound += 1;
                g.unresolved_refs.push(r);
            }
        }
    }
    (stats, bases, rest)
}

/// `class`'s nearest ancestor (breadth-first, at most 8 levels) whose file
/// minted `<ancestor key>.<name>`.
fn inherited(g: &RepoGraph, bases: &Bases, class: NodeId, name: &str) -> Option<NodeId> {
    let mut level = bases.get(&class).cloned().unwrap_or_default();
    let mut seen: HashSet<NodeId> = HashSet::from([class]);
    for _ in 0..8 {
        let mut next = Vec::new();
        for a in level {
            if !seen.insert(a) {
                continue;
            }
            let key = g.nav.name_by_id.get(&a)?;
            let found = enclosing_module(&g.nav, a)
                .and_then(|m| g.symbols.module_symbols.get(&m))
                .and_then(|s| s.get(&format!("{key}.{name}")).copied());
            if found.is_some() {
                return found;
            }
            next.extend(bases.get(&a).cloned().unwrap_or_default());
        }
        if next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

#[derive(Default)]
pub(crate) struct CallStats {
    local: usize,
    module_table: usize,
    require_alias: usize,
    inherited: usize,
    unbound: usize,
    ambiguous: usize,
}

impl CallStats {
    pub(crate) fn marker(&self) -> Option<String> {
        let bound = self.local + self.module_table + self.require_alias + self.inherited;
        (bound + self.unbound > 0).then(|| {
            format!(
                "[lua-calls] bound={bound} (local={} module_table={} require_alias={} inherited={}) unbound={} ambiguous={}",
                self.local,
                self.module_table,
                self.require_alias,
                self.inherited,
                self.unbound,
                self.ambiguous
            )
        })
    }
}

/// Bind the Lua call sites (module doc); an unbound one joins
/// `unresolved_calls`.
pub(crate) fn resolve_lua_calls(
    g: &mut RepoGraph,
    calls: &[CallSite],
    aliases: &Aliases,
    bases: &Bases,
) -> CallStats {
    let mut stats = CallStats::default();
    for call in calls {
        let module = enclosing_module(&g.nav, call.from);
        let symbols = module.and_then(|m| g.symbols.module_symbols.get(&m));
        let bound: Option<(NodeId, &'static str)> = match &call.qualifier {
            CallQualifier::Bare(key) => symbols
                .and_then(|s| s.get(key).copied())
                .map(|to| (to, "local_call")),
            CallQualifier::Attribute { base, name } => {
                let in_file = symbols
                    .and_then(|s| s.get(&format!("{base}.{name}")).copied())
                    .map(|to| (to, "module_table"));
                let through_class = || {
                    let class = symbols
                        .and_then(|s| s.get(base).copied())
                        .filter(|id| is_class(g, *id))?;
                    inherited(g, bases, class, name).map(|to| (to, "inherited_method"))
                };
                in_file.or_else(through_class).or_else(|| {
                    let target = module
                        .and_then(|m| aliases.get(&m))
                        .and_then(|a| a.get(base))?;
                    // A set: NodeId has no order, and only "exactly one" matters.
                    let members: HashSet<NodeId> = g
                        .symbols
                        .module_symbols
                        .get(target)?
                        .iter()
                        .filter(|(key, _)| last_segment(key) == name)
                        .map(|(_, id)| *id)
                        .collect();
                    let members: Vec<NodeId> = members.into_iter().collect();
                    match members.as_slice() {
                        [only] => Some((*only, "require_alias")),
                        [] => None,
                        _ => {
                            stats.ambiguous += 1;
                            None
                        }
                    }
                })
            }
            _ => None,
        };
        match bound {
            Some((to, rule)) => {
                match rule {
                    "local_call" => stats.local += 1,
                    "module_table" => stats.module_table += 1,
                    "inherited_method" => stats.inherited += 1,
                    _ => stats.require_alias += 1,
                }
                let ev = graph_evidence("graph:calls", rule).line(call.line);
                push_edge(g, call.from, to, edge_category::CALLS, ev);
            }
            None => {
                stats.unbound += 1;
                g.unresolved_calls.push(call.clone());
            }
        }
    }
    stats
}

/// `T.sub.f` -> `f`; a nested local's `outer.f@12` -> `f@12` never matches a
/// member name, so a collided local is no require-alias target.
fn last_segment(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(from: &str, spec: &str) -> Vec<String> {
        candidates(from, spec)
            .into_iter()
            .map(|c| format!("{}:{}", c.rule, c.qname))
            .collect()
    }

    #[test]
    fn candidates_go_sibling_then_ancestors_file_before_init() {
        let got = order("plugin::app::main", "app.util");
        assert_eq!(
            &got[..6],
            [
                "require_sibling:plugin::app::app.util",
                "require_sibling:plugin::app::app.util::init",
                "require_sibling:plugin::app::app::util",
                "require_sibling:plugin::app::app::util::init",
                "require_path:plugin::app::app::util",
                "require_path:plugin::app::app::util::init",
            ]
        );
        // The repo root's lua/ root comes last but one family.
        assert!(got.contains(&"require_root:lua::app::util".to_string()));
        assert!(got.contains(&"require_path:app::util::init".to_string()));
        let root = got
            .iter()
            .position(|c| c == "require_path:app::util")
            .unwrap();
        let lua = got
            .iter()
            .position(|c| c == "require_root:lua::app::util")
            .unwrap();
        assert!(root < lua);
    }
}
