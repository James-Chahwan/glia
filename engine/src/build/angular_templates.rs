//! DT.5b: Angular `templateUrl` templates bind their component's members.
//!
//! DT.5a binds a component's inline `template:` and `host:` metadata inside
//! the per-file extractors (`angular_template::bind_inline`), because that
//! text is the component file's own. An external template is a SECOND file:
//! the per-file pass sees only the component's source and the parse cache
//! keys on that source alone, so the read belongs here, in a post-cache graft
//! over the walked `files` (the [`super::grafts`] rule: every pass there reads
//! something that is not a function of one file's content). A cached
//! component parse is bound again every build, so an edit to the template
//! alone moves the edges and incremental == clean.
//!
//! The walk admits the templates: a `.component.html` (LA.6c) or any `.html`
//! with a same-stem `.ts` sibling (`orders.page.html`, Angular 20's
//! `user-profile.html`; `walk::is_component_template_sibling`). The router
//! skips the latter (no language, no route branch), so the html is never
//! parsed, const-scanned or a MODULE.
//!
//! Each `@Component(` / `@Directive(` decorator of a TS-family file whose
//! `templateUrl:` is one string literal names a template relative to the
//! component's directory (`./x.html`, `../shared/x.html` and a bare
//! `x.html` alike, as the Angular CLI reads it). The walked template is
//! scanned by DT.5a's `scan_template` and bound by `bind_component` against
//! CLASS `<module qname>::<Class>` in the component file's parse, with
//! `BindSite { path: <template path>, base_row: 0 }`: rules `template_call` /
//! `template_ref`, EVIDENCE `extractor:angular_template` at the template's
//! repo-relative path and 0-based row, so the evidence fill never moves it.
//! An edge the parse already holds for the same (CLASS, METHOD, category)
//! (a host binding's, say) is kept and the template's dropped.
//!
//! DT.5c: a template call through a component member (`theme.toggle()`) is a
//! receiver CallSite from the CLASS (`this.theme`), pushed into the parse's
//! calls before the graph build reads them, so the graph binds it through
//! the member's declared type. A CallSite has no file and the graph locates a
//! CALLS in its caller's file, so it sits at the component's `templateUrl`
//! row in its TS file (`BindSite::call_row`), not the template's. A site the
//! parse already holds (same caller and receiver) is kept. A `templateUrl`
//! naming no walked file (outside the walk, gated, a typo) counts `missing`
//! and binds nothing; a template no `templateUrl` names binds nothing.
//!
//! Deterministic: files in walk order, decorators in source order, refs in
//! scan order (deduped as in DT.5a), and every map a BTreeMap. It mints no
//! node, so the LB.8 ordering rule holds wherever it runs; it runs after the
//! RPC needles, so no DC.4 `NeedleKey` input moves.
//!
//! fired_on, on stderr, once per repo where a component names a templateUrl:
//! `[angular-template] external repo=<label> components=<c> templates=<t>
//! missing=<m> calls=<k> uses=<u> unbound=<n> shadowed=<s> no_class=<x>
//! receivers=<v>`. Decorator-bound members are the component file's own
//! text, bound per file by `bind_inline` (its `decorated=` field).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::evidence;
use glia_code_domain::{FileParse, GRAPH_TYPE, edge_category, node_kind};
use glia_code_extractors::angular;
use glia_code_extractors::angular_template::{self, BindSite};
use glia_core::{NodeId, RepoId};

use crate::extract::{TS_FAMILY, detect_language};
use crate::route::ModuleQnames;

/// Per-repo tallies of the `templateUrl` graft (the fired_on marker's).
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct TemplateUrlStats {
    /// Decorated classes whose `templateUrl:` is one string literal.
    pub(super) components: usize,
    /// Distinct walked templates those name and the graft read.
    pub(super) templates: usize,
    /// `templateUrl`s naming no walked file.
    pub(super) missing: usize,
    /// CLASS -> METHOD CALLS / USES the templates added.
    pub(super) calls: usize,
    pub(super) uses: usize,
    /// Member refs naming no METHOD of the class.
    pub(super) unbound: usize,
    /// Member refs a template local of that name shadowed.
    pub(super) shadowed: usize,
    /// Decorators whose class is no CLASS of the component file's parse.
    pub(super) no_class: usize,
    /// DT.5c: receiver CallSites the templates added to the parses.
    pub(super) receivers: usize,
}

impl TemplateUrlStats {
    /// fired_on marker, once per repo with a `templateUrl` component:
    ///   `[angular-template] external repo=<label> components=<c> templates=<t> missing=<m> calls=<k> uses=<u> unbound=<n> shadowed=<s> no_class=<x> receivers=<v>`
    pub(super) fn report(&self, repo_label: &str) {
        if let Some(line) = self.marker(repo_label) {
            eprintln!("{line}");
        }
    }

    fn marker(&self, repo_label: &str) -> Option<String> {
        (self.components > 0).then(|| {
            format!(
                "[angular-template] external repo={repo_label} components={} templates={} missing={} calls={} uses={} unbound={} shadowed={} no_class={} receivers={}",
                self.components,
                self.templates,
                self.missing,
                self.calls,
                self.uses,
                self.unbound,
                self.shadowed,
                self.no_class,
                self.receivers
            )
        })
    }
}

/// Bind every walked `templateUrl` template to its component's METHODs (see
/// the module doc). `modules` is the router's MODULE plan over the same
/// `files`; a panic while reading one component file is caught, quietly,
/// and recorded in `parse_errors`.
pub(super) fn graft_template_urls(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
    modules: &ModuleQnames,
    parse_errors: &mut Vec<String>,
) -> TemplateUrlStats {
    let mut stats = TemplateUrlStats::default();
    let is_component_file = |path: &str, source: &str| {
        detect_language(path).is_some_and(|lang| TS_FAMILY.contains(&lang))
            && source.contains("templateUrl")
    };
    if !files.iter().any(|(p, s)| is_component_file(p, s)) {
        return stats;
    }
    // Walked path -> its text.
    let by_path: BTreeMap<&str, &str> = files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    // MODULE id -> the parses whose first node it is (every code parser emits
    // the file's MODULE first), in sorted language order then parse order.
    // The LA.6c bare template parse holds no node, so it is never a candidate.
    let mut langs: Vec<&'static str> = parses_by_lang.keys().copied().collect();
    langs.sort_unstable();
    let mut by_module: BTreeMap<u64, Vec<(&'static str, usize)>> = BTreeMap::new();
    for lang in &langs {
        for (slot, fp) in parses_by_lang.get(lang).into_iter().flatten().enumerate() {
            if let Some(first) = fp.nodes.first() {
                by_module.entry(first.id.0).or_default().push((*lang, slot));
            }
        }
    }
    let mut read: BTreeSet<String> = BTreeSet::new();
    for (path, source) in files {
        if !is_component_file(path, source) {
            continue;
        }
        let Ok(decorated) = crate::parallel::quiet(|| angular::decorated_classes(source)) else {
            parse_errors.push(format!("{path}: PANIC (angular templateUrl)"));
            continue;
        };
        let module_qname = modules.module_qname(path);
        let candidates = by_module.get(&modules.module_id(path, repo).0);
        for dc in decorated {
            let Some((url, url_row)) = &dc.template_url else {
                continue;
            };
            stats.components += 1;
            let Some((template_path, text)) = resolve_template_url(path, url)
                .and_then(|t| by_path.get_key_value(t.as_str()).map(|(k, v)| (*k, *v)))
            else {
                stats.missing += 1;
                continue;
            };
            read.insert(template_path.to_string());
            let class_id = dc.name.as_ref().map(|name| {
                NodeId::from_parts(
                    GRAPH_TYPE,
                    repo,
                    node_kind::CLASS,
                    &format!("{module_qname}::{name}"),
                )
            });
            let slot = class_id.and_then(|id| {
                candidates
                    .into_iter()
                    .flatten()
                    .copied()
                    .find(|(lang, slot)| {
                        parses_by_lang
                            .get(lang)
                            .and_then(|p| p.get(*slot))
                            .is_some_and(|fp| fp.nav.kind_by_id.get(&id) == Some(&node_kind::CLASS))
                    })
            });
            let (Some(class_id), Some((lang, slot))) = (class_id, slot) else {
                stats.no_class += 1;
                continue;
            };
            let Some(fp) = parses_by_lang.get_mut(lang).and_then(|p| p.get_mut(slot)) else {
                stats.no_class += 1;
                continue;
            };
            let site = BindSite {
                path: template_path,
                base_row: 0,
                host: false,
                call_row: Some(*url_row),
            };
            let bound = crate::parallel::quiet(|| {
                let refs = angular_template::scan_template(text);
                angular_template::bind_component(&fp.nodes, &fp.nav, class_id, &refs, &site)
            });
            let Ok(mut binding) = bound else {
                parse_errors.push(format!("{template_path}: PANIC (angular templateUrl)"));
                continue;
            };
            stats.unbound += binding.unbound;
            stats.shadowed += binding.shadowed;
            evidence::stamp_missing(&mut binding.edges, angular_template::EMITTER);
            for e in binding.edges {
                let held = fp
                    .edges
                    .iter()
                    .any(|x| x.from == e.from && x.to == e.to && x.category == e.category);
                if held {
                    continue;
                }
                if e.category == edge_category::CALLS {
                    stats.calls += 1;
                } else if e.category == edge_category::USES {
                    stats.uses += 1;
                }
                fp.edges.push(e);
            }
            for c in binding.call_sites {
                let held = fp
                    .calls
                    .iter()
                    .any(|x| x.from == c.from && x.qualifier == c.qualifier);
                if !held {
                    stats.receivers += 1;
                    fp.calls.push(c);
                }
            }
        }
    }
    stats.templates = read.len();
    stats
}

/// The repo-relative path a component at `component` names with `url`: its
/// directory joined with `url`, `.` and empty segments dropped and `..`
/// popping one (Angular reads `x.html` and `./x.html` alike). `None` for an
/// absolute or scheme URL, an empty one, or one that climbs above the repo
/// root.
fn resolve_template_url(component: &str, url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('/') || url.contains("://") {
        return None;
    }
    let mut parts: Vec<&str> = match component.rsplit_once('/') {
        Some((dir, _)) => dir.split('/').filter(|s| !s.is_empty()).collect(),
        None => Vec::new(),
    };
    for seg in url.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_urls_resolve_against_the_component_directory() {
        let c = "web/src/app/orders/orders.page.ts";
        assert_eq!(
            resolve_template_url(c, "./orders.page.html").as_deref(),
            Some("web/src/app/orders/orders.page.html")
        );
        assert_eq!(
            resolve_template_url(c, "orders.page.html").as_deref(),
            Some("web/src/app/orders/orders.page.html")
        );
        assert_eq!(
            resolve_template_url(c, "../shared/./card.html").as_deref(),
            Some("web/src/app/shared/card.html")
        );
        assert_eq!(
            resolve_template_url("x.ts", "./x.html").as_deref(),
            Some("x.html")
        );
        assert_eq!(resolve_template_url("a/x.ts", "../../x.html"), None);
        assert_eq!(resolve_template_url(c, "/abs/x.html"), None);
        assert_eq!(resolve_template_url(c, "https://cdn/x.html"), None);
        assert_eq!(resolve_template_url(c, "  "), None);
    }

    #[test]
    fn marker_fires_only_with_a_component() {
        assert_eq!(TemplateUrlStats::default().marker("r"), None);
        let s = TemplateUrlStats {
            components: 2,
            templates: 2,
            calls: 5,
            uses: 1,
            ..TemplateUrlStats::default()
        };
        assert_eq!(
            s.marker("web").as_deref(),
            Some(
                "[angular-template] external repo=web components=2 templates=2 missing=0 calls=5 uses=1 unbound=0 shadowed=0 no_class=0 receivers=0"
            )
        );
    }
}
