//! The path resolver: a path or path fragment named in a text, resolved to the
//! candidate files it can mean in the one file space. Owner: EB.5c.
//!
//! This is rank_eval.rs's `KPaths` (`new`, `map`, `file`, `dir`, `block`) with
//! `kpath_norm` ([`norm`]), `kbraces` and `kglob`, ported rule for rule. Every
//! names-file edge of typed id routing (IDR, EB.6) goes through it: a row
//! names `src/billing/export.rs`, `export.rs`, `billing/` or `src/{a,b}.rs`,
//! often an old path git later renamed. Its maps are BTreeMaps where the
//! harness used hash maps; they are only looked up, so nothing changes order.
//!
//! The routes, in [`PathResolver::file`]'s order: `exact` (a candidate path),
//! `rename` (an old path the history's rename fold carries into a candidate),
//! `suffix` / `name` (a unique `/`-segment suffix, `name` when it holds no
//! `/`), `context` (several suffix hits, narrowed by the block's other
//! resolved directories to the one sharing the most leading segments), and
//! `rename-suffix` (a unique suffix of a renamed path); [`PathResolver::dir`]
//! adds `dir` / `dir-suffix` (at most [`DIR_CAP`] files) and a glob is `glob`
//! (braces expanded, at most [`DIR_CAP`] hits).
//!
//! The renames come from the history snapshot ([`rename_pairs`]): its window
//! (2000 commits by default) where rank_eval read `git log -M
//! --diff-filter=R` over all history, so an older rename is invisible.
//!
//! Fired_on, stderr, printed by the caller once per routed text
//! ([`PathResolver::report`]): `[id-paths] repo=<label> surface=<s>
//! candidates=<c> renamed=<h> resolved=<n> routes=<route:n,..> ambiguous=<a>
//! over_cap=<o> unresolved=<u>`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};


use super::text::Candidates;

/// rank_eval's `KDIR_CAP`: a directory or glob naming more files names no
/// particular place.
pub const DIR_CAP: usize = 30;
/// rank_eval's rename-chain bound.
const MAX_HOPS: usize = 64;
/// rank_eval `kbraces`' pattern cap.
const MAX_BRACES: usize = 64;

/// The history snapshot's rename pairs `(old, new)`, oldest first (a later
/// rename of one old path wins when they are followed forward). It lives in
/// `external::history` beside the fold it mirrors and is re-exported here.
pub use crate::external::rename_pairs;

/// One normalised path mention ([`norm`]).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PTok {
    /// A file path or file name (an extension the candidates have).
    File(String),
    /// A directory (a trailing `/`, or a `/`-path with no known extension).
    Dir(String),
    /// A brace or `*` pattern holding a `/` or a `.`.
    Glob(String),
}

/// rank_eval `ext_of`: the lowercased extension of the last segment, empty
/// for none or a dotfile.
fn ext_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// rank_eval `dir_of`: everything before the last `/`, empty for none.
fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// rank_eval `kpath_norm`, exactly: a token normalised to a path mention.
/// Two rounds of trailing `.,:;!?` trims, `**` emphasis (not a `/**` glob
/// part) and `_` / `~` emphasis; URLs, `http*`, `www.` and `@` tokens with
/// no `/` dropped; `#L..` and `:<digit>..` line ranges cut; `./`, `../` and
/// `/` prefixes removed; then at least 3 chars with a letter and no `..`.
pub fn norm(raw: &str, exts: &BTreeSet<String>) -> Option<PTok> {
    let mut t: &str = raw;
    for _ in 0..2 {
        t = t.trim_end_matches(['.', ',', ':', ';', '!', '?']);
        if let Some(x) = t.strip_prefix("**").filter(|x| !x.starts_with('/')) {
            t = x;
        }
        if let Some(x) = t.strip_suffix("**").filter(|x| !x.ends_with('/')) {
            t = x;
        }
        t = t.trim_matches(|c: char| c == '_' || c == '~');
    }
    if t.contains("://") || t.starts_with("http") || t.starts_with("www.") || t.contains('@') && !t.contains('/') {
        return None;
    }
    let mut s = t.to_string();
    if let Some(i) = s.find("#L") {
        s.truncate(i);
    }
    if let Some(i) = s
        .char_indices()
        .find(|&(i, c)| c == ':' && s[i + 1..].starts_with(|d: char| d.is_ascii_digit()))
        .map(|x| x.0)
    {
        s.truncate(i);
    }
    let mut s = s.as_str();
    loop {
        if let Some(x) = s.strip_prefix("./") {
            s = x;
        } else if let Some(x) = s.strip_prefix("../") {
            s = x;
        } else if let Some(x) = s.strip_prefix('/') {
            s = x;
        } else {
            break;
        }
    }
    let s = s.trim_end_matches(['.', ',', ':']);
    if s.len() < 3 || !s.chars().any(|c| c.is_ascii_alphabetic()) || s.contains("..") {
        return None;
    }
    if s.contains('{') || s.contains('*') {
        return (s.contains('/') || s.contains('.')).then(|| PTok::Glob(s.to_string()));
    }
    if let Some(d) = s.strip_suffix('/') {
        return (!d.is_empty()).then(|| PTok::Dir(d.to_string()));
    }
    let e = ext_of(s);
    if !e.is_empty() && exts.contains(&e) {
        return Some(PTok::File(s.to_string()));
    }
    if s.contains('/') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | '@' | '+')) {
        return Some(PTok::Dir(s.to_string()));
    }
    None
}

/// rank_eval `kbraces`: `{a,b}` alternatives expanded (cartesian, at most 64
/// patterns); an unclosed `{` is dropped.
fn braces(p: &str) -> Vec<String> {
    let Some(o) = p.find('{') else { return vec![p.to_string()] };
    let Some(c) = p[o..].find('}').map(|x| o + x) else { return vec![p.replace('{', "")] };
    let (head, alts, tail) = (&p[..o], &p[o + 1..c], &p[c + 1..]);
    let mut out = Vec::new();
    for t in braces(tail) {
        for a in alts.split(',') {
            out.push(format!("{head}{}{t}", a.trim()));
            if out.len() >= MAX_BRACES {
                return out;
            }
        }
    }
    out
}

/// rank_eval `kglob`: `*` and `?` stay inside a segment, `**` crosses
/// segments.
fn glob(p: &[u8], s: &[u8]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    if p.starts_with(b"**") {
        let rest = if p.len() > 2 && p[2] == b'/' { &p[3..] } else { &p[2..] };
        return (0..=s.len()).any(|k| (k == 0 || s[k - 1] == b'/') && glob(rest, &s[k..])) || glob(rest, s);
    }
    match p[0] {
        b'*' => (0..=s.len()).take_while(|&k| k == 0 || s[k - 1] != b'/').any(|k| glob(&p[1..], &s[k..])),
        b'?' => !s.is_empty() && s[0] != b'/' && glob(&p[1..], &s[1..]),
        c => !s.is_empty() && s[0] == c && glob(&p[1..], &s[1..]),
    }
}

/// The marker's counts, accumulated across calls.
#[derive(Debug, Default)]
struct Counts {
    routes: BTreeMap<&'static str, usize>,
    resolved: usize,
    ambiguous: usize,
    over_cap: usize,
    unresolved: usize,
}

/// rank_eval's `KPaths`: the candidate files, every historical path the
/// rename fold carries into one, and the suffix / directory indexes over
/// both. Indices are positions in the [`Candidates`] it was built from.
#[non_exhaustive]
#[derive(Debug)]
pub struct PathResolver {
    cand: Vec<String>,
    exact: BTreeMap<String, usize>,
    hist: BTreeMap<String, usize>,
    suffix: BTreeMap<String, Vec<usize>>,
    hsuffix: BTreeMap<String, Vec<usize>>,
    dir_files: BTreeMap<String, Vec<usize>>,
    dir_suffix: BTreeMap<String, Vec<String>>,
    exts: BTreeSet<String>,
    counts: RefCell<Counts>,
}

fn add_dirs(segs: &[&str], f: usize, dir_files: &mut BTreeMap<String, Vec<usize>>) {
    for end in 1..segs.len() {
        dir_files.entry(segs[..end].join("/")).or_default().push(f);
    }
}

impl PathResolver {
    /// rank_eval `KPaths::new` over `cands` and the `(old, new)` renames,
    /// oldest first ([`rename_pairs`]): a later rename of one old path wins.
    pub fn new(cands: &Candidates, renames: &[(String, String)]) -> PathResolver {
        let cand = cands.paths.clone();
        let exact: BTreeMap<String, usize> = cand.iter().enumerate().map(|(i, p)| (p.clone(), i)).collect();
        let mut fwd: BTreeMap<&str, &str> = BTreeMap::new();
        for (a, b) in renames {
            fwd.insert(a.as_str(), b.as_str());
        }
        let mut hist: BTreeMap<String, usize> = BTreeMap::new();
        for &old in fwd.keys() {
            if exact.contains_key(old) {
                continue;
            }
            let mut p = old;
            for _ in 0..MAX_HOPS {
                match fwd.get(p) {
                    Some(&n) => {
                        p = n;
                        if exact.contains_key(p) {
                            break;
                        }
                    }
                    None => break,
                }
            }
            if let Some(&f) = exact.get(p) {
                hist.insert(old.to_string(), f);
            }
        }
        let mut suffix: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut dir_files: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (f, p) in cand.iter().enumerate() {
            let segs: Vec<&str> = p.split('/').collect();
            for k in 0..segs.len() {
                suffix.entry(segs[k..].join("/")).or_default().push(f);
            }
            add_dirs(&segs, f, &mut dir_files);
        }
        let mut hsuffix: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (old, &f) in &hist {
            let segs: Vec<&str> = old.split('/').collect();
            for k in 0..segs.len() {
                hsuffix.entry(segs[k..].join("/")).or_default().push(f);
            }
            add_dirs(&segs, f, &mut dir_files);
        }
        for v in suffix.values_mut().chain(hsuffix.values_mut()).chain(dir_files.values_mut()) {
            v.sort_unstable();
            v.dedup();
        }
        let mut dir_suffix: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for d in dir_files.keys() {
            let segs: Vec<&str> = d.split('/').collect();
            for k in 0..segs.len() {
                dir_suffix.entry(segs[k..].join("/")).or_default().push(d.clone());
            }
        }
        for v in dir_suffix.values_mut() {
            v.sort();
            v.dedup();
        }
        let exts = cand.iter().map(|p| ext_of(p)).filter(|e| !e.is_empty()).collect();
        PathResolver {
            cand,
            exact,
            hist,
            suffix,
            hsuffix,
            dir_files,
            dir_suffix,
            exts,
            counts: RefCell::new(Counts::default()),
        }
    }

    /// The candidate paths, in index order.
    pub fn candidates(&self) -> &[String] {
        &self.cand
    }

    /// The candidates' extensions, the set [`norm`] reads.
    pub fn exts(&self) -> &BTreeSet<String> {
        &self.exts
    }

    /// Old paths the rename fold carries into a candidate.
    pub fn renamed(&self) -> usize {
        self.hist.len()
    }

    /// rank_eval `KPaths::map`: the candidate a path at some revision is
    /// now: itself, or the file git renamed it into.
    pub fn map(&self, p: &str) -> Option<usize> {
        self.exact.get(p).or_else(|| self.hist.get(p)).copied()
    }

    /// rank_eval `KPaths::dir`: the candidates under one directory (a full
    /// directory or a suffix naming exactly one), route `dir` / `dir-suffix`;
    /// None when ambiguous or over [`DIR_CAP`] files.
    pub fn dir(&self, d: &str) -> Option<(Vec<usize>, &'static str)> {
        let full: Vec<&String> = match self.dir_files.get_key_value(d) {
            Some((k, _)) => vec![k],
            None => self.dir_suffix.get(d).map(|v| v.iter().collect()).unwrap_or_default(),
        };
        if full.len() != 1 {
            if full.len() > 1 {
                self.counts.borrow_mut().ambiguous += 1;
            }
            return None;
        }
        let files = self.dir_files.get(full[0])?;
        if files.len() > DIR_CAP {
            self.counts.borrow_mut().over_cap += 1;
            return None;
        }
        Some((files.clone(), if full[0] == d { "dir" } else { "dir-suffix" }))
    }

    /// rank_eval `KPaths::file`: one normalised file mention resolved:
    /// exact, renamed, a unique suffix or bare name, a suffix `ctx` (the
    /// directories of the block's other resolved files) disambiguates, or a
    /// unique suffix of a renamed path.
    pub fn file(&self, t: &str, ctx: &[String]) -> Option<(usize, &'static str)> {
        self.file_counted(t, ctx, true)
    }

    fn file_counted(&self, t: &str, ctx: &[String], count: bool) -> Option<(usize, &'static str)> {
        if let Some(&f) = self.exact.get(t) {
            return Some((f, "exact"));
        }
        if let Some(&f) = self.hist.get(t) {
            return Some((f, "rename"));
        }
        if let Some(v) = self.suffix.get(t) {
            if v.len() == 1 {
                return Some((v[0], if t.contains('/') { "suffix" } else { "name" }));
            }
            // the block's other paths pick the candidate in their directory tree
            let mut best: (usize, Vec<usize>) = (0, Vec::new());
            for &f in v {
                let fd: Vec<&str> = dir_of(&self.cand[f]).split('/').collect();
                let mut s = 0;
                for c in ctx {
                    let cd: Vec<&str> = c.split('/').collect();
                    s = s.max(fd.iter().zip(&cd).take_while(|(a, b)| a == b).count());
                }
                if s > best.0 {
                    best = (s, vec![f]);
                } else if s == best.0 && s > 0 {
                    best.1.push(f);
                }
            }
            if best.1.len() == 1 {
                return Some((best.1[0], "context"));
            }
            if count {
                self.counts.borrow_mut().ambiguous += 1;
            }
            return None;
        }
        if let Some(v) = self.hsuffix.get(t)
            && v.len() == 1
        {
            return Some((v[0], "rename-suffix"));
        }
        None
    }

    /// One glob (rank_eval `block`'s Glob arm): braces expanded, a
    /// brace-free alternative read as a file or directory, a pattern with no
    /// `/` dropped, one whose first segment starts no candidate matched under
    /// `**/`; None over [`DIR_CAP`] hits.
    fn glob(&self, g: &str) -> Option<BTreeSet<usize>> {
        let mut got: BTreeSet<usize> = BTreeSet::new();
        for p in braces(g) {
            if !p.contains('*') {
                match norm(&p, &self.exts) {
                    Some(PTok::File(t)) => got.extend(self.file(&t, &[]).map(|x| x.0)),
                    Some(PTok::Dir(d)) => got.extend(self.dir(&d).map(|x| x.0).unwrap_or_default()),
                    _ => {}
                }
                continue;
            }
            // a glob with no directory part (`client.*`) names no particular place
            if !p.contains('/') {
                self.counts.borrow_mut().unresolved += 1;
                continue;
            }
            let anchored = self.cand.iter().any(|c| c.split('/').next() == p.split('/').next());
            let pat = if anchored { p.clone() } else { format!("**/{p}") };
            for (f, c) in self.cand.iter().enumerate() {
                if glob(pat.as_bytes(), c.as_bytes()) {
                    got.insert(f);
                }
            }
        }
        if got.len() > DIR_CAP {
            self.counts.borrow_mut().over_cap += 1;
            return None;
        }
        Some(got)
    }

    /// rank_eval `KPaths::block`: every path mention among one block's
    /// tokens, resolved in order; a file ambiguous by suffix waits until the
    /// block's unique files give it context, directories expand, globs match.
    /// Output `(candidate, route, raw token)`, sorted by (candidate, route)
    /// and deduped on that pair (the first token in block order kept).
    pub fn block(&self, toks: &[String]) -> Vec<(usize, &'static str, String)> {
        let mut out: Vec<(usize, &'static str, String)> = Vec::new();
        let mut pending: Vec<&String> = Vec::new();
        let mut ctx: Vec<String> = Vec::new();
        for raw in toks {
            let Some(pt) = norm(raw, &self.exts) else { continue };
            match pt {
                PTok::File(t) => match self.file_counted(&t, &[], false) {
                    Some((f, route)) => {
                        ctx.push(dir_of(&self.cand[f]).to_string());
                        out.push((f, route, raw.clone()));
                    }
                    None if self.suffix.get(&t).is_some_and(|v| v.len() > 1) => pending.push(raw),
                    None => self.counts.borrow_mut().unresolved += 1,
                },
                PTok::Dir(d) => {
                    if let Some((fs, route)) = self.dir(&d) {
                        for f in fs {
                            out.push((f, route, raw.clone()));
                        }
                    }
                }
                PTok::Glob(g) => {
                    for f in self.glob(&g).unwrap_or_default() {
                        out.push((f, "glob", raw.clone()));
                    }
                }
            }
        }
        for raw in pending {
            if let Some(PTok::File(t)) = norm(raw, &self.exts)
                && let Some((f, route)) = self.file(&t, &ctx)
            {
                out.push((f, route, raw.clone()));
            }
        }
        out.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        out.dedup_by(|b, a| (a.0, a.1) == (b.0, b.1));
        let mut c = self.counts.borrow_mut();
        c.resolved += out.len();
        for (_, route, _) in &out {
            *c.routes.entry(route).or_default() += 1;
        }
        out
    }

    /// The marker line's text (routes in route-name order).
    pub fn report_line(&self, label: &str, surface: &str) -> String {
        let c = self.counts.borrow();
        let routes: Vec<String> = c.routes.iter().map(|(r, n)| format!("{r}:{n}")).collect();
        format!(
            "[id-paths] repo={label} surface={surface} candidates={} renamed={} resolved={} routes={} ambiguous={} over_cap={} unresolved={}",
            self.cand.len(),
            self.hist.len(),
            c.resolved,
            routes.join(","),
            c.ambiguous,
            c.over_cap,
            c.unresolved
        )
    }

    /// Print the `[id-paths]` marker on stderr: the caller's once per routed
    /// text.
    pub fn report(&self, label: &str, surface: &str) {
        eprintln!("{}", self.report_line(label, surface));
    }
}
