//! Angular template bindings to the component's own members (DT.5a).
//!
//! A binding in a component's inline `template:` or in its `host:` metadata
//! that calls or reads a METHOD of that very class is a call or reference the
//! source spells out at a site: a CALLS (the binding calls it) or USES (it
//! names it) edge from the component CLASS, EVIDENCE
//! `extractor:angular_template` (rules `template_call` / `template_ref` /
//! `host_call` / `host_ref`) at the binding's row. Without it every
//! template-bound handler of a component is a `dead_symbol` row: liveness never
//! walks down from a class to its methods.
//!
//! [`scan_template`] reads one template as text, byte by byte, and slices
//! only at ASCII delimiters (UTF-8 safe). The binding sites are `{{ }}`
//! interpolations; the values of `[x]=` / `[(x)]=` / `(x)=` / `*x=` /
//! `bind-x=` / `on-x=` / `bindon-x=` attributes; any other attribute value's
//! `{{ }}`; the `@if` / `@else if` / `@for` / `@switch` / `@case` headers, a
//! `@defer` header's `when <expr>` and its `interaction` / `viewport` / `hover`
//! trigger args; `@let x = ..;`. Each expression yields its top-level
//! identifiers (never a property after `.`, an object key, a string, a `$`
//! name or a keyword) and its pipe names, minus the template locals in scope
//! at the site (`@for` / `*ngFor` / `@if .. as` / `let-` / `#ref` / `@let`).
//! [`bind_component`] keeps the names that are METHODs of the class; a field,
//! a signal (STATE_VAR), an inherited or unknown name binds nothing.
//!
//! DT.5c adds two readings. A binding expression's `<member>.<m>(..)`, with
//! `<member>` a top-level member ref (no template local in scope) and exactly
//! one hop (`a.b.c(` and `a().b(` record nothing), is a [`ReceiverRef`]; the
//! component's binding turns it into a [`CallSite`] from the CLASS with
//! receiver `this.<member>`, which the graph binds through the member's
//! declared type (`graph::calls::receiver_type`, rule `receiver_type`), so a
//! template call of an injected service's method is a CALLS. And a class
//! member Angular itself invokes, named by a member decorator
//! (`angular::MemberDecorator`: a `@HostListener` method, a `@HostBinding`
//! getter, an `@Input` / view / content query setter), is a USES from its
//! class at the decorator's row (rules `host_listener` / `host_binding` /
//! `input_setter` / `query_setter`).
//!
//! fired_on, on stderr, one line per TS file with an inline template, a host
//! binding or a decorator-bound member: `[angular-template] inline
//! file=<path> components=<c> calls=<k> uses=<u> unbound=<n> shadowed=<s>
//! unread=<r> no_class=<m> receivers=<v> decorated=<d>`.

use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, GRAPH_TYPE, edge_category, line_of, node_kind,
};
use glia_core::{Confidence, Edge, EdgeCategoryId, Node, NodeId, RepoId};

use crate::angular::{self, DecoratedMember, HostEntry};

/// The evidence emitter of every edge this module mints.
pub const EMITTER: &str = "extractor:angular_template";

/// A top-level identifier a binding expression reads: `called` when a `(`
/// follows it, `row` 0-based from the scanned text's start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRef {
    pub name: String,
    pub called: bool,
    pub row: u32,
}

/// DT.5c: a call through a component member, `<member>.<method>(..)` (also
/// `member?.method(` and `this.member.method(`), one hop, the member a ref no
/// template local shadows; `row` 0-based from the scanned text's start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverRef {
    pub member: String,
    pub method: String,
    pub row: u32,
}

/// A pipe a binding applies (`| date`), 0-based row from the text's start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeRef {
    pub name: String,
    pub row: u32,
}

/// What one template (or one `host:` object) reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateRefs {
    pub members: Vec<MemberRef>,
    /// DT.5c: the one-hop calls through a member, in scan order.
    pub receivers: Vec<ReceiverRef>,
    pub pipes: Vec<PipeRef>,
    /// Member refs dropped because a template local of that name was in scope.
    pub shadowed: usize,
}

/// Where a [`TemplateRefs`] sits: the file, the row its text starts at (0 for
/// host entries, whose rows are the file's) and whether it is `host:`.
///
/// DT.5c: `call_row` is the row every receiver [`CallSite`] takes instead of
/// its binding's: a CallSite carries no file, and the graph locates a CALLS
/// in its caller's file, so an external template's receiver calls sit at the
/// component's `templateUrl` row in its TS file. `None` for an inline
/// template or host binding, whose rows are the TS file's own.
#[derive(Debug, Clone, Copy)]
pub struct BindSite<'a> {
    pub path: &'a str,
    pub base_row: u32,
    pub host: bool,
    pub call_row: Option<u32>,
}

/// The edges a component's bindings mint and the marker's counts.
#[derive(Debug, Clone, Default)]
pub struct TemplateBinding {
    /// CLASS -> METHOD CALLS / USES, one per (method, category), each at its
    /// lowest binding row.
    pub edges: Vec<Edge>,
    /// Decorated classes with an inline template or a host binding.
    pub components: usize,
    pub calls: usize,
    pub uses: usize,
    /// Member refs naming no METHOD of the class.
    pub unbound: usize,
    pub shadowed: usize,
    /// `template:` values that are no single string literal.
    pub unread: usize,
    /// Decorators whose class is no CLASS of the file.
    pub no_class: usize,
    /// DT.5c: one receiver CallSite per (member, method), from the CLASS, at
    /// its lowest binding row (or the [`BindSite::call_row`]); the caller
    /// adds them to the parse's calls for the graph to bind.
    pub call_sites: Vec<CallSite>,
    /// `call_sites.len()`, for the marker.
    pub receivers: usize,
    /// DT.5c: decorator-bound members naming a METHOD of their class.
    pub decorated: usize,
}

impl TemplateBinding {
    /// Fold `other` in: an edge already held for the same (from, to,
    /// category) keeps the lower row (ties keep the one held).
    pub fn absorb(&mut self, other: TemplateBinding) {
        for e in other.edges {
            let row = edge_row(&e);
            match self
                .edges
                .iter_mut()
                .find(|x| x.from == e.from && x.to == e.to && x.category == e.category)
            {
                Some(held) if row < edge_row(held) => *held = e,
                Some(_) => {}
                None => self.edges.push(e),
            }
        }
        for c in other.call_sites {
            match self
                .call_sites
                .iter_mut()
                .find(|x| x.from == c.from && x.qualifier == c.qualifier)
            {
                Some(held) if c.line < held.line => *held = c,
                Some(_) => {}
                None => self.call_sites.push(c),
            }
        }
        self.components += other.components;
        self.unbound += other.unbound;
        self.shadowed += other.shadowed;
        self.unread += other.unread;
        self.no_class += other.no_class;
        self.decorated += other.decorated;
        self.recount();
    }

    fn recount(&mut self) {
        self.receivers = self.call_sites.len();
        self.calls = self
            .edges
            .iter()
            .filter(|e| e.category == edge_category::CALLS)
            .count();
        self.uses = self
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES)
            .count();
    }

    /// The per-file fired_on line, `None` when the file had no inline
    /// template, host binding or decorator-bound member.
    pub fn marker(&self, path: &str) -> Option<String> {
        (self.components > 0).then(|| {
            format!(
                "[angular-template] inline file={path} components={} calls={} uses={} unbound={} shadowed={} unread={} no_class={} receivers={} decorated={}",
                self.components,
                self.calls,
                self.uses,
                self.unbound,
                self.shadowed,
                self.unread,
                self.no_class,
                self.receivers,
                self.decorated
            )
        })
    }
}

fn edge_row(e: &Edge) -> u32 {
    Evidence::read(&e.cells)
        .and_then(|ev| ev.line)
        .unwrap_or(u32::MAX)
}

/// The per-file entry: every `@Component` / `@Directive` class of `source`
/// binds its inline template (rows offset to the file), its host entries and
/// (DT.5c) its decorator-bound members against CLASS
/// `<module_qname>::<Class>`; prints the fired_on marker. A class counts
/// toward `components` when it has any of the three. Costs a `contains` and
/// nothing else on a file with no such decorator.
pub fn bind_inline(
    source: &str,
    path: &str,
    nodes: &[Node],
    nav: &CodeNav,
    module_qname: &str,
    repo: RepoId,
) -> TemplateBinding {
    let mut out = TemplateBinding::default();
    if !(source.contains("@Component(") || source.contains("@Directive(")) {
        return out;
    }
    for dc in angular::decorated_classes(source) {
        if dc.template.is_none()
            && !dc.template_unread
            && dc.host.is_empty()
            && dc.members.is_empty()
        {
            continue;
        }
        out.components += 1;
        out.unread += usize::from(dc.template_unread);
        let class_id = dc
            .name
            .as_ref()
            .map(|name| {
                let qname = format!("{module_qname}::{name}");
                NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, &qname)
            })
            .filter(|id| nav.kind_by_id.get(id) == Some(&node_kind::CLASS));
        let Some(class_id) = class_id else {
            out.no_class += 1;
            continue;
        };
        if let Some(t) = &dc.template {
            let refs = scan_template(&t.text);
            let site = BindSite {
                path,
                base_row: line_of(source, t.start),
                host: false,
                call_row: None,
            };
            out.absorb(bind_component(nodes, nav, class_id, &refs, &site));
        }
        if !dc.host.is_empty() {
            let refs = scan_host(&dc.host);
            let site = BindSite {
                path,
                base_row: 0,
                host: true,
                call_row: None,
            };
            out.absorb(bind_component(nodes, nav, class_id, &refs, &site));
        }
        if !dc.members.is_empty() {
            out.absorb(bind_members(nodes, nav, class_id, &dc.members, path));
        }
    }
    if let Some(m) = out.marker(path) {
        eprintln!("{m}");
    }
    out
}

/// The METHOD children of `class_id` by simple name (`nodes` order, first
/// wins).
fn class_methods<'a>(
    nodes: &[Node],
    nav: &'a CodeNav,
    class_id: NodeId,
) -> BTreeMap<&'a str, NodeId> {
    let mut methods: BTreeMap<&str, NodeId> = BTreeMap::new();
    for n in nodes {
        if nav.kind_by_id.get(&n.id) == Some(&node_kind::METHOD)
            && nav.parent_of.get(&n.id) == Some(&class_id)
            && let Some(name) = nav.name_by_id.get(&n.id)
        {
            methods.entry(name.as_str()).or_insert(n.id);
        }
    }
    methods
}

/// DT.5c: the USES edges `class_id`'s decorator-bound members mint: one per
/// METHOD the member names, at the decorator's row, its rule the
/// decorator's ([`angular::MemberDecorator::rule`]), deduped by method at
/// the lowest row. A member naming no METHOD of the class counts unbound.
pub fn bind_members(
    nodes: &[Node],
    nav: &CodeNav,
    class_id: NodeId,
    members: &[DecoratedMember],
    path: &str,
) -> TemplateBinding {
    let methods = class_methods(nodes, nav, class_id);
    let mut out = TemplateBinding::default();
    for m in members {
        let Some(&id) = methods.get(m.name.as_str()) else {
            out.unbound += 1;
            continue;
        };
        out.decorated += 1;
        let ev = Evidence::emitter(EMITTER)
            .rule(m.decorator.rule())
            .at(path, m.row);
        out.absorb(TemplateBinding {
            edges: vec![
                Edge::new(class_id, id, edge_category::USES, Confidence::Strong)
                    .with_cell(ev.to_cell()),
            ],
            ..TemplateBinding::default()
        });
    }
    out
}

/// The CLASS -> METHOD edges `refs` mint for `class_id`: its METHOD children
/// by simple name (`nodes` order, first wins), a called ref a CALLS, any other
/// a USES, deduped by (method, category) at the lowest row. DT.5c: each
/// receiver ref is a CallSite from `class_id` with receiver `this.<member>`
/// (one per (member, method), lowest row), whatever the class declares: the
/// graph types the member.
pub fn bind_component(
    nodes: &[Node],
    nav: &CodeNav,
    class_id: NodeId,
    refs: &TemplateRefs,
    site: &BindSite,
) -> TemplateBinding {
    let methods = class_methods(nodes, nav, class_id);
    let mut out = TemplateBinding {
        shadowed: refs.shadowed,
        ..TemplateBinding::default()
    };
    // (method name, category) -> (method, lowest row)
    let mut hits: BTreeMap<(&str, u32), (NodeId, u32)> = BTreeMap::new();
    for m in &refs.members {
        let Some((&name, &id)) = methods.get_key_value(m.name.as_str()) else {
            out.unbound += 1;
            continue;
        };
        if id == class_id {
            out.unbound += 1;
            continue;
        }
        let cat = if m.called {
            edge_category::CALLS
        } else {
            edge_category::USES
        };
        let row = site.base_row.saturating_add(m.row);
        hits.entry((name, cat.0))
            .and_modify(|h| h.1 = h.1.min(row))
            .or_insert((id, row));
    }
    let (call_rule, ref_rule) = if site.host {
        ("host_call", "host_ref")
    } else {
        ("template_call", "template_ref")
    };
    for ((_, cat), (id, row)) in hits {
        let cat = EdgeCategoryId(cat);
        let rule = if cat == edge_category::CALLS {
            call_rule
        } else {
            ref_rule
        };
        let ev = Evidence::emitter(EMITTER).rule(rule).at(site.path, row);
        out.edges
            .push(Edge::new(class_id, id, cat, Confidence::Strong).with_cell(ev.to_cell()));
    }
    // (member, method) -> lowest row
    let mut receivers: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    for r in &refs.receivers {
        let row = site
            .call_row
            .unwrap_or_else(|| site.base_row.saturating_add(r.row));
        receivers
            .entry((r.member.as_str(), r.method.as_str()))
            .and_modify(|held| *held = (*held).min(row))
            .or_insert(row);
    }
    out.call_sites = receivers
        .into_iter()
        .map(|((member, method), line)| CallSite {
            from: class_id,
            qualifier: CallQualifier::ComplexReceiver {
                receiver: format!("this.{member}"),
                name: method.to_string(),
            },
            line,
        })
        .collect();
    out.recount();
    out
}

/// What a component's `host:` entries read: an `(event)` key's statement and
/// a `[prop]` key's expression, each at its entry's row; a plain key
/// (`class`, `role`) reads nothing.
pub fn scan_host(entries: &[HostEntry]) -> TemplateRefs {
    let mut out = TemplateRefs::default();
    for e in entries {
        let k = e.key.trim();
        let bound =
            (k.starts_with('(') && k.ends_with(')')) || (k.starts_with('[') && k.ends_with(']'));
        if !bound {
            continue;
        }
        let b = e.value.as_bytes();
        let mut toks = Vec::new();
        lex(b, 0, b.len(), false, &mut toks);
        for t in toks {
            if t.pipe {
                out.pipes.push(PipeRef {
                    name: t.name,
                    row: e.row,
                });
            } else {
                if let Some(method) = t.recv {
                    out.receivers.push(ReceiverRef {
                        member: t.name.clone(),
                        method,
                        row: e.row,
                    });
                }
                out.members.push(MemberRef {
                    name: t.name,
                    called: t.called,
                    row: e.row,
                });
            }
        }
    }
    out
}

/// The binding sites of one Angular template (see the module doc), each
/// member ref and pipe at its 0-based row from the text's start.
pub fn scan_template(text: &str) -> TemplateRefs {
    let b = mask_comments(text.as_bytes());
    let nl: Vec<usize> = b
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| (c == b'\n').then_some(i))
        .collect();
    let mut s = Scanner {
        b: &b,
        nl,
        frames: vec![Vec::new()],
        elems: Vec::new(),
        wide: BTreeSet::new(),
        cands: Vec::new(),
        out: TemplateRefs::default(),
    };
    s.walk();
    s.finish()
}

// ----------------------------------------------------------------------------
// Template walk
// ----------------------------------------------------------------------------

/// A member ref not shadowed where it was read; `#ref` names, known only once
/// the whole template is read, may still shadow it.
struct Cand {
    name: String,
    called: bool,
    pos: usize,
    /// DT.5c: the method a one-hop `name.<m>(` calls.
    recv: Option<String>,
}

/// Locals scoped to one element: from its open tag to the end of its
/// matching close tag (its own attributes for a void / self-closing one).
struct ElemScope {
    start: usize,
    end: usize,
    locals: Vec<String>,
}

struct Scanner<'a> {
    b: &'a [u8],
    nl: Vec<usize>,
    /// Block and brace frames, each holding the locals it scopes;
    /// `frames[0]` is the template root and never pops.
    frames: Vec<Vec<String>>,
    elems: Vec<ElemScope>,
    /// Template-wide locals: `#x` / `ref-x`, and element locals whose element
    /// never closes.
    wide: BTreeSet<String>,
    cands: Vec<Cand>,
    out: TemplateRefs,
}

impl Scanner<'_> {
    fn row(&self, pos: usize) -> u32 {
        u32::try_from(self.nl.partition_point(|&p| p < pos)).unwrap_or(u32::MAX)
    }

    fn walk(&mut self) {
        let n = self.b.len();
        let mut i = 0;
        while i < n {
            i = match self.b[i] {
                b'{' => self.open_brace(i),
                b'}' => {
                    if self.frames.len() > 1 {
                        self.frames.pop();
                    }
                    i + 1
                }
                b'<' => match self.b.get(i + 1) {
                    Some(c) if c.is_ascii_alphabetic() => self.tag(i),
                    Some(b'/' | b'!' | b'?') => past_gt(self.b, i + 1),
                    _ => i + 1,
                },
                b'@' => self.at_block(i),
                _ => i + 1,
            };
        }
    }

    fn finish(mut self) -> TemplateRefs {
        for c in std::mem::take(&mut self.cands) {
            if self.wide.contains(&c.name) {
                self.out.shadowed += 1;
            } else {
                let row = self.row(c.pos);
                if let Some(method) = c.recv {
                    self.out.receivers.push(ReceiverRef {
                        member: c.name.clone(),
                        method,
                        row,
                    });
                }
                self.out.members.push(MemberRef {
                    name: c.name,
                    called: c.called,
                    row,
                });
            }
        }
        self.out
    }

    /// Read the expression `b[start..end]`: its pipes, and its member refs not
    /// shadowed by `extra` or a local in scope at its site.
    fn expr(&mut self, start: usize, end: usize, header: bool, extra: &[String]) {
        let mut toks = Vec::new();
        lex(self.b, start, end.min(self.b.len()), header, &mut toks);
        for t in toks {
            if t.pipe {
                let row = self.row(t.pos);
                self.out.pipes.push(PipeRef { name: t.name, row });
                continue;
            }
            let local = |l: &String| *l == t.name;
            let shadowed = extra.iter().any(local)
                || self.frames.iter().flatten().any(local)
                || self
                    .elems
                    .iter()
                    .any(|e| e.start <= t.pos && t.pos < e.end && e.locals.iter().any(local));
            if shadowed {
                self.out.shadowed += 1;
            } else {
                self.cands.push(Cand {
                    name: t.name,
                    called: t.called,
                    pos: t.pos,
                    recv: t.recv,
                });
            }
        }
    }

    /// A `{` in text: `{{` opens an interpolation (read first, so a `<` in it
    /// opens no tag); `{{{` is an ICU case brace then an interpolation; any
    /// other `{` is a counted brace that closes no block.
    fn open_brace(&mut self, i: usize) -> usize {
        let b = self.b;
        if b.get(i + 1) == Some(&b'{') && b.get(i + 2) != Some(&b'{') {
            let s = i + 2;
            let e = interp_end(b, s, b.len());
            self.expr(s, e, false, &[]);
            return (e + 2).min(b.len());
        }
        self.frames.push(Vec::new());
        i + 1
    }

    /// Every `{{ }}` of a plain attribute value `b[s..e]`.
    fn interpolations(&mut self, s: usize, e: usize) {
        let mut i = s;
        while i + 1 < e {
            if self.b[i] == b'{' && self.b[i + 1] == b'{' {
                let ie = interp_end(self.b, i + 2, e);
                self.expr(i + 2, ie, false, &[]);
                i = ie + 2;
            } else {
                i += 1;
            }
        }
    }

    /// An open tag at `b[i] == '<'`: its attributes' bindings, the locals it
    /// declares, and a `<script>` / `<style>` body skipped. Returns the
    /// offset to resume at.
    fn tag(&mut self, i: usize) -> usize {
        let b = self.b;
        let n = b.len();
        let ns = i + 1;
        let mut j = ns;
        while j < n && is_tag_name(b[j]) {
            j += 1;
        }
        let ne = j;
        let mut attrs: Vec<AttrSpan> = Vec::new();
        let mut self_closing = false;
        loop {
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= n {
                break;
            }
            if b[j] == b'>' {
                j += 1;
                break;
            }
            if b[j] == b'/' && b.get(j + 1) == Some(&b'>') {
                self_closing = true;
                j += 2;
                break;
            }
            let as_ = j;
            while j < n && !ends_attr_name(b, j) {
                j += 1;
            }
            if j == as_ {
                j += 1;
                continue;
            }
            let ae = j;
            let mut k = skip_ws(b, j, n);
            let mut value = None;
            if k < n && b[k] == b'=' {
                k = skip_ws(b, k + 1, n);
                if k < n && matches!(b[k], b'"' | b'\'') {
                    let q = b[k];
                    let vs = k + 1;
                    let ve = b[vs..].iter().position(|&c| c == q).map_or(n, |p| vs + p);
                    value = Some((vs, ve));
                    j = (ve + 1).min(n);
                } else {
                    let ve = b[k..]
                        .iter()
                        .position(|&c| c.is_ascii_whitespace() || c == b'>')
                        .map_or(n, |p| k + p);
                    value = Some((k, ve));
                    j = ve;
                }
            }
            attrs.push((as_, ae, value));
        }
        let tag_end = j;
        self.element(i, (ns, ne), &attrs, self_closing, tag_end);
        let name = &b[ns..ne];
        if !self_closing
            && (name.eq_ignore_ascii_case(b"script") || name.eq_ignore_ascii_case(b"style"))
        {
            return raw_text_end(b, tag_end, name);
        }
        tag_end
    }

    fn element(
        &mut self,
        open: usize,
        (ns, ne): (usize, usize),
        attrs: &[AttrSpan],
        self_closing: bool,
        tag_end: usize,
    ) {
        let b = self.b;
        let mut locals: Vec<String> = Vec::new();
        let mut micros: Vec<Micro> = Vec::new();
        for &(s, e, value) in attrs {
            let name = &b[s..e];
            if let Some(x) = name
                .strip_prefix(b"#")
                .or_else(|| name.strip_prefix(b"ref-"))
            {
                if is_ident_name(x) {
                    self.wide.insert(ascii(x));
                }
            } else if let Some(x) = name.strip_prefix(b"let-") {
                if is_ident_name(x) {
                    locals.push(ascii(x));
                }
            } else if name.starts_with(b"*")
                && let Some((vs, ve)) = value
            {
                let m = micro(b, vs, ve);
                locals.extend(m.decls.iter().cloned());
                micros.push(m);
            }
        }
        // A microsyntax's first segment (`let x of <expr>`, `<cond> as y`)
        // reads before the element's locals exist.
        for m in &micros {
            for &(s, e) in &m.first {
                self.expr(s, e, true, &[]);
            }
        }
        if !locals.is_empty() {
            let tag_name = &b[ns..ne];
            let end = if self_closing || is_void(tag_name) {
                Some(tag_end)
            } else {
                find_close(b, tag_end, tag_name)
            };
            match end {
                Some(end) => self.elems.push(ElemScope {
                    start: open,
                    end,
                    locals,
                }),
                // Unmatched: template-wide, so an edge is lost, never a wrong one added.
                None => self.wide.extend(locals),
            }
        }
        for m in &micros {
            for &(s, e) in &m.rest {
                self.expr(s, e, true, &[]);
            }
        }
        for &(s, e, value) in attrs {
            let Some((vs, ve)) = value else { continue };
            let name = &b[s..e];
            let bracketed = (name.starts_with(b"[") && name.ends_with(b"]"))
                || (name.starts_with(b"(") && name.ends_with(b")"));
            if bracketed
                || name.starts_with(b"bind-")
                || name.starts_with(b"on-")
                || name.starts_with(b"bindon-")
            {
                self.expr(vs, ve, false, &[]);
            } else if !(name.starts_with(b"*")
                || name.starts_with(b"#")
                || name.starts_with(b"let-")
                || name.starts_with(b"ref-"))
            {
                self.interpolations(vs, ve);
            }
        }
    }

    /// An `@` in text: a control-flow block (header read, `{` pushed with
    /// the locals it scopes) or `@let`; anything else is text.
    fn at_block(&mut self, i: usize) -> usize {
        let b = self.b;
        let n = b.len();
        let ws = i + 1;
        let mut we = ws;
        while we < n && b[we].is_ascii_alphabetic() {
            we += 1;
        }
        let word = &b[ws..we];
        let mut j = skip_ws(b, we, n);
        let mut locals = Vec::new();
        match word {
            b"if" | b"for" | b"switch" | b"case" | b"defer" => {
                let Some(close) = header_close(b, j) else {
                    return we;
                };
                locals = self.header(word, j + 1, close);
                j = skip_ws(b, close + 1, n);
            }
            b"else" => {
                if word_at(b, j, n, b"if") {
                    let k = skip_ws(b, j + 2, n);
                    let Some(close) = header_close(b, k) else {
                        return we;
                    };
                    locals = self.header(b"if", k + 1, close);
                    j = skip_ws(b, close + 1, n);
                }
            }
            b"empty" | b"default" => {}
            b"placeholder" | b"loading" | b"error" => {
                if let Some(close) = header_close(b, j) {
                    j = skip_ws(b, close + 1, n);
                }
            }
            b"let" => return self.let_decl(j),
            _ => return i + 1,
        }
        if b.get(j) == Some(&b'{') {
            self.frames.push(locals);
            j + 1
        } else {
            j.max(we)
        }
    }

    /// The header `b[s..e]` of a `@<word> (..)` block; returns the locals the
    /// block scopes.
    fn header(&mut self, word: &[u8], s: usize, e: usize) -> Vec<String> {
        let b = self.b;
        let segs = split_top(b, s, e, b";");
        let mut locals = Vec::new();
        match word {
            b"for" => {
                let Some(&(s0, e0)) = segs.first() else {
                    return locals;
                };
                let s0 = skip_ws(b, s0, e0);
                if let Some((vs, ve)) = ident_at(b, s0, e0) {
                    locals.push(ascii(&b[vs..ve]));
                    let k = skip_ws(b, ve, e0);
                    let k = if word_at(b, k, e0, b"of") { k + 2 } else { k };
                    self.expr(k, e0, true, &[]);
                }
                for &(ss, se) in &segs[1..] {
                    let ss = skip_ws(b, ss, se);
                    if word_at(b, ss, se, b"let") {
                        for (as_, ae) in split_top(b, ss + 3, se, b",") {
                            let as_ = skip_ws(b, as_, ae);
                            if let Some((vs, ve)) = ident_at(b, as_, ae) {
                                locals.push(ascii(&b[vs..ve]));
                            }
                        }
                    }
                }
                for &(ss, se) in &segs[1..] {
                    let ss = skip_ws(b, ss, se);
                    if word_at(b, ss, se, b"track") {
                        self.expr(ss + 5, se, true, &locals);
                    }
                }
            }
            b"if" => {
                if let Some(&(s0, e0)) = segs.first() {
                    self.expr(s0, e0, true, &[]);
                }
                for &(ss, se) in &segs[1..] {
                    let ss = skip_ws(b, ss, se);
                    if word_at(b, ss, se, b"as") {
                        let k = skip_ws(b, ss + 2, se);
                        if let Some((vs, ve)) = ident_at(b, k, se) {
                            locals.push(ascii(&b[vs..ve]));
                        }
                    }
                }
            }
            b"defer" => {
                for &(ss, se) in &segs {
                    let mut k = skip_ws(b, ss, se);
                    for prefix in [&b"prefetch"[..], b"hydrate"] {
                        if word_at(b, k, se, prefix) {
                            k = skip_ws(b, k + prefix.len(), se);
                        }
                    }
                    if word_at(b, k, se, b"when") {
                        self.expr(k + 4, se, true, &[]);
                    } else if word_at(b, k, se, b"on") {
                        for (ts, te) in split_top(b, k + 2, se, b",") {
                            let ts = skip_ws(b, ts, te);
                            let Some((vs, ve)) = ident_at(b, ts, te) else {
                                continue;
                            };
                            let trigger = &b[vs..ve];
                            let p = skip_ws(b, ve, te);
                            if matches!(trigger, b"interaction" | b"viewport" | b"hover")
                                && let Some(close) = header_close(b, p).filter(|&c| c < te)
                            {
                                self.expr(p + 1, close, true, &[]);
                            }
                        }
                    }
                }
            }
            // @switch / @case: one expression.
            _ => self.expr(s, e, true, &[]),
        }
        locals
    }

    /// `@let x = <expr>;` with `b[j]` at `x`: the expression is read, then x
    /// scopes the rest of the enclosing block.
    fn let_decl(&mut self, j: usize) -> usize {
        let b = self.b;
        let n = b.len();
        let Some((vs, ve)) = ident_at(b, j, n) else {
            return j;
        };
        let eq = skip_ws(b, ve, n);
        if b.get(eq) != Some(&b'=') {
            return ve;
        }
        let end = statement_end(b, eq + 1, n);
        self.expr(eq + 1, end, false, &[]);
        let name = ascii(&b[vs..ve]);
        if let Some(top) = self.frames.last_mut() {
            top.push(name);
        }
        if b.get(end) == Some(&b';') {
            end + 1
        } else {
            end
        }
    }
}

/// A parsed `*x="..."` microsyntax: the names it declares (`let x`,
/// `as y`) and its expression ranges, the first segment's apart.
struct Micro {
    decls: Vec<String>,
    first: Vec<(usize, usize)>,
    rest: Vec<(usize, usize)>,
}

fn micro(b: &[u8], vs: usize, ve: usize) -> Micro {
    let mut m = Micro {
        decls: Vec::new(),
        first: Vec::new(),
        rest: Vec::new(),
    };
    for (idx, (s, e)) in split_top(b, vs, ve, b";,").into_iter().enumerate() {
        let mut s = skip_ws(b, s, e);
        if s >= e {
            continue;
        }
        let mut keyed = idx > 0;
        if word_at(b, s, e, b"let") {
            let k = skip_ws(b, s + 3, e);
            let Some((ns, ne)) = ident_at(b, k, e) else {
                continue;
            };
            m.decls.push(ascii(&b[ns..ne]));
            s = skip_ws(b, ne, e);
            if s >= e || b[s] == b'=' {
                // `let i = index`: a context name, never an expression.
                continue;
            }
            keyed = true;
        }
        if keyed {
            s = strip_key(b, s, e);
        }
        if word_at(b, s, e, b"as") {
            let k = skip_ws(b, s + 2, e);
            if let Some((ns, ne)) = ident_at(b, k, e) {
                m.decls.push(ascii(&b[ns..ne]));
            }
            continue;
        }
        let mut e = e;
        if let Some((at, name)) = trailing_as(b, s, e) {
            m.decls.push(name);
            e = at;
        }
        if idx == 0 {
            m.first.push((s, e));
        } else {
            m.rest.push((s, e));
        }
    }
    m
}

/// Past a microsyntax key at `b[s]` (`trackBy:`, `of`, `else`): an
/// identifier followed by `:`, or by whitespace and the start of an operand.
fn strip_key(b: &[u8], s: usize, e: usize) -> usize {
    let Some((_, ke)) = ident_at(b, s, e) else {
        return s;
    };
    let j = skip_ws(b, ke, e);
    if j < e && b[j] == b':' && b.get(j + 1) != Some(&b':') {
        return j + 1;
    }
    let operand =
        j < e && (is_ident_byte(b[j]) || matches!(b[j], b'\'' | b'"' | b'`' | b'[' | b'{'));
    if j > ke && operand { j } else { s }
}

/// The last top-level `as <name>` closing `b[s..e]`: its offset and the name.
fn trailing_as(b: &[u8], s: usize, e: usize) -> Option<(usize, String)> {
    let mut i = s;
    let mut last = None;
    while i < e {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_quoted(b, i, e);
                continue;
            }
            c if is_ident_byte(c) => {
                let st = i;
                while i < e && is_ident_byte(b[i]) {
                    i += 1;
                }
                if &b[st..i] == b"as" && st > s && b[st - 1].is_ascii_whitespace() {
                    last = Some(st);
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let at = last?;
    let k = skip_ws(b, at + 2, e);
    let (ns, ne) = ident_at(b, k, e)?;
    (skip_ws(b, ne, e) >= e).then(|| (at, ascii(&b[ns..ne])))
}

// ----------------------------------------------------------------------------
// Expression lexer
// ----------------------------------------------------------------------------

/// JavaScript words that are never a component member.
const KEYWORDS: &[&[u8]] = &[
    b"true",
    b"false",
    b"null",
    b"undefined",
    b"this",
    b"typeof",
    b"void",
    b"new",
    b"in",
    b"instanceof",
];

/// Control-flow and microsyntax words, never refs inside a header or a
/// microsyntax. In a plain binding they are ordinary names: `{{ count() }}`
/// reads the component's `count`, since no implicit local is bare there.
const HEADER_WORDS: &[&[u8]] = &[
    b"let", b"of", b"as", b"track", b"when", b"on", b"else", b"if", b"then", b"index", b"count",
    b"first", b"last", b"even", b"odd",
];

struct Tok {
    name: String,
    pos: usize,
    called: bool,
    pipe: bool,
    /// DT.5c: the method of a one-hop `name.<m>(` ([`receiver_call`]).
    recv: Option<String>,
}

/// The top-level identifiers and pipe names of the expression `b[start..end]`.
/// An identifier is no member ref after `.` / `?.` / `!.` (a property; `...x`
/// is a spread and stays one), as an object key (`{ active: x }`), when it
/// starts with `$` (`$event`, `$index`, `$any`), or as a keyword; `this.x`
/// reads `x`. A `|` that is not `||` names the pipe after it.
fn lex(b: &[u8], start: usize, end: usize, header: bool, out: &mut Vec<Tok>) {
    let mut i = start;
    let mut prev = b'(';
    let mut dots = 0usize;
    let mut after_this = false;
    let mut pipe_next = false;
    while i < end {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' | b'`' => {
                i = skip_quoted(b, i, end);
                (prev, dots, after_this, pipe_next) = (b'"', 0, false, false);
            }
            b'0'..=b'9' => {
                while i < end && (is_ident_byte(b[i]) || b[i] == b'.') {
                    i += 1;
                }
                (prev, dots, after_this) = (b'0', 0, false);
            }
            c if is_ident_byte(c) => {
                let s = i;
                while i < end && is_ident_byte(b[i]) {
                    i += 1;
                }
                let name = &b[s..i];
                let dotted = dots > 0 && dots != 3;
                let this_member = after_this;
                let before = prev;
                (prev, dots, after_this) = (b'a', 0, false);
                let j = skip_ws(b, i, end);
                let next = b.get(j).copied().filter(|_| j < end).unwrap_or(0);
                if std::mem::take(&mut pipe_next) {
                    if name[0] != b'$' {
                        out.push(Tok {
                            name: ascii(name),
                            pos: s,
                            called: false,
                            pipe: true,
                            recv: None,
                        });
                    }
                    continue;
                }
                if name[0] == b'$' || (dotted && !this_member) {
                    continue;
                }
                if name == b"this" {
                    after_this = next == b'.';
                    continue;
                }
                if KEYWORDS.contains(&name) || (header && HEADER_WORDS.contains(&name)) {
                    continue;
                }
                if next == b':' && b.get(j + 1) != Some(&b':') && matches!(before, b'{' | b',') {
                    continue;
                }
                let called = next == b'('
                    || (next == b'?'
                        && b.get(j + 1) == Some(&b'.')
                        && b.get(skip_ws(b, j + 2, end)) == Some(&b'('));
                out.push(Tok {
                    name: ascii(name),
                    pos: s,
                    called,
                    pipe: false,
                    recv: receiver_call(b, j, end),
                });
            }
            b'|' => {
                if b.get(i + 1) == Some(&b'|') && i + 1 < end {
                    i += 2;
                } else {
                    pipe_next = true;
                    i += 1;
                }
                (prev, dots, after_this) = (b'|', 0, false);
            }
            b'.' => {
                dots += 1;
                prev = b'.';
                i += 1;
            }
            _ => {
                (prev, dots, after_this) = (c, 0, false);
                i += 1;
            }
        }
    }
}

/// DT.5c: with `b[j]` the first non-space byte after a member ref, the
/// method of a one-hop call through it: `.m(`, `?.m(` or `!.m(`, then `(` or
/// `?.(` (an optional call). A second hop (`.m.n(`), an index or a property
/// read is `None`.
fn receiver_call(b: &[u8], j: usize, end: usize) -> Option<String> {
    let at = |k: usize| b.get(k).copied().filter(|_| k < end);
    let dot = match (at(j)?, at(j + 1)) {
        (b'.', Some(c)) if c != b'.' => j + 1,
        (b'?' | b'!', Some(b'.')) => j + 2,
        _ => return None,
    };
    let (ms, me) = ident_at(b, skip_ws(b, dot, end), end)?;
    let p = skip_ws(b, me, end);
    let called = at(p) == Some(b'(')
        || (at(p) == Some(b'?')
            && at(p + 1) == Some(b'.')
            && at(skip_ws(b, p + 2, end)) == Some(b'('));
    called.then(|| ascii(&b[ms..me]))
}

// ----------------------------------------------------------------------------
// Byte helpers (every slice cut at an ASCII byte)
// ----------------------------------------------------------------------------

/// `<!-- .. -->` turned to spaces, newlines kept (an unclosed comment runs to
/// the end).
fn mask_comments(src: &[u8]) -> Vec<u8> {
    let mut b = src.to_vec();
    let mut i = 0;
    while i + 4 <= b.len() {
        if &b[i..i + 4] != b"<!--" {
            i += 1;
            continue;
        }
        let end = b[i + 4..]
            .windows(3)
            .position(|w| w == b"-->")
            .map_or(b.len(), |p| i + 4 + p + 3);
        for c in &mut b[i..end] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
        i = end;
    }
    b
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// One attribute of an open tag: its name's byte range, then its value's
/// byte range when it has one.
type AttrSpan = (usize, usize, Option<(usize, usize)>);

/// Whether `b[j]` ends an attribute name: whitespace, `=`, `>`, a quote, or
/// the `/>` of a self-closing tag.
fn ends_attr_name(b: &[u8], j: usize) -> bool {
    b[j].is_ascii_whitespace()
        || matches!(b[j], b'=' | b'>' | b'"' | b'\'')
        || (b[j] == b'/' && b.get(j + 1) == Some(&b'>'))
}

fn is_tag_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':' | b'.')
}

fn is_ident_name(x: &[u8]) -> bool {
    x.first().is_some_and(|c| !c.is_ascii_digit()) && x.iter().all(|&c| is_ident_byte(c))
}

fn ascii(x: &[u8]) -> String {
    String::from_utf8_lossy(x).into_owned()
}

fn skip_ws(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// The identifier starting at `b[s]`, if one does.
fn ident_at(b: &[u8], s: usize, e: usize) -> Option<(usize, usize)> {
    let c = *b.get(s).filter(|_| s < e)?;
    if !is_ident_byte(c) || c.is_ascii_digit() {
        return None;
    }
    let mut i = s;
    while i < e && is_ident_byte(b[i]) {
        i += 1;
    }
    Some((s, i))
}

/// `w` as a whole word at `b[s..e]`.
fn word_at(b: &[u8], s: usize, e: usize, w: &[u8]) -> bool {
    s + w.len() <= e
        && b.get(s..s + w.len()) == Some(w)
        && b.get(s + w.len())
            .is_none_or(|&c| s + w.len() >= e || !is_ident_byte(c))
}

/// Past the quoted literal opening at `b[i]` (escapes honoured), or `end`.
fn skip_quoted(b: &[u8], i: usize, end: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < end {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return j + 1,
            _ => j += 1,
        }
    }
    end
}

/// The offset of the `}}` closing an interpolation whose body starts at `s`
/// (quotes honoured), or `end`.
fn interp_end(b: &[u8], s: usize, end: usize) -> usize {
    let mut i = s;
    while i < end {
        match b[i] {
            b'\'' | b'"' | b'`' => i = skip_quoted(b, i, end),
            b'}' if b.get(i + 1) == Some(&b'}') && i + 1 < end => return i,
            _ => i += 1,
        }
    }
    end
}

/// With `b[j] == '('`, the offset of its matching `)` (quotes and nesting
/// honoured).
fn header_close(b: &[u8], j: usize) -> Option<usize> {
    if b.get(j) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = j;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_quoted(b, i, b.len());
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return (b[i] == b')').then_some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `b[s..e]` split at every top-level byte of `seps` (outside quotes and
/// brackets).
fn split_top(b: &[u8], s: usize, e: usize, seps: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut from = s;
    let mut i = s;
    while i < e {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_quoted(b, i, e);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            c if depth == 0 && seps.contains(&c) => {
                out.push((from, i));
                from = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push((from, e));
    out
}

/// The first top-level `;` from `s`, or the bracket closing the enclosing
/// block when the `;` is missing, or `end`.
fn statement_end(b: &[u8], s: usize, end: usize) -> usize {
    let mut depth = 0usize;
    let mut i = s;
    while i < end {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_quoted(b, i, end);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => return i,
            b')' | b']' | b'}' => depth -= 1,
            b';' if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    end
}

/// Past the next `>` from `i`, quotes honoured.
fn past_gt(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => i = skip_quoted(b, i, b.len()),
            b'>' => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

fn is_void(name: &[u8]) -> bool {
    const VOID: &[&[u8]] = &[
        b"area", b"base", b"br", b"col", b"embed", b"hr", b"img", b"input", b"link", b"meta",
        b"param", b"source", b"track", b"wbr",
    ];
    VOID.iter().any(|v| name.eq_ignore_ascii_case(v))
}

/// `name` as a whole tag name at `b[at..]` (case-insensitive).
fn tag_name_at(b: &[u8], at: usize, name: &[u8]) -> bool {
    b.get(at..at + name.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(name))
        && b.get(at + name.len()).is_none_or(|&c| !is_tag_name(c))
}

/// The end of the close tag matching an element named `name` whose open tag
/// ends at `from`, counting nested same-name elements; `None` when it never
/// closes.
fn find_close(b: &[u8], from: usize, name: &[u8]) -> Option<usize> {
    let mut depth = 1usize;
    let mut k = from;
    while k < b.len() {
        if b[k] != b'<' {
            k += 1;
            continue;
        }
        if b.get(k + 1) == Some(&b'/') && tag_name_at(b, k + 2, name) {
            let e = past_gt(b, k + 2);
            depth -= 1;
            if depth == 0 {
                return Some(e);
            }
            k = e;
        } else if tag_name_at(b, k + 1, name) {
            let e = past_gt(b, k + 1);
            if !(e >= 2 && b[e - 2] == b'/') {
                depth += 1;
            }
            k = e;
        } else {
            k += 1;
        }
    }
    None
}

/// Past the `</name ..>` closing a raw-text element (`script` / `style`).
fn raw_text_end(b: &[u8], from: usize, name: &[u8]) -> usize {
    let mut k = from;
    while k + 1 < b.len() {
        if b[k] == b'<' && b[k + 1] == b'/' && tag_name_at(b, k + 2, name) {
            return past_gt(b, k + 2);
        }
        k += 1;
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(name, called)` of every member ref, sorted.
    fn refs(text: &str) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = scan_template(text)
            .members
            .into_iter()
            .map(|m| (m.name, m.called))
            .collect();
        v.sort();
        v
    }

    fn r(name: &str, called: bool) -> (String, bool) {
        (name.to_string(), called)
    }

    #[test]
    fn every_binding_form() {
        let t = r#"<a [href]="link" [(ngModel)]="draft" (click)="go(); log()" *ngIf="shown()"
             bind-title="caption" on-focus="onFocus()" bindon-value="val" title="Hi {{ who() }}">{{ text() }}</a>
            @if (open()) { x } @else if (closed()) { y } @else { z }
            @switch (mode()) { @case (modeA()) { a } @default { b } }
            @let total = sum();"#;
        assert_eq!(
            refs(t),
            vec![
                r("caption", false),
                r("closed", true),
                r("draft", false),
                r("go", true),
                r("link", false),
                r("log", true),
                r("mode", true),
                r("modeA", true),
                r("onFocus", true),
                r("open", true),
                r("shown", true),
                r("sum", true),
                r("text", true),
                r("val", false),
                r("who", true),
            ]
        );
    }

    #[test]
    fn comments_strings_and_plain_attributes_read_nothing() {
        let t = r#"<!-- <b (click)="hidden()">{{ gone() }}</b> -->
            <p class="save()" title="reload">{{ 'label()' }} {{ "x" + y }}</p>"#;
        assert_eq!(refs(t), vec![r("y", false)]);
    }

    #[test]
    fn pipes_or_keys_this_and_dollar_names() {
        let t = r#"{{ a || b() | date: fmt() | uppercase }}
            <i [ngClass]="{ active: isActive(), 'x-y': flag }" (click)="this.save($event); $any(e).id"></i>
            <li *ngFor="let it of items; trackBy: trackFn"></li>"#;
        let got = scan_template(t);
        let mut pipes: Vec<&str> = got.pipes.iter().map(|p| p.name.as_str()).collect();
        pipes.sort_unstable();
        assert_eq!(pipes, ["date", "uppercase"]);
        assert_eq!(
            refs(t),
            vec![
                r("a", false),
                r("b", true),
                r("e", false),
                r("flag", false),
                r("fmt", true),
                r("isActive", true),
                r("items", false),
                r("save", true),
                r("trackFn", false),
            ]
        );
    }

    #[test]
    fn interpolation_is_read_before_tags_and_let_attr_reads_nothing() {
        let t = r#"{{ a<b }} <ng-template let-row="item"><i>{{ row }}</i></ng-template> {{ c() }}"#;
        assert_eq!(refs(t), vec![r("a", false), r("b", false), r("c", true)]);
    }

    #[test]
    fn defer_reads_when_and_trigger_args_only() {
        assert_eq!(
            refs("@defer (on viewport; when ready()) { <x/> } @placeholder (minimum 500ms) { p }"),
            vec![r("ready", true)]
        );
        assert_eq!(
            refs("@defer (on interaction(btn), timer(5s); prefetch on idle) { x }"),
            vec![r("btn", false)]
        );
    }

    #[test]
    fn for_locals_scope_header_track_and_block_only() {
        let t = "{{ row() }}\n@for (entry of offers(); track trackById($index, entry); let i = $index) {\n  {{ entry.note }} {{ i }}\n}\n@for (row of rows; track row) { {{ row }} }\n{{ entry() }}";
        let got = scan_template(t);
        let mut v: Vec<(String, bool)> = got
            .members
            .iter()
            .map(|m| (m.name.clone(), m.called))
            .collect();
        v.sort();
        assert_eq!(
            v,
            vec![
                r("entry", true),
                r("offers", true),
                r("row", true),
                r("rows", false),
                r("trackById", true),
            ]
        );
        // entry (track), entry.note, i, row (track), row (block)
        assert_eq!(got.shadowed, 5);
    }

    #[test]
    fn element_and_if_locals() {
        let t = r#"<li *ngFor="let user of users(); let i = index; index as j">{{ user.name }} {{ i }} {{ j }}</li>
            {{ user() }}
            <div *ngIf="profile$ | async as profile"><b>{{ profile }}</b></div>
            @if (account(); as acct) { {{ acct }} } @else { {{ acct() }} }
            <ng-template #empty let-x><i>{{ x }}</i></ng-template>
            {{ x() }} {{ empty }}
            <input #box (keyup)="find(box.value)"/>"#;
        assert_eq!(
            refs(t),
            vec![
                r("account", true),
                r("acct", true),
                r("find", true),
                r("profile$", false),
                r("user", true),
                r("users", true),
                r("x", true),
            ]
        );
    }

    #[test]
    fn unmatched_element_local_is_template_wide() {
        let t = r#"<div *ngFor="let it of list"> {{ it }} {{ it() }}"#;
        assert_eq!(refs(t), vec![r("list", false)]);
    }

    #[test]
    fn icu_braces_never_close_a_block() {
        let t = "@for (n of nums; track n) {\n  <span i18n>{count, plural, =0 {none} other {{{n}} items}}</span>\n  {{ n }}\n}\n{{ n() }}";
        let got = scan_template(t);
        let v: Vec<(&str, u32)> = got
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.row))
            .collect();
        assert_eq!(v, vec![("nums", 0), ("n", 4)]);
        assert_eq!(got.shadowed, 3);
    }

    #[test]
    fn rows_survive_a_four_byte_char() {
        let t = "<p>\u{1F600} {{ one() }}</p>\n\u{1F600}\n<b (click)=\"two()\">\u{1F600}</b>";
        let got = scan_template(t);
        let v: Vec<(&str, u32)> = got
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.row))
            .collect();
        assert_eq!(v, vec![("one", 0), ("two", 2)]);
    }

    #[test]
    fn script_and_style_bodies_are_skipped() {
        let t =
            "<style>.a { color: red } {{ no() }}</style><script>{{ nope() }}</script>{{ yes() }}";
        assert_eq!(refs(t), vec![r("yes", true)]);
    }

    #[test]
    fn host_reads_event_and_property_keys_only() {
        let entries = [
            HostEntry {
                key: "class".into(),
                value: "busy()".into(),
                row: 3,
            },
            HostEntry {
                key: "(window:resize)".into(),
                value: "onResize($event)".into(),
                row: 4,
            },
            HostEntry {
                key: "[class.on]".into(),
                value: "isOn".into(),
                row: 5,
            },
        ];
        let got = scan_host(&entries);
        assert_eq!(
            got.members,
            vec![
                MemberRef {
                    name: "onResize".into(),
                    called: true,
                    row: 4
                },
                MemberRef {
                    name: "isOn".into(),
                    called: false,
                    row: 5
                },
            ]
        );
    }

    fn class_with(members: &[(&str, glia_core::NodeKindId)]) -> (Vec<Node>, CodeNav, NodeId) {
        let repo = RepoId(1);
        let class_q = "m::C";
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, class_q);
        let mut nav = CodeNav::default();
        nav.record(class_id, "C", class_q, node_kind::CLASS, None);
        let mut nodes = vec![Node {
            id: class_id,
            repo,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        }];
        for (name, kind) in members {
            let q = format!("{class_q}::{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, *kind, &q);
            nav.record(id, name, &q, *kind, Some(class_id));
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
        }
        (nodes, nav, class_id)
    }

    #[test]
    fn binds_methods_only_and_dedupes_at_the_lowest_row() {
        let (nodes, nav, class_id) = class_with(&[
            ("save", node_kind::METHOD),
            ("label", node_kind::METHOD),
            ("items", node_kind::STATE_VAR),
        ]);
        let refs = scan_template(
            "<b (click)=\"save()\"></b>\n{{ save() }} {{ items() }} {{ count }} {{ label }}\n{{ label }}",
        );
        let site = BindSite {
            path: "m.ts",
            base_row: 10,
            host: false,
            call_row: None,
        };
        let mut got = bind_component(&nodes, &nav, class_id, &refs, &site);
        assert_eq!((got.calls, got.uses, got.unbound), (1, 1, 2));
        let rows: Vec<(String, u32, Option<String>)> = got
            .edges
            .iter()
            .map(|e| {
                let ev = Evidence::read(&e.cells).expect("evidence");
                (nav.name_by_id[&e.to].clone(), ev.line.unwrap_or(0), ev.rule)
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("label".to_string(), 11, Some("template_ref".to_string())),
                ("save".to_string(), 10, Some("template_call".to_string())),
            ]
        );
        // A host binding at a lower row replaces the template's.
        let host = scan_host(&[HostEntry {
            key: "(click)".into(),
            value: "save()".into(),
            row: 2,
        }]);
        got.absorb(bind_component(
            &nodes,
            &nav,
            class_id,
            &host,
            &BindSite {
                path: "m.ts",
                base_row: 0,
                host: true,
                call_row: None,
            },
        ));
        let save = got
            .edges
            .iter()
            .find(|e| nav.name_by_id[&e.to] == "save")
            .and_then(|e| Evidence::read(&e.cells))
            .expect("save edge");
        assert_eq!(
            (save.line, save.rule.as_deref()),
            (Some(2), Some("host_call"))
        );
        assert_eq!((got.calls, got.uses), (1, 1));
        assert!(
            got.edges
                .iter()
                .all(|e| e.from == class_id && e.to != class_id)
        );
    }

    #[test]
    fn bind_inline_reads_template_and_host_against_the_file_class() {
        let src = "@Component({\n  host: { '(keydown)': 'key()' },\n  template: `\n    <b (click)=\"go()\">{{ nope() }}</b>`,\n})\nexport class C {}\n";
        let (nodes, nav, class_id) =
            class_with(&[("go", node_kind::METHOD), ("key", node_kind::METHOD)]);
        let out = bind_inline(src, "m.ts", &nodes, &nav, "m", RepoId(1));
        let mut got: Vec<(String, u32)> = out
            .edges
            .iter()
            .map(|e| {
                assert_eq!(e.from, class_id);
                let ev = Evidence::read(&e.cells).expect("evidence");
                (nav.name_by_id[&e.to].clone(), ev.line.unwrap_or(0))
            })
            .collect();
        got.sort();
        assert_eq!(got, vec![("go".to_string(), 3), ("key".to_string(), 1)]);
        assert_eq!(
            out.marker("m.ts").as_deref(),
            Some(
                "[angular-template] inline file=m.ts components=1 calls=2 uses=0 unbound=1 shadowed=0 unread=0 no_class=0 receivers=0 decorated=0"
            )
        );
        // No decorator: nothing read, no marker.
        let none = bind_inline(
            "export class C { go() {} }",
            "m.ts",
            &nodes,
            &nav,
            "m",
            RepoId(1),
        );
        assert!(none.edges.is_empty() && none.marker("m.ts").is_none());
    }

    /// `(member, method, row)` of every receiver ref, in scan order.
    fn receivers(text: &str) -> Vec<(String, String, u32)> {
        scan_template(text)
            .receivers
            .into_iter()
            .map(|r| (r.member, r.method, r.row))
            .collect()
    }

    /// DT.5c: a one-hop call through a member is a receiver ref (also
    /// `?.`, `this.` and a host entry's statement); a chain, a call result's
    /// method, a property read and a template local's call are not, and the
    /// member is still an ordinary (unbound) member ref.
    #[test]
    fn receiver_refs_are_one_hop_calls_through_an_unshadowed_member() {
        let t = "<b (click)=\"theme.toggle()\">{{ auth?.logout() }}</b>\n\
                 <i (click)=\"this.store.save(x); auth.session.end()\"></i>\n\
                 <i [title]=\"items().map(f) + svc.label\" (blur)=\"svc.check?.()\"></i>\n\
                 @for (auth of sessions; track $index) { <b (click)=\"auth.stop()\"></b> }\n\
                 <input #box (keyup)=\"box.focus()\"/>\n\
                 {{ auth.logout() }}";
        assert_eq!(
            receivers(t),
            vec![
                ("theme".to_string(), "toggle".to_string(), 0),
                ("auth".to_string(), "logout".to_string(), 0),
                ("store".to_string(), "save".to_string(), 1),
                ("svc".to_string(), "check".to_string(), 2),
                ("auth".to_string(), "logout".to_string(), 5),
            ]
        );
        let got = scan_template(t);
        assert!(got.members.iter().any(|m| m.name == "theme" && !m.called));
        // The @for block's `auth` and the `#box` ref shadow their calls.
        assert_eq!(got.shadowed, 2);
        let host = scan_host(&[HostEntry {
            key: "(document:click)".into(),
            value: "tracker.hit($event)".into(),
            row: 4,
        }]);
        assert_eq!(
            host.receivers,
            vec![ReceiverRef {
                member: "tracker".into(),
                method: "hit".into(),
                row: 4
            }]
        );
    }

    /// DT.5c: each receiver ref is one CallSite from the CLASS with receiver
    /// `this.<member>`, deduped by (member, method) at the lowest row: an
    /// inline template's rows are the file's (`base_row` + row), an external
    /// template's are its `call_row`.
    #[test]
    fn receiver_call_sites_come_from_the_class() {
        let (nodes, nav, class_id) = class_with(&[("save", node_kind::METHOD)]);
        let refs = scan_template(
            "<b (click)=\"theme.toggle()\"></b>\n{{ theme.toggle() }} {{ auth.logout() }}",
        );
        let site = |call_row| BindSite {
            path: "m.html",
            base_row: 7,
            host: false,
            call_row,
        };
        let shape = |b: &TemplateBinding| -> Vec<(String, String, u32)> {
            b.call_sites
                .iter()
                .map(|c| {
                    assert_eq!(c.from, class_id);
                    match &c.qualifier {
                        CallQualifier::ComplexReceiver { receiver, name } => {
                            (receiver.clone(), name.clone(), c.line)
                        }
                        q => panic!("unexpected qualifier {q:?}"),
                    }
                })
                .collect()
        };
        let inline = bind_component(&nodes, &nav, class_id, &refs, &site(None));
        assert_eq!(
            shape(&inline),
            vec![
                ("this.auth".to_string(), "logout".to_string(), 8),
                ("this.theme".to_string(), "toggle".to_string(), 7),
            ]
        );
        assert_eq!((inline.receivers, inline.calls, inline.uses), (2, 0, 0));
        let external = bind_component(&nodes, &nav, class_id, &refs, &site(Some(3)));
        assert_eq!(
            shape(&external),
            vec![
                ("this.auth".to_string(), "logout".to_string(), 3),
                ("this.theme".to_string(), "toggle".to_string(), 3),
            ]
        );
        // Absorbing a lower-row duplicate keeps one site at that row.
        let mut both = external;
        both.absorb(inline);
        assert_eq!(
            shape(&both),
            vec![
                ("this.auth".to_string(), "logout".to_string(), 3),
                ("this.theme".to_string(), "toggle".to_string(), 3),
            ]
        );
        assert_eq!(both.receivers, 2);
    }

    /// DT.5c: a decorated class's decorator-bound METHODs are USES from the
    /// class at the decorator's row, with the decorator's rule; a decorated
    /// field and an undecorated method bind nothing. A class with members
    /// alone (a `templateUrl` component) fires the marker.
    #[test]
    fn bind_inline_binds_decorator_bound_members() {
        let src = "@Component({ templateUrl: './s.html' })\n\
                   export class C {\n\
                   \x20 @HostListener('window:resize', ['$event'])\n\
                   \x20 onResize(): void {}\n\
                   \x20 @HostBinding('class.open') get isOpen() { return true; }\n\
                   \x20 @Input() set title(v: string) {}\n\
                   \x20 @ViewChild('c') private set canvas(el: unknown) {}\n\
                   \x20 @Input() size = 1;\n\
                   \x20 close(): void {}\n\
                   }\n";
        let (nodes, nav, class_id) = class_with(&[
            ("onResize", node_kind::METHOD),
            ("isOpen", node_kind::METHOD),
            ("title", node_kind::METHOD),
            ("canvas", node_kind::METHOD),
            ("close", node_kind::METHOD),
        ]);
        let out = bind_inline(src, "m.ts", &nodes, &nav, "m", RepoId(1));
        let got: Vec<(String, String, u32)> = out
            .edges
            .iter()
            .map(|e| {
                assert_eq!((e.from, e.category), (class_id, edge_category::USES));
                let ev = Evidence::read(&e.cells).expect("evidence");
                assert_eq!(ev.emitter, EMITTER);
                (
                    nav.name_by_id[&e.to].clone(),
                    ev.rule.unwrap_or_default(),
                    ev.line.unwrap_or(0),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("onResize".to_string(), "host_listener".to_string(), 2),
                ("isOpen".to_string(), "host_binding".to_string(), 4),
                ("title".to_string(), "input_setter".to_string(), 5),
                ("canvas".to_string(), "query_setter".to_string(), 6),
            ]
        );
        assert_eq!(
            out.marker("m.ts").as_deref(),
            Some(
                "[angular-template] inline file=m.ts components=1 calls=0 uses=4 unbound=0 shadowed=0 unread=0 no_class=0 receivers=0 decorated=4"
            )
        );
    }
}
