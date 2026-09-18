//! Angular component/service/directive/pipe/guard/module extraction.
//!
//! Pattern-based, runs per-file on TS under Angular-detected projects. Emits:
//!   - COMPONENT: class with `@Component({...})` decorator.
//!   - SERVICE: class with `@Injectable({...})` decorator.
//!   - DIRECTIVE: class with `@Directive({...})` decorator.
//!   - PIPE: class with `@Pipe({...})` decorator.
//!   - GUARD: classes ending in `Guard` (CanActivate/CanDeactivate patterns).
//!
//! Angular Router tables (`{ path: 'users', component: UsersComponent,
//! children: [...] }`) are the shared route-table walker's
//! (`crate::nav_routes`, LA.6b).

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, NodeKindId, RepoId};

pub struct AngularNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

pub fn extract_angular_nodes(
    source: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
) -> AngularNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();

    for (name, kind) in scan_decorated_classes(source) {
        let qname = format!("{module_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        nav.record(id, &name, &qname, kind, Some(module_id));
    }

    AngularNodes { nodes, nav }
}

fn scan_decorated_classes(source: &str) -> Vec<(String, NodeKindId)> {
    let mut out: Vec<(String, NodeKindId)> = Vec::new();
    // Match each `@<Decorator>(` occurrence; balance-skip its parenthesised
    // metadata (which routinely spans many lines for `@Component({selector,
    // templateUrl, imports, ...})`); then look for the next `class <Name>`
    // declaration. Line-based state machines fail here because Angular
    // decorator metadata is multi-line by convention.
    const DECORATORS: &[(&str, NodeKindId)] = &[
        ("@Component(", node_kind::COMPONENT),
        ("@Injectable(", node_kind::SERVICE),
        ("@Directive(", node_kind::DIRECTIVE),
        ("@Pipe(", node_kind::PIPE),
    ];

    for &(needle, kind) in DECORATORS {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let arg_start = pos + needle.len();
            let close = match find_balanced_paren(&source[arg_start..]) {
                Some(off) => arg_start + off,
                None => {
                    search_from = arg_start;
                    continue;
                }
            };
            // After `)`, scan forward until we find `class <Name>`. Allow any
            // amount of whitespace, `export`, `default`, `abstract`, comments.
            if let Some(name) = find_next_class_name(&source[close + 1..]) {
                out.push((name, kind));
            }
            search_from = close + 1;
        }
    }

    // Convention-based: classes ending in `Guard` without a matching @ decorator
    // get GUARD kind.
    for line in source.lines() {
        let t = line.trim_start();
        if let Some(name) = extract_class_name(t) {
            if name.ends_with("Guard") && !out.iter().any(|(n, _)| n == &name) {
                out.push((name, node_kind::GUARD));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
    out.dedup();
    out
}

/// Given `s` starting after a `(`, return the offset of the matching `)`,
/// skipping nested parens, brackets, braces, and string literals.
fn find_balanced_paren(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 && c == b')' {
                    return Some(i);
                }
            }
            b'\'' | b'"' | b'`' => {
                let delim = c;
                i += 1;
                while i < bytes.len() && bytes[i] != delim {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Walk forward from `s` to find the first `class <Name>` declaration.
/// Skips whitespace, line/block comments, and the `export`/`default`/
/// `abstract` modifiers commonly preceding `class` in TS.
fn find_next_class_name(s: &str) -> Option<String> {
    for line in s.lines() {
        let t = line.trim_start();
        if t.is_empty()
            || t.starts_with("//")
            || t.starts_with("/*")
            || t.starts_with("*")
            || t.starts_with(')')
        {
            continue;
        }
        if let Some(name) = extract_class_name(t) {
            return Some(name);
        }
        // If the line starts with another `@<Decorator>` (additional decorators
        // are legal in TS), keep scanning rather than aborting.
        if t.starts_with('@') {
            continue;
        }
        // Otherwise we've gone too far — between this decorator and the next
        // class declaration there's other code; abort to avoid false attribution.
        return None;
    }
    None
}

fn extract_class_name(line: &str) -> Option<String> {
    let trimmed = line
        .trim_start_matches("export ")
        .trim_start_matches("default ");
    let rest = trimmed.strip_prefix("class ")?;
    let name_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(rest.len());
    if name_end == 0 {
        None
    } else {
        Some(rest[..name_end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{CallQualifier, cell_type, edge_category};
    use repo_graph_core::CellPayload;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn detects_component_service_directive_pipe() {
        let src = r#"
@Component({ selector: 'app-user' })
export class UserComponent {}

@Injectable({ providedIn: 'root' })
export class UserService {}

@Directive({ selector: '[appHi]' })
export class HiDirective {}

@Pipe({ name: 'upper' })
export class UpperPipe {}
"#;
        let r = extract_angular_nodes(src, "test", module_id(), repo());
        let get = |kind: NodeKindId| -> Vec<&str> {
            r.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == kind)
                .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
                .collect()
        };
        assert!(get(node_kind::COMPONENT).contains(&"UserComponent"));
        assert!(get(node_kind::SERVICE).contains(&"UserService"));
        assert!(get(node_kind::DIRECTIVE).contains(&"HiDirective"));
        assert!(get(node_kind::PIPE).contains(&"UpperPipe"));
    }

    #[test]
    fn detects_multi_line_decorator_metadata() {
        // Real Angular convention: the @Component arg is a multi-line object
        // literal between the decorator and the class. Was a hard miss in the
        // 2026-05-05 framework-coverage check (angular-realworld emitted 0
        // COMPONENT despite 30+ files using this exact shape).
        let src = r#"
import { ChangeDetectionStrategy, Component } from '@angular/core';

@Component({
  selector: 'app-root',
  templateUrl: './app.component.html',
  imports: [HeaderComponent, RouterOutlet, FooterComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class AppComponent {}

@Component({
  selector: 'app-profile',
  styleUrl: './profile.component.scss',
  template: `<div>profile</div>`,
})
export class ProfileComponent {}

@Injectable({
  providedIn: 'root',
})
export class JwtService {}
"#;
        let r = extract_angular_nodes(src, "test", module_id(), repo());
        let comps: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::COMPONENT)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(comps.contains(&"AppComponent"), "got components: {comps:?}");
        assert!(
            comps.contains(&"ProfileComponent"),
            "got components: {comps:?}"
        );

        let services: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::SERVICE)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(services.contains(&"JwtService"));
    }

    #[test]
    fn detects_guard() {
        let src = "export class AuthGuard { canActivate() { return true; } }";
        let r = extract_angular_nodes(src, "test", module_id(), repo());
        let guards: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::GUARD)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(guards.contains(&"AuthGuard"));
    }

    #[test]
    fn detects_routes() {
        let src = r#"
const routes: Routes = [
    { path: '', component: HomeComponent },
    { path: 'users', component: UsersComponent },
    { path: 'users/:id', component: UserDetailComponent },
];
RouterModule.forRoot(routes);
"#;
        // LA.6b: Angular Router tables are the shared walker's; the Angular
        // extractor itself mints no ROUTE any more.
        let own = extract_angular_nodes(src, "test", module_id(), repo());
        assert!(own.nav.kind_by_id.values().all(|k| *k != node_kind::ROUTE));
        let r = crate::nav_routes::extract_route_tables(src, module_id(), repo());
        let names: Vec<&str> = r
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(names.contains(&"/users"));
        assert!(names.contains(&"/users/:id"));
        assert!(names.contains(&"/"));
        // LB.4c: a nav page lives in the `page:<path>` qname namespace, its
        // display name is the bare path.
        let mut qnames: Vec<&str> = r
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, q)| q.as_str())
            .collect();
        qnames.sort_unstable();
        assert_eq!(qnames, ["page:/", "page:/users", "page:/users/:id"]);
        assert_eq!(r.nav_routes, 3, "A3.4: every client-router ROUTE counted");
        // A3.4: each one carries the `provenance: nav_route` ORIGIN mark, which
        // is what `graph::nav::is_nav_route` reads to keep it out of
        // the HTTP pairing index.
        let route_ids: Vec<_> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(route_ids.len(), 3);
        for id in route_ids {
            let node = r.nodes.iter().find(|n| n.id == id).expect("route node");
            assert!(
                node.cells.iter().any(|c| c.kind == cell_type::ORIGIN
                    && matches!(&c.payload, CellPayload::Json(j)
                                if j.contains("\"provenance\":\"nav_route\""))),
                "nav route must carry the ORIGIN provenance mark"
            );
        }
        // LA.6b: `component: X` binds each route to its page component.
        let mut bound: Vec<(&str, &str)> = r
            .refs
            .iter()
            .filter(|x| x.category == edge_category::HANDLED_BY)
            .filter_map(|x| match &x.qualifier {
                CallQualifier::Bare(h) => {
                    Some((r.nav.qname_by_id.get(&x.from)?.as_str(), h.as_str()))
                }
                _ => None,
            })
            .collect();
        bound.sort_unstable();
        assert_eq!(
            bound,
            [
                ("page:/", "HomeComponent"),
                ("page:/users", "UsersComponent"),
                ("page:/users/:id", "UserDetailComponent"),
            ]
        );
    }
}
