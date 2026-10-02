//! Endpoint fold (A11.2): resolve a client call's base URL through the repo
//! [`ConstTable`], split the authority off, and key the ENDPOINT on the path.
//!
//! `` this.http.get(`${environment.apiUrl}/users`) `` parses to
//! `endpoint:GET:${…}/users`. The HTTP resolver can only pair that through its
//! BaseFold tier: Medium confidence, and blind to which service the base
//! names. With `environment.apiUrl = 'http://users-service:8080'` in the repo
//! table, this pass re-keys the node to `endpoint:GET:/users`, which pairs at
//! the Exact tier. It also records `"host":"users-service:8080"` on the
//! ENDPOINT_HIT cell, the input A11.4's host narrowing reads. When the base is
//! bound differently per deployment (`environment.ts` vs
//! `environment.prod.ts`), it also records every binding's authority as
//! `"hosts":[…]`, first binding first (A11.4, see [`deployment_hosts`]).
//!
//! WHERE IT RUNS. After the parse cache, over every FileParse of the repo,
//! whether it came from the cache or not (`build_graphs_for_repo`). The cache
//! holds the PRE-fold parse (`route.rs` stores a clone before this runs), so it
//! stays a pure per-file parser cache, and a constant changed in another file
//! is re-folded on the next build (the cache rule in `constants.rs`).
//!
//! WHAT IT READS, per ENDPOINT node entry, from its single ENDPOINT_HIT cell:
//! - `template` (TypeScript): the literal with each `${expr}` source kept. It
//!   is folded through the table. Spans that do not resolve come back as `${…}`.
//! - `raw` (A3.3, TypeScript + Dart): the literal before host/query stripping.
//!   Only its authority is new information.
//! - otherwise the qname's own path.
//!
//! [`url_split`] then gives `(host, path)`.
//!
//! OWNERSHIP. A3.1's BaseFold tier and this pass split the `${base}` shape:
//! this pass owns bases the table CAN resolve. A base it cannot resolve leaves
//! the node untouched, so BaseFold still pairs `/{}/users` at Medium. A3.3
//! owns stripping. This pass never strips the path a second time; it only
//! reads the authority back off `raw`. Both go through the one authority
//! splitter in `code_domain::endpoint`.
//!
//! ZERO-CHANGE GUARANTEE. If a node's path does not move and it has no host to
//! record, nothing about it changes: not its id, its edges, its nav entry, or
//! its cell bytes.
//!
//! PRE-SET HOSTS (A11.5). Go, Python, Java, Swift and Dart clients write
//! `"host"` themselves at extraction, from the literal they saw. That host
//! stands: the pass never replaces it, only counts it, so an entry whose path
//! does not move is left byte-identical. Both kinds of host feed the one
//! `[endpoint-host]` line.
//!
//! OVERLAY CONSTANTS (LF.2d). A key pinned from `.glia/overlay.toml`
//! `[constants]` (`ConstTable::pin`) folds like a source binding. When the
//! folded template read one ([`ConstTable::pinned_keys_in`]), the entry is
//! inference, not extraction: its ENDPOINT_HIT gains
//! `"overlay":"const:<KEY>[,<KEY>...]"` and the node takes the pin's
//! confidence ([`pin_confidence`], Weak), so every pairing it makes is at most
//! Weak. An entry no pin reached is untouched.
//!
//! EXTERNAL CALL SITES (CG.4a). After every parse of the repo is folded,
//! [`mark_external`] appends `"external":true` to the ENDPOINT_HIT of a call
//! site whose host (1) is recorded, (2) is written in the call's own literal
//! ([`host_is_literal`]: no template, or the template's text before its
//! first `${` spells the whole authority), (3) is a public DNS name
//! ([`is_public_host`]: dotted, not an IP literal, not an internal TLD, not an
//! RFC 2606 documentation name) and (4) whose site ([`site_of`]) the repo's
//! configuration does not name ([`configured_sites`]: every URL a
//! config-shaped or overlay-pinned constant holds, and every host a
//! const-sourced call site folded to). quokka's nominatim lookups are marked;
//! Kina's `${environment.apiUrl}` calls to its own public backend are not.
//! The sites are collected into a set over EVERY parse before any entry is
//! marked, so the order the build hands the parses in never matters. Only
//! the payload of a marked entry changes; every other entry is byte-identical.
//!
//! BUILDER PREFIXES (CH.5b). A TypeScript call site whose URL was read
//! through a URL builder (`this.urls.buildApiUrl('protected/friends')`, CH.3a:
//! ENDPOINT_HIT `"wrapper"`, CH.3b: `"wrapper_of"` = the receiver's type)
//! is keyed by the path the builder was handed, not the one it returns. When
//! that builder's own code reads exactly one API-prefix member (CH.3b's
//! build-time `NavFact::UrlPrefixKey`) and the repo's configuration binds that
//! member to ONE path (`environment.apiPrefix = '/api'`), the fold puts the
//! prefix in front: `endpoint:GET:/protected/friends` becomes
//! `endpoint:GET:/api/protected/friends`, its ENDPOINT_HIT gains `"prefix"`
//! and `"prefix_from"` (the member), and the node becomes Medium (the
//! builder's transform is known: one inference step, never Strong).
//! [`PrefixContext`] gathers the builders over EVERY parse of the repo before
//! any entry is folded. A call site with no `wrapper_of`, a builder with no
//! fact (quokka's `buildApiRootUrl`), a member bound to two paths, or a path
//! already under the prefix is left exactly as it was.
//!
//! DART PROJECT BASES (CH.5c). A Dio call site does not name its client
//! (quokka_android's clients come out of a Riverpod provider), so a Dart
//! client's base path is a fact about its PROJECT: the owner
//! ([`http_owner::OwnerIndex`]) of the file. Every `BaseOptions(baseUrl: X)` /
//! `<recv>.options.baseUrl = X` a project's Dart files hold (CH.5a's
//! `NavFact::ClientBase`) is resolved by [`resolve_client_base`]: a string
//! literal, else a URL-shaped getter / constant of that name (CH.5a's
//! `NavFact::ValueLiteral`: `Env.apiBaseUrl` -> `${…}://${…}/api`), else the
//! const table's EXACT key (`resolve_expr_strict`, never the lenient
//! last-segment lookup: quokka_web's environment.ts binds a bare `apiBaseUrl`
//! to the Angular app's root URL). A base copied from an existing request
//! (`retry.baseUrl`) or a root URL adds nothing. When every other base of the
//! project agrees on ONE non-root path, each root-relative, host-less Dart
//! ENDPOINT of that project moves under it like a builder-read call site:
//! `endpoint:GET:/protected/friends` -> `endpoint:GET:/api/protected/friends`,
//! `"prefix"` / `"prefix_from"` (the base expression), Medium (a project-level
//! inference: the call site does not name its client). A project with an
//! unresolvable base or two disagreeing paths is not prefixed and named once
//! on an `[endpoint-base]` line, so an overlay `[constants]` pin of the
//! expression's exact key can unblock it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use glia_code_domain::endpoint::{endpoint_qname, url_split};
use glia_code_domain::glia_config::Origin;
use glia_code_domain::project_roots::ProjectRoot;
use glia_code_domain::{CodeNav, FileParse, GRAPH_TYPE, NavFact, cell_type, node_kind};
use glia_code_extractors::constants::{ConstTable, fold_interpolations};
use glia_core::{CellPayload, Confidence, Node, NodeId, RepoId};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http_owner;
use crate::rekey::rewrite_node_id;

/// What the pass did to one repo, for the `[endpoint-fold]`,
/// `[endpoint-host]`, `[endpoint-external]` and `[endpoint-prefix]` markers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct FoldStats {
    /// ENDPOINT node entries (TypeScript: call sites) whose path, and so
    /// whose identity, changed.
    pub folded: usize,
    /// ENDPOINT node entries that gained a `host` from this pass.
    pub hosts: usize,
    /// A11.5: ENDPOINT node entries whose ENDPOINT_HIT already carried a
    /// `host` when they reached the pass (written by the parser). Disjoint
    /// from `hosts`: the pass never re-records a pre-set host.
    pub preset: usize,
    /// CG.4a: ENDPOINT node entries (call sites) marked `"external":true`.
    pub external: usize,
    /// CG.4a: distinct sites the repo's configuration names.
    pub configured: usize,
    /// CH.5b: ENDPOINT node entries (TypeScript call sites) the builder
    /// prefix step moved under their URL builder's configured API prefix.
    /// Each is also counted in `folded`.
    pub prefixed_wrapper: usize,
    /// CH.5c: ENDPOINT node entries (Dart call sites) the project-base step
    /// moved under their project's Dio base path: the `base=` of
    /// `[endpoint-prefix]`. Each is also counted in `folded`.
    pub prefixed_base: usize,
    /// CH.5b: the distinct prefixes the prefix steps applied.
    pub prefixes: BTreeSet<String>,
    /// CH.5c: Dart projects whose Dio bases block the project-base step, by
    /// owner (`.` = the repo root), with the reason, for `[endpoint-base]`.
    pub blocked_bases: BTreeMap<String, String>,
}

impl FoldStats {
    fn add(&mut self, other: FoldStats) {
        self.folded += other.folded;
        self.hosts += other.hosts;
        self.preset += other.preset;
        self.external += other.external;
        self.configured += other.configured;
        self.prefixed_wrapper += other.prefixed_wrapper;
        self.prefixed_base += other.prefixed_base;
        self.prefixes.extend(other.prefixes);
        self.blocked_bases.extend(other.blocked_bases);
    }

    /// The fired_on markers, once per repo, each only when non-zero:
    /// A11.2's `[endpoint-fold]` when the pass changed something, and
    /// A11.5's `[endpoint-host]` when any client endpoint carries an
    /// authority, whichever side recorded it.
    pub(crate) fn report(&self, repo_label: &str) {
        if self.folded + self.hosts > 0 {
            eprintln!(
                "[endpoint-fold] folded {} endpoint paths, captured {} hosts repo={repo_label}",
                self.folded, self.hosts
            );
        }
        let carried = self.hosts + self.preset;
        if carried > 0 {
            eprintln!(
                "[endpoint-host] {carried} client endpoints carry an authority \
                 (preset={}, captured={}) repo={repo_label}",
                self.preset, self.hosts
            );
        }
        if self.external > 0 {
            eprintln!(
                "[endpoint-external] {} client endpoint sites name a public host outside \
                 the repo's configuration (configured sites={}) repo={repo_label}",
                self.external, self.configured
            );
        }
        // CH.5b fired_on marker, once per repo where a prefix step moved an
        // entry:
        //   `[endpoint-prefix] prefixed <n> endpoint paths (wrapper=<w> base=<b>) prefixes=<p,...> repo=<label>`
        let prefixed = self.prefixed_wrapper + self.prefixed_base;
        if prefixed > 0 {
            let prefixes: Vec<&str> = self.prefixes.iter().map(String::as_str).collect();
            eprintln!(
                "[endpoint-prefix] prefixed {prefixed} endpoint paths (wrapper={} base={}) \
                 prefixes={} repo={repo_label}",
                self.prefixed_wrapper,
                self.prefixed_base,
                prefixes.join(",")
            );
        }
        // CH.5c diagnostic, once per Dart project whose bases block the
        // project-base step:
        //   `[endpoint-base] project <owner> not prefixed: <reason> repo=<label>`
        for (project, reason) in &self.blocked_bases {
            eprintln!("[endpoint-base] project {project} not prefixed: {reason} repo={repo_label}");
        }
    }
}

/// Fold every FileParse of one repo, then mark its external call sites
/// (CG.4a) over the same parses. `roots` are the walk's project roots: the
/// owners [`PrefixContext`] keys URL builders by (CH.5b).
pub(crate) fn fold_repo<'a>(
    parses: impl IntoIterator<Item = &'a mut FileParse>,
    consts: &ConstTable,
    repo: RepoId,
    roots: &[ProjectRoot],
) -> FoldStats {
    let mut parses: Vec<&mut FileParse> = parses.into_iter().collect();
    let ctx = PrefixContext::build(&parses, consts, roots);
    let mut stats = FoldStats {
        blocked_bases: ctx.blocked_bases.clone(),
        ..FoldStats::default()
    };
    for fp in parses.iter_mut() {
        stats.add(fold_endpoint_paths_with(fp, consts, repo, &ctx));
    }
    stats.add(mark_external(&mut parses, consts));
    stats
}

// ============================================================================
// CH.5b: URL builders that read a configured API prefix
// ============================================================================

/// The prefix a URL builder puts in front of the path it is handed (CH.5b),
/// or a Dart project's Dio base path (CH.5c).
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuilderPrefix {
    /// The configured path, `/`-led, no trailing `/` (`/api`).
    path: String,
    /// The ENDPOINT_HIT `prefix_from`: the API-prefix member the builder
    /// reads (`apiPrefix`), or the Dio base expression (`Env.apiBaseUrl`).
    key: String,
    /// LF.2d: the overlay-pinned constant keys the value came through, so the
    /// entry records them as `overlay` and is Weak like any pinned fold.
    pins: Vec<String>,
}

/// CH.5b: the repo's URL builders that carry a `NavFact::UrlPrefixKey`. Built
/// over EVERY parse of the repo before any entry is folded, into BTreeMaps
/// whose merges are order-free, so the HashMap order the build hands the
/// parses in never shows.
#[derive(Debug, Default)]
struct PrefixContext {
    /// `(owner, type simple name, method name)` -> the builder's prefix, or
    /// `None` when it is ambiguous (its member binds two paths, or two facts
    /// for one builder disagree).
    builders: BTreeMap<(Option<String>, String, String), Option<BuilderPrefix>>,
    /// Every owner that declares a METHOD of one of those `(type, method)`
    /// names, fact or not: a call site's builder is found under another
    /// owner only when exactly one owner declares it.
    declared: BTreeMap<(String, String), BTreeSet<Option<String>>>,
    owners: http_owner::OwnerIndex,
    /// CH.5c: owner -> the ONE base path its Dart Dio clients agree on.
    dart: BTreeMap<Option<String>, BuilderPrefix>,
    /// CH.5c: owner label -> why its Dart bases block the project-base step.
    blocked_bases: BTreeMap<String, String>,
}

impl PrefixContext {
    fn build(parses: &[&mut FileParse], consts: &ConstTable, roots: &[ProjectRoot]) -> Self {
        let mut ctx = PrefixContext {
            owners: http_owner::OwnerIndex::from_roots(roots),
            ..PrefixContext::default()
        };
        (ctx.dart, ctx.blocked_bases) = dart_bases(parses, consts, &ctx.owners);
        let owner_of = |fp: &FileParse| {
            http_owner::module_file(fp).and_then(|f| ctx.owners.owner_of(&f).map(String::from))
        };
        // One resolution per member, shared by every builder that reads it.
        let mut values: BTreeMap<String, Option<BuilderPrefix>> = BTreeMap::new();
        let mut builders: BTreeMap<(Option<String>, String, String), Option<BuilderPrefix>> =
            BTreeMap::new();
        for fp in parses {
            let mut facts: Vec<(NodeId, &str)> = fp
                .nav
                .nav_facts
                .iter()
                .flat_map(|(scope, facts)| {
                    facts.iter().filter_map(move |f| match f {
                        NavFact::UrlPrefixKey { key } => Some((*scope, key.as_str())),
                        _ => None,
                    })
                })
                .collect();
            if facts.is_empty() {
                continue;
            }
            facts.sort_by(|a, b| a.0.0.cmp(&b.0.0).then_with(|| a.1.cmp(b.1)));
            let owner = owner_of(fp);
            for (scope, key) in facts {
                let Some((class, method)) = builder_name(&fp.nav, scope) else {
                    continue;
                };
                let prefix = values
                    .entry(key.to_string())
                    .or_insert_with(|| configured_prefix(key, consts))
                    .clone();
                builders
                    .entry((owner.clone(), class, method))
                    .and_modify(|seen| {
                        if *seen != prefix {
                            *seen = None;
                        }
                    })
                    .or_insert(prefix);
            }
        }
        if builders.is_empty() {
            return ctx;
        }
        let names: BTreeSet<(&str, &str)> = builders
            .keys()
            .map(|(_, c, m)| (c.as_str(), m.as_str()))
            .collect();
        let mut declared: BTreeMap<(String, String), BTreeSet<Option<String>>> = BTreeMap::new();
        for fp in parses {
            let mut owner: Option<Option<String>> = None;
            for (id, kind) in &fp.nav.kind_by_id {
                if *kind != node_kind::METHOD {
                    continue;
                }
                let Some(name) = builder_name(&fp.nav, *id) else {
                    continue;
                };
                if names.contains(&(name.0.as_str(), name.1.as_str())) {
                    let owner = owner.get_or_insert_with(|| owner_of(fp)).clone();
                    declared.entry(name).or_default().insert(owner);
                }
            }
        }
        ctx.builders = builders;
        ctx.declared = declared;
        ctx
    }

    /// The prefix of the builder an entry's URL was read through: its
    /// `wrapper` method on its `wrapper_of` type, under the call site's own
    /// owner, else under the ONE owner that declares that builder. An entry
    /// with no `wrapper_of` (an untyped or inherited receiver, a `function`
    /// callback, a builder not reached through `this.<f>`) names no builder.
    fn lookup(&self, fields: &Fields) -> Option<&BuilderPrefix> {
        if self.builders.is_empty() {
            return None;
        }
        let name = (
            fields.str("wrapper_of")?.to_string(),
            fields.str("wrapper")?.to_string(),
        );
        let owner = fields
            .str("file")
            .and_then(|f| self.owners.owner_of(f))
            .map(String::from);
        let hit = |owner: &Option<String>| {
            self.builders
                .get(&(owner.clone(), name.0.clone(), name.1.clone()))
        };
        if let Some(found) = hit(&owner) {
            return found.as_ref();
        }
        let mut owners = self.declared.get(&name)?.iter();
        match (owners.next(), owners.next()) {
            (Some(only), None) => hit(only)?.as_ref(),
            _ => None,
        }
    }

    /// CH.5c: the Dio base path of a Dart call site's project: a Dart entry
    /// (its `file` ends `.dart`) whose qname path is root-relative (not a
    /// `${…}` base-relative one), under an owner whose bases agree.
    fn dart_base(&self, fields: &Fields, qpath: &str) -> Option<&BuilderPrefix> {
        if self.dart.is_empty() || !qpath.starts_with('/') {
            return None;
        }
        let file = fields.str("file").filter(|f| f.ends_with(".dart"))?;
        self.dart.get(&self.owners.owner_of(file).map(String::from))
    }
}

// ============================================================================
// CH.5c: a Dart project's Dio base path
// ============================================================================

/// What one Dio base expression resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Base {
    /// A `/`-led path, no trailing `/` (`/api`), with the overlay-pinned
    /// constant keys it was read through (LF.2d).
    Path(String, Vec<String>),
    /// The root (`https://api.x`, `/`, `${…}://${…}`): nothing to prefix.
    Root,
    /// A base copied from an existing request or client (`retry.baseUrl`):
    /// whatever it holds came from another base of the project.
    Copy,
    /// Nothing the build can read resolves it.
    Unresolved,
}

/// CH.5a's URL-shaped Dart getters / constants (`NavFact::ValueLiteral`), by
/// name: per owner, and over the whole repo for a project that reads one
/// declared in another (a shared package). A name bound to two values in one
/// map is `None` there. Order-free: a binding, once `None`, stays `None`.
#[derive(Debug, Default)]
struct DartValues {
    by_owner: BTreeMap<Option<String>, BTreeMap<String, Option<String>>>,
    all: BTreeMap<String, Option<String>>,
}

impl DartValues {
    fn insert(&mut self, owner: &Option<String>, name: &str, value: &str) {
        let bind = |map: &mut BTreeMap<String, Option<String>>| {
            map.entry(name.to_string())
                .and_modify(|seen| {
                    if seen.as_deref() != Some(value) {
                        *seen = None;
                    }
                })
                .or_insert_with(|| Some(value.to_string()));
        };
        bind(self.by_owner.entry(owner.clone()).or_default());
        bind(&mut self.all);
    }

    /// The value `expr` names: its exact name, else its last two dotted
    /// segments (`core.Env.apiBaseUrl` -> `Env.apiBaseUrl`), in the owner's
    /// own map first, then the repo's. `Some(None)` = bound, but ambiguously.
    fn lookup(&self, owner: &Option<String>, expr: &str) -> Option<Option<&str>> {
        let segs: Vec<&str> = expr.split('.').collect();
        let tail = (segs.len() > 2).then(|| segs[segs.len() - 2..].join("."));
        let keys: Vec<&str> = std::iter::once(expr).chain(tail.as_deref()).collect();
        let maps = [self.by_owner.get(owner), Some(&self.all)];
        maps.into_iter()
            .flatten()
            .find_map(|map| keys.iter().find_map(|k| map.get(*k).map(Option::as_deref)))
    }
}

/// CH.5c: resolve one `NavFact::ClientBase` expression of a Dart file under
/// `owner`, in order: (a) a string literal, (b) a ValueLiteral of that name,
/// (c) the const table's exact key, (d) a copied `.baseUrl`, (e) nothing.
/// (c) is [`ConstTable::resolve_expr_strict`], never `resolve_expr`: the
/// table is repo-wide and language-blind, and its last-segment fallback
/// would answer `Env.apiBaseUrl` with another app's bare `apiBaseUrl`.
fn resolve_client_base(
    expr: &str,
    owner: &Option<String>,
    values: &DartValues,
    consts: &ConstTable,
) -> Base {
    let expr = expr.trim();
    if expr.starts_with(['\'', '"', 'r']) && expr.ends_with(['\'', '"']) {
        return dart_literal(expr).map_or(Base::Unresolved, |v| base_of_value(&v, Vec::new()));
    }
    if let Some(value) = values.lookup(owner, expr) {
        return value.map_or(Base::Unresolved, |v| base_of_value(v, Vec::new()));
    }
    if let Some(value) = consts.resolve_expr_strict(expr) {
        let pins = consts
            .pinned_keys_in(&format!("${{{expr}}}"))
            .into_iter()
            .map(String::from)
            .collect();
        return base_of_value(value, pins);
    }
    if expr.ends_with(".baseUrl") {
        return Base::Copy;
    }
    Base::Unresolved
}

/// A Dart string literal (`'..'`, `".."`, raw `r'..'`) as one value, every
/// `$name` / `${expr}` interpolation `${…}`. None when `expr` is not ONE
/// plain literal: adjacent or concatenated strings, a triple-quoted one, or
/// an escaped quote inside (all read as unresolved, never guessed).
fn dart_literal(expr: &str) -> Option<String> {
    let (raw, body) = match expr.strip_prefix('r') {
        Some(rest) => (true, rest),
        None => (false, expr),
    };
    let quote = body.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let inner = body.strip_prefix(quote)?.strip_suffix(quote)?;
    if inner.contains(quote) {
        return None;
    }
    if raw {
        return Some(inner.to_string());
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '$' if chars.peek() == Some(&'{') => {
                let mut depth = 0usize;
                for n in chars.by_ref() {
                    match n {
                        '{' => depth += 1,
                        '}' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                out.push_str("${…}");
            }
            '$' if chars
                .peek()
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '_') =>
            {
                while chars
                    .peek()
                    .is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_')
                {
                    chars.next();
                }
                out.push_str("${…}");
            }
            _ => out.push(c),
        }
    }
    Some(out)
}

/// The base a resolved value names: the path of a URL (`${…}://${…}/api` ->
/// `/api`) or of a `/`-led value, query cut and trailing `/` trimmed; Root
/// when that leaves nothing; Unresolved for a value with no path (a bare word,
/// a whole-URL `${…}`) or an interpolated one (`/v${…}`).
fn base_of_value(value: &str, pins: Vec<String>) -> Base {
    let path = if value.contains("://") || value.starts_with('/') {
        url_split(value).1
    } else {
        None
    };
    let Some(path) = path else {
        return Base::Unresolved;
    };
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        Base::Root
    } else if path.contains("${") {
        Base::Unresolved
    } else {
        Base::Path(path.to_string(), pins)
    }
}

/// CH.5c: every Dart project's Dio base path, and the projects whose bases
/// block it (by owner label, `.` = the repo root, with the reason). Facts are
/// gathered per owner, read in (file, line, expression) order, so neither the
/// parse order nor the HashMap order of `nav_facts` shows. A project is
/// blocked by any unresolved base (an unknown client might use another base;
/// the first such expression is named) or by two distinct paths; Root and
/// Copy bases are ignored, so a project with no path is simply not prefixed.
fn dart_bases(
    parses: &[&mut FileParse],
    consts: &ConstTable,
    owners: &http_owner::OwnerIndex,
) -> (
    BTreeMap<Option<String>, BuilderPrefix>,
    BTreeMap<String, String>,
) {
    let mut facts: BTreeMap<Option<String>, Vec<(String, u32, String)>> = BTreeMap::new();
    let mut values = DartValues::default();
    for fp in parses {
        let mut found = fp
            .nav
            .nav_facts
            .values()
            .flatten()
            .filter(|f| matches!(f, NavFact::ClientBase { .. } | NavFact::ValueLiteral { .. }));
        if found.next().is_none() {
            continue;
        }
        let Some(file) = http_owner::module_file(fp).filter(|f| f.ends_with(".dart")) else {
            continue;
        };
        let owner = owners.owner_of(&file).map(String::from);
        for fact in fp.nav.nav_facts.values().flatten() {
            match fact {
                NavFact::ClientBase { via, expr, line } if via == "dio" => {
                    facts.entry(owner.clone()).or_default().push((
                        file.clone(),
                        *line,
                        expr.clone(),
                    ));
                }
                NavFact::ValueLiteral { name, value } => values.insert(&owner, name, value),
                _ => {}
            }
        }
    }
    let mut bases: BTreeMap<Option<String>, BuilderPrefix> = BTreeMap::new();
    let mut blocked: BTreeMap<String, String> = BTreeMap::new();
    for (owner, mut list) in facts {
        list.sort();
        list.dedup();
        let label = owner.clone().unwrap_or_else(|| ".".to_string());
        let mut paths: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
        let mut unresolved: Option<&str> = None;
        for (_, _, expr) in &list {
            match resolve_client_base(expr, &owner, &values, consts) {
                Base::Path(path, pins) => {
                    paths.entry(path).or_insert_with(|| (expr.clone(), pins));
                }
                Base::Root | Base::Copy => {}
                Base::Unresolved => {
                    unresolved.get_or_insert(expr.as_str());
                }
            }
        }
        if let Some(expr) = unresolved {
            blocked.insert(label, format!("unresolved base {expr}"));
            continue;
        }
        if paths.len() > 1 {
            let all: Vec<&str> = paths.keys().map(String::as_str).collect();
            blocked.insert(label, format!("bases disagree on {}", all.join(",")));
            continue;
        }
        if let Some((path, (key, pins))) = paths.into_iter().next() {
            bases.insert(owner, BuilderPrefix { path, key, pins });
        }
    }
    (bases, blocked)
}

/// A builder scope's `(type simple name, method name)`: a METHOD whose parent
/// is a type (not a MODULE). A module-level function names no type, and a
/// call site that names no `wrapper_of` is never prefixed, so it keys
/// nothing.
fn builder_name(nav: &CodeNav, scope: NodeId) -> Option<(String, String)> {
    if nav.kind_by_id.get(&scope) != Some(&node_kind::METHOD) {
        return None;
    }
    let method = nav.name_by_id.get(&scope)?;
    let parent = nav.parent_of.get(&scope)?;
    if nav.kind_by_id.get(parent) == Some(&node_kind::MODULE) {
        return None;
    }
    let class = nav.name_by_id.get(parent)?;
    Some((class.clone(), method.clone()))
}

/// The ONE path the repo's configuration binds API-prefix member `key` to:
/// every value of the bare key and of every dotted key ending `.<key>`
/// (`environment.apiPrefix`, `DEFAULT_CONFIG.apiPrefix`), each binding of
/// each, as a `/`-led path with the query cut and trailing `/` trimmed.
/// `None` when there is no value, when any value is not such a path (empty,
/// `/`, relative, interpolated, a bare authority), or when two differ.
fn configured_prefix(key: &str, consts: &ConstTable) -> Option<BuilderPrefix> {
    let dotted = format!(".{key}");
    let mut paths: BTreeSet<String> = BTreeSet::new();
    let mut pins: BTreeSet<String> = BTreeSet::new();
    let bare = consts.candidates(key).into_iter().map(|v| (key, v));
    let nested = consts.entries().filter(|(k, _)| k.ends_with(&dotted));
    for (k, value) in bare.chain(nested) {
        let path = url_split(value).1?;
        let path = path.trim_end_matches('/');
        if path.is_empty() || path.contains("${") {
            return None;
        }
        paths.insert(path.to_string());
        if consts.is_pinned(k) {
            pins.insert(k.to_string());
        }
    }
    let mut it = paths.into_iter();
    let (Some(path), None) = (it.next(), it.next()) else {
        return None;
    };
    Some(BuilderPrefix {
        path,
        key: key.to_string(),
        pins: pins.into_iter().collect(),
    })
}

/// `path` already sits under `prefix` (a builder that strips a duplicate
/// prefix, or a call site that wrote it out).
fn under_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// What one ENDPOINT node entry becomes.
struct Plan {
    /// Unchanged when only a host was captured.
    id: NodeId,
    name: String,
    qname: String,
    payload: String,
    moved: bool,
    host: bool,
    /// The node's new confidence: LF.2d's Weak when an overlay constant
    /// folded it, CH.5b's / CH.5c's Medium when a prefix step moved it.
    confidence: Option<Confidence>,
    /// CH.5b: the builder prefix the entry was moved under.
    prefix: Option<String>,
    /// CH.5c: that prefix is the project's Dio base path, not a builder's.
    base: bool,
}

/// LF.2d: the confidence of an entry an overlay constant folded. `[constants]`
/// is a flat `NAME = "literal"` table with no per-key `origin`, so every pin
/// has the default stanza origin (`llm`): Weak.
fn pin_confidence() -> Confidence {
    Origin::default().confidence()
}

/// Fold the ENDPOINT nodes of one file in place, prefixing builder-read call
/// sites through `ctx` (CH.5b).
fn fold_endpoint_paths_with(
    fp: &mut FileParse,
    consts: &ConstTable,
    repo: RepoId,
    ctx: &PrefixContext,
) -> FoldStats {
    // Node entries grouped by their current id, in first-seen order. The
    // TypeScript parser pushes one entry per call site, so an id can repeat.
    let mut order: Vec<NodeId> = Vec::new();
    let mut groups: HashMap<NodeId, Vec<(usize, Option<Plan>)>> = HashMap::new();
    let mut stats = FoldStats::default();
    for (idx, node) in fp.nodes.iter().enumerate() {
        if fp.nav.kind_by_id.get(&node.id) != Some(&node_kind::ENDPOINT) {
            continue;
        }
        stats.preset += usize::from(arrived_with_host(node));
        let plan = plan_entry(node, &fp.nav, consts, repo, ctx);
        groups
            .entry(node.id)
            .or_insert_with(|| {
                order.push(node.id);
                Vec::new()
            })
            .push((idx, plan));
    }

    for old in order {
        let Some(entries) = groups.remove(&old) else {
            continue;
        };
        if entries.iter().all(|(_, p)| p.is_none()) {
            continue;
        }
        let targets: Vec<NodeId> = entries
            .iter()
            .map(|(_, p)| p.as_ref().map_or(old, |p| p.id))
            .collect();
        if !retarget_edges(fp, old, &targets) {
            continue;
        }
        update_nav(&mut fp.nav, old, &targets, &entries);
        for (idx, plan) in entries {
            let Some(plan) = plan else { continue };
            let Some(node) = fp.nodes.get_mut(idx) else {
                continue;
            };
            node.id = plan.id;
            if let Some(c) = plan.confidence {
                node.confidence = c;
            }
            for cell in node
                .cells
                .iter_mut()
                .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
            {
                cell.payload = CellPayload::Json(plan.payload.clone());
            }
            stats.folded += usize::from(plan.moved);
            stats.hosts += usize::from(plan.host);
            if let Some(p) = plan.prefix {
                if plan.base {
                    stats.prefixed_base += 1;
                } else {
                    stats.prefixed_wrapper += 1;
                }
                stats.prefixes.insert(p);
            }
        }
    }
    stats
}

/// Decide what one node entry becomes, or None to leave it alone.
///
/// An entry is only planned when it carries exactly one JSON ENDPOINT_HIT
/// cell, which is what every client emitter writes. Anything else is left
/// alone rather than guessed at.
///
/// CH.5b: a host-less entry read through a URL builder `ctx` knows is moved
/// under the builder's prefix. A builder's argument is usually relative
/// (`raw` `protected/friends`), so such an entry's path is its own qname path
/// when neither the fold nor `raw` yields one.
///
/// CH.5c: failing that, a host-less, root-relative Dart entry is moved under
/// its project's Dio base path ([`PrefixContext::dart_base`]).
fn plan_entry(
    node: &Node,
    nav: &CodeNav,
    consts: &ConstTable,
    repo: RepoId,
    ctx: &PrefixContext,
) -> Option<Plan> {
    let qname = nav.qname_by_id.get(&node.id)?;
    let (method, qpath) = qname.strip_prefix("endpoint:")?.split_once(':')?;
    let (_, mut fields) = single_hit(node)?;

    let folded = fields
        .str("template")
        .and_then(|t| fold_interpolations(t, consts));
    // LF.2d: the overlay constants that fold read, if it folded at all.
    let mut pins: Vec<String> = match (&folded, fields.str("template")) {
        (Some(_), Some(t)) => consts.pinned_keys_in(t).into_iter().map(String::from).collect(),
        _ => Vec::new(),
    };
    let input = folded
        .as_deref()
        .or_else(|| fields.str("raw"))
        .unwrap_or(qpath);
    let (host, path) = url_split(input);
    // CH.5b / CH.5c: only a call site that names no authority at all (none
    // in its URL, none written by its parser) can sit under a prefix: its URL
    // builder's, else (Dart) its project's Dio base path.
    let (builder, base) = match (&host, fields.str("host")) {
        (None, None) => match ctx.lookup(&fields) {
            Some(b) => (Some(b), false),
            None => (ctx.dart_base(&fields, qpath), true),
        },
        _ => (None, false),
    };
    let mut path = match (path, builder) {
        (Some(p), _) => p,
        (None, Some(_)) if qpath.starts_with('/') => qpath.to_string(),
        (None, _) => return None,
    };
    let builder = builder.filter(|b| !under_prefix(&path, &b.path));
    if let Some(b) = builder {
        path = format!("{}{path}", b.path);
    }
    // An interpolated authority (`https://${…}/x`) names no service, and a
    // host the parser already wrote (A11.5) is not re-recorded.
    let host = host.filter(|h| !h.contains("${") && fields.str("host").is_none());
    let moved = path != qpath;
    if !moved && host.is_none() {
        return None;
    }
    // Only a folded base can have deployment alternatives.
    let hosts = match (&folded, fields.str("template")) {
        (Some(_), Some(t)) => deployment_hosts(t, consts, host.as_deref()),
        _ => Vec::new(),
    };

    if moved {
        fields.set("path", Value::from(path.as_str()));
        fields.set("folded_from", Value::from(qpath));
    }
    if let Some(h) = &host {
        fields.set("host", Value::from(h.as_str()));
    }
    if !hosts.is_empty() {
        fields.set("hosts", Value::from(hosts));
    }
    if let Some(b) = builder {
        fields.set("prefix", Value::from(b.path.as_str()));
        fields.set("prefix_from", Value::from(b.key.as_str()));
        for k in &b.pins {
            if !pins.contains(k) {
                pins.push(k.clone());
            }
        }
    }
    if !pins.is_empty() {
        fields.set("overlay", Value::from(format!("const:{}", pins.join(","))));
    }
    let payload = serde_json::to_string(&fields).ok()?;
    // `url_split` only ever returns a path starting with `/`, so this is
    // byte-identical to the literal shape; it keeps ONE qname builder (LB.5).
    let new_qname = endpoint_qname(method, &path);
    let id = if moved {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENDPOINT, &new_qname)
    } else {
        node.id
    };
    Some(Plan {
        id,
        name: format!("{method} {path}"),
        qname: new_qname,
        payload,
        moved,
        host: host.is_some(),
        // A pin wins (Weak); a builder prefix is one known transform, and a
        // project base path one project-level inference: Medium, never Strong.
        confidence: if !pins.is_empty() {
            Some(pin_confidence())
        } else {
            builder.map(|_| Confidence::Medium)
        },
        prefix: builder.map(|b| b.path.clone()),
        base: base && builder.is_some(),
    })
}

/// A11.5: the entry's ENDPOINT_HIT already names a `host`, written at
/// extraction by a parser that saw a literal authority.
fn arrived_with_host(node: &Node) -> bool {
    node.cells
        .iter()
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .any(|c| match &c.payload {
            CellPayload::Json(json) => {
                serde_json::from_str::<Fields>(json).is_ok_and(|f| f.str("host").is_some())
            }
            _ => false,
        })
}

/// A11.4: the authority of EVERY binding of the template's leading base, for
/// the `"hosts"` field. `environment.ts` and `environment.prod.ts` routinely
/// bind `environment.apiUrl` to different hosts, and the fold above used only
/// the first, so HTTP host narrowing has to see the whole set.
///
/// A binding with no authority (a relative `/api` base: same origin, service
/// unknown) contributes `""`, which the resolver reads as "do not narrow".
///
/// Empty, so no field is written, unless the bindings disagree. That keeps
/// every single-binding payload byte-identical. Also empty when the set would
/// not start with the `host` the fold recorded (the lenient last-segment
/// lookup resolved a different key), so `hosts[0]` is always `host`.
fn deployment_hosts(template: &str, consts: &ConstTable, host: Option<&str>) -> Vec<String> {
    let Some(expr) = template
        .strip_prefix("${")
        .and_then(|inner| inner.split_once('}'))
        .map(|(expr, _)| expr)
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for value in consts.candidates(expr) {
        let h = url_split(value)
            .0
            .filter(|h| !h.contains("${"))
            .unwrap_or_default();
        if !out.contains(&h) {
            out.push(h);
        }
    }
    let consistent = out.first().map(String::as_str) == Some(host.unwrap_or(""));
    if out.len() < 2 || !consistent {
        return Vec::new();
    }
    out
}

/// Point this file's edges at the new ids. Returns false, changing nothing,
/// when the entries of `old` split across several ids and the CALLS edges
/// cannot be paired with them one-to-one.
///
/// All entries go to one id: every edge touching `old` follows it. They
/// split, which only happens when two TypeScript call sites share a
/// placeholder path and only one of their bases resolves: the parser pushed
/// each call site's node and its CALLS edge in the same order, so the k-th
/// edge into `old` belongs to the k-th entry.
fn retarget_edges(fp: &mut FileParse, old: NodeId, targets: &[NodeId]) -> bool {
    let Some(&first) = targets.first() else {
        return false;
    };
    if targets.iter().all(|t| *t == first) {
        if first != old {
            for e in fp.edges.iter_mut() {
                if e.to == old {
                    e.to = first;
                }
                if e.from == old {
                    e.from = first;
                }
            }
        }
        return true;
    }
    if fp.edges.iter().any(|e| e.from == old) {
        return false;
    }
    let into: Vec<usize> = fp
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.to == old)
        .map(|(i, _)| i)
        .collect();
    if into.len() != targets.len() {
        return false;
    }
    for (i, t) in into.into_iter().zip(targets) {
        if let Some(e) = fp.edges.get_mut(i) {
            e.to = *t;
        }
    }
    true
}

/// Give every new id a nav entry, and retire `old` when no entry is left on it.
fn update_nav(
    nav: &mut CodeNav,
    old: NodeId,
    targets: &[NodeId],
    entries: &[(usize, Option<Plan>)],
) {
    let parent = nav.parent_of.get(&old).copied();
    let moved: Vec<&Plan> = entries
        .iter()
        .filter_map(|(_, p)| p.as_ref())
        .filter(|p| p.moved)
        .collect();
    let Some(first) = moved.first() else {
        return;
    };
    if !targets.contains(&old) {
        rewrite_node_id(nav, old, first.id);
    }
    for plan in moved {
        nav.name_by_id.insert(plan.id, plan.name.clone());
        nav.qname_by_id.insert(plan.id, plan.qname.clone());
        nav.kind_by_id.insert(plan.id, node_kind::ENDPOINT);
        if let Some(p) = parent
            && !nav.parent_of.contains_key(&plan.id)
        {
            nav.parent_of.insert(plan.id, p);
            nav.children_of.entry(p).or_default().push(plan.id);
        }
    }
}

/// The one JSON ENDPOINT_HIT cell of a node, with its index in `cells`, or
/// None when the node carries none, several, or a non-JSON one: the shape
/// every client emitter writes, and the only one the fold touches.
fn single_hit(node: &Node) -> Option<(usize, Fields)> {
    let mut hits = node
        .cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == cell_type::ENDPOINT_HIT);
    let (Some((idx, cell)), None) = (hits.next(), hits.next()) else {
        return None;
    };
    let CellPayload::Json(json) = &cell.payload else {
        return None;
    };
    Some((idx, serde_json::from_str(json).ok()?))
}

// ============================================================================
// CG.4a: external call sites
// ============================================================================

/// TLDs no public DNS name ends in: loopback, mDNS, cluster and home-network
/// names, and the RFC 2606 / 6761 reserved TLDs. `.home.arpa` is checked on
/// its own.
const INTERNAL_TLDS: &[&str] = &[
    "localhost",
    "local",
    "internal",
    "lan",
    "test",
    "example",
    "invalid",
    "svc",
    "localdomain",
];

/// RFC 2606 reserved documentation sites: the bench fixtures' and most docs'
/// `api.example.com` names nobody's service.
const RESERVED_SITES: &[&str] = &["example.com", "example.net", "example.org"];

/// Second-level labels a two-letter ccTLD registry sells names under
/// (`co.uk`, `com.au`, `ac.jp`). Label-based, not a public-suffix list: an
/// SLD missing here falls back to two labels, which only widens a site.
const REGISTRY_SLDS: &[&str] = &["co", "com", "net", "org", "gov", "edu", "ac"];

/// Every ENDPOINT node entry of `fp` with its single JSON ENDPOINT_HIT, as
/// `(node index, cell index, fields)`. The nav-kind gate keeps CB.21's
/// WS_CLIENT / GRPC_CLIENT hits, which carry a `host` too, out.
fn endpoint_hits(fp: &FileParse) -> impl Iterator<Item = (usize, usize, Fields)> + '_ {
    fp.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
        .filter_map(|(i, n)| single_hit(n).map(|(c, f)| (i, c, f)))
}

/// `[user@]host[:port]` -> `host`, lower-cased, a trailing root `.` dropped.
/// A bracketed IPv6 literal keeps its brackets. The engine's twin of
/// glia-graph's crate-private `host::host_name`; ASCII splits only.
fn host_only(authority: &str) -> String {
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// The registrable site of a host name: its last two labels, or its last
/// three when the TLD has two letters and the label before it is a registry
/// label (`a.b.co.uk` -> `b.co.uk`). A name of one or two labels is itself.
fn site_of(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let registry = n >= 3
        && labels.get(n - 1).is_some_and(|tld| tld.len() == 2)
        && labels.get(n - 2).is_some_and(|sld| REGISTRY_SLDS.contains(sld));
    let keep = if registry { 3 } else { 2 };
    labels.get(n.saturating_sub(keep)..).unwrap_or_default().join(".")
}

/// A host name (already [`host_only`]) that can only be a public DNS name:
/// dotted, not an IPv4 / IPv6 literal, no internal TLD, not under
/// `.home.arpa`, and not an RFC 2606 documentation site.
fn is_public_host(host: &str) -> bool {
    if !host.contains('.') || host.starts_with('[') || host.contains(':') {
        return false;
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return false;
    }
    let tld = host.rsplit('.').next().unwrap_or_default();
    if INTERNAL_TLDS.contains(&tld) || host == "home.arpa" || host.ends_with(".home.arpa") {
        return false;
    }
    !RESERVED_SITES.contains(&site_of(host).as_str())
}

/// The recorded host was written in the call's own literal: no `template`
/// (a `raw` literal, or a host the parser wrote at extraction, A11.5), or a
/// template whose text before its first `${` spells the whole authority
/// (`https://nominatim.openstreetmap.org/search?q=${q}`). A template that
/// starts with a base (`${environment.apiUrl}/x`) or interpolates into the
/// authority (`https://${API_HOST}/x`) is const-sourced.
fn host_is_literal(fields: &Fields) -> bool {
    let Some(template) = fields.str("template") else {
        return true;
    };
    let head = template.split("${").next().unwrap_or_default();
    let Some((_, after)) = head.split_once("://") else {
        return false;
    };
    let authority = after.split(['/', '?', '#']).next().unwrap_or_default();
    fields
        .str("host")
        .is_some_and(|h| !authority.is_empty() && host_only(authority) == host_only(h))
}

/// A constant key that names configuration, not a local: dotted
/// (`environment.apiBaseUrl`), SCREAMING_CASE (`API_BASE_URL`), or pinned by
/// `.glia/overlay.toml [constants]` (the user's escape hatch, LF.2d).
fn config_shaped(key: &str, consts: &ConstTable) -> bool {
    let screaming = key
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && key.bytes().any(|b| b.is_ascii_uppercase());
    key.contains('.') || screaming || consts.is_pinned(key)
}

/// Add the site of one authority to `sites`.
fn insert_site(sites: &mut BTreeSet<String>, authority: &str) {
    let host = host_only(authority);
    if !host.is_empty() && !host.contains("${") {
        sites.insert(site_of(&host));
    }
}

/// The sites the repo's configuration names: the authority of every URL a
/// config-shaped constant holds (every deployment binding, overlay pins
/// included), and every `host` / `hosts` entry of a call site whose host is
/// const-sourced. A BTreeSet over every parse, so no parse order shows.
fn configured_sites(parses: &[&mut FileParse], consts: &ConstTable) -> BTreeSet<String> {
    let mut sites = BTreeSet::new();
    for (key, value) in consts.entries() {
        if !value.contains("://") || !config_shaped(key, consts) {
            continue;
        }
        if let (Some(authority), _) = url_split(value) {
            insert_site(&mut sites, &authority);
        }
    }
    for fp in parses {
        for (_, _, fields) in endpoint_hits(fp) {
            if host_is_literal(&fields) {
                continue;
            }
            if let Some(h) = fields.str("host") {
                insert_site(&mut sites, h);
            }
            for h in fields.strs("hosts") {
                insert_site(&mut sites, h);
            }
        }
    }
    sites
}

/// One call site is external: it is not marked yet, its host is literal and
/// public, and its site is not configured.
fn is_external(fields: &Fields, sites: &BTreeSet<String>) -> bool {
    if fields.has("external") || !host_is_literal(fields) {
        return false;
    }
    let Some(host) = fields.str("host").map(host_only) else {
        return false;
    };
    is_public_host(&host) && !sites.contains(&site_of(&host))
}

/// CG.4a: append `"external":true` to the ENDPOINT_HIT of every external
/// call site of the repo. Runs after the fold, so it sees every host the fold
/// recorded; changes nothing but the payload of a marked entry.
fn mark_external(parses: &mut [&mut FileParse], consts: &ConstTable) -> FoldStats {
    let sites = configured_sites(parses, consts);
    let mut stats = FoldStats {
        configured: sites.len(),
        ..FoldStats::default()
    };
    for fp in parses.iter_mut() {
        let marks: Vec<(usize, usize, String)> = endpoint_hits(fp)
            .filter(|(_, _, fields)| is_external(fields, &sites))
            .filter_map(|(node, cell, mut fields)| {
                fields.set("external", Value::Bool(true));
                Some((node, cell, serde_json::to_string(&fields).ok()?))
            })
            .collect();
        for (node, cell, payload) in marks {
            if let Some(c) = fp.nodes.get_mut(node).and_then(|n| n.cells.get_mut(cell)) {
                c.payload = CellPayload::Json(payload);
                stats.external += 1;
            }
        }
    }
    stats
}

/// A JSON object that keeps its key order through a rewrite. The workspace
/// builds serde_json without `preserve_order`, so a `serde_json::Value`
/// round trip would sort the keys of every folded payload. Unknown fields
/// survive either way.
struct Fields(Vec<(String, Value)>);

impl Fields {
    fn str(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_str())
    }

    fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    /// The string entries of an array field; empty when absent or not one.
    fn strs(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_array())
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    /// Replace `key` in place, or append it.
    fn set(&mut self, key: &str, value: Value) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((key.to_string(), value)),
        }
    }
}

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Fields;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = map.next_entry::<String, Value>()? {
                    out.push(entry);
                }
                Ok(Fields(out))
            }
        }
        d.deserialize_map(ObjectVisitor)
    }
}

impl Serialize for Fields {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(k, v)| (k, v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::edge_category;
    use glia_core::{Cell, Confidence, Edge};

    fn repo() -> RepoId {
        RepoId(7)
    }

    /// One file folded on its own, with no builder facts (the pre-CH.5b
    /// pass): every A11.x / LF.2d test keeps this call.
    fn fold_endpoint_paths(fp: &mut FileParse, consts: &ConstTable, repo: RepoId) -> FoldStats {
        fold_endpoint_paths_with(fp, consts, repo, &PrefixContext::default())
    }

    fn ep_id(method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            &format!("endpoint:{method}:{path}"),
        )
    }

    fn table() -> ConstTable {
        ConstTable::scan_file(
            "export const environment = {\n  apiUrl: 'http://users-service:8080',\n};\n",
            "typescript",
        )
    }

    fn func() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "src::svc::Svc::list")
    }

    /// One call site the way the TypeScript parser emits it: an ENDPOINT
    /// entry with one ENDPOINT_HIT cell, then a CALLS edge into it. The nav
    /// entry is recorded once per id, under `func()`, so the parent/children
    /// bookkeeping is exercised too.
    fn push_call(fp: &mut FileParse, path: &str, payload: &str) -> NodeId {
        push_call_as(fp, "GET", path, payload)
    }

    /// [`push_call`] for another HTTP method.
    fn push_call_as(fp: &mut FileParse, method: &str, path: &str, payload: &str) -> NodeId {
        let id = ep_id(method, path);
        fp.nodes.push(Node {
            id,
            repo: repo(),
            confidence: Confidence::Medium,
            cells: vec![Cell {
                kind: cell_type::ENDPOINT_HIT,
                payload: CellPayload::Json(payload.to_string()),
            }],
        });
        fp.edges.push(Edge {
            from: func(),
            to: id,
            category: edge_category::CALLS,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        if !fp.nav.kind_by_id.contains_key(&id) {
            fp.nav.record(
                id,
                &format!("{method} {path}"),
                &format!("endpoint:{method}:{path}"),
                node_kind::ENDPOINT,
                Some(func()),
            );
        }
        id
    }

    fn file() -> FileParse {
        let mut fp = FileParse::default();
        fp.nodes.push(Node {
            id: func(),
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![],
        });
        fp.nav.record(
            func(),
            "list",
            "src::svc::Svc::list",
            node_kind::METHOD,
            None,
        );
        fp
    }

    fn payload(fp: &FileParse, idx: usize) -> &str {
        match &fp.nodes[idx].cells[0].payload {
            CellPayload::Json(s) => s,
            other => panic!("not json: {other:?}"),
        }
    }

    fn same(a: &FileParse, b: &FileParse) -> bool {
        a.nodes == b.nodes
            && a.edges == b.edges
            && a.nav.name_by_id == b.nav.name_by_id
            && a.nav.qname_by_id == b.nav.qname_by_id
            && a.nav.kind_by_id == b.nav.kind_by_id
            && a.nav.parent_of == b.nav.parent_of
            && a.nav.children_of == b.nav.children_of
    }

    /// The zero-change guarantee: a relative path, an unresolvable base and a
    /// relative hint are all left exactly as parsed.
    #[test]
    fn unfoldable_endpoints_are_left_byte_identical() {
        let mut fp = file();
        push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong"}"#,
        );
        push_call(
            &mut fp,
            "/users/${…}",
            r#"{"method":"GET","path":"/users/${…}","file":"a.ts","line":2,"col":1,"confidence":"medium","template":"/users/${id}"}"#,
        );
        push_call(
            &mut fp,
            "${…}/orders",
            r#"{"method":"GET","path":"${…}/orders","file":"a.ts","line":3,"col":1,"confidence":"medium","template":"${this.base}/orders"}"#,
        );
        push_call(
            &mut fp,
            "auth/login",
            r#"{"method":"GET","path":"auth/login","file":"a.ts","line":4,"col":1,"confidence":"weak"}"#,
        );
        // The query A3.3 already stripped is not a reason to touch the node.
        push_call(
            &mut fp,
            "/search",
            r#"{"method":"GET","path":"/search","file":"a.ts","line":5,"col":1,"confidence":"strong","raw":"/search?q=1"}"#,
        );
        let before = fp.clone();
        let stats = fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!(stats, FoldStats::default());
        assert!(same(&fp, &before), "unfoldable endpoints must not change");
    }

    #[test]
    fn resolvable_base_is_folded_and_its_host_recorded() {
        let mut fp = file();
        let old = push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        let stats = fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                hosts: 1,
                preset: 0,
                ..FoldStats::default()
            }
        );

        let new = ep_id("GET", "/users");
        assert_eq!(fp.nodes[1].id, new);
        assert_eq!(
            payload(&fp, 1),
            r#"{"method":"GET","path":"/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${environment.apiUrl}/users","folded_from":"${…}/users","host":"users-service:8080"}"#,
            "key order kept, new fields appended"
        );
        assert_eq!(fp.edges[0].to, new);
        assert!(fp.edges.iter().all(|e| e.to != old));

        assert_eq!(
            fp.nav.qname_by_id.get(&new).map(String::as_str),
            Some("endpoint:GET:/users")
        );
        assert_eq!(
            fp.nav.name_by_id.get(&new).map(String::as_str),
            Some("GET /users")
        );
        assert_eq!(fp.nav.kind_by_id.get(&new), Some(&node_kind::ENDPOINT));
        assert_eq!(fp.nav.parent_of.get(&new), Some(&func()));
        assert_eq!(fp.nav.children_of.get(&func()), Some(&vec![new]));
        for gone in [
            fp.nav.name_by_id.contains_key(&old),
            fp.nav.qname_by_id.contains_key(&old),
            fp.nav.kind_by_id.contains_key(&old),
            fp.nav.parent_of.contains_key(&old),
        ] {
            assert!(!gone, "old id must leave the nav");
        }
    }

    /// A3.3 already took the host out of the path; the fold only reads it back
    /// off `raw`, so the id is untouched and the cell gains `host`.
    #[test]
    fn absolute_literal_keeps_its_id_and_gains_a_host() {
        let mut fp = file();
        let id = push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong","raw":"https://u:p@api.example.com/users?x=1"}"#,
        );
        // An interpolated authority is not a host.
        push_call(
            &mut fp,
            "/orders",
            r#"{"method":"GET","path":"/orders","file":"a.ts","line":2,"col":1,"confidence":"medium","raw":"https://${…}/orders","template":"https://${host}/orders"}"#,
        );
        let edges = fp.edges.clone();
        let stats = fold_endpoint_paths(&mut fp, &ConstTable::default(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 0,
                hosts: 1,
                preset: 0,
                ..FoldStats::default()
            }
        );
        assert_eq!(fp.nodes[1].id, id);
        assert!(payload(&fp, 1).ends_with(
            r#""raw":"https://u:p@api.example.com/users?x=1","host":"api.example.com"}"#
        ));
        assert!(!payload(&fp, 1).contains("folded_from"));
        assert!(
            !payload(&fp, 2).contains(r#""host":"#),
            "{}",
            payload(&fp, 2)
        );
        assert_eq!(fp.edges, edges);
    }

    /// A11.5: a host the parser already wrote is counted as `preset` and left
    /// alone. A Go-shaped entry (no `raw`) and a Dart-shaped one (`raw` whose
    /// authority the pass would otherwise capture) both come out
    /// byte-identical, and neither is counted as captured.
    #[test]
    fn preset_hosts_are_counted_and_left_byte_identical() {
        let mut fp = file();
        push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"client.go","line":9,"col":15,"confidence":"strong","host":"api.example.com"}"#,
        );
        push_call(
            &mut fp,
            "/orders",
            r#"{"method":"POST","path":"/orders","file":"lib/api.dart","line":3,"col":5,"confidence":"strong","raw":"http://svc:8080/orders?x=1","host":"svc:8080"}"#,
        );
        // No host anywhere: neither counter moves.
        push_call(
            &mut fp,
            "/health",
            r#"{"method":"GET","path":"/health","file":"client.go","line":12,"col":3,"confidence":"strong"}"#,
        );
        let before = fp.clone();
        let stats = fold_endpoint_paths(&mut fp, &ConstTable::default(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 0,
                hosts: 0,
                preset: 2,
                ..FoldStats::default()
            }
        );
        assert!(same(&fp, &before), "a pre-set host must not be re-recorded");
    }

    /// A11.4: a base bound differently per deployment records every binding's
    /// authority, first binding first and `""` for a relative one. A base
    /// bound once, or twice to the same host, records no `hosts` at all.
    #[test]
    fn per_deployment_bindings_record_the_host_set() {
        let mut consts = ConstTable::scan_file(
            "export const environment = { apiUrl: 'http://users-svc.prod.svc.cluster.local' };\n",
            "typescript",
        );
        for src in [
            "export const environment = { apiUrl: 'http://users-service:8080' };\n",
            "export const environment = { apiUrl: 'http://users-service:8080/v1' };\n",
            "export const environment = { apiUrl: '/api' };\n",
        ] {
            consts.merge_from(&ConstTable::scan_file(src, "typescript"));
        }
        let mut fp = file();
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","template":"${environment.apiUrl}/users"}"#,
        );
        let stats = fold_endpoint_paths(&mut fp, &consts, repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                hosts: 1,
                preset: 0,
                ..FoldStats::default()
            }
        );
        assert_eq!(
            payload(&fp, 1),
            r#"{"method":"GET","path":"/users","template":"${environment.apiUrl}/users","folded_from":"${…}/users","host":"users-svc.prod.svc.cluster.local","hosts":["users-svc.prod.svc.cluster.local","users-service:8080",""]}"#
        );

        // One distinct host across two bindings: nothing new is written.
        let mut same = ConstTable::scan_file(
            "export const environment = { apiUrl: 'http://users-service:8080' };\n",
            "typescript",
        );
        same.merge_from(&ConstTable::scan_file(
            "export const environment = { apiUrl: 'http://users-service:8080/v2' };\n",
            "typescript",
        ));
        let mut fp = file();
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","template":"${environment.apiUrl}/users"}"#,
        );
        fold_endpoint_paths(&mut fp, &same, repo());
        assert!(!payload(&fp, 1).contains("hosts"), "{}", payload(&fp, 1));

        // A literal authority has no alternatives.
        assert!(deployment_hosts("https://a/x", &consts, Some("a")).is_empty());
        // The set must start with the host the fold recorded.
        assert!(deployment_hosts("${environment.apiUrl}/x", &consts, Some("other")).is_empty());
    }

    /// A11.4 end to end on the `xstack-host-pairing` fixture, through
    /// `generate_many`: two Go services serve the same paths, the client's
    /// base names `users-service`, and only the users repo declares it.
    #[test]
    fn xstack_host_pairing_fixture_pairs_only_with_the_named_service() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bench/substrate-gap/fixtures/xstack-host-pairing"
        );
        let r = crate::generate_many(&[
            format!("{root}/web"),
            format!("{root}/users"),
            format!("{root}/orders"),
        ])
        .expect("fixture builds");
        let m = &r.merged;
        let alias = m
            .node_id_by_qname("infra:service:users-service")
            .expect("compose alias");
        let users_repo = m
            .graphs
            .iter()
            .find(|g| g.nodes.iter().any(|n| n.id == alias))
            .map(|g| g.repo)
            .expect("alias has a repo");
        let route_repo: HashMap<NodeId, RepoId> = m
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter().map(move |n| (n.id, g.repo)))
            .collect();
        let calls: Vec<_> = m
            .cross_edges
            .iter()
            .filter(|e| e.category == glia_code_domain::edge_category::HTTP_CALLS)
            .collect();
        // Before A11.4: 4 (each endpoint paired with both services).
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls.iter().all(|e| route_repo.get(&e.to) == Some(&users_repo)),
            "every HTTP_CALLS target must be a users-service route"
        );
        for q in ["endpoint:GET:/users", "endpoint:GET:/users/${…}"] {
            let ep = m.node_id_by_qname(q).expect(q);
            assert!(calls.iter().any(|e| e.from == ep), "{q} lost its pairing");
        }
    }

    /// Two call sites on one placeholder path, only one of whose bases
    /// resolves: each keeps its own CALLS edge.
    #[test]
    fn split_call_sites_keep_their_own_edges() {
        let mut fp = file();
        let old = push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":1,"col":1,"confidence":"medium","template":"${other.base}/users"}"#,
        );
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":2,"col":1,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        let stats = fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                hosts: 1,
                preset: 0,
                ..FoldStats::default()
            }
        );
        let new = ep_id("GET", "/users");
        assert_eq!((fp.nodes[1].id, fp.nodes[2].id), (old, new));
        assert_eq!((fp.edges[0].to, fp.edges[1].to), (old, new));
        // Both ids are navigable, both under the same parent.
        assert!(fp.nav.qname_by_id.contains_key(&old));
        assert_eq!(fp.nav.parent_of.get(&new), Some(&func()));
        assert_eq!(fp.nav.children_of.get(&func()), Some(&vec![old, new]));
    }

    /// Two call sites already on `/users` and on the placeholder: the fold
    /// lands the second on the first's id without duplicating its nav entry.
    #[test]
    fn fold_onto_an_existing_id_merges_cleanly() {
        let mut fp = file();
        let existing = push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong"}"#,
        );
        let old = push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":2,"col":1,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!((fp.nodes[1].id, fp.nodes[2].id), (existing, existing));
        assert!(fp.edges.iter().all(|e| e.to == existing));
        assert_eq!(fp.nav.children_of.get(&func()), Some(&vec![existing]));
        assert!(!fp.nav.qname_by_id.contains_key(&old));
    }

    /// End to end on the `angular-base-url` fixture, through `generate_many`,
    /// the path grade.py takes. The base URL is bound in ANOTHER file, and the
    /// folded endpoint pairs with the Go route across the repo boundary.
    #[test]
    fn angular_base_url_fixture_folds_and_pairs_across_repos() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bench/substrate-gap/fixtures/angular-base-url"
        );
        let r = crate::generate_many(&[format!("{root}/client"), format!("{root}/server")])
            .expect("fixture builds");
        let m = &r.merged;
        let ep = m
            .node_id_by_qname("endpoint:GET:/users")
            .expect("folded endpoint");
        assert!(m.node_id_by_qname("endpoint:GET:${…}/users").is_none());
        // LB.11a: the Go route is one node per (method, path).
        let route = m.node_id_by_qname("GET /users").expect("go route");
        assert!(
            m.cross_edges.iter().any(|e| e.from == ep
                && e.to == route
                && e.category == glia_code_domain::edge_category::HTTP_CALLS),
            "HTTP_CALLS endpoint:GET:/users -> GET /users"
        );
        let payloads: Vec<&str> = m
            .graphs
            .iter()
            .flat_map(|g| &g.nodes)
            .filter(|n| n.id == ep)
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(payloads.len(), 1, "{payloads:?}");
        let p = payloads[0];
        for want in [
            r#""path":"/users""#,
            r#""template":"${environment.apiUrl}/users""#,
            r#""folded_from":"${…}/users""#,
            r#""host":"users-service:8080""#,
        ] {
            assert!(p.contains(want), "{want} missing from {p}");
        }
    }

    /// A parse served from the cache is folded too, and the cache keeps the
    /// PRE-fold parse, so an edit to the constant's file re-folds on the next
    /// build instead of replaying a fold made against the old value.
    #[test]
    fn cached_parses_are_folded_and_the_cache_stays_pre_fold() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bench/substrate-gap/fixtures/angular-base-url/client"
        );
        let mut cache = crate::ParseCache::new();
        let cold = crate::generate_one_with_cache(root, &mut cache).expect("cold build");
        let warm = crate::generate_one_with_cache(root, &mut cache).expect("warm build");
        assert_eq!(cache.stats.reparsed, 0, "{:?}", cache.stats);
        assert!(cache.stats.reused > 0, "{:?}", cache.stats);
        for r in [&cold, &warm] {
            assert!(r.merged.node_id_by_qname("endpoint:GET:/users").is_some());
            assert!(
                r.merged
                    .node_id_by_qname("endpoint:GET:${…}/users")
                    .is_none()
            );
        }

        let src = std::fs::read_to_string(format!("{root}/users.service.ts")).expect("source");
        let cached = cache
            .get(
                "users.service.ts",
                crate::cache::content_hash(&src),
                "typescript",
            )
            .expect("users.service.ts is cached");
        let qnames: Vec<&String> = cached.nav.qname_by_id.values().collect();
        assert!(
            qnames.iter().any(|q| *q == "endpoint:GET:${…}/users"),
            "cache must hold the parser's own identity: {qnames:?}"
        );
        assert!(!qnames.iter().any(|q| *q == "endpoint:GET:/users"));
    }

    /// LF.2d: a base the source cannot bind (`process.env`), pinned by an
    /// overlay constant, folds; the entry records which pin it read and
    /// becomes Weak. The same template against the source-only table is left
    /// alone, and a source-folded entry is not marked.
    #[test]
    fn pinned_base_folds_marks_overlay_and_weakens() {
        let json = r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${GATEWAY}/users"}"#;
        let mut consts = ConstTable::scan_file("const GATEWAY = process.env.GATEWAY_URL;\n", "typescript");
        assert!(consts.get("GATEWAY").is_none(), "an env read never binds");

        let mut fp = file();
        push_call(&mut fp, "${…}/users", json);
        let before = fp.clone();
        assert_eq!(fold_endpoint_paths(&mut fp, &consts, repo()), FoldStats::default());
        assert!(same(&fp, &before));

        assert!(consts.pin("GATEWAY", "/orders-svc"));
        let stats = fold_endpoint_paths(&mut fp, &consts, repo());
        assert_eq!(stats.folded, 1);
        let new = ep_id("GET", "/orders-svc/users");
        assert_eq!(fp.nodes[1].id, new);
        assert_eq!(fp.nodes[1].confidence, Confidence::Weak);
        assert_eq!(fp.nav.qname_by_id.get(&new).map(String::as_str), Some("endpoint:GET:/orders-svc/users"));
        let v: Value = serde_json::from_str(payload(&fp, 1)).unwrap();
        assert_eq!(v["overlay"], "const:GATEWAY");
        assert_eq!(v["path"], "/orders-svc/users");
        assert_eq!(v["folded_from"], "${…}/users");
        assert!(fp.edges.iter().any(|e| e.to == new), "the CALLS edge follows the node");

        // A source binding folds without the overlay mark or a confidence change.
        let mut fp = file();
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        assert_eq!(fold_endpoint_paths(&mut fp, &consts_with_source(), repo()).folded, 1);
        assert_eq!(fp.nodes[1].confidence, Confidence::Medium);
        assert!(!payload(&fp, 1).contains("overlay"));
    }

    /// The source table plus an unrelated pin: a pin the template never reads
    /// marks nothing.
    fn consts_with_source() -> ConstTable {
        let mut t = table();
        assert!(t.pin("GATEWAY", "/orders-svc"));
        t
    }

    // ---- CH.5b: URL builders that read a configured API prefix ------------

    /// quokka's builder file reduced to its nav: a MODULE (whose POSITION
    /// names `file`, the owner pass's fallback), a CLASS `class`, and one
    /// METHOD per `(name, prefix key)` (`None` = no UrlPrefixKey fact).
    fn builder_parse(file: &str, class: &str, methods: &[(&str, Option<&str>)]) -> FileParse {
        let mut fp = FileParse::default();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, file);
        fp.nodes.push(Node {
            id: module,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(r#"{{"file":"{file}","line":0}}"#)),
            }],
        });
        fp.nav.record(module, file, file, node_kind::MODULE, None);
        let cq = format!("{file}::{class}");
        let cid = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, &cq);
        fp.nav.record(cid, class, &cq, node_kind::CLASS, Some(module));
        for (name, key) in methods {
            let mq = format!("{cq}::{name}");
            let mid = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, &mq);
            fp.nav.record(mid, name, &mq, node_kind::METHOD, Some(cid));
            if let Some(key) = key {
                fp.nav.record_fact(mid, NavFact::UrlPrefixKey { key: key.to_string() });
            }
        }
        fp
    }

    /// The quokka builder: `buildApiUrl` reads `apiPrefix`, `buildApiRootUrl`
    /// reads none.
    fn quokka_builder() -> FileParse {
        builder_parse(
            "src/app/api-url-builder.service.ts",
            "ApiUrlBuilderService",
            &[("buildApiUrl", Some("apiPrefix")), ("buildApiRootUrl", None)],
        )
    }

    fn env_prefix(value: &str) -> ConstTable {
        ts_table(&format!("export const environment = {{\n  apiPrefix: '{value}',\n}};\n"))
    }

    /// A builder-read call site's ENDPOINT_HIT, the way CH.3a / CH.3b write it.
    fn builder_hit(path: &str, raw: &str, wrapper: &str, wrapper_of: Option<&str>) -> String {
        let of = wrapper_of.map_or(String::new(), |t| format!(r#","wrapper_of":"{t}""#));
        format!(
            r#"{{"method":"GET","path":"{path}","file":"src/app/friend.service.ts","line":11,"col":21,"confidence":"weak","raw":"{raw}","wrapper":"{wrapper}"{of}}}"#
        )
    }

    /// Fold `caller` beside `builder` over one repo with no nested roots.
    fn fold_pair(builder: &mut FileParse, caller: &mut FileParse, consts: &ConstTable) -> FoldStats {
        fold_repo(vec![builder, caller], consts, repo(), &[])
    }

    /// CH.5b (a): quokka's `buildApiUrl('protected/friends')`, the builder's
    /// `apiPrefix` bound to `/api` by the environment: the call site moves to
    /// `/api/protected/friends`, records the prefix and its member, becomes
    /// Medium, and its CALLS edge and nav entry follow it.
    #[test]
    fn builder_fact_prefixes_the_call_site() {
        let mut builder = quokka_builder();
        let mut caller = file();
        let old = push_call(
            &mut caller,
            "/protected/friends",
            &builder_hit("/protected/friends", "protected/friends", "buildApiUrl", Some("ApiUrlBuilderService")),
        );
        caller.nodes[1].confidence = Confidence::Weak;
        let stats = fold_pair(&mut builder, &mut caller, &env_prefix("/api"));
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                prefixed_wrapper: 1,
                prefixes: BTreeSet::from(["/api".to_string()]),
                ..FoldStats::default()
            }
        );
        let new = ep_id("GET", "/api/protected/friends");
        assert_eq!(caller.nodes[1].id, new);
        assert_eq!(caller.nodes[1].confidence, Confidence::Medium, "Weak -> Medium, never Strong");
        assert_eq!(
            payload(&caller, 1),
            r#"{"method":"GET","path":"/api/protected/friends","file":"src/app/friend.service.ts","line":11,"col":21,"confidence":"weak","raw":"protected/friends","wrapper":"buildApiUrl","wrapper_of":"ApiUrlBuilderService","folded_from":"/protected/friends","prefix":"/api","prefix_from":"apiPrefix"}"#
        );
        assert_eq!(caller.edges[0].to, new, "the CALLS edge follows the node");
        assert_eq!(
            caller.nav.qname_by_id.get(&new).map(String::as_str),
            Some("endpoint:GET:/api/protected/friends")
        );
        assert_eq!(caller.nav.parent_of.get(&new), Some(&func()));
        assert!(!caller.nav.qname_by_id.contains_key(&old));

        // A trailing `/` on the configured value and a second, agreeing
        // binding (`environment.prod.ts`) give the same prefix.
        let mut consts = env_prefix("/api/");
        consts.merge_from(&ts_table("export const DEFAULT_CONFIG = { apiPrefix: '/api' };\n"));
        let mut builder = quokka_builder();
        let mut caller = file();
        push_call(
            &mut caller,
            "/protected/friends",
            &builder_hit("/protected/friends", "protected/friends", "buildApiUrl", Some("ApiUrlBuilderService")),
        );
        assert_eq!(fold_pair(&mut builder, &mut caller, &consts).prefixed_wrapper, 1);
        assert_eq!(caller.nodes[1].id, new);

        // LF.2d: a prefix read off an overlay pin is Weak and says so.
        let mut pinned = ConstTable::default();
        assert!(pinned.pin("apiPrefix", "/gw"));
        let mut builder = quokka_builder();
        let mut caller = file();
        push_call(
            &mut caller,
            "/protected/friends",
            &builder_hit("/protected/friends", "protected/friends", "buildApiUrl", Some("ApiUrlBuilderService")),
        );
        fold_pair(&mut builder, &mut caller, &pinned);
        assert_eq!(caller.nodes[1].id, ep_id("GET", "/gw/protected/friends"));
        assert_eq!(caller.nodes[1].confidence, Confidence::Weak);
        assert!(payload(&caller, 1).ends_with(
            r#""prefix":"/gw","prefix_from":"apiPrefix","overlay":"const:apiPrefix"}"#
        ));
    }

    /// CH.5b (b): the root builder reads no prefix (quokka's
    /// `buildApiRootUrl('healthz')`), and a builder call whose receiver type
    /// is unknown (no `wrapper_of`) names no builder: both byte-identical.
    #[test]
    fn root_builder_without_fact_is_untouched() {
        let mut builder = quokka_builder();
        let mut caller = file();
        push_call(
            &mut caller,
            "/healthz",
            &builder_hit("/healthz", "healthz", "buildApiRootUrl", Some("ApiUrlBuilderService")),
        );
        push_call(
            &mut caller,
            "/protected/later",
            &builder_hit("/protected/later", "protected/later", "buildApiUrl", None),
        );
        let before = caller.clone();
        let stats = fold_pair(&mut builder, &mut caller, &env_prefix("/api"));
        assert_eq!(stats, FoldStats::default());
        assert!(same(&caller, &before));
    }

    /// CH.5b (c): a member bound to two paths, or to a value that is no
    /// `/`-led path, prefixes nothing.
    #[test]
    fn disagreeing_prefix_values_fold_nothing() {
        let mut two = env_prefix("/api");
        two.merge_from(&env_prefix("/v2"));
        let mut nested = ts_table("const apiPrefix = '/api';\n");
        nested.merge_from(&ts_table("export const DEFAULT_CONFIG = { apiPrefix: '/v2' };\n"));
        for consts in [two, nested, env_prefix("api"), env_prefix("/"), ConstTable::default()] {
            let mut builder = quokka_builder();
            let mut caller = file();
            push_call(
                &mut caller,
                "/protected/friends",
                &builder_hit("/protected/friends", "protected/friends", "buildApiUrl", Some("ApiUrlBuilderService")),
            );
            let before = caller.clone();
            assert_eq!(fold_pair(&mut builder, &mut caller, &consts), FoldStats::default());
            assert!(same(&caller, &before));
        }
        assert!(configured_prefix("apiPrefix", &env_prefix("${base}/api")).is_none());
        assert_eq!(
            configured_prefix("apiPrefix", &env_prefix("http://gw:8080/api?x=1")).map(|p| p.path),
            Some("/api".to_string())
        );
    }

    /// CH.5b (d): a path already under the prefix (written out at the call
    /// site, or a builder that strips a duplicate) is left alone; a path that
    /// only starts with the same letters is not under it.
    #[test]
    fn already_prefixed_path_is_untouched() {
        let mut builder = quokka_builder();
        let mut caller = file();
        push_call(
            &mut caller,
            "/api/protected/x",
            &builder_hit("/api/protected/x", "api/protected/x", "buildApiUrl", Some("ApiUrlBuilderService")),
        );
        push_call(
            &mut caller,
            "/api",
            &builder_hit("/api", "/api", "buildApiUrl", Some("ApiUrlBuilderService")),
        );
        let before = caller.clone();
        assert_eq!(fold_pair(&mut builder, &mut caller, &env_prefix("/api")), FoldStats::default());
        assert!(same(&caller, &before));
        assert!(under_prefix("/api/x", "/api") && under_prefix("/api", "/api"));
        assert!(!under_prefix("/apiary/x", "/api"));
    }

    /// CH.5b (e): the same method name on another type is another builder,
    /// and a module-level function keys nothing. Under nested roots, a call
    /// site finds its own owner's builder first, another owner's only when
    /// that owner is the one declaring it.
    #[test]
    fn other_class_same_method_name_does_not_match() {
        let mut builder = quokka_builder();
        let mut caller = file();
        push_call(
            &mut caller,
            "/protected/friends",
            &builder_hit("/protected/friends", "protected/friends", "buildApiUrl", Some("OtherBuilder")),
        );
        let before = caller.clone();
        assert_eq!(fold_pair(&mut builder, &mut caller, &env_prefix("/api")), FoldStats::default());
        assert!(same(&caller, &before));

        let mut module_fn = FileParse::default();
        let f = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "urls::buildApiUrlFrom");
        let m = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "urls");
        module_fn.nav.record(m, "urls", "urls", node_kind::MODULE, None);
        module_fn.nav.record(f, "buildApiUrlFrom", "urls::buildApiUrlFrom", node_kind::FUNCTION, Some(m));
        module_fn.nav.record_fact(f, NavFact::UrlPrefixKey { key: "apiPrefix".into() });
        let ctx = PrefixContext::build(&[&mut module_fn], &env_prefix("/api"), &[]);
        assert!(ctx.builders.is_empty(), "{ctx:?}");

        // Owners `web` and `shared`: the builder lives in `shared` alone, so
        // a `web` call site finds it there.
        let roots = [
            ProjectRoot::new("web".into(), "npm", "package.json", None),
            ProjectRoot::new("shared".into(), "npm", "package.json", None),
        ];
        let hit = |file: &str| {
            builder_hit("/protected/friends", "protected/friends", "buildApiUrl", Some("ApiUrlBuilderService"))
                .replace("src/app/friend.service.ts", file)
        };
        let mut shared = builder_parse(
            "shared/src/api-url-builder.service.ts",
            "ApiUrlBuilderService",
            &[("buildApiUrl", Some("apiPrefix"))],
        );
        let mut caller = file();
        push_call(&mut caller, "/protected/friends", &hit("web/src/friend.service.ts"));
        let stats = fold_repo(vec![&mut shared, &mut caller], &env_prefix("/api"), repo(), &roots);
        assert_eq!(stats.prefixed_wrapper, 1);
        assert_eq!(caller.nodes[1].id, ep_id("GET", "/api/protected/friends"));

        // `web` declares its own ApiUrlBuilderService.buildApiUrl, which
        // reads no prefix: `web`'s call site is not prefixed through `shared`.
        let mut own = builder_parse("web/src/api-url-builder.service.ts", "ApiUrlBuilderService", &[("buildApiUrl", None)]);
        let mut shared = builder_parse(
            "shared/src/api-url-builder.service.ts",
            "ApiUrlBuilderService",
            &[("buildApiUrl", Some("apiPrefix"))],
        );
        let mut caller = file();
        push_call(&mut caller, "/protected/friends", &hit("web/src/friend.service.ts"));
        let before = caller.clone();
        let stats = fold_repo(vec![&mut own, &mut shared, &mut caller], &env_prefix("/api"), repo(), &roots);
        assert_eq!(stats.prefixed_wrapper, 0);
        assert!(same(&caller, &before));
    }

    /// CH.5b (f): a builder template (CH.3a) keeps its placeholder and its
    /// `template` source; only the path moves.
    #[test]
    fn template_entry_keeps_its_placeholders() {
        let mut builder = quokka_builder();
        let mut caller = file();
        push_call_as(
            &mut caller,
            "POST",
            "/protected/friends/accept/${…}",
            r#"{"method":"POST","path":"/protected/friends/accept/${…}","file":"src/app/friend.service.ts","line":15,"col":21,"confidence":"weak","raw":"protected/friends/accept/${…}","template":"protected/friends/accept/${publicId}","wrapper":"buildApiUrl","wrapper_of":"ApiUrlBuilderService"}"#,
        );
        let stats = fold_pair(&mut builder, &mut caller, &env_prefix("/api"));
        assert_eq!((stats.folded, stats.prefixed_wrapper), (1, 1));
        let new = ep_id("POST", "/api/protected/friends/accept/${…}");
        assert_eq!(caller.nodes[1].id, new);
        assert_eq!(caller.edges[0].to, new);
        assert_eq!(
            payload(&caller, 1),
            r#"{"method":"POST","path":"/api/protected/friends/accept/${…}","file":"src/app/friend.service.ts","line":15,"col":21,"confidence":"weak","raw":"protected/friends/accept/${…}","template":"protected/friends/accept/${publicId}","wrapper":"buildApiUrl","wrapper_of":"ApiUrlBuilderService","folded_from":"/protected/friends/accept/${…}","prefix":"/api","prefix_from":"apiPrefix"}"#
        );
    }

    /// CH.5b end to end on the `ts-api-prefix-builder` fixture, through
    /// `generate_many`: the buildApiUrl call sites pair EXACT with the gin
    /// routes under `/api`, and the root builder's `/healthz` keeps its key.
    #[test]
    fn ts_api_prefix_builder_fixture_pairs_exact() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bench/substrate-gap/fixtures/ts-api-prefix-builder"
        );
        let r = crate::generate_many(&[format!("{root}/web"), format!("{root}/server")])
            .expect("fixture builds");
        let m = &r.merged;
        for gone in ["endpoint:GET:/protected/friends", "endpoint:GET:/api/healthz"] {
            assert!(m.node_id_by_qname(gone).is_none(), "{gone} must not exist");
        }
        let hit_of = |ep: NodeId| -> String {
            m.graphs
                .iter()
                .flat_map(|g| &g.nodes)
                .filter(|n| n.id == ep)
                .flat_map(|n| &n.cells)
                .find_map(|c| match (&c.payload, c.kind == cell_type::ENDPOINT_HIT) {
                    (CellPayload::Json(s), true) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        };
        for (ep, route, prefixed) in [
            ("endpoint:GET:/api/protected/friends", "GET /api/protected/friends", true),
            ("endpoint:POST:/api/protected/friends/accept/${…}", "POST /api/protected/friends/accept/:publicId", true),
            ("endpoint:GET:/healthz", "GET /healthz", false),
        ] {
            let e = m.node_id_by_qname(ep).unwrap_or_else(|| panic!("{ep} missing"));
            let to = m.node_id_by_qname(route).unwrap_or_else(|| panic!("{route} missing"));
            let call = m
                .cross_edges
                .iter()
                .find(|x| x.from == e && x.category == glia_code_domain::edge_category::HTTP_CALLS)
                .unwrap_or_else(|| panic!("{ep} unpaired"));
            assert_eq!(call.to, to, "{ep}");
            let evidence = call
                .cells
                .iter()
                .find_map(|c| match (&c.payload, c.kind == cell_type::EVIDENCE) {
                    (CellPayload::Json(s), true) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or_default();
            assert!(evidence.contains(r#""rule":"exact""#), "{ep}: {evidence}");
            assert_eq!(hit_of(e).contains(r#""prefix":"/api","prefix_from":"apiPrefix""#), prefixed, "{ep}");
        }
    }

    // ---- CG.4a: external call sites ----------------------------------------

    /// Fold one file the way the build does, marking included.
    fn fold_one(fp: &mut FileParse, consts: &ConstTable) -> FoldStats {
        fold_repo(std::iter::once(fp), consts, repo(), &[])
    }

    fn ts_table(src: &str) -> ConstTable {
        ConstTable::scan_file(src, "typescript")
    }

    // ---- CH.5c: a Dart project's Dio base path ------------------------------

    /// A Dart file's MODULE (its POSITION names `file`) carrying `facts`, the
    /// way CH.5a records ClientBase / ValueLiteral.
    fn dart_module(file: &str, facts: Vec<NavFact>) -> FileParse {
        let mut fp = FileParse::default();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, file);
        fp.nodes.push(Node {
            id: module,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(r#"{{"file":"{file}","line":0}}"#)),
            }],
        });
        fp.nav.record(module, file, file, node_kind::MODULE, None);
        for fact in facts {
            fp.nav.record_fact(module, fact);
        }
        fp
    }

    fn client_base(expr: &str, line: u32) -> NavFact {
        NavFact::ClientBase {
            via: "dio".into(),
            expr: expr.into(),
            line,
        }
    }

    fn value_literal(name: &str, value: &str) -> NavFact {
        NavFact::ValueLiteral {
            name: name.into(),
            value: value.into(),
        }
    }

    /// quokka_android's two clients (`Env.apiBaseUrl`) and the getter that
    /// holds `'$scheme://$host/api'`.
    fn quokka_android() -> (FileParse, FileParse) {
        (
            dart_module(
                "app/lib/core/api_client.dart",
                vec![client_base("Env.apiBaseUrl", 20), client_base("Env.apiBaseUrl", 34)],
            ),
            dart_module(
                "app/lib/core/env.dart",
                vec![value_literal("Env.apiBaseUrl", "${…}://${…}/api")],
            ),
        )
    }

    /// A Dio call site's ENDPOINT_HIT, the way the Dart parser writes it.
    fn dart_hit(method: &str, path: &str, file: &str) -> String {
        format!(
            r#"{{"method":"{method}","path":"{path}","file":"{file}","line":9,"col":11,"confidence":"strong"}}"#
        )
    }

    /// Two nested Flutter projects, `app` and `other`.
    fn dart_roots() -> Vec<ProjectRoot> {
        ["app", "other"]
            .into_iter()
            .map(|r| ProjectRoot::new(r.into(), "pub", "pubspec.yaml", None))
            .collect()
    }

    /// One Dart caller file under `app` with a `POST <path>` call site.
    fn dart_caller(path: &str) -> FileParse {
        let mut caller = file();
        push_call_as(&mut caller, "POST", path, &dart_hit("POST", path, "app/lib/f.dart"));
        caller.nodes[1].confidence = Confidence::Strong;
        caller
    }

    fn fold_dart(parses: Vec<&mut FileParse>, consts: &ConstTable) -> FoldStats {
        fold_repo(parses, consts, repo(), &dart_roots())
    }

    /// CH.5c (a): quokka_android. Its Dio bases read `Env.apiBaseUrl`, a
    /// getter returning `${…}://${…}/api`, so the project's root-relative
    /// call `POST /protected/friends` moves to `/api/protected/friends`,
    /// records the path and the expression, and becomes Medium; its CALLS
    /// edge and nav entry follow.
    #[test]
    fn dart_project_base_path_prefixes_its_calls() {
        let (mut client, mut env) = quokka_android();
        let mut caller = dart_caller("/protected/friends");
        let old = caller.nodes[1].id;
        let stats = fold_dart(vec![&mut client, &mut env, &mut caller], &ConstTable::default());
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                prefixed_base: 1,
                prefixes: BTreeSet::from(["/api".to_string()]),
                ..FoldStats::default()
            }
        );
        let new = ep_id("POST", "/api/protected/friends");
        assert_eq!(caller.nodes[1].id, new);
        assert_eq!(caller.nodes[1].confidence, Confidence::Medium, "Strong -> Medium");
        assert_eq!(
            payload(&caller, 1),
            r#"{"method":"POST","path":"/api/protected/friends","file":"app/lib/f.dart","line":9,"col":11,"confidence":"strong","folded_from":"/protected/friends","prefix":"/api","prefix_from":"Env.apiBaseUrl"}"#
        );
        assert_eq!(caller.edges[0].to, new, "the CALLS edge follows the node");
        assert_eq!(
            caller.nav.qname_by_id.get(&new).map(String::as_str),
            Some("endpoint:POST:/api/protected/friends")
        );
        assert!(!caller.nav.qname_by_id.contains_key(&old));

        // The parse order never shows.
        let (mut client, mut env) = quokka_android();
        let mut again = dart_caller("/protected/friends");
        fold_dart(vec![&mut again, &mut env, &mut client], &ConstTable::default());
        assert!(same(&caller, &again));
    }

    /// CH.5c (b): a base copied from an existing request (`retry.baseUrl`)
    /// and a host-only literal base (a root path) add nothing; the project
    /// keeps its one path.
    #[test]
    fn copied_base_is_ignored() {
        let (mut client, mut env) = quokka_android();
        let mut retry = dart_module(
            "app/lib/core/auth_interceptor.dart",
            vec![
                client_base("retry.baseUrl", 56),
                client_base("'https://api.example.net'", 60),
            ],
        );
        let mut caller = dart_caller("/protected/friends");
        let stats = fold_dart(
            vec![&mut client, &mut env, &mut retry, &mut caller],
            &ConstTable::default(),
        );
        assert_eq!(stats.prefixed_base, 1);
        assert!(stats.blocked_bases.is_empty());
        assert_eq!(caller.nodes[1].id, ep_id("POST", "/api/protected/friends"));
    }

    /// CH.5c (c): a base nothing resolves blocks its whole project (an
    /// unknown client might use another base), and so do two bases that
    /// disagree. Every entry stays byte-identical and the project is named
    /// once for `[endpoint-base]`.
    #[test]
    fn unresolved_base_blocks_the_project() {
        let (mut client, mut env) = quokka_android();
        let mut cfg = dart_module("app/lib/cfg.dart", vec![client_base("cfg.url", 3)]);
        let mut caller = dart_caller("/protected/friends");
        let before = caller.clone();
        let stats = fold_dart(
            vec![&mut client, &mut env, &mut cfg, &mut caller],
            &ConstTable::default(),
        );
        assert_eq!(
            stats,
            FoldStats {
                blocked_bases: BTreeMap::from([(
                    "app".to_string(),
                    "unresolved base cfg.url".to_string()
                )]),
                ..FoldStats::default()
            }
        );
        assert!(same(&caller, &before));

        let (mut client, mut env) = quokka_android();
        let mut v2 = dart_module("app/lib/v2.dart", vec![client_base("'/v2/'", 1)]);
        let mut caller = dart_caller("/protected/friends");
        let stats = fold_dart(
            vec![&mut client, &mut env, &mut v2, &mut caller],
            &ConstTable::default(),
        );
        assert_eq!(
            stats.blocked_bases.get("app").map(String::as_str),
            Some("bases disagree on /api,/v2")
        );
        assert_eq!(stats.prefixed_base, 0);
        assert!(same(&caller, &before));
    }

    /// CH.5c (d): the const table is repo-wide and language-blind. An
    /// Angular `environment.ts` binds a bare `apiBaseUrl` to its own root
    /// URL; the lenient lookup would answer the Dart expression with it (a
    /// root, so the project would silently lose its block). The strict
    /// lookup does not, so with no ValueLiteral the project is blocked.
    #[test]
    fn lenient_const_never_answers_a_dart_base() {
        let consts = ts_table(
            "export const environment = {\n  apiBaseUrl: 'http://localhost:8080',\n};\n",
        );
        assert_eq!(
            consts.resolve_expr("AppConfig.apiBaseUrl"),
            Some("http://localhost:8080"),
            "the lenient lookup would answer"
        );
        let mut client = dart_module(
            "app/lib/core/api_client.dart",
            vec![client_base("AppConfig.apiBaseUrl", 20)],
        );
        let mut caller = dart_caller("/protected/friends");
        let before = caller.clone();
        let stats = fold_dart(vec![&mut client, &mut caller], &consts);
        assert_eq!(stats.prefixed_base, 0);
        assert_eq!(
            stats.blocked_bases.get("app").map(String::as_str),
            Some("unresolved base AppConfig.apiBaseUrl")
        );
        assert!(same(&caller, &before));

        // An exact key resolves (an overlay pin is how a blocked project is
        // unblocked), and the entry says the pin produced it: Weak.
        let mut pinned = consts.clone();
        assert!(pinned.pin("AppConfig.apiBaseUrl", "https://api.example.net/v1"));
        let mut client = dart_module(
            "app/lib/core/api_client.dart",
            vec![client_base("AppConfig.apiBaseUrl", 20)],
        );
        let mut caller = dart_caller("/protected/friends");
        let stats = fold_dart(vec![&mut client, &mut caller], &pinned);
        assert_eq!(stats.prefixed_base, 1);
        assert_eq!(caller.nodes[1].id, ep_id("POST", "/v1/protected/friends"));
        assert_eq!(caller.nodes[1].confidence, Confidence::Weak);
        assert!(payload(&caller, 1).ends_with(
            r#""prefix":"/v1","prefix_from":"AppConfig.apiBaseUrl","overlay":"const:AppConfig.apiBaseUrl"}"#
        ));
    }

    /// CH.5c (e): only the project's own root-relative, host-less Dart
    /// entries move. A Dart entry of another project, one whose parser wrote
    /// a host, a `${…}`-based one, one already under the path, and a
    /// TypeScript entry of the same project keep their bytes.
    #[test]
    fn other_project_and_host_entries_untouched() {
        let (mut client, mut env) = quokka_android();
        let mut caller = file();
        push_call_as(&mut caller, "GET", "/x", &dart_hit("GET", "/x", "other/lib/x.dart"));
        push_call_as(
            &mut caller,
            "GET",
            "/hosted",
            r#"{"method":"GET","path":"/hosted","file":"app/lib/f.dart","line":2,"col":3,"confidence":"strong","raw":"https://api.example.net/hosted","host":"api.example.net"}"#,
        );
        push_call_as(&mut caller, "GET", "${…}/based", &dart_hit("GET", "${…}/based", "app/lib/f.dart"));
        push_call_as(&mut caller, "GET", "/api/already", &dart_hit("GET", "/api/already", "app/lib/f.dart"));
        push_call_as(&mut caller, "GET", "/ts", &dart_hit("GET", "/ts", "app/src/a.ts"));
        let before = caller.clone();
        let stats = fold_dart(vec![&mut client, &mut env, &mut caller], &ConstTable::default());
        assert_eq!(stats.prefixed_base, 0);
        assert_eq!(stats.folded, 0);
        assert_eq!(stats.preset, 1, "the parser's host is counted, never re-recorded");
        assert!(same(&caller, &before));
    }

    /// CH.5c: what one base expression resolves to, in resolution order.
    #[test]
    fn client_base_resolution_order() {
        let mut values = DartValues::default();
        let app = Some("app".to_string());
        values.insert(&app, "Env.apiBaseUrl", "${…}://${…}/api");
        values.insert(&None, "Shared.base", "https://h/shared/");
        values.insert(&None, "Twice.base", "/a");
        values.insert(&Some("other".to_string()), "Twice.base", "/b");
        let consts = ConstTable::default();
        let path = |p: &str| Base::Path(p.to_string(), Vec::new());
        let at = |e: &str| resolve_client_base(e, &app, &values, &consts);
        assert_eq!(at("'https://x/api/'"), path("/api"));
        assert_eq!(at("\"/v1?x=1\""), path("/v1"));
        assert_eq!(at("r'/raw$x'"), path("/raw$x"));
        assert_eq!(at("'$scheme://${cfg.host}/gw'"), path("/gw"));
        assert_eq!(at("'https://x'"), Base::Root);
        assert_eq!(at("'$base'"), Base::Unresolved, "a whole-URL variable");
        assert_eq!(at("'/v${n}'"), Base::Unresolved, "an interpolated path");
        assert_eq!(at("'a' + b"), Base::Unresolved);
        assert_eq!(at("Env.apiBaseUrl"), path("/api"));
        assert_eq!(at("core.Env.apiBaseUrl"), path("/api"), "an import prefix");
        assert_eq!(at("Shared.base"), path("/shared"), "another project's value");
        assert_eq!(at("Twice.base"), Base::Unresolved, "bound to two values");
        assert_eq!(at("retry.baseUrl"), Base::Copy);
        assert_eq!(at("cfg.url"), Base::Unresolved);
    }

    /// quokka's nominatim call: the template's authority is literal, so the
    /// fold records `host` off `raw` and the mark appends `"external":true`.
    /// Nothing else about the entry moves.
    #[test]
    fn literal_public_host_is_marked() {
        let mut fp = file();
        let id = push_call(
            &mut fp,
            "/search",
            r#"{"method":"GET","path":"/search","file":"geo.ts","line":3,"col":5,"confidence":"medium","template":"https://nominatim.openstreetmap.org/search?q=${q}&format=json","raw":"https://nominatim.openstreetmap.org/search?q=${…}&format=json"}"#,
        );
        let edges = fp.edges.clone();
        let nav = fp.nav.qname_by_id.clone();
        let stats = fold_one(&mut fp, &ConstTable::default());
        assert_eq!((stats.hosts, stats.external, stats.configured), (1, 1, 0));
        assert_eq!(fp.nodes[1].id, id);
        assert_eq!(fp.nodes[1].confidence, Confidence::Medium);
        assert_eq!(
            payload(&fp, 1),
            r#"{"method":"GET","path":"/search","file":"geo.ts","line":3,"col":5,"confidence":"medium","template":"https://nominatim.openstreetmap.org/search?q=${q}&format=json","raw":"https://nominatim.openstreetmap.org/search?q=${…}&format=json","host":"nominatim.openstreetmap.org","external":true}"#
        );
        assert_eq!(fp.edges, edges);
        assert_eq!(fp.nav.qname_by_id, nav);
    }

    /// Kina's shape: the host comes out of `${environment.apiUrl}`, bound in a
    /// config file. A const-sourced host is never external, even when the key
    /// that bound it is not config-shaped (a bare `base`).
    #[test]
    fn const_sourced_host_is_never_marked() {
        for (src, template) in [
            (
                "export const environment = {\n  apiUrl: 'https://api.kinaswap.com/api',\n};\n",
                "${environment.apiUrl}/trades",
            ),
            ("const base = 'https://api.kinaswap.com/api';\n", "${base}/trades"),
        ] {
            let mut fp = file();
            push_call(
                &mut fp,
                "${…}/trades",
                &format!(
                    r#"{{"method":"GET","path":"${{…}}/trades","file":"a.ts","line":1,"col":1,"confidence":"medium","template":"{template}"}}"#
                ),
            );
            let stats = fold_one(&mut fp, &ts_table(src));
            assert_eq!((stats.folded, stats.hosts, stats.external), (1, 1, 0), "{src}");
            assert!(payload(&fp, 1).contains(r#""host":"api.kinaswap.com""#));
            assert!(!payload(&fp, 1).contains("external"), "{}", payload(&fp, 1));
        }
    }

    /// A literal host whose site a config-shaped constant names is the repo's
    /// own backend. A bare local `url` holding the same site configures
    /// nothing: any file can bind a local URL constant.
    #[test]
    fn configured_site_is_not_external() {
        let json = r#"{"method":"GET","path":"/orders","file":"a.ts","line":1,"col":1,"confidence":"strong","raw":"https://api.shop.io/orders"}"#;
        let configured = ts_table("export const environment = {\n  apiUrl: 'https://api.shop.io',\n};\n");
        let mut fp = file();
        push_call(&mut fp, "/orders", json);
        let stats = fold_one(&mut fp, &configured);
        assert_eq!((stats.hosts, stats.external, stats.configured), (1, 0, 1));
        assert!(!payload(&fp, 1).contains("external"));

        // SCREAMING_CASE is config-shaped too, and the site covers every
        // host under it.
        let screaming = ts_table("export const API_BASE_URL = 'https://gateway.shop.io/v1';\n");
        let mut fp = file();
        push_call(&mut fp, "/orders", json);
        assert_eq!(fold_one(&mut fp, &screaming).external, 0);

        let local = ts_table("const url = 'https://api.shop.io';\n");
        assert_eq!(local.get("url"), Some("https://api.shop.io"));
        let mut fp = file();
        push_call(&mut fp, "/orders", json);
        let stats = fold_one(&mut fp, &local);
        assert_eq!((stats.external, stats.configured), (1, 0));
        assert!(payload(&fp, 1).ends_with(r#""host":"api.shop.io","external":true}"#));
    }

    /// A11.5: a host the parser wrote from the literal it saw (Dart, Go ...)
    /// has no template, so it is literal.
    #[test]
    fn preset_host_is_literal() {
        let json = r#"{"method":"POST","path":"/v1/charges","file":"lib/pay.dart","line":3,"col":5,"confidence":"strong","raw":"https://api.stripe.com/v1/charges","host":"api.stripe.com"}"#;
        let mut fp = file();
        push_call(&mut fp, "/v1/charges", json);
        let stats = fold_one(&mut fp, &ConstTable::default());
        assert_eq!((stats.preset, stats.hosts, stats.external), (1, 0, 1));
        assert_eq!(
            payload(&fp, 1),
            format!("{},\"external\":true}}", json.strip_suffix('}').unwrap())
        );
    }

    /// Local, private, cluster-internal and reserved documentation hosts are
    /// never public, so none is marked: the bench fixtures' api.example.com
    /// keeps pairing with its own server.
    #[test]
    fn internal_and_reserved_hosts_are_not_public() {
        let mut fp = file();
        for (i, raw) in [
            "http://localhost:3701/x",
            "http://10.0.0.5/x",
            "http://users-svc/x",
            "http://users.default.svc.cluster.local/x",
            "https://api.example.com/x",
            "http://x.test/x",
            "http://[::1]:8080/x",
            "http://db.internal/x",
            "http://nas.home.arpa/x",
            "http://printer.lan/x",
            "https://docs.example.org/x",
        ]
        .iter()
        .enumerate()
        {
            push_call(
                &mut fp,
                "/x",
                &format!(r#"{{"method":"GET","path":"/x","line":{i},"raw":"{raw}"}}"#),
            );
        }
        let stats = fold_one(&mut fp, &ConstTable::default());
        assert_eq!((stats.hosts, stats.external), (11, 0));
        assert!(fp.nodes.iter().skip(1).all(|n| match &n.cells[0].payload {
            CellPayload::Json(s) => !s.contains("external"),
            _ => false,
        }));
        for h in [
            "localhost", "10.0.0.5", "users-svc", "api.example.com", "example.net", "x.test",
            "[::1]", "fe80::1", "a.localdomain", "home.arpa", "x.invalid", "x.example", "",
        ] {
            assert!(!is_public_host(h), "{h}");
        }
        for h in ["nominatim.openstreetmap.org", "api.stripe.com", "a.b.co.uk", "example.io"] {
            assert!(is_public_host(h), "{h}");
        }
    }

    #[test]
    fn site_of_handles_registry_slds() {
        assert_eq!(site_of("a.b.co.uk"), "b.co.uk");
        assert_eq!(site_of("api.kinaswap.com"), "kinaswap.com");
        assert_eq!(site_of("openstreetmap.org"), "openstreetmap.org");
        assert_eq!(site_of("nominatim.openstreetmap.org"), "openstreetmap.org");
        assert_eq!(site_of("www.gov.au"), "www.gov.au");
        assert_eq!(site_of("x.y.com.au"), "y.com.au");
        assert_eq!(site_of("x.y.ab.uk"), "ab.uk", "an unknown SLD falls back to two labels");
        assert_eq!(site_of("localhost"), "localhost");
        assert_eq!(host_only("u:p@API.Shop.io:8443"), "api.shop.io");
        assert_eq!(host_only("[::1]:8080"), "[::1]");
        assert_eq!(host_only("api.shop.io."), "api.shop.io");
    }

    /// `.glia/overlay.toml [constants]` is the escape hatch: a pinned URL
    /// configures its site whatever the key looks like.
    #[test]
    fn overlay_pin_configures_a_host() {
        let json = r#"{"method":"GET","path":"/search","file":"geo.ts","line":3,"col":5,"raw":"https://nominatim.openstreetmap.org/search?q=1"}"#;
        for key in ["GEO_BASE", "geoBase"] {
            let mut consts = ConstTable::default();
            assert!(consts.pin(key, "https://nominatim.openstreetmap.org"));
            let mut fp = file();
            push_call(&mut fp, "/search", json);
            let stats = fold_one(&mut fp, &consts);
            assert_eq!((stats.external, stats.configured), (0, 1), "{key}");
            assert!(!payload(&fp, 1).contains("external"));
        }
        // Without the pin, a lower-case key configures nothing.
        let mut fp = file();
        push_call(&mut fp, "/search", json);
        assert_eq!(fold_one(&mut fp, &ts_table("const geoBase = 'https://nominatim.openstreetmap.org';\n")).external, 1);
    }

    /// Sites come from EVERY parse of the repo before any entry is marked, so
    /// the build's HashMap-ordered parses mark the same entries in any order.
    /// Marking twice changes nothing.
    #[test]
    fn marking_is_order_independent_and_idempotent() {
        let literal = r#"{"method":"GET","path":"/orders","raw":"https://api.shop.io/orders"}"#;
        let sourced = r#"{"method":"GET","path":"${…}/users","template":"${base}/users"}"#;
        let consts = ts_table("const base = 'https://gw.shop.io';\n");
        for flip in [false, true] {
            let mut a = file();
            push_call(&mut a, "/orders", literal);
            let mut b = file();
            push_call(&mut b, "${…}/users", sourced);
            let parses: Vec<&mut FileParse> = if flip { vec![&mut b, &mut a] } else { vec![&mut a, &mut b] };
            let stats = fold_repo(parses, &consts, repo(), &[]);
            assert_eq!((stats.external, stats.configured), (0, 1), "flip={flip}");
            assert!(!payload(&a, 1).contains("external"));
        }

        let mut fp = file();
        push_call(&mut fp, "/orders", literal);
        assert_eq!(fold_one(&mut fp, &ConstTable::default()).external, 1);
        let once = fp.clone();
        let again = mark_external(&mut [&mut fp], &ConstTable::default());
        assert_eq!(again.external, 0);
        assert!(same(&fp, &once));
    }

    /// CB.21 puts `host` on WS_CLIENT / GRPC_CLIENT ENDPOINT_HITs: only an
    /// ENDPOINT entry is an HTTP call site, so nothing else is marked.
    #[test]
    fn only_endpoint_entries_are_marked() {
        let mut fp = file();
        let ws = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::WS_CLIENT, "ws_client:/stream");
        fp.nodes.push(Node {
            id: ws,
            repo: repo(),
            confidence: Confidence::Medium,
            cells: vec![Cell {
                kind: cell_type::ENDPOINT_HIT,
                payload: CellPayload::Json(r#"{"via":"ws","host":"stream.binance.com"}"#.into()),
            }],
        });
        fp.nav.record(ws, "/stream", "ws_client:/stream", node_kind::WS_CLIENT, Some(func()));
        let before = fp.clone();
        assert_eq!(fold_one(&mut fp, &ConstTable::default()), FoldStats::default());
        assert!(same(&fp, &before));
    }
}
