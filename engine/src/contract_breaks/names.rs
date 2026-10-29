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
//!
//!    A removed name COUNTS (FH.7c) only at a wire position: a token of a
//!    string with no whitespace in it (a quoted key, a struct tag, a JSON
//!    field, a path segment; a word of a sentence is not one), or an
//!    identifier written as a key (`name:` / `name?:`: an object-literal key,
//!    a DTO or type field; never inside a parameter list, never after a `.`,
//!    never on a block line such as `if x:` or `case x:`). Any other
//!    identifier counts only when the hunk pairs it with a rename. Never a
//!    name, whatever its position: a token of an import statement, or an
//!    identifier the file binds by importing an external package (an import
//!    whose library the file's IMPORTS cell lists); nor a name read from a
//!    hunk of a test file (the file's MODULE stamped ORIGIN `test_fixture`),
//!    nor one whose client uses (step 5) all sit in test files. A hunk of a
//!    generated file (its MODULE stamped ORIGIN `generated`) is read against
//!    that file's own lines: a generated SDK is its clients' interface, so a
//!    name that leaves it is removed for them, whatever the rest of the
//!    change does with the word. The marker counts what each rule dropped.
//! 3. THE EXCHANGES. Every pair edge (HTTP_CALLS, GRPC_CALLS, RPC_CALLS,
//!    GRAPHQL_CALLS, QUEUE_FLOWS, WS_CONNECTS) from a client node (ENDPOINT,
//!    GRPC_CLIENT, RPC_CALL, GRAPHQL_OPERATION, QUEUE_PRODUCER, WS_CLIENT) to
//!    a server node (ROUTE, GRPC_SERVICE, RPC_PROCEDURE, GRAPHQL_RESOLVER,
//!    QUEUE_CONSUMER, WS_HANDLER) of the worktree's repo; a client end in a
//!    test file (stamped ORIGIN `test_fixture`, or placed in a test file) is a
//!    test of the server, not a client, and pairs nothing. The server side is
//!    the server node, its HANDLED_BY handlers (no handler: the MODULE of the
//!    server node's file, an inline closure's), and what they reach within
//!    [`SERVER_DEPTH`] hops of CALLS / USES / INJECTS / RETURNS_TYPE /
//!    IMPLEMENTS / INHERITS_FROM / IMPORTS / HANDLED_BY inside that repo. A
//!    GENERATED SDK (FH.7c) is a transparent hop: a client end in the
//!    worktree's repo stamped ORIGIN `generated` (a generated fetch client),
//!    with the generated callables calling it and the type declarations their
//!    text names in that file (followed through the types they name), belongs
//!    to the server side at no depth, and a step into a generated node spends
//!    none. An exchange CHANGED when a hunk of the diff falls in the span of
//!    one of those nodes (a MODULE spans its file), whether or not it removed
//!    a name; its names are the counted removed names on those hunks' `-`
//!    lines.
//! 4. THE CLIENTS. For each changed exchange's client nodes: the client's file
//!    and the files of the callables that CALLS / USES it (the response
//!    handling code; through a generated callable, the callables that call
//!    it, so a generated client's own file is a root only when nothing
//!    outside it calls it; a caller in a test file is no root), then every
//!    file their MODULEs import or are imported by within [`CLIENT_DEPTH`]
//!    IMPORTS hops (leaving a generated MODULE spends no hop), in any repo of
//!    the build (a `--with` client too), less the exchange's server-side
//!    files and every generated file.
//! 5. A USE. In a client file, a token whose normalised form is one of the
//!    exchange's names, outside a comment. A GENERIC name (normalised shorter
//!    than 4, or in [`GENERIC`]) counts only at a property access (`x.type`)
//!    or when the change renamed it.
//! 6. ROWS. One per (name, client file, line) (step 7 folds some), sorted
//!    by (repo label, file, line, name); `position` says how the line uses
//!    it (`property`, `key`, `string`, `identifier`, or `folded`). The tier is `derived` when the pair edge is
//!    a fact and the name is a rename pair, else `heuristic`; a row of several
//!    exchanges keeps its best tier, then the least (client, server) qname. A
//!    row blocks only under [`NameBreakArgs::min_tier`] `heuristic`.
//!    [`NameBreaks::fixed`] counts the (name, client file) pairs the change
//!    itself fixed: a client file of the change that used the name at the
//!    base and no longer does, so the fixed state of a drift reads `rows=0
//!    fixed=<n>`.
//! 7. FOLDS (FH.7d). A rename of a generic field (`id -> messageId`) matches
//!    every `.id` of a frontend; a reviewer needs one row saying so, not one
//!    per site. A site is ANCHORED when it is the exchange's own field
//!    access: a property, key (a shorthand `{ name }` too) or string use
//!    inside one of the exchange's client functions (the FUNCTION / METHOD
//!    roots of step 4), or a property read off a receiver the same line
//!    reads another of the exchange's names off (`Tp(data.id, data.params)`:
//!    the body, read field by field). A generic name keeps a row per
//!    anchored or key site (`name:`, `"name":`, a shorthand `{ name }`
//!    outside an import statement); its other, free-text sites fold. Any
//!    name matched in more than 25 client files keeps only its anchored
//!    sites (all of them fold too when they alone span more than 25 files).
//!    A name's folded sites are ONE row at its first site, position
//!    `folded`, with [`NameBreak::folded`] (`generic` or `cap`),
//!    `client_sites`, `client_files` and the first 3 `sample_files`, its
//!    tier, client and server the best of the sites'. Only free text folds:
//!    a key-class site of a renamed generic field is still a row of its own.
//!
//! The diff side reads only the change's hunks, and the client side only the
//! files the boundary reaches, so a big change stays bounded. fired_on, once
//! per [`name_breaks`] call: `[name-breaks] base=<rev> exchanges=<e>
//! names=<n> renames=<r> clients=<c> rows=<k> fixed=<f> dropped_library=<l>
//! dropped_test=<t> unpaired_identifiers=<u> sdk_hops=<h> folded_generic=<g>
//! folded_cap=<p>` (c = client files scanned; l / t / u the distinct removed
//! names each step-2 rule dropped; h the generated client ends walked
//! through; g / p the folded rows of each step-7 reason, counted in k).

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

/// Client files a name may be matched in before its unanchored sites fold
/// into one row (step 7).
const NAME_SITE_CAP: usize = 25;

/// How many of a folded row's files it names ([`NameBreak::sample_files`]).
const SAMPLE_FILES: usize = 3;

/// Normalised names too common on any wire to count at a plain identifier:
/// they need a property access or a rename pair (step 5), and a client site
/// of one is a row of its own only when it is key-class (step 7). The
/// cohort's `names.py` noise list, normalised, plus the short wire words
/// (any name shorter than 4 is generic too: `id`, `ids`, `key`).
pub const GENERIC: &[&str] = &[
    "id",
    "ids",
    "key",
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

/// The generated callables whose text step 3 reads type names from.
const CALLABLE_KINDS: [NodeKindId; 2] = [node_kind::FUNCTION, node_kind::METHOD];

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
    /// A folded row only (step 7): why its sites folded, `generic` (a
    /// generic name's free-text matches) or `cap` (the name is matched in
    /// more than 25 client files). The row's file, line, client, server and
    /// tier are its first site's (the best tier's); its position `folded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folded: Option<&'static str>,
    /// A folded row only: the client sites (file, line) it stands for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_sites: Option<usize>,
    /// A folded row only: the distinct client files of those sites.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_files: Option<usize>,
    /// A folded row only: its first 3 files, in (repo label, file) order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample_files: Vec<String>,
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
    /// Distinct removed names never counted because every occurrence was an
    /// import statement's token or an identifier bound to an external
    /// package's import (step 2). The marker's; not serialised.
    #[serde(skip)]
    pub dropped_library: usize,
    /// Distinct removed names read only from hunks of test files (step 2).
    #[serde(skip)]
    pub dropped_test: usize,
    /// Distinct removed names only ever a bare identifier with no rename pair
    /// or a word of a sentence string (step 2).
    #[serde(skip)]
    pub unpaired_identifiers: usize,
    /// Generated client ends of changed exchanges walked through (step 3).
    #[serde(skip)]
    pub sdk_hops: usize,
    /// Folded rows whose reason is `generic` (step 7).
    #[serde(skip)]
    pub folded_generic: usize,
    /// Folded rows whose reason is `cap` (step 7).
    #[serde(skip)]
    pub folded_cap: usize,
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
        "[name-breaks] base={} exchanges={} names={} renames={} clients={} rows={} fixed={} \
         dropped_library={} dropped_test={} unpaired_identifiers={} sdk_hops={} \
         folded_generic={} folded_cap={}",
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
        answer.dropped_library,
        answer.dropped_test,
        answer.unpaired_identifiers,
        answer.sdk_hops,
        answer.folded_generic,
        answer.folded_cap,
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
        dropped_library: 0,
        dropped_test: 0,
        unpaired_identifiers: 0,
        sdk_hops: 0,
        folded_generic: 0,
        folded_cap: 0,
    };
    if diff.iter().all(|f| f.hunks.is_empty()) {
        return out;
    }
    let Some(provider) = provider_repo(repo, after) else {
        return out;
    };
    let index = Index::new(after);
    let classes = ClassSets::of(diff);
    // A generated file is its clients' interface: a name leaves it when it
    // leaves that file, whatever the rest of the change does with the word.
    let own_classes: Vec<Option<ClassSets>> = diff
        .iter()
        .map(|f| {
            let module = index.module_at.get(&(provider, f.path.clone()));
            module
                .is_some_and(|m| index.generated.contains(m))
                .then(|| ClassSets::of(std::slice::from_ref(f)))
        })
        .collect();
    let by_file: BTreeMap<&str, usize> = diff
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.as_str(), i))
        .collect();
    let mut texts = TextCache::new(after);
    // Step 2's file context (test file, external import bindings), per diff
    // file, and each hunk's removed names, read once.
    let ctxs: Vec<FileCtx> = diff
        .iter()
        .map(|f| FileCtx::of(f, provider, &index, &mut texts))
        .collect();
    let mut found: HashMap<(usize, usize), Vec<Found>> = HashMap::new();

    // Step 3: the exchanges, keyed by server node, clients with their best tier.
    let mut pairs: BTreeMap<u64, BTreeMap<u64, Tier>> = BTreeMap::new();
    for e in after.merged.all_edges() {
        if !PAIR_CATEGORIES.contains(&e.category)
            || !index.kind_in(e.to, &SERVER_KINDS)
            || !index.kind_in(e.from, &CLIENT_KINDS)
            || index.repo.get(&e.to) != Some(&provider)
            || index.tests.contains(&e.from)
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
    let mut touched_exchanges = 0;
    // Step 2's drops, by normalised name.
    let (mut library, mut test, mut unpaired) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut sdk_hops: BTreeSet<u64> = BTreeSet::new();
    for (&server, clients) in &pairs {
        let server = NodeId(server);
        let sdk: Vec<NodeId> = clients
            .keys()
            .map(|&c| NodeId(c))
            .filter(|c| index.generated.contains(c) && index.repo.get(c) == Some(&provider))
            .collect();
        let (reached, server_files) = index.server_side(server, provider, &sdk, &mut texts);
        let mut names = BTreeSet::new();
        let mut touched = false;
        for (file, spans) in &reached {
            let Some(&fi) = by_file.get(file.as_str()) else {
                continue;
            };
            let ctx = &ctxs[fi];
            for (hi, h) in diff[fi].hunks.iter().enumerate() {
                if !spans.iter().any(|&(s, e)| h.touches(s, e)) {
                    continue;
                }
                touched = true;
                let list = found
                    .entry((fi, hi))
                    .or_insert_with(|| h.names(own_classes[fi].as_ref().unwrap_or(&classes), ctx));
                for found in list.iter() {
                    match found.status {
                        Status::Library => {
                            library.insert(found.normalised.clone());
                            continue;
                        }
                        _ if ctx.test => {
                            test.insert(found.normalised.clone());
                            continue;
                        }
                        Status::Unpaired => {
                            unpaired.insert(found.normalised.clone());
                            continue;
                        }
                        Status::Counted => {}
                    }
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
        if !touched {
            continue;
        }
        touched_exchanges += 1;
        sdk_hops.extend(sdk.iter().map(|c| c.0));
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
    out.exchanges = touched_exchanges;
    out.sdk_hops = sdk_hops.len();
    let counted = |n: &String| changed.get(n).is_some_and(|c| c.exchanges > 0);
    out.dropped_library = library.iter().filter(|n| !counted(n)).count();
    out.dropped_test = test
        .iter()
        .filter(|n| !counted(n) && !library.contains(*n))
        .count();
    out.unpaired_identifiers = unpaired
        .iter()
        .filter(|n| !counted(n) && !library.contains(*n) && !test.contains(*n))
        .count();

    // Steps 4-6: the client files, the uses, the rows.
    let diff_files: HashSet<&str> = diff.iter().map(|f| f.path.as_str()).collect();
    let mut rows: BTreeMap<(String, String, i64, String), Site> = BTreeMap::new();
    let mut scanned: BTreeSet<(u64, String)> = BTreeSet::new();
    let mut fixed: BTreeSet<(String, String)> = BTreeSet::new();
    // Per name: whether some site sits outside a test file (step 2).
    let mut outside_tests: BTreeMap<String, bool> = BTreeMap::new();
    let server_qname = |id: NodeId| index.loc.locate(id).qname;
    for x in &exchanges {
        let server = server_qname(x.server);
        let spans = index.client_spans(&x.clients);
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
            let fn_spans = spans.get(&file_key).map(Vec::as_slice).unwrap_or_default();
            let uses = texts.uses(*repo_id, file, &x.names, &changed, fn_spans);
            let used: BTreeSet<&str> = uses.iter().map(|u| u.normalised.as_str()).collect();
            if *repo_id == provider && diff_files.contains(file.as_str()) {
                let base_text = git_text(repo, &["show", &format!("{sha}:./{file}")])
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                for u in scan_uses(&base_text, file, &x.names, &changed, &[]) {
                    if !used.contains(u.normalised.as_str()) {
                        fixed.insert((u.normalised, file.clone()));
                    }
                }
            }
            let test_file = index.test_files.contains(&file_key);
            for u in uses {
                let Some(name) = changed.get(&u.normalised) else {
                    continue;
                };
                *outside_tests.entry(u.normalised.clone()).or_default() |= !test_file;
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
                    folded: None,
                    client_sites: None,
                    client_files: None,
                    sample_files: Vec::new(),
                };
                let slot = rows.entry(key).or_insert_with(|| Site {
                    row: candidate.clone(),
                    anchored: false,
                    key: false,
                });
                slot.anchored |= u.anchored;
                slot.key |= u.key;
                if better(&candidate, &slot.row) {
                    slot.row = candidate;
                }
            }
        }
    }
    // A name only test files use is never a changed name.
    let test_only: BTreeSet<String> = outside_tests
        .into_iter()
        .filter(|(_, outside)| !outside)
        .map(|(n, _)| n)
        .collect();
    out.dropped_test += test_only.len();
    let sites: Vec<Site> = rows
        .into_values()
        .filter(|s| !test_only.contains(&s.row.normalised))
        .collect();
    let (rows, generic, cap) = fold(sites);
    out.rows = rows;
    out.folded_generic = generic;
    out.folded_cap = cap;
    out.blocking = out.rows.iter().filter(|r| r.blocking).count();
    out.clients = scanned.len();
    out.fixed = fixed.len();
    let mut names: Vec<ChangedName> = changed
        .into_values()
        .filter(|c| c.exchanges > 0 && !test_only.contains(&c.normalised))
        .collect();
    names.sort_by(|a, b| (&a.file, a.line, &a.normalised).cmp(&(&b.file, b.line, &b.normalised)));
    out.names = names;
    out
}

/// One client site's row before step 7, with what step 7 reads: whether
/// some exchange anchors it ([`Use::anchored`]) and whether it is a key.
struct Site {
    row: NameBreak,
    anchored: bool,
    key: bool,
}

/// Step 7: the sites (in row order) folded per name. A name matched in more
/// than [`NAME_SITE_CAP`] client files keeps only its anchored sites (and
/// those only while they span no more than the cap); a generic name keeps
/// its anchored and key sites. Every other site of the name folds into one
/// row at its first site, carrying the counts. Returns the rows in (repo
/// label, file, line, name) order and the folded rows by reason (generic,
/// cap).
fn fold(sites: Vec<Site>) -> (Vec<NameBreak>, usize, usize) {
    // Per name: the (repo label, file) of every site, of its anchored ones.
    type Files<'s> = BTreeSet<(&'s str, &'s str)>;
    let mut files: BTreeMap<&str, (Files, Files)> = BTreeMap::new();
    for s in &sites {
        let slot = files.entry(s.row.normalised.as_str()).or_default();
        let file = (s.row.repo.as_str(), s.row.file.as_str());
        slot.0.insert(file);
        if s.anchored {
            slot.1.insert(file);
        }
    }
    let over: BTreeMap<String, (bool, bool)> = files
        .into_iter()
        .map(|(n, (all, anchored))| {
            (
                n.to_string(),
                (all.len() > NAME_SITE_CAP, anchored.len() > NAME_SITE_CAP),
            )
        })
        .collect();
    let mut kept: Vec<NameBreak> = Vec::new();
    // Per name, its folded sites in row order and the reason.
    let mut folded: BTreeMap<String, (Vec<NameBreak>, &'static str)> = BTreeMap::new();
    for s in sites {
        let (cap, anchored_cap) = over.get(&s.row.normalised).copied().unwrap_or_default();
        let reason = if cap && (!s.anchored || anchored_cap) {
            Some("cap")
        } else if is_generic(&s.row.normalised) && !s.anchored && !s.key {
            Some("generic")
        } else {
            None
        };
        match reason {
            None => kept.push(s.row),
            Some(r) => {
                let slot = folded
                    .entry(s.row.normalised.clone())
                    .or_insert_with(|| (Vec::new(), r));
                if r == "cap" {
                    slot.1 = r;
                }
                slot.0.push(s.row);
            }
        }
    }
    let (mut generic, mut cap) = (0, 0);
    for (_, (group, reason)) in folded {
        if reason == "cap" {
            cap += 1;
        } else {
            generic += 1;
        }
        let distinct: BTreeSet<(&str, &str)> = group
            .iter()
            .map(|r| (r.repo.as_str(), r.file.as_str()))
            .collect();
        let sample_files: Vec<String> = distinct
            .iter()
            .take(SAMPLE_FILES)
            .map(|(_, f)| f.to_string())
            .collect();
        let client_files = distinct.len();
        let client_sites = group.len();
        // The first site's place, the best tier's (client, server).
        let mut best = group[0].clone();
        for r in &group[1..] {
            if better(r, &best) {
                best.tier = r.tier;
                best.client = r.client.clone();
                best.server = r.server.clone();
            }
        }
        kept.push(NameBreak {
            position: "folded",
            folded: Some(reason),
            client_sites: Some(client_sites),
            client_files: Some(client_files),
            sample_files,
            ..best
        });
    }
    kept.sort_by(|a, b| {
        (&a.repo, &a.file, a.line, &a.normalised).cmp(&(&b.repo, &b.file, b.line, &b.normalised))
    });
    (kept, generic, cap)
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
    /// Nodes stamped ORIGIN `generated` / `generated_proto`, and every node
    /// placed in the file of a MODULE so stamped (step 3).
    generated: HashSet<NodeId>,
    /// `(repo, file)` of every MODULE stamped ORIGIN `test_fixture` (step 2).
    test_files: HashSet<(u64, String)>,
    /// Nodes stamped ORIGIN `test_fixture`, and every node placed in a test
    /// file: a client end or caller here is a test, not a client (step 4).
    tests: HashSet<NodeId>,
    /// `(repo, file)` -> the library names its MODULE's IMPORTS cell lists.
    libs: HashMap<(u64, String), Vec<String>>,
}

/// The ORIGIN cell's `provenance`, when a node has one.
fn provenance(cells: &[glia_core::Cell]) -> Option<String> {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .find_map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => {
                serde_json::from_str::<serde_json::Value>(s)
                    .ok()?
                    .get("provenance")?
                    .as_str()
                    .map(str::to_string)
            }
            _ => None,
        })
}

/// The library names of a node's IMPORTS cell (a JSON string array).
fn library_cell(cells: &[glia_core::Cell]) -> Vec<String> {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::IMPORTS)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => {
                serde_json::from_str::<Vec<String>>(s).ok()
            }
            _ => None,
        })
        .flatten()
        .collect()
}

impl<'a> Index<'a> {
    fn new(after: &'a GenerateResult) -> Self {
        let merged = &after.merged;
        let loc = Locator::new(merged);
        let mut kind = HashMap::new();
        let mut repo = HashMap::new();
        let mut place = HashMap::new();
        let mut module_at = HashMap::new();
        let mut generated = HashSet::new();
        let mut test_files = HashSet::new();
        let mut tests = HashSet::new();
        let mut libs = HashMap::new();
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
                let origin = provenance(&n.cells);
                if matches!(origin.as_deref(), Some("generated" | "generated_proto")) {
                    generated.insert(n.id);
                }
                if origin.as_deref() == Some("test_fixture") {
                    tests.insert(n.id);
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
                    let key = (g.repo.0, file.clone());
                    if origin.as_deref() == Some("test_fixture") {
                        test_files.insert(key.clone());
                    }
                    libs.entry(key.clone())
                        .or_insert_with(|| library_cell(&n.cells));
                    module_at.entry(key).or_insert(n.id);
                }
                place.insert(n.id, (file, rows));
            }
        }
        // A node of a generated MODULE's file is generated too (a client
        // end the provenance pass leaves unstamped).
        let generated_files: HashSet<(u64, &String)> = module_at
            .iter()
            .filter(|(_, m)| generated.contains(*m))
            .map(|((r, f), _)| (*r, f))
            .collect();
        let more: Vec<NodeId> = place
            .iter()
            .filter(|(id, (file, _))| {
                repo.get(*id)
                    .is_some_and(|r| generated_files.contains(&(*r, file)))
            })
            .map(|(id, _)| *id)
            .collect();
        generated.extend(more);
        let more: Vec<NodeId> = place
            .iter()
            .filter(|(id, (file, _))| {
                repo.get(*id)
                    .is_some_and(|r| test_files.contains(&(*r, (*file).clone())))
            })
            .map(|(id, _)| *id)
            .collect();
        tests.extend(more);
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
            generated,
            test_files,
            tests,
            libs,
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

    /// The callables that CALLS / USES `id`, in edge order.
    fn callers(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.inn
            .get(&id)
            .into_iter()
            .flatten()
            .filter(|(_, cat)| *cat == edge_category::CALLS || *cat == edge_category::USES)
            .map(|(from, _)| *from)
    }

    /// Step 3's generated hop: `ends` (generated client ends) and every
    /// generated node that calls one, transitively, in discovery order.
    fn through_generated(&self, ends: &[NodeId]) -> Vec<NodeId> {
        let mut seen: HashSet<NodeId> = ends.iter().copied().collect();
        let mut out: Vec<NodeId> = Vec::new();
        let mut queue: VecDeque<NodeId> = ends.iter().copied().collect();
        while let Some(n) = queue.pop_front() {
            out.push(n);
            for from in self.callers(n) {
                if self.generated.contains(&from) && seen.insert(from) {
                    queue.push_back(from);
                }
            }
        }
        out
    }

    /// Step 3: per file of `provider`, the spans the server side of `server`
    /// reaches, and the files of that side. `sdk` are the exchange's
    /// generated client ends in `provider`: they, the generated callables
    /// calling them and the type declarations those callables name join the
    /// side at no depth, and a step into a generated node spends none.
    fn server_side(
        &self,
        server: NodeId,
        provider: u64,
        sdk: &[NodeId],
        texts: &mut TextCache,
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
        let hop = self.through_generated(sdk);
        start.extend(hop.iter().copied());
        // A 0-1 walk: a step into a generated node costs nothing.
        let mut dist: HashMap<NodeId, usize> = HashMap::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
        for s in start {
            if dist.insert(s, 0).is_none() {
                queue.push_back((s, 0));
            }
        }
        let mut done: HashSet<NodeId> = HashSet::new();
        let mut reached: BTreeMap<String, Vec<Span>> = BTreeMap::new();
        let mut files = BTreeSet::new();
        while let Some((id, depth)) = queue.pop_front() {
            if dist.get(&id).is_some_and(|&d| d < depth) || !done.insert(id) {
                continue;
            }
            if let Some((file, span)) = self.place.get(&id)
                && self.repo.get(&id) == Some(&provider)
            {
                reached.entry(file.clone()).or_default().push(*span);
                files.insert(file.clone());
            }
            if dist.len() >= SERVER_VISIT_CAP {
                continue;
            }
            for (to, cat) in self.out.get(&id).into_iter().flatten() {
                if !SERVER_WALK.contains(cat) || self.repo.get(to) != Some(&provider) {
                    continue;
                }
                let free = self.generated.contains(to);
                let next = depth + usize::from(!free);
                if next > SERVER_DEPTH
                    || dist.get(to).is_some_and(|&d| d <= next)
                    || (dist.len() >= SERVER_VISIT_CAP && !dist.contains_key(to))
                {
                    continue;
                }
                dist.insert(*to, next);
                if free {
                    queue.push_front((*to, next));
                } else {
                    queue.push_back((*to, next));
                }
            }
        }
        // The generated callables' types, read from their file's text.
        for id in hop {
            let Some((file, span)) = self.place.get(&id) else {
                continue;
            };
            if self.repo.get(&id) != Some(&provider) || !self.kind_in(id, &CALLABLE_KINDS) {
                continue;
            }
            for t in texts.sdk_types(provider, file, *span) {
                reached.entry(file.clone()).or_default().push(t);
            }
        }
        (reached, files)
    }

    /// Step 4's roots of client `c`: `c` and the callables that call it,
    /// walked through generated callers, less generated and test nodes.
    fn roots(&self, c: NodeId) -> Vec<NodeId> {
        let mut roots: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::from([c]);
        let mut stack: Vec<NodeId> = vec![c];
        while let Some(n) = stack.pop() {
            let generated = self.generated.contains(&n);
            if !generated && !self.tests.contains(&n) {
                roots.push(n);
            }
            if generated || n == c {
                for from in self.callers(n) {
                    if seen.insert(from) {
                        stack.push(from);
                    }
                }
            }
        }
        roots
    }

    /// Step 7's client functions of `clients`: per `(repo, file)`, the spans
    /// of the FUNCTION / METHOD roots (step 4) placed there, in root order.
    fn client_spans(&self, clients: &BTreeMap<u64, Tier>) -> HashMap<(u64, String), Vec<Span>> {
        let mut out: HashMap<(u64, String), Vec<Span>> = HashMap::new();
        for &client in clients.keys() {
            for r in self.roots(NodeId(client)) {
                if !self.kind_in(r, &CALLABLE_KINDS) {
                    continue;
                }
                if let (Some(repo), Some((file, span))) = (self.repo.get(&r), self.place.get(&r)) {
                    out.entry((*repo, file.clone())).or_default().push(*span);
                }
            }
        }
        out
    }

    /// Step 4: every client file of `clients`, keyed `(repo, file)`, with the
    /// client it is reached from (best tier, then least node id: the qname
    /// tie-break is the row's) and that client's pair tier. A generated
    /// client or caller is walked through to the callables calling it, and
    /// an IMPORTS hop out of a generated MODULE spends no depth; a generated
    /// file is never a client file.
    fn client_files(&self, clients: &BTreeMap<u64, Tier>) -> BTreeMap<(u64, String), (u64, Tier)> {
        let mut out: BTreeMap<(u64, String), (u64, Tier)> = BTreeMap::new();
        for (&client, &tier) in clients {
            let c = NodeId(client);
            let mut roots = self.roots(c);
            if roots.is_empty() {
                // A generated client nothing outside the SDK calls: its file
                // is the root, and the walk leaves it at no depth.
                roots.push(c);
            }
            let mut dist: HashMap<NodeId, usize> = HashMap::new();
            let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
            for r in roots {
                let m = self.module_of(r);
                if dist.insert(m, 0).is_none() {
                    queue.push_back((m, 0));
                }
            }
            let mut done: HashSet<NodeId> = HashSet::new();
            while let Some((m, depth)) = queue.pop_front() {
                if dist.get(&m).is_some_and(|&d| d < depth) || !done.insert(m) {
                    continue;
                }
                let generated = self.generated.contains(&m);
                if !generated
                    && let (Some(repo), Some((file, _))) = (self.repo.get(&m), self.place.get(&m))
                {
                    let slot = out.entry((*repo, file.clone())).or_insert((client, tier));
                    if tier < slot.1 {
                        *slot = (client, tier);
                    }
                }
                let next = depth + usize::from(!generated);
                if next > CLIENT_DEPTH {
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
                    let to = self.module_of(*other);
                    if dist.get(&to).is_some_and(|&d| d <= next) {
                        continue;
                    }
                    dist.insert(to, next);
                    if next == depth {
                        queue.push_front((to, next));
                    } else {
                        queue.push_back((to, next));
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
    /// Some token of the line uses it as a key (`name:`, `"name":`, a
    /// shorthand `{ name }` outside an import statement).
    key: bool,
    /// Step 7: the exchange's own field access. A property, key or string
    /// use inside one of the exchange's client functions, or a property read
    /// off a receiver the same line reads another of the exchange's names
    /// off (`data.id, data.params`: the exchange's body, read field by field).
    anchored: bool,
}

/// File texts under the build's repo roots, read once per `(repo, file)`,
/// and a generated file's type declarations, read once.
struct TextCache<'a> {
    after: &'a GenerateResult,
    texts: HashMap<(u64, String), String>,
    decls: HashMap<(u64, String), HashMap<String, Span>>,
}

impl<'a> TextCache<'a> {
    fn new(after: &'a GenerateResult) -> Self {
        TextCache {
            after,
            texts: HashMap::new(),
            decls: HashMap::new(),
        }
    }

    /// The text of `file` in `repo` (empty when unreadable or over
    /// [`FILE_CAP`]).
    fn text(&mut self, repo: u64, file: &str) -> &str {
        let key = (repo, file.to_string());
        let after = self.after;
        self.texts.entry(key).or_insert_with(|| {
            after
                .repo_roots
                .get(&repo)
                .map(|root| PathBuf::from(root).join(file))
                .and_then(|p| std::fs::read(p).ok())
                .filter(|b| b.len() <= FILE_CAP)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default()
        })
    }

    fn uses(
        &mut self,
        repo: u64,
        file: &str,
        names: &BTreeSet<String>,
        changed: &BTreeMap<String, ChangedName>,
        spans: &[Span],
    ) -> Vec<Use> {
        scan_uses(self.text(repo, file), file, names, changed, spans)
    }

    /// Step 3: the spans of the type declarations of generated `file` that
    /// the rows `span` covers name, then the ones those name, each once.
    fn sdk_types(&mut self, repo: u64, file: &str, span: Span) -> Vec<Span> {
        let key = (repo, file.to_string());
        if !self.decls.contains_key(&key) {
            let decls = type_decls(self.text(repo, file));
            self.decls.insert(key.clone(), decls);
        }
        let (Some(text), Some(decls)) = (self.texts.get(&key), self.decls.get(&key)) else {
            return Vec::new();
        };
        let lines: Vec<&str> = text.lines().collect();
        let idents = |(s, e): Span| -> Vec<String> {
            let (s, e) = (s.max(0) as usize, e.max(0) as usize);
            lines
                .iter()
                .take(e.saturating_add(1))
                .skip(s)
                .flat_map(|l| line_tokens(l, false))
                .filter(|t| t.class == Class::Ident)
                .map(|t| t.raw)
                .collect()
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        let mut queue: VecDeque<String> = idents(span).into();
        while let Some(name) = queue.pop_front() {
            let Some(&decl) = decls.get(&name) else {
                continue;
            };
            if !seen.insert(name) {
                continue;
            }
            out.push(decl);
            queue.extend(idents(decl));
        }
        out
    }
}

/// The type declarations of a file by name (first wins), each with its
/// 0-based rows: a `type` / `interface` / `class` / `enum` / `struct` /
/// `record` line (after `export` / `declare` / `public` / `data` and the
/// like) to the line its braces close, or its own line when it ends in `;`
/// before any brace, or its indented block when it ends in `:`.
fn type_decls(text: &str) -> HashMap<String, Span> {
    const MODIFIERS: &[&str] = &[
        "export", "declare", "default", "public", "internal", "abstract", "sealed", "data",
        "partial", "final", "open",
    ];
    const KEYWORDS: &[&str] = &["type", "interface", "class", "enum", "struct", "record"];
    let lines: Vec<&str> = text.lines().collect();
    let mut out: HashMap<String, Span> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let mut words = line.split_whitespace().peekable();
        while words.peek().is_some_and(|w| MODIFIERS.contains(w)) {
            words.next();
        }
        if !words.next().is_some_and(|w| KEYWORDS.contains(&w)) {
            continue;
        }
        let Some(name) = words.next().map(|w| {
            w.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                .collect::<String>()
        }) else {
            continue;
        };
        if name.is_empty() || out.contains_key(&name) {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let block = line.trim_end().ends_with(':');
        let mut depth: i64 = 0;
        let mut opened = false;
        let mut end = i;
        for (j, l) in lines.iter().enumerate().skip(i).take(2000) {
            end = j;
            if block {
                let t = l.trim();
                if j > i && !t.is_empty() && l.len() - l.trim_start().len() <= indent {
                    end = j - 1;
                    break;
                }
                continue;
            }
            for c in l.bytes() {
                match c {
                    b'{' => {
                        depth += 1;
                        opened = true;
                    }
                    b'}' => depth -= 1,
                    _ => {}
                }
            }
            if (opened && depth <= 0) || (!opened && l.trim_end().ends_with(';')) {
                break;
            }
        }
        out.insert(name, (i as i64, end as i64));
    }
    out
}

// ---- step 2's file context -------------------------------------------------------

/// What step 2 knows about one diff file.
struct FileCtx {
    /// Its MODULE is stamped ORIGIN `test_fixture`.
    test: bool,
    /// Normalised identifiers the file binds by importing an external
    /// package: an import whose library its MODULE's IMPORTS cell lists, read
    /// from the worktree text and the hunks' removed lines.
    external: HashSet<String>,
}

impl FileCtx {
    fn of(f: &FileDiff, provider: u64, index: &Index, texts: &mut TextCache) -> Self {
        let key = (provider, f.path.clone());
        let test = index.test_files.contains(&key);
        let mut external = HashSet::new();
        let libs = index.libs.get(&key).map(Vec::as_slice).unwrap_or_default();
        if let Some(lang) = crate::extract::detect_language(&f.path)
            && !libs.is_empty()
        {
            let mut text = texts.text(provider, &f.path).to_string();
            for h in &f.hunks {
                for l in &h.minus {
                    text.push('\n');
                    text.push_str(l);
                }
            }
            for (names, spec) in import_bindings(&text, lang) {
                let lib = glia_code_domain::library_name(&spec, lang);
                if lib.is_some_and(|l| libs.contains(&l)) {
                    external.extend(names.iter().map(|n| normalise(n)));
                }
            }
        }
        FileCtx { test, external }
    }
}

/// The import statements of `text` as (bound names, module spec), for the
/// languages step 2 reads: the TS family, Python, Go, Rust, the JVM family
/// and C#, Dart. Bound names are every identifier of the statement but its
/// keywords (an imported name and its alias alike).
fn import_bindings(text: &str, lang: &str) -> Vec<(Vec<String>, String)> {
    const SKIP: &[&str] = &[
        "import", "export", "from", "as", "type", "typeof", "default", "use", "pub", "crate",
        "self", "super", "static", "using", "show", "hide", "deferred", "const", "let", "var",
        "require",
    ];
    let idents = |s: &str| -> Vec<String> {
        line_tokens(s, false)
            .into_iter()
            .filter(|t| t.class == Class::Ident && !SKIP.contains(&t.raw.as_str()))
            .map(|t| t.raw)
            .collect()
    };
    let quoted = |s: &str| -> Option<String> {
        let at = s.rfind(['\'', '"'])?;
        let q = s.as_bytes()[at];
        let open = s[..at].rfind(q as char)?;
        Some(s[open + 1..at].to_string())
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        let starts = |p: &str| t.starts_with(p);
        match lang {
            "typescript" | "javascript" | "react" | "angular" | "vue" => {
                if starts("import ")
                    || starts("import{")
                    || (starts("export ") && t.contains(" from "))
                {
                    // To the line holding the module spec.
                    let mut stmt = String::new();
                    let mut j = i;
                    while j < lines.len() && j < i + 64 {
                        stmt.push_str(lines[j]);
                        stmt.push('\n');
                        let l = lines[j];
                        if (l.contains(" from ") || l.trim_start().starts_with("from "))
                            && (l.contains('\'') || l.contains('"'))
                            || (j == i && !l.contains('{') && (l.contains('\'') || l.contains('"')))
                        {
                            break;
                        }
                        j += 1;
                    }
                    if let Some(spec) = quoted(&stmt) {
                        let head = stmt.rsplit_once(" from ").map_or("", |(h, _)| h);
                        out.push((idents(head), spec));
                    }
                    i = j + 1;
                    continue;
                }
                if t.contains("require(")
                    && let Some((head, call)) = t.split_once("require(")
                    && let Some(spec) = quoted(call.split(')').next().unwrap_or(""))
                {
                    let head = head.split('=').next().unwrap_or("");
                    out.push((idents(head), spec));
                }
            }
            "python" => {
                if let Some(rest) = t.strip_prefix("import ") {
                    for part in rest.split(',') {
                        let mut it = part.split(" as ");
                        let spec = it.next().unwrap_or("").trim().to_string();
                        let mut names: Vec<String> = spec
                            .split('.')
                            .next()
                            .map(str::to_string)
                            .into_iter()
                            .collect();
                        names.extend(it.next().map(|a| a.trim().to_string()));
                        out.push((names, spec));
                    }
                } else if let Some(rest) = t.strip_prefix("from ")
                    && let Some((spec, names)) = rest.split_once(" import ")
                {
                    let mut stmt = names.to_string();
                    let mut j = i;
                    if names.contains('(') {
                        while !lines[j].contains(')') && j + 1 < lines.len() && j < i + 64 {
                            j += 1;
                            stmt.push('\n');
                            stmt.push_str(lines[j]);
                        }
                    }
                    out.push((idents(&stmt), spec.trim().to_string()));
                    i = j + 1;
                    continue;
                }
            }
            "go" => {
                let one = |l: &str| -> Option<(Vec<String>, String)> {
                    let spec = quoted(l)?;
                    let alias = l.split('"').next().unwrap_or("").trim();
                    let name = if alias.is_empty() || alias == "import" {
                        let mut segs = spec.rsplit('/');
                        let last = segs.next().unwrap_or("");
                        let versioned = last.len() > 1
                            && last.starts_with('v')
                            && last[1..].bytes().all(|b| b.is_ascii_digit());
                        if versioned {
                            segs.next().unwrap_or(last)
                        } else {
                            last
                        }
                        .to_string()
                    } else {
                        alias.trim_start_matches("import").trim().to_string()
                    };
                    Some((vec![name], spec))
                };
                if t == "import (" {
                    let mut j = i + 1;
                    while j < lines.len() && lines[j].trim() != ")" {
                        out.extend(one(lines[j].trim()));
                        j += 1;
                    }
                    i = j + 1;
                    continue;
                }
                if starts("import ") {
                    out.extend(one(t));
                }
            }
            "rust" if starts("use ") || starts("pub use ") => {
                let mut stmt = t.to_string();
                let mut j = i;
                while !stmt.contains(';') && j + 1 < lines.len() && j < i + 64 {
                    j += 1;
                    stmt.push_str(lines[j]);
                }
                let spec = stmt
                    .trim_start_matches("pub ")
                    .trim_start_matches("use ")
                    .trim()
                    .to_string();
                out.push((idents(&stmt), spec));
                i = j + 1;
                continue;
            }
            "java" | "kotlin" | "scala" | "csharp" => {
                let rest = t
                    .strip_prefix("import ")
                    .or_else(|| t.strip_prefix("using "))
                    .map(|r| r.trim_start_matches("static ").trim_end_matches(';').trim());
                if let Some(rest) = rest
                    && !rest.contains('(')
                {
                    let (alias, path) = match rest.split_once('=') {
                        Some((a, p)) => (Some(a.trim()), p.trim()),
                        None => match rest.split_once(" as ") {
                            Some((p, a)) => (Some(a.trim()), p.trim()),
                            None => (None, rest),
                        },
                    };
                    let mut names: Vec<String> = path
                        .rsplit('.')
                        .next()
                        .filter(|n| *n != "*")
                        .map(str::to_string)
                        .into_iter()
                        .collect();
                    names.extend(alias.map(str::to_string));
                    out.push((names, path.to_string()));
                }
            }
            "dart" => {
                if (starts("import ") || starts("export "))
                    && let Some(spec) = t.split(['\'', '"']).nth(1)
                {
                    let tail = t.rsplit(['\'', '"']).next().unwrap_or("");
                    out.push((idents(tail), spec.to_string()));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// A line that is an import statement (or a line of one a statement's own
/// line names): its tokens are module specifiers and bindings, never a name.
fn import_line(line: &str, ext: &str) -> bool {
    let t = line.trim();
    let word = |w: &str, after: &[char]| t.strip_prefix(w).is_some_and(|r| r.starts_with(after));
    let from_tail = (t.starts_with('}') || t.starts_with("from "))
        && (t.contains(" from '") || t.contains(" from \"") || t.starts_with("from "));
    word("import", &[' ', '{', '\'', '"', '*'])
        || word("use", &[' '])
        || t.starts_with("pub use ")
        || t.starts_with("#include")
        || t.starts_with("@import")
        || t.starts_with("require_relative")
        || (t.starts_with("using ") && t.ends_with(';') && !t.contains('=') && !t.contains('('))
        || ((t.starts_with("export ") || t.starts_with("part "))
            && (t.contains(" from ") || ext == "dart"))
        || (t.contains("require(") && (t.contains('\'') || t.contains('"')))
        || (t.starts_with("require ") && (t.contains('\'') || t.contains('"')))
        || from_tail
        || (ext == "go" && go_import_spec(t))
}

/// A Go import block's line: an optional alias, then one quoted path.
fn go_import_spec(t: &str) -> bool {
    let Some(open) = t.find('"') else {
        return false;
    };
    let alias = t[..open].trim();
    t.ends_with('"')
        && t.len() > open + 1
        && !t[open + 1..t.len() - 1].contains(['"', ' '])
        && (alias.is_empty()
            || alias == "_"
            || alias == "."
            || alias
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

/// Step 5: the uses of `names` in `text`, one per (name, line), the
/// strongest position of the line kept. `spans` are the 0-based rows of the
/// exchange's client functions in this file ([`Use::anchored`]).
fn scan_uses(
    text: &str,
    file: &str,
    names: &BTreeSet<String>,
    changed: &BTreeMap<String, ChangedName>,
    spans: &[Span],
) -> Vec<Use> {
    let hash = hash_comments(file);
    let ext = file
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    // Per (line, name): the strongest position, key, anchored.
    let mut out: BTreeMap<(i64, String), (&'static str, bool, bool)> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        if comment_line(line, hash) {
            continue;
        }
        let row = i as i64;
        let import = import_line(line, &ext);
        let in_fn = spans.iter().any(|&(s, e)| s <= row && row <= e);
        let toks: Vec<Tok> = line_tokens(line, hash)
            .into_iter()
            .filter(|t| names.contains(&t.norm))
            .collect();
        // Receivers this line reads two or more of the names off.
        let mut read_off: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for t in &toks {
            if let Some(r) = t.receiver.as_deref() {
                read_off.entry(r).or_default().insert(t.norm.as_str());
            }
        }
        for t in &toks {
            let renamed = changed.get(&t.norm).is_some_and(|c| c.renamed_to.is_some());
            if is_generic(&t.norm) && !renamed && !t.property {
                continue;
            }
            let key = t.key || (t.shorthand && !import);
            let field = t.property || key || (t.class == Class::Str && !t.wordy);
            let shape = t
                .receiver
                .as_deref()
                .and_then(|r| read_off.get(r))
                .is_some_and(|n| n.len() >= 2);
            let anchored = (in_fn && field) || shape;
            let pos = t.position();
            let slot = out
                .entry((row + 1, t.norm.clone()))
                .or_insert((pos, false, false));
            if position_rank(pos) < position_rank(slot.0) {
                slot.0 = pos;
            }
            slot.1 |= key;
            slot.2 |= anchored;
        }
    }
    out.into_iter()
        .map(|((line, normalised), (position, key, anchored))| Use {
            normalised,
            line,
            position,
            key,
            anchored,
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
    /// The file's extension, ASCII lower case.
    ext: String,
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
                ext: file
                    .path
                    .rsplit_once('.')
                    .map(|(_, e)| e.to_ascii_lowercase())
                    .unwrap_or_default(),
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
    status: Status,
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

    /// The removed names on this hunk's `-` lines, with their rename pairs
    /// and step 2's reading of each (the best of its occurrences).
    fn names(&self, sets: &ClassSets, ctx: &FileCtx) -> Vec<Found> {
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
            let import = import_line(old, &self.ext);
            let block = block_line(old);
            for t in name_tokens(old, self.hash) {
                if !is_name(&t) || !sets.lost(t.class, &t.norm) || !sets.removed(&t.norm) {
                    continue;
                }
                let renamed = renames.get(&t.norm);
                let status =
                    if import || (t.class == Class::Ident && ctx.external.contains(&t.norm)) {
                        Status::Library
                    } else if renamed.is_some() || wire_position(&t, block) {
                        Status::Counted
                    } else {
                        Status::Unpaired
                    };
                if let Some(&(at, class)) = seen.get(&t.norm) {
                    let have = out[at].status;
                    let spell = status > have
                        || (status == have && class == Class::Ident && t.class == Class::Str);
                    if spell {
                        out[at].status = status.max(have);
                        out[at].raw = t.raw.clone();
                        seen.insert(t.norm.clone(), (at, t.class));
                    }
                    continue;
                }
                seen.insert(t.norm.clone(), (out.len(), t.class));
                out.push(Found {
                    raw: t.raw.clone(),
                    normalised: t.norm.clone(),
                    renamed_to: renamed.map(|(n, _, _)| n.clone()),
                    line: renamed.map_or(self.new_start.max(1), |(_, at, _)| *at),
                    status,
                });
            }
        }
        out
    }
}

/// Step 2's reading of a removed name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    /// A bare identifier with no rename pair, or a word of a sentence string.
    Unpaired,
    /// An import statement's token, or an identifier bound to an external
    /// package's import.
    Library,
    /// At a wire position, or renamed: a name.
    Counted,
}

/// Lines whose trailing `x:` opens a block or a branch, not a key.
const BLOCK_WORDS: &[&str] = &[
    "if", "elif", "else", "for", "while", "with", "def", "class", "try", "except", "finally",
    "case", "default", "match", "switch", "when", "lambda", "async",
];

fn block_line(line: &str) -> bool {
    let t = line.trim_start();
    let word: String = t
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    BLOCK_WORDS.contains(&word.as_str())
}

/// Step 2's wire position: a token of a string with no whitespace, or an
/// identifier written as a key outside a parameter list, a property access
/// and a block line.
fn wire_position(t: &Tok, block: bool) -> bool {
    match t.class {
        Class::Str => !t.wordy,
        Class::Ident => t.key && !t.property && !t.param && !block,
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
    /// Right before a `:` (not `::`), or a `?:` / `!:` (an optional or
    /// definite field).
    key: bool,
    /// A string token of a literal holding whitespace (a sentence, not a
    /// name).
    wordy: bool,
    /// An identifier whose innermost bracket open on its line is a `(`: a
    /// parameter list or a call's arguments.
    param: bool,
    /// A property's receiver: the dotted identifier chain right before its
    /// `.` (`res.data` of `res.data.id`, `?.` read as `.`); `None` for a
    /// non-property or a receiver that is no plain chain (`f().id`).
    receiver: Option<String>,
    /// An identifier alone between `{` / `,` and `,` / `}` inside a `{` of
    /// its line: a shorthand object key or destructured field (step 7 only).
    shorthand: bool,
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
        // A struct tag's keys are names, though the tag holds spaces.
        return toks
            .into_iter()
            .filter(|t| t.class == Class::Str)
            .map(|t| Tok { wordy: false, ..t })
            .collect();
    }
    toks
}

enum Mode {
    Code {
        brace: u32,
    },
    /// The quote, and whether the literal holds whitespace.
    Str(u8, bool),
}

/// Whether the literal opening at `start` (just past its quote `q`) holds
/// whitespace before it closes (a template's `${..}` skipped) or the line
/// ends.
fn literal_wordy(b: &[u8], start: usize, q: u8) -> bool {
    let mut j = start;
    while j < b.len() {
        let c = b[j];
        if c == b'\\' {
            j += 2;
            continue;
        }
        if c == q {
            return false;
        }
        if q == b'`' && c == b'$' && b.get(j + 1) == Some(&b'{') {
            let mut depth = 0usize;
            while j < b.len() {
                match b[j] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            j += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            return true;
        }
        j += 1;
    }
    false
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
    // Brackets open on this line, innermost last (step 2's `param`).
    let mut brackets: Vec<u8> = Vec::new();
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
                stack.push(Mode::Str(c, literal_wordy(b, i + 1, c)));
                i += 1;
                continue;
            }
            if c == b'{' {
                if let Some(Mode::Code { brace }) = stack.last_mut() {
                    *brace += 1;
                }
                brackets.push(c);
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
                brackets.pop();
            } else if c == b'(' || c == b'[' {
                brackets.push(c);
            } else if c == b')' || c == b']' {
                brackets.pop();
            }
            if ident_start(c) {
                let mut j = i + 1;
                while j < n && ident_char(b[j]) {
                    j += 1;
                }
                let param = brackets.last() == Some(&b'(');
                let brace = brackets.last() == Some(&b'{');
                push_tok(&mut out, line, i, j, Class::Ident, None, param);
                if brace && let Some(t) = out.last_mut() {
                    t.shorthand = shorthand_at(b, i, j);
                }
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
        let Some(&Mode::Str(q, wordy)) = stack.last() else {
            break;
        };
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
            push_tok(&mut out, line, i, j, Class::Str, Some(q), false);
            if let Some(t) = out.last_mut() {
                t.wordy = wordy;
            }
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
fn push_tok(
    out: &mut Vec<Tok>,
    line: &str,
    i: usize,
    j: usize,
    class: Class,
    quote: Option<u8>,
    param: bool,
) {
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
    let receiver = property.then(|| receiver_before(b, k - 1)).flatten();
    let mut k = j;
    if let Some(q) = quote
        && b.get(k) == Some(&q)
    {
        k += 1;
    }
    let optional = quote.is_none()
        && matches!(b.get(k), Some(b'?' | b'!'))
        && b.get(k + 1) == Some(&b':')
        && b.get(k + 2) != Some(&b':');
    while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
        k += 1;
    }
    let key = optional || (b.get(k) == Some(&b':') && b.get(k + 1) != Some(&b':'));
    out.push(Tok {
        class,
        raw: raw.to_string(),
        norm,
        property,
        key,
        wordy: false,
        param,
        receiver,
        shorthand: false,
    });
}

/// Whether the identifier `b[i..j]`, inside a `{` of its line, stands alone
/// between `{` / `,` and `,` / `}`: an object literal's shorthand key
/// (`{ password }`) or a destructured field.
fn shorthand_at(b: &[u8], i: usize, j: usize) -> bool {
    let before = b[..i].iter().rev().find(|c| !c.is_ascii_whitespace());
    let after = b[j..].iter().find(|c| !c.is_ascii_whitespace());
    matches!(before, Some(b'{' | b',')) && matches!(after, Some(b',' | b'}'))
}

/// The dotted identifier chain ending right before `b[dot]` (a `.`), its
/// optional-chaining `?.` read as `.`: `None` when no identifier precedes
/// the dot.
fn receiver_before(b: &[u8], dot: usize) -> Option<String> {
    let mut k = dot;
    if k > 0 && b[k - 1] == b'?' {
        k -= 1;
    }
    let end = k;
    while k > 0 && (ident_char(b[k - 1]) || b[k - 1] == b'.' || b[k - 1] == b'?') {
        k -= 1;
    }
    let chain: String = String::from_utf8_lossy(&b[k..end]).replace("?.", ".");
    let chain = chain.trim_start_matches(['.', '?']).to_string();
    let first = chain.bytes().next()?;
    ident_start(first).then_some(chain)
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

    fn no_ctx() -> FileCtx {
        FileCtx {
            test: false,
            external: HashSet::new(),
        }
    }

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
        let found = diff[0].hunks[0].names(&sets, &no_ctx());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].raw, "oldName");
        assert_eq!(found[0].renamed_to.as_deref(), Some("newName"));
        assert_eq!(found[0].line, 4);
        // `moved` left code and reached a string: not removed.
        assert!(diff[0].hunks[2].names(&sets, &no_ctx()).is_empty());
        assert!(diff[0].hunks[2].touches(11, 11));
        assert!(!diff[0].hunks[2].touches(13, 20));
    }

    #[test]
    fn a_rename_is_spelled_as_the_wire_spells_it() {
        // The field and its tag both move; the tag's spelling names the pair.
        let text = "diff --git a/r.go b/r.go\n--- a/r.go\n+++ b/r.go\n@@ -9 +9 @@ type R struct {\n-\tID      string `json:\"id,omitempty\"`\n+\tMessageID     string `json:\"messageId,omitempty\"`\n";
        let diff = parse_diff(text);
        let sets = ClassSets::of(&diff);
        let found = diff[0].hunks[0].names(&sets, &no_ctx());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].raw, "id");
        assert_eq!(found[0].renamed_to.as_deref(), Some("messageId"));
        assert_eq!(found[0].line, 9);
    }

    #[test]
    fn a_wire_position_is_a_name_shaped_string_or_a_key_outside_parameters() {
        let toks = |l: &str| -> Vec<(String, bool)> {
            let block = block_line(l);
            line_tokens(l, false)
                .into_iter()
                .map(|t| {
                    let w = wire_position(&t, block);
                    (t.norm, w)
                })
                .collect()
        };
        // A quoted key and a value string are names; a sentence is not.
        assert_eq!(
            toks("  \"user_id\": x, note: 'two words',"),
            vec![
                ("userid".into(), true),
                ("x".into(), false),
                ("note".into(), true),
                ("two".into(), false),
                ("words".into(), false),
            ]
        );
        // `?:` / `!:` fields are keys; a parameter's annotation is not.
        assert_eq!(
            toks("  password?: string; token!: string;"),
            vec![
                ("password".into(), true),
                ("string".into(), false),
                ("token".into(), true),
                ("string".into(), false),
            ]
        );
        assert!(
            toks("function f(id: string) { return { id: 1 } }")
                .iter()
                .filter(|(n, _)| n == "id")
                .map(|(_, w)| *w)
                .eq([false, true])
        );
        // A block line's trailing colon and a property are no key.
        assert!(toks("if user.active:").iter().all(|(_, w)| !w));
        // A Go tag's keys are names though the tag holds spaces.
        let tag: Vec<(String, bool)> =
            name_tokens("\tID string `json:\"id\" binding:\"required\"`", false)
                .into_iter()
                .map(|t| (t.norm.clone(), wire_position(&t, false)))
                .collect();
        assert!(tag.contains(&("id".into(), true)), "{tag:?}");
    }

    #[test]
    fn import_lines_and_external_bindings_are_read_per_language() {
        assert!(import_line("import { a as b } from 'lib';", "ts"));
        assert!(import_line("} from \"lib\";", "ts"));
        assert!(import_line("from pkg.mod import thing", "py"));
        assert!(import_line("\tchi \"github.com/go-chi/chi/v5\"", "go"));
        assert!(import_line("use serde::Deserialize;", "rs"));
        assert!(!import_line("const user = useUser();", "ts"));
        assert!(!import_line("\"id\",", "go"));
        let ts = import_bindings(
            "import {\n  formatDistance,\n  parse as parseDate,\n} from 'date-fns';\nimport z from 'zod';\nconst { join } = require('path');\n",
            "typescript",
        );
        assert_eq!(
            ts,
            vec![
                (
                    vec!["formatDistance".into(), "parse".into(), "parseDate".into()],
                    "date-fns".into()
                ),
                (vec!["z".into()], "zod".into()),
                (vec!["join".into()], "path".into()),
            ]
        );
        let go = import_bindings(
            "import (\n\t\"fmt\"\n\tchi \"github.com/go-chi/chi/v5\"\n\t\"github.com/gin-gonic/gin\"\n)\n",
            "go",
        );
        assert_eq!(
            go.iter().map(|(n, _)| n[0].as_str()).collect::<Vec<_>>(),
            vec!["fmt", "chi", "gin"]
        );
        let py = import_bindings(
            "import numpy as np\nfrom flask import (\n  Flask,\n  jsonify,\n)\n",
            "python",
        );
        assert_eq!(py[0].0, vec!["numpy".to_string(), "np".to_string()]);
        assert_eq!(
            py[1],
            (vec!["Flask".into(), "jsonify".into()], "flask".into())
        );
        let rs = import_bindings("use serde::{Deserialize, Serialize as Ser};\n", "rust");
        assert_eq!(rs[0].0, vec!["serde", "Deserialize", "Serialize", "Ser"]);
        let jvm = import_bindings(
            "import com.acme.api.Client;\nimport static org.x.Y.z;\n",
            "java",
        );
        assert_eq!(
            jvm[0],
            (vec!["Client".into()], "com.acme.api.Client".into())
        );
        assert_eq!(jvm[1].0, vec!["z".to_string()]);
    }

    #[test]
    fn type_declarations_span_their_braces() {
        let text = "/** gen */\nexport type A = {\n    b: B;\n};\nexport type B = {\n    c: string;\n};\nexport type Alias = string;\nexport function f(): A {\n    return x;\n}\n";
        let d = type_decls(text);
        assert_eq!(d.get("A"), Some(&(1, 3)));
        assert_eq!(d.get("B"), Some(&(4, 6)));
        assert_eq!(d.get("Alias"), Some(&(7, 7)));
        assert!(!d.contains_key("f"));
        let py = type_decls("class M(BaseModel):\n    a: int\n    b: str\n\nx = 1\n");
        assert_eq!(py.get("M"), Some(&(0, 3)));
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
        let uses = scan_uses(
            "const type = 1;\nx = r.type;\n",
            "c.ts",
            &names,
            &changed,
            &[],
        );
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].line, 2);
        assert_eq!(uses[0].position, "property");
    }

    fn renamed(names: &[(&str, &str)]) -> BTreeMap<String, ChangedName> {
        names
            .iter()
            .map(|(from, to)| {
                (
                    from.to_string(),
                    ChangedName {
                        name: from.to_string(),
                        normalised: from.to_string(),
                        renamed_to: Some(to.to_string()),
                        file: "s.go".into(),
                        line: 1,
                        exchanges: 1,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_receiver_is_the_dotted_chain_before_the_property() {
        let recv = |line: &str, name: &str| {
            line_tokens(line, false)
                .into_iter()
                .find(|t| t.raw == name)
                .and_then(|t| t.receiver)
        };
        assert_eq!(recv("x = res.data.id;", "id").as_deref(), Some("res.data"));
        assert_eq!(
            recv("x = res?.data?.id;", "id").as_deref(),
            Some("res.data")
        );
        assert_eq!(recv("x = this.id;", "id").as_deref(), Some("this"));
        assert_eq!(recv("x = f().id;", "id"), None);
        assert_eq!(recv("x = id;", "id"), None);
    }

    #[test]
    fn a_site_is_anchored_by_its_client_function_or_the_exchanges_shape() {
        let changed = renamed(&[("id", "messageId"), ("params", "messageParams")]);
        let names: BTreeSet<String> = changed.keys().cloned().collect();
        let text = "function f(data) {\n  return data.id;\n}\n\
                    m = Tp(data.id, data.params);\n\
                    n = a.id + b.params;\n\
                    o = { id: 1 };\n\
                    p = { id, x };\n\
                    import { id } from './ids';\n";
        let uses = scan_uses(text, "c.ts", &names, &changed, &[(0, 2)]);
        let got: Vec<(i64, &str, &str, bool, bool)> = uses
            .iter()
            .map(|u| (u.line, u.normalised.as_str(), u.position, u.key, u.anchored))
            .collect();
        assert_eq!(
            got,
            vec![
                (2, "id", "property", false, true),
                (4, "id", "property", false, true),
                (4, "params", "property", false, true),
                (5, "id", "property", false, false),
                (5, "params", "property", false, false),
                (6, "id", "key", true, false),
                (7, "id", "identifier", true, false),
                (8, "id", "identifier", false, false),
            ]
        );
    }
}
