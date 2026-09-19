//! Import resolution — one pass per import-path dialect (dotted / Go / slash /
//! TypeScript), all of them writing IMPORTS edges and module import bindings.

use repo_graph_code_domain::{ImportStmt, ImportTarget, edge_category};
use repo_graph_core::NodeId;

use crate::build::{DirImport, GoPackages};
use crate::calls::{graph_evidence, push_edge, unique_global_function, unique_global_module};
use crate::types::RepoGraph;

// ============================================================================
// Import resolution
// ============================================================================

pub(crate) fn resolve_imports_python(g: &mut RepoGraph, imports: &[ImportStmt]) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };

        match &stmt.target {
            ImportTarget::Module { path, alias } => {
                // `import foo.bar` — convert `.` → `::` and look up by qname.
                let target_qname = path.replace('.', "::");
                if let Some(target_id) = g.symbols.module_by_qname.get(&target_qname).copied() {
                    import_edge(g, from_mod_id, target_id, "module", stmt.line);
                    let bound_name = alias.clone().unwrap_or_else(|| {
                        path.split('.').next().unwrap_or(path).to_string()
                    });
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound_name, target_id);
                } else {
                    // Tail fallback (Pattern B): `import com.example.util.Helper`
                    // (class) or `require app.util` (module) where the file layout
                    // doesn't mirror the package path, so the full qname misses.
                    // Bind the final segment — as a unique global class/interface
                    // (module_symbols, incl. PACKAGE members) or a unique module by
                    // short name. Miss-only + ambiguity-safe (returns None on tie).
                    let tail = path
                        .rsplit(|c| c == '.' || c == ':' || c == '/')
                        .next()
                        .unwrap_or(path);
                    if let Some(target_id) =
                        unique_global_function(g, tail).or_else(|| unique_global_module(g, tail))
                    {
                        import_edge(g, from_mod_id, target_id, "tail_unique", stmt.line);
                        g.symbols
                            .module_import_bindings
                            .entry(from_mod_id)
                            .or_default()
                            .insert(alias.clone().unwrap_or_else(|| tail.to_string()), target_id);
                    }
                }
            }
            ImportTarget::Symbol { module, name, alias, level } => {
                let target_module_qname = resolve_module_reference(&stmt.from_module, module, *level);

                // Try `module::name` as a submodule first — matches Python's
                // `from pkg import mod` → edge to pkg.mod if submodule exists.
                let submodule_qname = if target_module_qname.is_empty() {
                    name.clone()
                } else {
                    format!("{target_module_qname}::{name}")
                };

                let bound = alias.clone().unwrap_or_else(|| name.clone());

                if let Some(submodule_id) = g.symbols.module_by_qname.get(&submodule_qname).copied()
                {
                    // `from pkg import mod` where mod is a submodule.
                    import_edge(g, from_mod_id, submodule_id, "submodule", stmt.line);
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound, submodule_id);
                } else if let Some(target_mod_id) = g
                    .symbols
                    .module_by_qname
                    .get(&target_module_qname)
                    .copied()
                {
                    // `from pkg.mod import Name` — target is a symbol inside pkg.mod.
                    import_edge(g, from_mod_id, target_mod_id, "symbol", stmt.line);
                    if let Some(symbol_id) = g
                        .symbols
                        .module_symbols
                        .get(&target_mod_id)
                        .and_then(|t| t.get(name))
                        .copied()
                    {
                        g.symbols
                            .module_import_bindings
                            .entry(from_mod_id)
                            .or_default()
                            .insert(bound, symbol_id);
                    }
                } else if let Some(symbol_id) = unique_global_function(g, name) {
                    // Tail fallback (Pattern B): `from a.b import Name` where `a.b`
                    // isn't a resolvable module (flat layout) — bind the imported
                    // symbol by its unique global name. Miss-only + ambiguity-safe.
                    import_edge(g, from_mod_id, symbol_id, "tail_unique", stmt.line);
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound, symbol_id);
                }
            }
        }
    }
}

/// Go imports: the parser has already stripped the go.mod prefix and produced
/// `ImportTarget::Module { path }` with `path` = repo-local `::` qname for
/// imports that resolve inside this module. External imports keep the raw
/// `std::io`-style form and won't match anything.
///
/// An import path names a package, which is a DIRECTORY, while every Go file
/// is its own MODULE (`internal/store/store.go` -> `internal::store::store`).
/// So the lookup order is: a MODULE whose qname is the path (unchanged from
/// before LA.13b), then the package directory `path` itself, bound to one of
/// its files ([`GoPackages::import_target`]; the call-level hook reaches the
/// package's other files), then the tail fallback. A directory whose only
/// file is the importer binds nothing and skips the tail fallback, which
/// could only guess a same-named package elsewhere. Returns the number of
/// imports the directory step bound (LA.13b, the `[go-package]` marker).
pub(crate) fn resolve_imports_go(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
    packages: &GoPackages,
) -> usize {
    let mut dir_bound = 0usize;
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        let ImportTarget::Module { path, alias } = &stmt.target else {
            continue;
        };
        let target = match g.symbols.module_by_qname.get(path).copied() {
            Some(exact) => Some((exact, "module")),
            None => match packages.import_target(path, from_mod_id) {
                DirImport::Bound(id) => {
                    dir_bound += 1;
                    Some((id, "package_dir"))
                }
                DirImport::OnlyImporter => None,
                // Tail fallback (Pattern B): the go.mod-stripped path doesn't
                // match a module qname or a package directory — bind the
                // imported package by its unique short name (last `::`
                // segment). Miss-only + ambiguity-safe.
                DirImport::NoDir => {
                    unique_global_module(g, path.rsplit("::").next().unwrap_or(path))
                        .map(|id| (id, "tail_unique"))
                }
            },
        };
        let Some((target_id, rule)) = target else {
            continue;
        };
        import_edge(g, from_mod_id, target_id, rule, stmt.line);
        let bound = alias
            .clone()
            .unwrap_or_else(|| path.rsplit("::").next().unwrap_or(path).to_string());
        g.symbols
            .module_import_bindings
            .entry(from_mod_id)
            .or_default()
            .insert(bound, target_id);
    }
    dir_bound
}

/// Ruby imports: `require 'foo/bar'` gives a slash-delimited path. Convert
/// slashes to `::` then look up directly (same shape as Go's resolver).
pub(crate) fn resolve_imports_slash(g: &mut RepoGraph, imports: &[ImportStmt]) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        let ImportTarget::Module { path, alias } = &stmt.target else {
            continue;
        };
        let target_qname = path.replace('/', "::");
        let Some(target_id) = g.symbols.module_by_qname.get(&target_qname).copied() else {
            continue;
        };
        import_edge(g, from_mod_id, target_id, "module", stmt.line);
        let bound = alias
            .clone()
            .unwrap_or_else(|| target_qname.rsplit("::").next().unwrap_or(&target_qname).to_string());
        g.symbols
            .module_import_bindings
            .entry(from_mod_id)
            .or_default()
            .insert(bound, target_id);
    }
}

/// TypeScript imports: the parser keeps import sources as raw strings
/// (`./user`, `@angular/core`). `resolve_source(from_qname, raw)` converts a
/// raw source string to a module qname; `None` marks the import external.
pub(crate) fn resolve_imports_ts<R: Fn(&str, &str) -> Option<String>>(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
    resolve_source: &R,
) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        match &stmt.target {
            ImportTarget::Module { path, alias } => {
                let Some(target_qname) = resolve_source(&stmt.from_module, path) else {
                    continue;
                };
                let Some(target_id) = g.symbols.module_by_qname.get(&target_qname).copied() else {
                    continue;
                };
                import_edge(g, from_mod_id, target_id, "module", stmt.line);
                // Namespace import alias is the binding; bare side-effect has none.
                if let Some(a) = alias {
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(a.clone(), target_id);
                }
            }
            ImportTarget::Symbol {
                module,
                name,
                alias,
                ..
            } => {
                let Some(target_qname) = resolve_source(&stmt.from_module, module) else {
                    continue;
                };
                let Some(target_mod_id) = g.symbols.module_by_qname.get(&target_qname).copied()
                else {
                    continue;
                };
                import_edge(g, from_mod_id, target_mod_id, "symbol", stmt.line);
                let bound = alias.clone().unwrap_or_else(|| name.clone());
                // Default import — bind to the module itself.
                // Named import — bind to the specific symbol inside that module.
                let target_id = if name == "default" {
                    Some(target_mod_id)
                } else {
                    g.symbols
                        .module_symbols
                        .get(&target_mod_id)
                        .and_then(|s| s.get(name))
                        .copied()
                };
                if let Some(t) = target_id {
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound, t);
                }
            }
        }
    }
}

/// Push one IMPORTS edge with its evidence (LC.3d): `graph:imports`, rule the
/// lookup that bound it: `module` (the import path is a module qname),
/// `submodule` (`from pkg import mod`), `symbol` (the module holding an
/// imported name), `package_dir` (a Go package directory, LA.13b) or
/// `tail_unique` (the repo-unique short name, a guess the full path could not
/// confirm).
/// `line` is the import statement's 0-based row (LC.3b, basis site).
fn import_edge(g: &mut RepoGraph, from: NodeId, to: NodeId, rule: &str, line: u32) {
    let ev = graph_evidence("graph:imports", rule).line(line);
    push_edge(g, from, to, edge_category::IMPORTS, ev);
}

/// Convert a (possibly relative) `from X import Y` module reference into an
/// absolute qname using `::` separators.
fn resolve_module_reference(from_module: &str, module_ref: &str, level: u32) -> String {
    if level == 0 {
        return module_ref.replace('.', "::");
    }
    // Relative: strip `level` trailing components from `from_module`, then
    // append `module_ref`. `level=1` pops 1 (the current file stays at package level).
    let mut parts: Vec<&str> = from_module.split("::").collect();
    for _ in 0..level {
        parts.pop();
    }
    let base = parts.join("::");
    if module_ref.is_empty() {
        base
    } else if base.is_empty() {
        module_ref.replace('.', "::")
    } else {
        format!("{base}::{}", module_ref.replace('.', "::"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_import_resolution() {
        assert_eq!(resolve_module_reference("myapp::users", "helpers", 1), "myapp::helpers");
        assert_eq!(resolve_module_reference("a::b::c", "d", 2), "a::d");
        assert_eq!(resolve_module_reference("a::b", "c.d", 0), "c::d");
        assert_eq!(resolve_module_reference("a::b::c", "", 1), "a::b");
    }
}
