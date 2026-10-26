//! Style-agnostic drift (FH.7a): the names a change removed or renamed on the
//! server side of a paired exchange, walked across the pairing to every
//! client, and each located client site still using an old name.
//!
//! The schema and model judges (the parent module, `models`) need a declared
//! or inferred model on both ends; most repos have neither for most of their
//! exchanges (a Zod object, a hand-written model class, a destructured
//! response). Every style still ends up as NAMES on the wire, and the
//! boundary comes from glia's pairing, so this is the fallback that works on
//! a style glia has never seen. It ranks and annotates: a row is HEURISTIC (or
//! DERIVED, below), never a fact, and it blocks only when the caller asks for
//! `heuristic` ([`NameBreakArgs::min_tier`]).
//!
//! 1. THE DIFF. `git diff -U0 -M <base>` of the working tree (`--relative`,
//!    so paths are the worktree's, as the graph's are), prose and data files
//!    (`.md`, `.txt`, `.po`, `.lock`, ..) left out. Every hunk keeps its
//!    removed and added lines. Lines are tokenised into IDENTIFIERS (code)
//!    and STRING tokens (the identifier-shaped words inside a quoted string,
//!    `-` allowed: a JSON key, a struct tag's name, a URL segment), skipping
//!    comments; a template literal's `${..}` is code again. A token is
//!    NORMALISED (ASCII lower case, `_` and `-` dropped), so `messageId`,
//!    `message_id` and `MESSAGE-ID` are one name. A line holding a struct tag
//!    (`json:"..."`, `yaml:`, `bson:`, ...) keeps only its string tokens: the
//!    tag, not the Go field, is the wire name. Language keywords and tag words
//!    are never names.
//! 2. REMOVED. A normalised name is removed when some class (identifier or
//!    string) holds it on a `-` line of the change and on no `+` line, and no
//!    class gained it (on a `+` line, on no `-` line): a name that only moved
//!    from code into a string, or from one file to another, is not removed.
//!    A RENAME PAIR is a removed and an added name at one position of two
//!    paired lines of one hunk: a `-` line an added line repeats token for
//!    token pairs with it first; the rest pair by the most shared tokens
//!    (common prefix plus suffix), then the most look-alike names; on a
//!    pair, what is left once the common prefix and suffix are stripped is
//!    aligned by position when both sides are as long.
//! 3. THE EXCHANGES. Every pair edge (HTTP_CALLS, GRPC_CALLS, RPC_CALLS,
//!    GRAPHQL_CALLS, QUEUE_FLOWS, WS_CONNECTS) from a client node (ENDPOINT,
//!    GRPC_CLIENT, RPC_CALL, GRAPHQL_OPERATION, QUEUE_PRODUCER, WS_CLIENT) to
//!    a server node (ROUTE, GRPC_SERVICE, RPC_PROCEDURE, GRAPHQL_RESOLVER,
//!    QUEUE_CONSUMER, WS_HANDLER) of the worktree's repo. The server side is
//!    the server node, its HANDLED_BY handlers (no handler: the MODULE of the
//!    server node's file, an inline closure's), and what they reach within
//!    [`SERVER_DEPTH`] hops of CALLS / USES / INJECTS / RETURNS_TYPE /
//!    IMPLEMENTS / INHERITS_FROM / IMPORTS / HANDLED_BY inside that repo. An
//!    exchange CHANGED when a hunk of the diff falls in the span of one of
//!    those nodes (a MODULE spans its file); its names are the removed names
//!    on those hunks' `-` lines.
//! 4. THE CLIENTS. For each changed exchange's client nodes: the client's file
//!    and the files of the callables that CALLS / USES it (the response
//!    handling code), then every file their MODULEs import or are imported by
//!    within [`CLIENT_DEPTH`] IMPORTS hops, in any repo of the build (a
//!    `--with` client too), less the exchange's server-side files.
//! 5. A USE. In a client file, a token whose normalised form is one of the
//!    exchange's names, outside a comment. A GENERIC name (normalised shorter
//!    than 4, or in [`GENERIC`]) counts only at a property access (`x.type`)
//!    or when the change renamed it.
//! 6. ROWS. One per (name, client file, line), sorted by (repo label, file,
//!    line, name); `position` says how the line uses it (`property`, `key`,
//!    `string` or `identifier`). The tier is `derived` when the pair edge is
//!    a fact and the name is a rename pair, else `heuristic`; a row of several
//!    exchanges keeps its best tier, then the least (client, server) qname. A
//!    row blocks only under [`NameBreakArgs::min_tier`] `heuristic`.
//!    [`NameBreaks::fixed`] counts the (name, client file) pairs the change
//!    itself fixed: a client file of the change that used the name at the
//!    base and no longer does, so the fixed state of a drift reads `rows=0
//!    fixed=<n>`.
//!
//! The diff side reads only the change's hunks, and the client side only the
//! files the boundary reaches, so a big change stays bounded. fired_on, once
//! per [`name_breaks`] call: `[name-breaks] base=<rev> exchanges=<e>
//! names=<n> renames=<r> clients=<c> rows=<k> fixed=<f>` (c = client files
//! scanned).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use glia_activation::profile::Tier;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};

use crate::answers::Locator;
use crate::build::GenerateResult;
use crate::git_rev;
use crate::why::tier_of;

/// The accepted [`NameBreakArgs::min_tier`] values, default first. A name row
/// blocks only under `heuristic`.
pub const NAME_MIN_TIERS: &[&str] = &[
    Tier::Fact.as_str(),
    Tier::Derived.as_str(),
    Tier::Heuristic.as_str(),
];

/// Hops from a server node and its handlers the server side reaches.
pub const SERVER_DEPTH: usize = 3;

/// IMPORTS hops from a client callable's file the client side reaches.
pub const CLIENT_DEPTH: usize = 2;

/// Nodes one exchange's server-side walk visits at most.
const SERVER_VISIT_CAP: usize = 4000;

/// The largest client file read; a larger one is skipped.
const FILE_CAP: usize = 4 * 1024 * 1024;

/// Normalised names too common on any wire to count at a plain identifier:
/// they need a property access or a rename pair (step 5). The cohort's
/// `names.py` noise list, normalised, plus the short wire words.
pub const GENERIC: &[&str] = &[
    "this",
    "that",
    "true",
    "false",
    "null",
    "none",
    "self",
    "string",
    "number",
    "boolean",
    "return",
    "const",
    "static",
    "class",
    "public",
    "private",
    "protected",
    "async",
    "await",
    "function",
    "import",
    "export",
    "default",
    "from",
    "type",
    "interface",
    "extends",
    "implements",
    "readonly",
    "optional",
    "required",
    "description",
    "example",
    "deprecated",
    "value",
    "values",
    "field",
    "fields",
    "name",
    "names",
    "data",
    "error",
    "errors",
    "message",
    "response",
    "request",
    "result",
    "results",
    "param",
    "params",
    "query",
    "body",
    "status",
    "code",
    "item",
    "items",
    "list",
    "option",
    "options",
    "config",
    "index",
    "count",
    "total",
    "int64",
    "int32",
    "float",
    "uint8",
    "json",
    "yaml",
    "omitempty",
    "override",
    "void",
    "final",
    "delete",
    "update",
    "create",
    "post",
    "patch",
    "date",
    "time",
    "text",
    "page",
    "size",
    "path",
    "file",
    "user",
    "users",
    "info",
    "meta",
    "state",
    "kind",
    "label",
    "title",
];

/// Language keywords and builtin type names: never an identifier-class name.
const KEYWORDS: &[&str] = &[
    // Go
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
    "nil",
    "true",
    "false",
    "string",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "float32",
    "float64",
    "bool",
    "byte",
    "rune",
    "error",
    "any",
    "make",
    "new",
    "len",
    "append",
    // TypeScript / JavaScript
    "abstract",
    "as",
    "async",
    "await",
    "boolean",
    "catch",
    "class",
    "constructor",
    "debugger",
    "declare",
    "delete",
    "do",
    "enum",
    "export",
    "extends",
    "finally",
    "from",
    "function",
    "get",
    "implements",
    "in",
    "infer",
    "instanceof",
    "is",
    "keyof",
    "let",
    "module",
    "namespace",
    "never",
    "null",
    "number",
    "object",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "set",
    "static",
    "super",
    "symbol",
    "this",
    "throw",
    "try",
    "typeof",
    "undefined",
    "unique",
    "unknown",
    "void",
    "while",
    "with",
    "yield",
    "console",
    // Python / Ruby / Dart / JVM / C#
    "and",
    "assert",
    "def",
    "del",
    "elif",
    "except",
    "global",
    "lambda",
    "nonlocal",
    "not",
    "or",
    "pass",
    "raise",
    "self",
    "none",
    "final",
    "late",
    "required",
    "dynamic",
    "end",
    "begin",
    "nil",
    "elsif",
    "unless",
    "until",
    "then",
    "val",
    "fun",
    "override",
    "open",
    "data",
    "object",
    "companion",
    "internal",
    "sealed",
    "lateinit",
    "using",
    "void",
    "long",
    "short",
    "double",
    "float",
    "char",
    "static",
    "throws",
    "synchronized",
    "volatile",
    "transient",
    "native",
    "strictfp",
    "async",
    "virtual",
    "readonly",
    "out",
    "ref",
];

/// Struct-tag keys and options: never a name, in any class.
const TAG_WORDS: &[&str] = &[
    "json",
    "yaml",
    "xml",
    "bson",
    "form",
    "query",
    "uri",
    "header",
    "msgpack",
    "mapstructure",
    "db",
    "gorm",
    "binding",
    "validate",
    "omitempty",
    "inline",
    "string",
    "required",
    "toml",
    "protobuf",
    "default",
];

/// Struct-tag keys: a line with `<key>:"..."` keeps its string tokens only.
const TAG_KEYS: &[&str] = &[
    "json",
    "yaml",
    "xml",
    "bson",
    "form",
    "msgpack",
    "mapstructure",
    "db",
    "toml",
];

/// The client -> server pair categories an exchange is read from.
const PAIR_CATEGORIES: [EdgeCategoryId; 6] = [
    edge_category::HTTP_CALLS,
    edge_category::GRPC_CALLS,
    edge_category::RPC_CALLS,
    edge_category::GRAPHQL_CALLS,
    edge_category::QUEUE_FLOWS,
    edge_category::WS_CONNECTS,
];

/// The server end of an exchange.
const SERVER_KINDS: [NodeKindId; 6] = [
    node_kind::ROUTE,
    node_kind::GRPC_SERVICE,
    node_kind::RPC_PROCEDURE,
    node_kind::GRAPHQL_RESOLVER,
    node_kind::QUEUE_CONSUMER,
    node_kind::WS_HANDLER,
];

/// The client end of an exchange.
const CLIENT_KINDS: [NodeKindId; 6] = [
    node_kind::ENDPOINT,
    node_kind::GRPC_CLIENT,
    node_kind::RPC_CALL,
    node_kind::GRAPHQL_OPERATION,
    node_kind::QUEUE_PRODUCER,
    node_kind::WS_CLIENT,
];

/// What the server-side walk follows.
const SERVER_WALK: [EdgeCategoryId; 8] = [
    edge_category::HANDLED_BY,
    edge_category::CALLS,
    edge_category::USES,
    edge_category::INJECTS,
    edge_category::RETURNS_TYPE,
    edge_category::IMPLEMENTS,
    edge_category::INHERITS_FROM,
    edge_category::IMPORTS,
];

/// How [`name_breaks`] gates. Built outside the crate from `Default` plus
/// field assignment.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct NameBreakArgs {
    /// The weakest tier at which a row blocks ([`NAME_MIN_TIERS`]): `fact`
    /// (the default) and `derived` block none, `heuristic` blocks every row.
    /// [`name_breaks`] refuses any other value.
    pub min_tier: &'static str,
}

impl Default for NameBreakArgs {
    fn default() -> Self {
        NameBreakArgs {
            min_tier: NAME_MIN_TIERS[0],
        }
    }
}

/// One name the change removed on the server side of a changed exchange.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ChangedName {
    /// The name as the removed line spells it (a string class spelling first:
    /// the wire name).
    pub name: String,
    /// Lower case, `_` and `-` dropped: what a client use is matched on.
    pub normalised: String,
    /// The name it was renamed to, when a hunk pairs one.
    pub renamed_to: Option<String>,
    /// Where the change removed it: the worktree-relative file and the
    /// 1-based line of the hunk (the paired added line for a rename).
    pub file: String,
    pub line: i64,
    /// The changed exchanges whose server side holds it.
    pub exchanges: usize,
}

/// One client site still using a name the change removed.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct NameBreak {
    /// The old name ([`ChangedName::name`]).
    pub name: String,
    pub normalised: String,
    /// The new name, when the change renamed it.
    pub renamed_to: Option<String>,
    /// The client file's repo label.
    pub repo: String,
    /// Relative to that repo's root.
    pub file: String,
    /// 1-based.
    pub line: i64,
    /// `property` (`x.name`), `key` (`name:`), `string` (inside a quoted
    /// string) or `identifier`.
    pub position: &'static str,
    /// The client node the boundary was crossed from.
    pub client: String,
    /// The changed server node.
    pub server: String,
    /// `derived` (a fact pair edge and a rename pair) or `heuristic`.
    pub tier: &'static str,
    /// Under [`NameBreakArgs::min_tier`] `heuristic` only.
    pub blocking: bool,
}

/// The name breaks of the working tree against `base` (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct NameBreaks {
    /// The rev as given.
    pub base: String,
    /// The gate the rows were judged under.
    pub min_tier: &'static str,
    /// Changed exchanges: paired server nodes a hunk falls in.
    pub exchanges: usize,
    /// The removed names, sorted by (file, line, name).
    pub names: Vec<ChangedName>,
    pub rows: Vec<NameBreak>,
    /// Client files scanned.
    pub clients: usize,
    /// (name, client file) pairs the change itself stopped using.
    pub fixed: usize,
    /// Rows that block.
    pub blocking: usize,
}

/// The name breaks of the working tree at `worktree` against git rev `base`
/// (module docs). `after` is the worktree's build: the worktree's repo plus
/// any client repos built beside it (`--with`), whose files are read under
/// [`GenerateResult::repo_roots`]. `Err` on an unknown min tier (checked
/// first), an unknown rev, or a failed `git diff`.
pub fn name_breaks(
    base: &str,
    worktree: &str,
    after: &GenerateResult,
    args: &NameBreakArgs,
) -> Result<NameBreaks, String> {
    if !NAME_MIN_TIERS.contains(&args.min_tier) {
        return Err(format!(
            "unknown name min tier `{}`: expected one of {}",
            args.min_tier,
            NAME_MIN_TIERS.join(", ")
        ));
    }
    let repo = Path::new(worktree);
    let rev = git_rev::resolve_rev(repo, base)?;
    let diff_text = git_text(
        repo,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "-U0",
            "-M",
            "--relative",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &rev.sha,
            "--",
        ],
    )?
    .ok_or_else(|| format!("git diff {} failed in {worktree}", rev.given))?;
    let mut diff = parse_diff(&diff_text);
    diff.retain(|f| !prose(&f.path));
    let answer = answer(&rev.given, &rev.sha, repo, &diff, after, args);
    eprintln!(
        "[name-breaks] base={} exchanges={} names={} renames={} clients={} rows={} fixed={}",
        answer.base,
        answer.exchanges,
        answer.names.len(),
        answer
            .names
            .iter()
            .filter(|n| n.renamed_to.is_some())
            .count(),
        answer.clients,
        answer.rows.len(),
        answer.fixed,
    );
    Ok(answer)
}

// ---- the answer --------------------------------------------------------------

/// One changed exchange: its server node, its clients with their best pair
/// edge tier, its names and its server-side files.
struct Exchange {
    server: NodeId,
    clients: BTreeMap<u64, Tier>,
    names: BTreeSet<String>,
    server_files: BTreeSet<String>,
}

fn answer(
    given: &str,
    sha: &str,
    repo: &Path,
    diff: &[FileDiff],
    after: &GenerateResult,
    args: &NameBreakArgs,
) -> NameBreaks {
    let mut out = NameBreaks {
        base: given.to_string(),
        min_tier: args.min_tier,
        exchanges: 0,
        names: Vec::new(),
        rows: Vec::new(),
        clients: 0,
        fixed: 0,
        blocking: 0,
    };
    if diff.iter().all(|f| f.hunks.is_empty()) {
        return out;
    }
    let Some(provider) = provider_repo(repo, after) else {
        return out;
    };
    let index = Index::new(after);
    let classes = ClassSets::of(diff);
    let by_file: BTreeMap<&str, &FileDiff> = diff.iter().map(|f| (f.path.as_str(), f)).collect();

    // Step 3: the exchanges, keyed by server node, clients with their best tier.
    let mut pairs: BTreeMap<u64, BTreeMap<u64, Tier>> = BTreeMap::new();
    for e in after.merged.all_edges() {
        if !PAIR_CATEGORIES.contains(&e.category)
            || !index.kind_in(e.to, &SERVER_KINDS)
            || !index.kind_in(e.from, &CLIENT_KINDS)
            || index.repo.get(&e.to) != Some(&provider)
        {
            continue;
        }
        let tier = tier_of(Evidence::of(e).as_ref(), e).0;
        let slot = pairs
            .entry(e.to.0)
            .or_default()
            .entry(e.from.0)
            .or_insert(tier);
        *slot = (*slot).min(tier);
    }

    let mut changed: BTreeMap<String, ChangedName> = BTreeMap::new();
    let mut exchanges: Vec<Exchange> = Vec::new();
    for (&server, clients) in &pairs {
        let server = NodeId(server);
        let (reached, server_files) = index.server_side(server, provider);
        let mut names = BTreeSet::new();
        for (file, spans) in &reached {
            let Some(fd) = by_file.get(file.as_str()) else {
                continue;
            };
            for h in &fd.hunks {
                if !spans.iter().any(|&(s, e)| h.touches(s, e)) {
                    continue;
                }
                for found in h.names(&classes) {
                    names.insert(found.normalised.clone());
                    let entry =
                        changed
                            .entry(found.normalised.clone())
                            .or_insert_with(|| ChangedName {
                                name: found.raw.clone(),
                                normalised: found.normalised.clone(),
                                renamed_to: None,
                                file: file.clone(),
                                line: found.line,
                                exchanges: 0,
                            });
                    if entry.renamed_to.is_none() && found.renamed_to.is_some() {
                        entry.renamed_to = found.renamed_to.clone();
                        entry.file = file.clone();
                        entry.line = found.line;
                    }
                }
            }
        }
        if names.is_empty() {
            continue;
        }
        for n in &names {
            if let Some(c) = changed.get_mut(n) {
                c.exchanges += 1;
            }
        }
        exchanges.push(Exchange {
            server,
            clients: clients.clone(),
            names,
            server_files,
        });
    }
    out.exchanges = exchanges.len();

    // Steps 4-6: the client files, the uses, the rows.
    let diff_files: HashSet<&str> = diff.iter().map(|f| f.path.as_str()).collect();
    let mut texts = TextCache::new(after);
    let mut rows: BTreeMap<(String, String, i64, String), NameBreak> = BTreeMap::new();
    let mut scanned: BTreeSet<(u64, String)> = BTreeSet::new();
    let mut fixed: BTreeSet<(String, String)> = BTreeSet::new();
    let server_qname = |id: NodeId| index.loc.locate(id).qname;
    for x in &exchanges {
        let server = server_qname(x.server);
        for (file_key, (client, tier)) in index.client_files(&x.clients) {
            let (repo_id, file) = &file_key;
            if *repo_id == provider && x.server_files.contains(file) {
                continue;
            }
            scanned.insert(file_key.clone());
            let label = after
                .repo_labels
                .get(repo_id)
                .cloned()
                .unwrap_or_else(|| format!("repo#{repo_id}"));
            let client_qname = index.loc.locate(NodeId(client)).qname;
            let uses = texts.uses(*repo_id, file, &x.names, &changed);
            let used: BTreeSet<&str> = uses.iter().map(|u| u.normalised.as_str()).collect();
            if *repo_id == provider && diff_files.contains(file.as_str()) {
                let base_text = git_text(repo, &["show", &format!("{sha}:./{file}")])
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                for u in scan_uses(&base_text, file, &x.names, &changed) {
                    if !used.contains(u.normalised.as_str()) {
                        fixed.insert((u.normalised, file.clone()));
                    }
                }
            }
            for u in uses {
                let Some(name) = changed.get(&u.normalised) else {
                    continue;
                };
                let renamed = name.renamed_to.is_some();
                let row_tier = if tier == Tier::Fact && renamed {
                    Tier::Derived
                } else {
                    Tier::Heuristic
                };
                let key = (label.clone(), file.clone(), u.line, u.normalised.clone());
                let candidate = NameBreak {
                    name: name.name.clone(),
                    normalised: u.normalised.clone(),
                    renamed_to: name.renamed_to.clone(),
                    repo: label.clone(),
                    file: file.clone(),
                    line: u.line,
                    position: u.position,
                    client: client_qname.clone(),
                    server: server.clone(),
                    tier: row_tier.as_str(),
                    blocking: args.min_tier == Tier::Heuristic.as_str(),
                };
                match rows.get(&key) {
                    Some(have) if !better(&candidate, have) => {}
                    _ => {
                        rows.insert(key, candidate);
                    }
                }
            }
        }
    }
    out.rows = rows.into_values().collect();
    out.blocking = out.rows.iter().filter(|r| r.blocking).count();
    out.clients = scanned.len();
    out.fixed = fixed.len();
    let mut names: Vec<ChangedName> = changed.into_values().filter(|c| c.exchanges > 0).collect();
    names.sort_by(|a, b| (&a.file, a.line, &a.normalised).cmp(&(&b.file, b.line, &b.normalised)));
    out.names = names;
    out
}

/// A row of several exchanges keeps its best tier, then the least (client,
/// server) qname, then the strongest position.
fn better(a: &NameBreak, b: &NameBreak) -> bool {
    let rank = |t: &str| Tier::parse(t).unwrap_or(Tier::Heuristic);
    (rank(a.tier), &a.client, &a.server) < (rank(b.tier), &b.client, &b.server)
}

/// The RepoId whose root is `worktree` (canonical paths compared); the only
/// repo of a one-repo build otherwise.
fn provider_repo(worktree: &Path, after: &GenerateResult) -> Option<u64> {
    let want = std::fs::canonicalize(worktree).ok();
    let hit = after.repo_roots.iter().find(|(_, root)| {
        let got = std::fs::canonicalize(root).ok();
        got.is_some() && got == want
    });
    if let Some((&id, _)) = hit {
        return Some(id);
    }
    let mut repos: BTreeSet<u64> = after.merged.graphs.iter().map(|g| g.repo.0).collect();
    (repos.len() == 1).then(|| repos.pop_first()).flatten()
}

// ---- the graph index ---------------------------------------------------------

/// A (file, 0-based first row, 0-based last row) span; `None` rows for a
/// MODULE: the whole file.
type Span = (i64, i64);

struct Index<'a> {
    loc: Locator<'a>,
    kind: HashMap<NodeId, NodeKindId>,
    repo: HashMap<NodeId, u64>,
    /// File and span, first instance in graph order.
    place: HashMap<NodeId, (String, Span)>,
    /// `(repo, file)` -> the file's MODULE, first in graph order.
    module_at: HashMap<(u64, String), NodeId>,
    out: HashMap<NodeId, Vec<(NodeId, EdgeCategoryId)>>,
    inn: HashMap<NodeId, Vec<(NodeId, EdgeCategoryId)>>,
}

impl<'a> Index<'a> {
    fn new(after: &'a GenerateResult) -> Self {
        let merged = &after.merged;
        let loc = Locator::new(merged);
        let mut kind = HashMap::new();
        let mut repo = HashMap::new();
        let mut place = HashMap::new();
        let mut module_at = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if repo.contains_key(&n.id) {
                    continue;
                }
                repo.insert(n.id, g.repo.0);
                let k = g.nav.kind_by_id.get(&n.id).copied();
                if let Some(k) = k {
                    kind.insert(n.id, k);
                }
                let span = position_span(&n.cells);
                let file = span
                    .as_ref()
                    .map(|(f, _)| f.clone())
                    .or_else(|| loc.locate(n.id).file);
                let Some(file) = file else {
                    continue;
                };
                let rows = if k == Some(node_kind::MODULE) {
                    (0, i64::MAX)
                } else {
                    match span {
                        Some((_, rows)) => rows,
                        None => {
                            let row = loc.locate(n.id).line.map_or(0, |l| l - 1);
                            (row, row)
                        }
                    }
                };
                if k == Some(node_kind::MODULE) {
                    module_at.entry((g.repo.0, file.clone())).or_insert(n.id);
                }
                place.insert(n.id, (file, rows));
            }
        }
        let mut out: HashMap<NodeId, Vec<(NodeId, EdgeCategoryId)>> = HashMap::new();
        let mut inn: HashMap<NodeId, Vec<(NodeId, EdgeCategoryId)>> = HashMap::new();
        for e in merged.all_edges() {
            out.entry(e.from).or_default().push((e.to, e.category));
            inn.entry(e.to).or_default().push((e.from, e.category));
        }
        Index {
            loc,
            kind,
            repo,
            place,
            module_at,
            out,
            inn,
        }
    }

    fn kind_in(&self, id: NodeId, kinds: &[NodeKindId]) -> bool {
        self.kind.get(&id).is_some_and(|k| kinds.contains(k))
    }

    /// The MODULE of `id`'s file, else `id`.
    fn module_of(&self, id: NodeId) -> NodeId {
        let (Some(repo), Some((file, _))) = (self.repo.get(&id), self.place.get(&id)) else {
            return id;
        };
        self.module_at
            .get(&(*repo, file.clone()))
            .copied()
            .unwrap_or(id)
    }

    /// Step 3: per file of `provider`, the spans the server side of `server`
    /// reaches, and the files of that side.
    fn server_side(
        &self,
        server: NodeId,
        provider: u64,
    ) -> (BTreeMap<String, Vec<Span>>, BTreeSet<String>) {
        let mut start = vec![server];
        let handlers: Vec<NodeId> = self
            .out
            .get(&server)
            .into_iter()
            .flatten()
            .filter(|(_, c)| *c == edge_category::HANDLED_BY)
            .map(|(to, _)| *to)
            .collect();
        if handlers.is_empty() {
            start.push(self.module_of(server));
        }
        start.extend(handlers);
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
        for s in start {
            if seen.insert(s) {
                queue.push_back((s, 0));
            }
        }
        let mut reached: BTreeMap<String, Vec<Span>> = BTreeMap::new();
        let mut files = BTreeSet::new();
        while let Some((id, depth)) = queue.pop_front() {
            if let Some((file, span)) = self.place.get(&id)
                && self.repo.get(&id) == Some(&provider)
            {
                reached.entry(file.clone()).or_default().push(*span);
                files.insert(file.clone());
            }
            if depth >= SERVER_DEPTH || seen.len() >= SERVER_VISIT_CAP {
                continue;
            }
            for (to, cat) in self.out.get(&id).into_iter().flatten() {
                if !SERVER_WALK.contains(cat)
                    || self.repo.get(to) != Some(&provider)
                    || seen.len() >= SERVER_VISIT_CAP
                {
                    continue;
                }
                if seen.insert(*to) {
                    queue.push_back((*to, depth + 1));
                }
            }
        }
        (reached, files)
    }

    /// Step 4: every client file of `clients`, keyed `(repo, file)`, with the
    /// client it is reached from (best tier, then least node id: the qname
    /// tie-break is the row's) and that client's pair tier.
    fn client_files(&self, clients: &BTreeMap<u64, Tier>) -> BTreeMap<(u64, String), (u64, Tier)> {
        let mut out: BTreeMap<(u64, String), (u64, Tier)> = BTreeMap::new();
        for (&client, &tier) in clients {
            let c = NodeId(client);
            let mut roots: Vec<NodeId> = vec![c];
            roots.extend(
                self.inn
                    .get(&c)
                    .into_iter()
                    .flatten()
                    .filter(|(_, cat)| *cat == edge_category::CALLS || *cat == edge_category::USES)
                    .map(|(from, _)| *from),
            );
            let mut seen: HashSet<NodeId> = HashSet::new();
            let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
            for r in roots {
                let m = self.module_of(r);
                if seen.insert(m) {
                    queue.push_back((m, 0));
                }
            }
            while let Some((m, depth)) = queue.pop_front() {
                if let (Some(repo), Some((file, _))) = (self.repo.get(&m), self.place.get(&m)) {
                    let slot = out.entry((*repo, file.clone())).or_insert((client, tier));
                    if tier < slot.1 {
                        *slot = (client, tier);
                    }
                }
                if depth >= CLIENT_DEPTH {
                    continue;
                }
                let edges = self
                    .out
                    .get(&m)
                    .into_iter()
                    .flatten()
                    .chain(self.inn.get(&m).into_iter().flatten());
                for (other, cat) in edges {
                    if *cat != edge_category::IMPORTS {
                        continue;
                    }
                    let next = self.module_of(*other);
                    if seen.insert(next) {
                        queue.push_back((next, depth + 1));
                    }
                }
            }
        }
        out
    }
}

/// `(file, (0-based first row, 0-based last row))` from a node's first
/// parseable POSITION cell.
fn position_span(cells: &[glia_core::Cell]) -> Option<(String, Span)> {
    for c in cells {
        if c.kind != cell_type::POSITION {
            continue;
        }
        if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
        {
            let file = v.get("file").and_then(|f| f.as_str())?.to_string();
            let start = v
                .get("start_line")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let end = v
                .get("end_line")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(start)
                .max(start);
            return Some((file, (start, end)));
        }
    }
    None
}

// ---- client files ------------------------------------------------------------

/// One use of a name in a file.
struct Use {
    normalised: String,
    line: i64,
    position: &'static str,
}

/// Client file texts, read once per `(repo, file)`.
struct TextCache<'a> {
    after: &'a GenerateResult,
    texts: HashMap<(u64, String), String>,
}

impl<'a> TextCache<'a> {
    fn new(after: &'a GenerateResult) -> Self {
        TextCache {
            after,
            texts: HashMap::new(),
        }
    }

    fn uses(
        &mut self,
        repo: u64,
        file: &str,
        names: &BTreeSet<String>,
        changed: &BTreeMap<String, ChangedName>,
    ) -> Vec<Use> {
        let key = (repo, file.to_string());
        if !self.texts.contains_key(&key) {
            let text = self
                .after
                .repo_roots
                .get(&repo)
                .map(|root| PathBuf::from(root).join(file))
                .and_then(|p| std::fs::read(p).ok())
                .filter(|b| b.len() <= FILE_CAP)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            self.texts.insert(key.clone(), text);
        }
        let text = self.texts.get(&key).map(String::as_str).unwrap_or("");
        scan_uses(text, file, names, changed)
    }
}

/// Step 5: the uses of `names` in `text`, one per (name, line), the
/// strongest position of the line kept.
fn scan_uses(
    text: &str,
    file: &str,
    names: &BTreeSet<String>,
    changed: &BTreeMap<String, ChangedName>,
) -> Vec<Use> {
    let hash = hash_comments(file);
    let mut out: BTreeMap<(i64, String), &'static str> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        if comment_line(line, hash) {
            continue;
        }
        for t in line_tokens(line, hash) {
            if !names.contains(&t.norm) {
                continue;
            }
            let renamed = changed.get(&t.norm).is_some_and(|c| c.renamed_to.is_some());
            if is_generic(&t.norm) && !renamed && !t.property {
                continue;
            }
            let at = (i as i64 + 1, t.norm.clone());
            let pos = t.position();
            let slot = out.entry(at).or_insert(pos);
            if position_rank(pos) < position_rank(slot) {
                *slot = pos;
            }
        }
    }
    out.into_iter()
        .map(|((line, normalised), position)| Use {
            normalised,
            line,
            position,
        })
        .collect()
}

fn position_rank(p: &str) -> u8 {
    match p {
        "property" => 0,
        "key" => 1,
        "string" => 2,
        _ => 3,
    }
}

fn is_generic(norm: &str) -> bool {
    norm.len() < 4 || GENERIC.contains(&norm)
}

// ---- the diff ----------------------------------------------------------------

/// One `-U0` hunk: its new-side start (1-based) and length, its removed
/// lines, its added lines with their 1-based new-side line.
struct Hunk {
    new_start: i64,
    new_len: i64,
    minus: Vec<String>,
    plus: Vec<(i64, String)>,
    /// The file's extension reads `#` as a comment.
    hash: bool,
}

struct FileDiff {
    path: String,
    hunks: Vec<Hunk>,
}

/// `git diff -U0` text into files and hunks. A deleted file is named by its
/// old path; binary files have no hunk.
fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut out: Vec<FileDiff> = Vec::new();
    let mut old_path: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            old_path = None;
            continue;
        }
        if let Some(p) = line.strip_prefix("--- ") {
            old_path = diff_path(p, "a/");
            continue;
        }
        if let Some(p) = line.strip_prefix("+++ ") {
            let path = diff_path(p, "b/").or_else(|| old_path.clone());
            if let Some(path) = path {
                out.push(FileDiff {
                    path,
                    hunks: Vec::new(),
                });
            }
            continue;
        }
        let Some(file) = out.last_mut() else {
            continue;
        };
        if let Some(header) = line.strip_prefix("@@ ") {
            let (new_start, new_len) = hunk_new_range(header);
            file.hunks.push(Hunk {
                new_start,
                new_len,
                minus: Vec::new(),
                plus: Vec::new(),
                hash: hash_comments(&file.path),
            });
            continue;
        }
        let Some(h) = file.hunks.last_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix('-') {
            h.minus.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('+') {
            let at = h.new_start + h.plus.len() as i64;
            h.plus.push((at, rest.to_string()));
        }
    }
    out
}

/// A `--- a/x` / `+++ b/x` path; `None` for `/dev/null`.
fn diff_path(p: &str, prefix: &str) -> Option<String> {
    let p = p.trim_end_matches('\t');
    if p == "/dev/null" {
        return None;
    }
    let p = p
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
        .unwrap_or(p);
    Some(p.strip_prefix(prefix).unwrap_or(p).to_string())
}

/// `(start, len)` of a hunk header's `+c[,d]`.
fn hunk_new_range(header: &str) -> (i64, i64) {
    let Some(plus) = header.split_whitespace().find(|w| w.starts_with('+')) else {
        return (1, 0);
    };
    let mut it = plus[1..].splitn(2, ',');
    let start = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let len = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    (start, len)
}

/// Which classes hold each normalised name on `-` and on `+` lines, over the
/// whole change.
struct ClassSets {
    minus: HashMap<Class, HashSet<String>>,
    plus: HashMap<Class, HashSet<String>>,
}

impl ClassSets {
    fn of(diff: &[FileDiff]) -> Self {
        let mut sets = ClassSets {
            minus: HashMap::new(),
            plus: HashMap::new(),
        };
        for h in diff.iter().flat_map(|f| f.hunks.iter()) {
            for l in &h.minus {
                for t in name_tokens(l, h.hash) {
                    sets.minus.entry(t.class).or_default().insert(t.norm);
                }
            }
            for (_, l) in &h.plus {
                for t in name_tokens(l, h.hash) {
                    sets.plus.entry(t.class).or_default().insert(t.norm);
                }
            }
        }
        sets
    }

    fn has(map: &HashMap<Class, HashSet<String>>, class: Class, norm: &str) -> bool {
        map.get(&class).is_some_and(|s| s.contains(norm))
    }

    /// Lost from `class` (on a `-` line, on no `+` line).
    fn lost(&self, class: Class, norm: &str) -> bool {
        Self::has(&self.minus, class, norm) && !Self::has(&self.plus, class, norm)
    }

    /// Gained in some class (on a `+` line, on no `-` line).
    fn gained(&self, norm: &str) -> bool {
        Class::BOTH
            .iter()
            .any(|&c| Self::has(&self.plus, c, norm) && !Self::has(&self.minus, c, norm))
    }

    /// Step 2's removed.
    fn removed(&self, norm: &str) -> bool {
        Class::BOTH.iter().any(|&c| self.lost(c, norm)) && !self.gained(norm)
    }

    /// The other side of a rename pair: gained and never removed.
    fn added(&self, norm: &str) -> bool {
        self.gained(norm) && !Class::BOTH.iter().any(|&c| self.lost(c, norm))
    }
}

/// A removed name found on a hunk.
struct Found {
    raw: String,
    normalised: String,
    renamed_to: Option<String>,
    /// 1-based new-side line.
    line: i64,
}

impl Hunk {
    /// Whether the hunk's new-side lines meet the 0-based span `[s, e]`. A
    /// pure deletion sits between new-side rows `new_start - 1` and
    /// `new_start`.
    fn touches(&self, s: i64, e: i64) -> bool {
        let first = (self.new_start - 1).max(0);
        let last = if self.new_len > 0 {
            first + self.new_len - 1
        } else {
            self.new_start
        };
        first <= e && last >= s
    }

    /// The removed names on this hunk's `-` lines, with their rename pairs.
    fn names(&self, sets: &ClassSets) -> Vec<Found> {
        // Old name -> (new spelling, its line, its class): a string class
        // pair (the wire name) wins over an identifier one, else the first.
        let mut renames: BTreeMap<String, (String, i64, Class)> = BTreeMap::new();
        for (mi, pi) in self.line_pairs() {
            let (at, new) = &self.plus[pi];
            let old = &self.minus[mi];
            let (_, pairs) = aligned(&line_tokens(old, self.hash), &line_tokens(new, self.hash));
            for (o, n) in pairs {
                if !(is_name(&o) && is_name(&n) && sets.removed(&o.norm) && sets.added(&n.norm)) {
                    continue;
                }
                let slot = renames
                    .entry(o.norm.clone())
                    .or_insert((n.raw.clone(), *at, n.class));
                if slot.2 == Class::Ident && n.class == Class::Str {
                    *slot = (n.raw.clone(), *at, n.class);
                }
            }
        }
        let mut out: Vec<Found> = Vec::new();
        // Normalised name -> (its index in `out`, the class its spelling came from).
        let mut seen: HashMap<String, (usize, Class)> = HashMap::new();
        for old in &self.minus {
            for t in name_tokens(old, self.hash) {
                if !is_name(&t) || !sets.lost(t.class, &t.norm) || !sets.removed(&t.norm) {
                    continue;
                }
                if let Some(&(at, class)) = seen.get(&t.norm) {
                    if class == Class::Ident && t.class == Class::Str {
                        out[at].raw = t.raw.clone();
                        seen.insert(t.norm.clone(), (at, Class::Str));
                    }
                    continue;
                }
                seen.insert(t.norm.clone(), (out.len(), t.class));
                let renamed = renames.get(&t.norm);
                out.push(Found {
                    raw: t.raw.clone(),
                    normalised: t.norm.clone(),
                    renamed_to: renamed.map(|(n, _, _)| n.clone()),
                    line: renamed.map_or(self.new_start.max(1), |(_, at, _)| *at),
                });
            }
        }
        out
    }
}

impl Hunk {
    /// The paired (removed line, added line) indices. A removed line an added
    /// line repeats token for token is consumed first (no rename on it); then
    /// every pair whose [`aligned`] tokens share at least one common token,
    /// strongest share first, then most [`akin`] aligned names (`id` /
    /// `messageId`), ties by removed line, then added line, each line used
    /// once.
    fn line_pairs(&self) -> Vec<(usize, usize)> {
        if self.minus.is_empty() || self.plus.is_empty() {
            return Vec::new();
        }
        let old: Vec<Vec<Tok>> = self
            .minus
            .iter()
            .map(|l| {
                if comment_line(l, self.hash) {
                    Vec::new()
                } else {
                    line_tokens(l, self.hash)
                }
            })
            .collect();
        let new: Vec<Vec<Tok>> = self
            .plus
            .iter()
            .map(|(_, l)| {
                if comment_line(l, self.hash) {
                    Vec::new()
                } else {
                    line_tokens(l, self.hash)
                }
            })
            .collect();
        // A line whose tokens a `+` line repeats exactly (a moved or
        // re-indented line) pairs with it first, in order, and with no other.
        let (mut used_m, mut used_p) = (HashSet::new(), HashSet::new());
        for (mi, o) in old.iter().enumerate() {
            if o.is_empty() {
                continue;
            }
            let same = new
                .iter()
                .enumerate()
                .find(|(pi, n)| !used_p.contains(pi) && same_tokens(o, n));
            if let Some((pi, _)) = same {
                used_m.insert(mi);
                used_p.insert(pi);
            }
        }
        let mut candidates: Vec<(usize, usize, usize, usize)> = Vec::new();
        for (mi, o) in old.iter().enumerate() {
            if used_m.contains(&mi) {
                continue;
            }
            for (pi, n) in new.iter().enumerate() {
                if used_p.contains(&pi) {
                    continue;
                }
                let (shared, pairs) = aligned(o, n);
                if shared > 0 && !pairs.is_empty() {
                    let affinity = pairs.iter().filter(|(a, b)| akin(&a.norm, &b.norm)).count();
                    candidates.push((shared, affinity, mi, pi));
                }
            }
        }
        candidates.sort_by(|a, b| {
            (b.0, b.1)
                .cmp(&(a.0, a.1))
                .then((a.2, a.3).cmp(&(b.2, b.3)))
        });
        let mut out = Vec::new();
        for (_, _, mi, pi) in candidates {
            if used_m.contains(&mi) || used_p.contains(&pi) {
                continue;
            }
            used_m.insert(mi);
            used_p.insert(pi);
            out.push((mi, pi));
        }
        out.sort_unstable();
        out
    }
}

/// Two lines with the same tokens, class and normalised name alike.
fn same_tokens(a: &[Tok], b: &[Tok]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.class == y.class && x.norm == y.norm)
}

/// Two normalised names that look like one renamed: one holds the other
/// (`id` / `messageid`), or they share a 3-character prefix or suffix.
fn akin(a: &str, b: &str) -> bool {
    let common = |x: &[u8], y: &[u8]| x.len() >= 3 && y.len() >= 3 && x[..3] == y[..3];
    let rev = |x: &str| x.bytes().rev().collect::<Vec<u8>>();
    a.contains(b) || b.contains(a) || common(a.as_bytes(), b.as_bytes()) || common(&rev(a), &rev(b))
}

/// The common prefix plus suffix length of two lines' tokens, and the token
/// pairs at one position of what is left, when it is as long on both sides
/// (else none).
fn aligned(old: &[Tok], new: &[Tok]) -> (usize, Vec<(Tok, Tok)>) {
    let same = |a: &Tok, b: &Tok| a.class == b.class && a.norm == b.norm;
    let mut p = 0;
    while p < old.len() && p < new.len() && same(&old[p], &new[p]) {
        p += 1;
    }
    let mut s = 0;
    while s < old.len() - p
        && s < new.len() - p
        && same(&old[old.len() - 1 - s], &new[new.len() - 1 - s])
    {
        s += 1;
    }
    let (o, n) = (&old[p..old.len() - s], &new[p..new.len() - s]);
    if o.len() != n.len() {
        return (p + s, Vec::new());
    }
    let pairs = o
        .iter()
        .zip(n)
        .filter(|(a, b)| !same(a, b))
        .map(|(a, b)| (a.clone(), b.clone()))
        .collect();
    (p + s, pairs)
}

// ---- tokens ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Class {
    Ident,
    Str,
}

impl Class {
    const BOTH: [Class; 2] = [Class::Ident, Class::Str];
}

#[derive(Clone, Debug)]
struct Tok {
    class: Class,
    raw: String,
    norm: String,
    /// Right after a `.`.
    property: bool,
    /// Right before a `:` (not `::`).
    key: bool,
}

impl Tok {
    fn position(&self) -> &'static str {
        if self.property {
            "property"
        } else if self.key {
            "key"
        } else if self.class == Class::Str {
            "string"
        } else {
            "identifier"
        }
    }
}

/// A token that can be a name: not a keyword (identifier class) nor a tag word.
fn is_name(t: &Tok) -> bool {
    !TAG_WORDS.contains(&t.norm.as_str())
        && (t.class != Class::Ident || !KEYWORDS.contains(&t.norm.as_str()))
}

/// Lower case, `_` and `-` dropped.
fn normalise(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != '_' && *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Prose and data files a change's names are never read from: their quoted
/// words (a README's `` `id` ``) would mask a removal in code.
const PROSE_EXTS: &[&str] = &[
    "md", "markdown", "mdx", "rst", "txt", "adoc", "asciidoc", "po", "pot", "csv", "tsv", "svg",
    "lock", "sum", "log",
];

fn prose(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    match name.rsplit_once('.') {
        Some((_, ext)) => PROSE_EXTS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// Files whose `#` starts a comment.
fn hash_comments(file: &str) -> bool {
    let ext = file.rsplit('.').next().unwrap_or("");
    matches!(
        ext,
        "py" | "rb" | "sh" | "yaml" | "yml" | "toml" | "pl" | "r" | "ex" | "exs" | "tf" | "cr"
    )
}

/// A line that is all comment.
fn comment_line(line: &str, hash: bool) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
        || t.starts_with("/*")
        || t.starts_with('*')
        || t.starts_with("<!--")
        || (hash && t.starts_with('#'))
}

/// A diff line's tokens for name finding: a comment line has none, and a
/// struct-tag line keeps its string tokens only (module docs, step 1).
fn name_tokens(line: &str, hash: bool) -> Vec<Tok> {
    if comment_line(line, hash) {
        return Vec::new();
    }
    let toks = line_tokens(line, hash);
    let tagged = toks
        .iter()
        .any(|t| t.class == Class::Str && t.key && TAG_KEYS.contains(&t.norm.as_str()));
    if tagged {
        return toks.into_iter().filter(|t| t.class == Class::Str).collect();
    }
    toks
}

enum Mode {
    Code { brace: u32 },
    Str(u8),
}

fn ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

fn ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// The identifier and string tokens of one line (module docs, step 1).
fn line_tokens(line: &str, hash: bool) -> Vec<Tok> {
    let b = line.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut stack: Vec<Mode> = vec![Mode::Code { brace: 0 }];
    let mut i = 0;
    while i < n {
        let c = b[i];
        let next = b.get(i + 1).copied();
        let in_code = matches!(stack.last(), Some(Mode::Code { .. }));
        if in_code {
            if c == b'/' && next == Some(b'/') {
                break;
            }
            if c == b'/' && next == Some(b'*') {
                match line[i + 2..].find("*/") {
                    Some(at) => {
                        i += 2 + at + 2;
                        continue;
                    }
                    None => break,
                }
            }
            if hash && c == b'#' {
                break;
            }
            if c == b'"' || c == b'\'' || c == b'`' {
                stack.push(Mode::Str(c));
                i += 1;
                continue;
            }
            if c == b'{' {
                if let Some(Mode::Code { brace }) = stack.last_mut() {
                    *brace += 1;
                }
            } else if c == b'}' {
                let close =
                    matches!(stack.last(), Some(Mode::Code { brace: 0 })) && stack.len() > 1;
                if close {
                    stack.pop();
                    i += 1;
                    continue;
                }
                if let Some(Mode::Code { brace }) = stack.last_mut() {
                    *brace = brace.saturating_sub(1);
                }
            }
            if ident_start(c) {
                let mut j = i + 1;
                while j < n && ident_char(b[j]) {
                    j += 1;
                }
                push_tok(&mut out, line, i, j, Class::Ident, None);
                i = j;
                continue;
            }
            if c.is_ascii_digit() {
                while i < n && ident_char(b[i]) {
                    i += 1;
                }
                continue;
            }
            i += 1;
            continue;
        }
        let Some(Mode::Str(q)) = stack.last() else {
            break;
        };
        let q = *q;
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == q {
            stack.pop();
            i += 1;
            continue;
        }
        if q == b'`' && c == b'$' && next == Some(b'{') {
            stack.push(Mode::Code { brace: 0 });
            i += 2;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i + 1;
            while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'-') {
                j += 1;
            }
            while j > i + 1 && b[j - 1] == b'-' {
                j -= 1;
            }
            push_tok(&mut out, line, i, j, Class::Str, Some(q));
            i = j;
            continue;
        }
        if c.is_ascii_digit() {
            while i < n && b[i].is_ascii_alphanumeric() {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    out
}

/// Push the token `line[i..j]`, its property and key flags read from the
/// bytes around it (`quote`: the string it sits in, skipped before a `:`).
fn push_tok(out: &mut Vec<Tok>, line: &str, i: usize, j: usize, class: Class, quote: Option<u8>) {
    let b = line.as_bytes();
    let raw = &line[i..j];
    let norm = normalise(raw);
    if norm.is_empty() {
        return;
    }
    let mut k = i;
    while k > 0 && (b[k - 1] == b' ' || b[k - 1] == b'\t') {
        k -= 1;
    }
    let property = k > 0 && b[k - 1] == b'.';
    let mut k = j;
    if let Some(q) = quote
        && b.get(k) == Some(&q)
    {
        k += 1;
    }
    while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
        k += 1;
    }
    let key = b.get(k) == Some(&b':') && b.get(k + 1) != Some(&b':');
    out.push(Tok {
        class,
        raw: raw.to_string(),
        norm,
        property,
        key,
    });
}

// ---- git -----------------------------------------------------------------------

/// Stdout of `git -C <repo> <args>` as text (lossy), `None` when git exits
/// non-zero; `Err` when git cannot run. No lock taken, no stdin read.
fn git_text(repo: &Path, args: &[&str]) -> Result<Option<String>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("git not found on PATH: {e}"))?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norms(line: &str) -> Vec<(Class, String, &'static str)> {
        line_tokens(line, false)
            .into_iter()
            .map(|t| {
                let p = t.position();
                (t.class, t.norm, p)
            })
            .collect()
    }

    #[test]
    fn tokens_read_code_strings_and_template_interpolations() {
        let got = norms("  return `#${msg.messageId}: ${msg.subject}`; // messageId");
        assert_eq!(
            got,
            vec![
                (Class::Ident, "return".into(), "identifier"),
                (Class::Ident, "msg".into(), "identifier"),
                (Class::Ident, "messageid".into(), "property"),
                (Class::Ident, "msg".into(), "identifier"),
                (Class::Ident, "subject".into(), "property"),
            ]
        );
        let tag = norms("\tMessageID string `json:\"message_id,omitempty\"`");
        assert_eq!(tag[2], (Class::Str, "json".into(), "key"));
        assert_eq!(tag[3], (Class::Str, "messageid".into(), "string"));
        assert_eq!(tag[4], (Class::Str, "omitempty".into(), "string"));
        let key = norms("  \"user-name\": 1, a::b");
        assert_eq!(key[0], (Class::Str, "username".into(), "key"));
        assert_eq!(key[1], (Class::Ident, "a".into(), "identifier"));
    }

    #[test]
    fn a_struct_tag_line_names_the_tag_not_the_field() {
        let t: Vec<String> = name_tokens("\tMessageID string `json:\"messageId\"`", false)
            .into_iter()
            .map(|t| t.norm)
            .collect();
        assert_eq!(t, vec!["json", "messageid"]);
    }

    #[test]
    fn a_hunk_pairs_a_rename_and_skips_a_moved_name() {
        let text = "diff --git a/m.go b/m.go\n--- a/m.go\n+++ b/m.go\n@@ -4 +4 @@ type M struct {\n-\tA string `json:\"oldName\"`\n+\tA string `json:\"newName\"`\n@@ -9,0 +10 @@\n+\tkept := \"moved\"\n@@ -12 +12,0 @@\n-\tmoved := 1\n";
        let diff = parse_diff(text);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].path, "m.go");
        assert_eq!(diff[0].hunks.len(), 3);
        let sets = ClassSets::of(&diff);
        let found = diff[0].hunks[0].names(&sets);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].raw, "oldName");
        assert_eq!(found[0].renamed_to.as_deref(), Some("newName"));
        assert_eq!(found[0].line, 4);
        // `moved` left code and reached a string: not removed.
        assert!(diff[0].hunks[2].names(&sets).is_empty());
        assert!(diff[0].hunks[2].touches(11, 11));
        assert!(!diff[0].hunks[2].touches(13, 20));
    }

    #[test]
    fn a_rename_is_spelled_as_the_wire_spells_it() {
        // The field and its tag both move; the tag's spelling names the pair.
        let text = "diff --git a/r.go b/r.go\n--- a/r.go\n+++ b/r.go\n@@ -9 +9 @@ type R struct {\n-\tID      string `json:\"id,omitempty\"`\n+\tMessageID     string `json:\"messageId,omitempty\"`\n";
        let diff = parse_diff(text);
        let sets = ClassSets::of(&diff);
        let found = diff[0].hunks[0].names(&sets);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].raw, "id");
        assert_eq!(found[0].renamed_to.as_deref(), Some("messageId"));
        assert_eq!(found[0].line, 9);
    }

    #[test]
    fn a_generic_name_needs_a_property_access() {
        let mut changed = BTreeMap::new();
        changed.insert(
            "type".to_string(),
            ChangedName {
                name: "type".into(),
                normalised: "type".into(),
                renamed_to: None,
                file: "s.go".into(),
                line: 1,
                exchanges: 1,
            },
        );
        let names: BTreeSet<String> = ["type".to_string()].into();
        let uses = scan_uses("const type = 1;\nx = r.type;\n", "c.ts", &names, &changed);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].line, 2);
        assert_eq!(uses[0].position, "property");
    }
}
