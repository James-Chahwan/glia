//! CA.4: inferred Go collection wrappers. A Go function that hands its own
//! parameter to the driver's collection call is a collection constructor:
//! quokka's only driver call is
//! `func NewCollection[T any](client *mongo.Client, database, name string) *Collection[T] { return &Collection[T]{col: client.Database(database).Collection(name)} }`,
//! so the data-entity extractor (which reads a literal first argument only)
//! mints nothing, while the identity sits at every call site of
//! `NewCollection[...](client, database, "users")`. Each such function
//! becomes a `kind = "data_entity"` stanza of the LF.2e / LG.3d wrapper stage
//! (`wrappers.rs`) with no `.glia/overlay.toml`; the stage reads and mints
//! its sites exactly like an overlay stanza's.
//!
//! INFERENCE, over the repo's Go files (`detect_language == "go"`) in walked
//! order:
//! 1. DIRECT: every [`data_entities::collection_param_calls`] call (a
//!    `COLLECTION_NEEDLES` call whose first argument is a bare identifier)
//!    that is not commented, inside a FUNCTION / METHOD of the file's parse
//!    (the innermost owner, `anchor::build_owner_index` + `owner_of_line`).
//!    The owner's declaration is read from the WALKED SOURCE over its
//!    POSITION line span (never a CODE cell): the first line in the span
//!    that starts with `func` and declares the owner's name. Its parameter
//!    list: `func`, an optional receiver `(..)`, the name, one optional
//!    `[..]` type-parameter list, then the balanced `(..)`, split on depth-0
//!    commas; an item's first token is its name, so a grouped
//!    `database, name string` gives `[database, name]`. The identifier's
//!    index there is the stanza's `name_arg`. An identifier that is not a
//!    parameter (a package const, `.Collection(tableName)`) makes no wrapper;
//!    an owner whose declaration does not read is `skipped_unparsed`, as is
//!    a call in a file with no parse.
//! 2. FORWARDING (depths 2..=[`MAX_DEPTH`]): a live call site `W(..)` of a
//!    wrapper found at the previous depth (the stage's own site rules:
//!    definitions and commented calls are not sites) whose argument at
//!    `W`'s `name_arg` is a bare identifier that is a parameter of the site's
//!    owner makes that owner a wrapper too, at that parameter's index
//!    (`NewNamedCollection(c, db, name)` forwarding into `NewCollection`).
//! 3. PRECISION: one wrapper per call name (the first found: depth, then
//!    walked file order, then source order). A name shorter than
//!    [`MIN_CALL_LEN`] bytes, or one that names two or more FUNCTION / METHOD
//!    nodes across the repo's Go parses, is dropped (`skipped_ambiguous`):
//!    the stage matches a call by name, so an ambiguous name would mint from
//!    another function's calls. A dropped wrapper forwards nothing.
//!
//! Only a direct hand-over of a parameter into a known driver needle (or
//! into another inferred wrapper) creates a wrapper: a one-hop syntactic
//! reading of a collection's name, the identity reading LA.4's const fold
//! does, never a value tracked through assignments. TypeScript / Java / C#
//! wrappers around the other needles are out of scope (every collection
//! stanza carries `languages = ["go"]`).
//!
//! Output sorted by (call, definition file). Each file's reading runs under
//! `catch_unwind`; a panic pushes `<path>: PANIC (inferred wrappers)`. The
//! `[wrappers] inferred` marker is `WrapperPass::report`'s.
//!
//! HTTP WRAPPERS (FH.4b), over the TS family (`typescript` / `angular` /
//! `vue`), Python and Go files, in walked order. An in-repo function that
//! hands its own parameter, unmodified or as one template hole, to the path
//! argument of an HTTP client call is a `kind = "http"` stanza: its literal
//! call sites mint the ENDPOINT the chain composes (`api(path, opts)` over
//! `` fetch(`${API_BASE}${path}`, { ...opts }) ``: `api("/users")` mints
//! `${API_BASE}/users`, which the endpoint fold resolves). No client library
//! is named here beyond what the parsers already mint on a literal:
//! 1. DIRECT: a client call is (a) in the TS family, every ENDPOINT the
//!    file's parse holds (the TS parser pushes one node per call site, an
//!    `<unresolved>` one for a non-literal URL), read back at its
//!    ENDPOINT_HIT `line` / `col` (whatever rule minted it: fetch, axios,
//!    HttpClient, the FH.4a shape rule), and (b) in Go and Python, whose
//!    parsers mint only on a literal URL, the client calls those parsers read
//!    ([`GO_NEEDLES`], [`PY_NEEDLES`]) with any argument. Its path is
//!    argument 0 (the URL argument of `http.NewRequest[WithContext]` /
//!    `.request(method, url)`); its method is the verb callee's, else the
//!    method argument, else the call's options argument (`{ method }`,
//!    `{ ...opts }`, `opts`, `lib.json({ method })`), else the hit's.
//!    The owner's declaration is read from the walked source over its
//!    POSITION span: Go as CA.4, Python `def`, TS / JS a function, method,
//!    arrow const or field; a destructured object parameter
//!    (`({ path, method }) =>`) names each property ([`ParamRef`]), and a
//!    Python `self` / `cls` takes no position.
//! 2. THE HOLE: the path argument is the owner's parameter itself, one
//!    `${p}` hole of a template literal or `{p}` of a Python f-string, or one
//!    `p` piece of a `+` concatenation. The text around it becomes the
//!    stanza's prefix / suffix (other holes and pieces as `${expr}`, the
//!    shape the endpoint fold reads). A hole right after `/`, `=`, `?`, `&`
//!    or `:` is a path parameter or a query value, not a forwarded path: no
//!    wrapper (`/users/${id}` keeps `getUser(id)` an ordinary function).
//! 3. THE METHOD: fixed (a verb callee, a literal `method`), forwarded (a
//!    `method` key or argument that is a parameter: [`MethodFrom::Arg`]) or
//!    read from the site's options argument ([`MethodFrom::Options`], with
//!    the inner call's method as the default).
//! 4. FORWARDING (depths 2..=[`MAX_DEPTH`]) and PRECISION as for the
//!    collection wrappers, per family, plus: a name that is an HTTP verb is
//!    dropped (`get(path)` would mint from every `map.get('/x')`).
//!
//! 5. PRECISION, FH.17 (TS family): a client call that is a verb member
//!    (`x.get(..)`, `x.delete(..)`) whose path the parser read as
//!    `<unresolved>` seeds nothing unless its receiver names an HTTP client
//!    ([`is_client_receiver`]: a word of `http` / `axios` / `client` / `api` /
//!    `fetch` / `request` / `rest` / ..): the CH.3c receiver gate keeps such a
//!    call when it cannot type the receiver, and a Map, store or cache whose
//!    type it could not see would otherwise make every `cache.get(key)` site
//!    an endpoint. Counted `phantom_dropped`. FH.17b: a receiver that is a
//!    default or named import (`import instance from '@/utils/request'`,
//!    `import { service as svc } from '../services/api'`) of a module whose
//!    specifier's last path words name a client ([`is_client_specifier`]:
//!    `request`, `http`, `api`, `client`, `axios`, `fetch` or `service`) is
//!    a client too ([`import_clients`]), counted `client_by_import`.
//! 6. OPTIONS OBJECTS, FH.17: a path argument that reads a `url` / `path` /
//!    `uri` key of a local object literal (`fetch(opts.url, opts)`,
//!    `axios(config)`), the object being the last `const` / `let` / `var` of
//!    that name in the owner's body before the call, holds the parameter
//!    when the object's value at that key does (step 2's hole rule); an
//!    options argument naming such an object gives its `method`
//!    ([`options_hole`], `options_object`). And a computed verb on a client
//!    (`client[method](`/rest${path}`)`, supertest's / axios's verb by value)
//!    is a client call whose method is the member's parameter
//!    ([`computed_verb_calls`], `computed_verb`).
//!
//! 7. HERITAGE, GD.26 (TS family): a direct or forwarding candidate whose
//!    owner is a METHOD of a CLASS C and that step 4's gate drops for its
//!    name (an HTTP verb, or one several callables share; never for its
//!    length) is a [`HeritageWrapper`] keyed (C, C's file, name), with the
//!    same shape; it forwards nothing. Its sites are not `<name>(` calls but
//!    `this.<name>(` / `super.<name>(` inside a class D's own lines
//!    ([`HeritageScope`], [`heritage_scopes`]): D is C, or D declares no
//!    member of that name and its `extends` base (a class of that name in
//!    D's file, else the classes so named in the files whose path's
//!    [`module_key`] is that of the specifier D imports the name from)
//!    reaches C, through in-repo intermediates up to [`MAX_HERITAGE_DEPTH`]
//!    hops. A member of that name on the way that is no heritage wrapper (an
//!    override) shadows it. Same-named copies of C (plane's three
//!    `APIService`s) all count: every chain must reach a wrapper, all of one
//!    shape, else the site is `ambiguous_base` and mints nothing. Because the
//!    wrapper must already hold a client call FH.17 keeps, a cache base class
//!    whose `get(key)` reads `this.store.get(key)` is no wrapper.
//!
//! The sites are read in `wrappers.rs` ([`read_http_site`]), minted with
//! EVIDENCE emitter `pass:inferred_wrapper` rule `wrapper_call`, Weak:
//! the path must be ONE literal starting with `/` or `http(s)://`, with no
//! whitespace, every `${..}` hole right after a `/`; the composed path is
//! `prefix + literal + suffix`. A site in test code (FH.17: the provenance
//! stamp's test paths, `_test.go`, `.spec.ts`, `test_*.py`, a test
//! directory, or the repo's declared `[walk] tests`) mints its ENDPOINT with
//! ORIGIN provenance `test_fixture` and a `"test_site":true` hit, which the
//! `http` pass pairs with nothing (a test helper drives the app's own
//! router, as photoprism's `PerformRequest(app, "GET", "/api/v1/..")` does).
//! A heritage site mints with rule `heritage_wrapper_call` and ORIGIN rule
//! `inferred:<class>.<verb>`. Counts: `[http-wrappers] wrappers=<w>
//! sites=<s> minted=<m> ambiguous=<a> test_sites=<t> phantom_dropped=<p>
//! client_by_import=<i> options_object=<o> computed_verb=<c> repo=<label>`,
//! with ` heritage=<classes> heritage_sites=<s> heritage_minted=<m>
//! ambiguous_base=<a> routes_released=<r>` before ` repo=` when a heritage
//! wrapper exists (`wrappers.rs`).

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use glia_code_domain::glia_config::{HTTP_METHODS, Origin, WrapperDecl};
use glia_code_domain::{FileParse, cell_type, edge_category, node_kind};
use glia_code_extractors::{anchor, data_entities, queue_topic};
use glia_core::{CellPayload, NodeId, RepoId};

use super::wrappers::NeedleSet;
use super::wrappers::{
    arg_region, commented_at, is_ident_byte, literal, live_call_args, skip_type_args, split_args,
};
use crate::build::memo::{BuildMemo, MemoRow, WrapperKey, parts_fp};
use crate::extract::{TS_FAMILY, detect_language};
use crate::route::ModuleQnames;

/// The one language the inference reads.
const LANG: &str = "go";

/// The deepest forwarding chain followed: the driver call's owner is depth 1.
pub(crate) const MAX_DEPTH: usize = 3;

/// Shortest wrapper name kept, in bytes.
pub(crate) const MIN_CALL_LEN: usize = 3;

/// The stanza's `flavor`: a driver collection is a NoSQL entity, the
/// extractor's namespace for every `COLLECTION_NEEDLES` call.
const FLAVOR: &str = "nosql";

/// One inferred wrapper: the stanza the wrapper stage reads, and where the
/// wrapper function is defined.
#[derive(Debug, Clone)]
pub(crate) struct InferredWrapper {
    /// `kind = "data_entity"`, `flavor = "nosql"`, `name_arg`,
    /// `languages = ["go"]`; its `origin` is not read for an inferred stanza.
    pub(crate) decl: WrapperDecl,
    /// The walked path of the file defining the wrapper.
    pub(crate) def_file: String,
    /// 0-based line of the wrapper's `func` keyword.
    pub(crate) def_line0: u32,
    /// 1 for a direct driver call, 2..=[`MAX_DEPTH`] for a forwarding one.
    pub(crate) depth: usize,
    /// FH.4b: an HTTP wrapper's composition; `None` for a collection one.
    pub(crate) http: Option<HttpShape>,
}

/// What [`infer_entity_wrappers`] found in one repo.
#[derive(Debug, Default)]
pub(crate) struct InferredWrappers {
    /// Sorted by (call, definition file).
    pub(crate) wrappers: Vec<InferredWrapper>,
    /// Candidates dropped by the precision gate (a short or ambiguous name).
    pub(crate) skipped_ambiguous: usize,
    /// Candidate calls whose owner's declaration did not read, or that sit
    /// in a file with no parse.
    pub(crate) skipped_unparsed: usize,
    /// FH.4b: the HTTP wrappers, sorted by (call, definition file).
    pub(crate) http: Vec<InferredWrapper>,
    /// FH.4b: HTTP candidates dropped by the precision gate.
    pub(crate) http_ambiguous: usize,
    /// FH.4b: client calls whose owner's declaration did not read.
    pub(crate) http_unparsed: usize,
    /// FH.17: TS `<unresolved>` verb calls on a receiver that is no HTTP
    /// client ([`is_client_receiver`]): no client call, so no candidate.
    pub(crate) http_phantom: usize,
    /// FH.17b: TS `<unresolved>` verb calls kept as client calls because
    /// their receiver is imported from a client module ([`import_clients`]).
    pub(crate) http_client_by_import: usize,
    /// FH.17: kept HTTP wrappers whose path reaches the client call through
    /// a local options object ([`options_hole`]).
    pub(crate) http_options_object: usize,
    /// FH.17: kept HTTP wrappers whose client call is a computed verb on a
    /// client (`client[method](path)`, [`computed_verb_calls`]).
    pub(crate) http_computed: usize,
    /// GD.26: the TS class members kept as heritage wrappers, sorted by
    /// (class, definition file, verb).
    pub(crate) heritage: Vec<HeritageWrapper>,
    /// GD.26: the TS classes whose `this.<verb>(` / `super.<verb>(` calls
    /// reach a [`HeritageWrapper`], in walked file order, then source order.
    pub(crate) heritage_scopes: Vec<HeritageScope>,
}

/// GD.26: an HTTP wrapper that is a member of a TS class C (`APIService.get(url)
/// { return this.axiosInstance.get(url, ..) }`), whose sites are the
/// `this.<verb>(` / `super.<verb>(` calls of C and of the classes extending
/// it ([`HeritageScope`]), never a bare `<verb>(` call. Kept where the plain
/// gate drops the member (an HTTP-verb name, or one several callables share).
#[derive(Debug, Clone)]
pub(crate) struct HeritageWrapper {
    /// C's name.
    pub(crate) class: String,
    /// The member's name, the `<verb>` a site calls on `this` / `super`.
    pub(crate) verb: String,
    /// The walked path of the file defining C.
    pub(crate) def_file: String,
    /// 0-based line of the member's declaration.
    pub(crate) def_line0: u32,
    /// The member's FH.4b reading: `path_arg`, prefix / suffix and method.
    pub(crate) shape: HttpShape,
    /// The `kind = "http"` stanza the wrapper stage reads its sites through.
    pub(crate) decl: WrapperDecl,
}

/// GD.26: what a `this.<verb>(` call in a [`HeritageScope`] reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeritageTarget {
    /// The [`InferredWrappers::heritage`] entry at this index: every base
    /// chain reaches a wrapper of one shape.
    Wrapper(usize),
    /// Some chain reaches a wrapper, another a non-wrapper member, no
    /// in-repo base or a wrapper of another shape: the site mints nothing.
    Ambiguous,
}

/// GD.26: one TS class D whose own lines hold heritage sites.
#[derive(Debug, Clone)]
pub(crate) struct HeritageScope {
    /// The walked path of D's file.
    pub(crate) file: String,
    /// D's POSITION span, 0-based lines, inclusive.
    pub(crate) start_line0: u32,
    pub(crate) end_line0: u32,
    /// The spans of classes nested in D: their lines are not D's.
    pub(crate) inner: Vec<(u32, u32)>,
    /// What `this.<verb>(` reaches, per verb, sorted by verb.
    pub(crate) targets: Vec<(String, HeritageTarget)>,
}

impl InferredWrappers {
    /// Wrappers that call the driver themselves.
    pub(crate) fn direct(&self) -> usize {
        self.wrappers.iter().filter(|w| w.depth == 1).count()
    }

    /// Wrappers that forward into another wrapper.
    pub(crate) fn forwarding(&self) -> usize {
        self.wrappers.iter().filter(|w| w.depth > 1).count()
    }
}

/// One walked Go file and its parse (found by its LB.9b MODULE id).
struct GoFile<'a> {
    path: &'a str,
    source: &'a str,
    parse: Option<&'a FileParse>,
}

impl<'a> GoFile<'a> {
    fn of(f: &CodeFile<'a>) -> Self {
        GoFile {
            path: f.path,
            source: f.source,
            parse: f.parse,
        }
    }
}

/// A wrapper before the precision gate.
#[derive(Clone)]
struct Candidate {
    call: String,
    name_arg: usize,
    def_file: String,
    def_line0: u32,
}

/// A FUNCTION / METHOD declaration as read from the source.
struct OwnerDecl {
    name: String,
    /// 0-based line of the `func` keyword.
    line0: u32,
    /// Parameter names in order; `""` for an item with no name token.
    params: Vec<String>,
}

/// Infer the repo's Go collection wrappers and (FH.4b) its HTTP wrappers
/// (see the module doc). Reads the parses and the walked sources; changes
/// nothing.
///
/// GA.21: with `memo` (the walked files' DC.4 content hashes, index-aligned
/// with `files`, and the held cache's build memos) every per-file reading
/// (the direct candidates, the heritage class record, each forwarding
/// depth's hits) is replayed from the memo's record of an unchanged file
/// (`build::memo::WrapperMemo`, keyed by [`WrapperKey`]) and only the
/// misses are computed, on the engine pool; the repo-wide steps (the
/// precision gate, the heritage join) run over the replayed and the fresh
/// readings alike, in walk order, so the result is the memo-free one. Sets
/// the memo's `wrappers_hit` / `wrappers_miss` (walked files of an
/// inference language). A hash list of another length disables the memo.
pub(crate) fn infer_entity_wrappers(
    parses_by_lang: &HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    modules: &ModuleQnames,
    repo: RepoId,
    memo: Option<(&[u64], &mut BuildMemo)>,
    parse_errors: &mut Vec<String>,
) -> InferredWrappers {
    let mut memo = memo.filter(|(hashes, _)| hashes.len() == files.len());
    let mut table = FileTable::open(
        parses_by_lang,
        files,
        modules,
        repo,
        memo.as_mut().map(|(h, m)| (*h, &mut **m)),
    );
    let mut out = infer_collections(&mut table, parses_by_lang, parse_errors);
    infer_http(&mut table, parses_by_lang, parse_errors, &mut out);
    if let Some((_, m)) = memo {
        table.close(m);
    }
    out
}

/// GA.21: one walked file's readings, the memo's per-file record
/// (`build::memo::WrapperMemo`). A field is `None` / short until a stage
/// computes it; a replayed record holds what the last build computed for
/// the same [`WrapperKey`].
#[derive(Default)]
struct WrapperFile {
    /// CA.4 step 1 (Go files).
    go_direct: Option<Reading<(Vec<Candidate>, usize)>>,
    /// FH.4b step 1 (every HTTP family).
    http_direct: Option<Reading<DirectHttp>>,
    /// CA.4 step 2, per depth from 2.
    go_forward: Vec<Forward<Candidate>>,
    /// FH.4b step 2, per depth from 2.
    http_forward: Vec<Forward<HttpCandidate>>,
    /// GD.26: the file's classes and value imports (TS family, read only
    /// when a heritage wrapper exists).
    classes: Option<ClassRecord>,
}

/// GA.21: one per-file reading.
#[derive(Clone)]
enum Reading<T> {
    /// The step's text gate kept it off the file.
    Gated,
    /// The reading panicked (replayed as the same `parse_errors` line).
    Panicked,
    Read(T),
}

/// GA.21: one `forwarding_*` reading: the candidates and the calls counted
/// unparsed.
type ForwardRead<C> = Reading<(Vec<C>, usize)>;

/// GA.21: one forwarding depth's hits in one file: the frontier calls (by
/// index) its text holds, each with its `forwarding_*` reading, valid for
/// the frontier whose fingerprint is `frontier` (`None`: a filler for a
/// depth this file was never read at, which matches no frontier).
struct Forward<C> {
    frontier: Option<u64>,
    hits: Vec<(usize, ForwardRead<C>)>,
}

/// GA.21: the walked files of every inference language, in walk order, with
/// their memo records.
struct FileTable<'a> {
    parses_by_lang: &'a HashMap<&'static str, Vec<FileParse>>,
    /// Each file's `parse` is `None` until [`FileTable::resolve`] finds it.
    files: Vec<CodeFile<'a>>,
    /// Per file: the index into [`HTTP_FAMILIES`] its language belongs to.
    family: Vec<usize>,
    /// Per file: its LB.9b MODULE id, which finds its parse.
    module_ids: Vec<NodeId>,
    resolved: Vec<bool>,
    /// Per family: MODULE id -> parse, built once a run resolves many files.
    by_family: Vec<Option<HashMap<NodeId, &'a FileParse>>>,
    recs: Vec<WrapperFile>,
    /// Per file: its memo key (`None` without a memo).
    keys: Vec<Option<WrapperKey>>,
    /// Per file: the path `String` of its last-run memo row, reused.
    row_paths: Vec<Option<String>>,
}

/// More misses than this in one batch build the family's MODULE id map;
/// fewer find each parse by a scan of the family's parses.
const SCAN_RESOLVE_MAX: usize = 64;

impl<'a> FileTable<'a> {
    /// Every walked file of an [`HTTP_FAMILIES`] language, and its record
    /// replayed from `memo` when its key matches.
    fn open(
        parses_by_lang: &'a HashMap<&'static str, Vec<FileParse>>,
        files: &'a [(String, String)],
        modules: &ModuleQnames,
        repo: RepoId,
        memo: Option<(&[u64], &mut BuildMemo)>,
    ) -> Self {
        let slots: Vec<usize> = (0..files.len()).collect();
        let (planned, _) = crate::parallel::par_map_ordered(&slots, |&i| {
            let (path, _) = &files[i];
            let lang = detect_language(path)?;
            let family = HTTP_FAMILIES.iter().position(|f| f.contains(&lang))?;
            Some((lang, family, modules.module_id(path, repo)))
        });
        let mut memo = memo;
        let mut rows = memo
            .as_mut()
            .and_then(|(_, m)| m.wrappers.take::<WrapperKey, WrapperFile>());
        let planned: Vec<(usize, (&'static str, usize, NodeId))> = planned
            .into_iter()
            .enumerate()
            .filter_map(|(i, p)| Some((i, p?)))
            .collect();
        // An unchanged walk hands out the rows by position.
        let aligned = rows.as_ref().is_some_and(|r| {
            let paths: Vec<&str> = planned.iter().map(|(i, _)| files[*i].0.as_str()).collect();
            r.aligned(&paths)
        });
        let n = planned.len();
        let mut table = FileTable {
            parses_by_lang,
            files: Vec::with_capacity(n),
            family: Vec::with_capacity(n),
            module_ids: Vec::with_capacity(n),
            resolved: Vec::with_capacity(n),
            by_family: HTTP_FAMILIES.iter().map(|_| None).collect(),
            recs: Vec::with_capacity(n),
            keys: Vec::with_capacity(n),
            row_paths: Vec::with_capacity(n),
        };
        let (mut hit, mut miss) = (0usize, 0usize);
        for (k, (i, (lang, family, module_id))) in planned.into_iter().enumerate() {
            let (path, source) = &files[i];
            let key = memo.as_ref().map(|(hashes, _)| WrapperKey {
                lang,
                content_hash: hashes.get(i).copied().unwrap_or(0),
                module_id,
            });
            let row = rows
                .as_mut()
                .and_then(|r| if aligned { r.row_at(k) } else { r.row(path) });
            let (rec, row_path) = match (row, key) {
                (Some(row), Some(k)) if row.key == k => {
                    hit += 1;
                    (row.value, Some(row.path))
                }
                (row, Some(_)) => {
                    miss += 1;
                    (WrapperFile::default(), row.map(|r| r.path))
                }
                (_, None) => (WrapperFile::default(), None),
            };
            table.files.push(CodeFile {
                path,
                source,
                lang,
                parse: None,
            });
            table.family.push(family);
            table.module_ids.push(module_id);
            table.resolved.push(false);
            table.recs.push(rec);
            table.keys.push(key);
            table.row_paths.push(row_path);
        }
        if let Some((_, m)) = memo {
            m.wrapper_stats.wrappers_hit = hit;
            m.wrapper_stats.wrappers_miss = miss;
        }
        table
    }

    /// The indices of the files of family `fi`, in walk order.
    fn members(&self, fi: usize) -> Vec<usize> {
        (0..self.files.len())
            .filter(|&i| self.family.get(i) == Some(&fi))
            .collect()
    }

    /// Find the parse of every file in `need` (the parse of its family whose
    /// first node is its MODULE id; the last such, as a map collected from
    /// the parses in order keeps). Only a file some reading recomputes needs
    /// one, so an unchanged tree finds none.
    fn resolve(&mut self, need: &[usize]) {
        let todo: Vec<usize> = need
            .iter()
            .copied()
            .filter(|&i| self.resolved.get(i) == Some(&false))
            .collect();
        if todo.is_empty() {
            return;
        }
        let parses_by_lang = self.parses_by_lang;
        let family_parses = |fi: usize| {
            HTTP_FAMILIES
                .get(fi)
                .into_iter()
                .flat_map(|family| family.iter())
                .filter_map(|l| parses_by_lang.get(l))
                .flatten()
        };
        for i in todo {
            let (Some(&fi), Some(&id)) = (self.family.get(i), self.module_ids.get(i)) else {
                continue;
            };
            let parse = if need.len() > SCAN_RESOLVE_MAX {
                let map = self.by_family.get_mut(fi).map(|m| {
                    m.get_or_insert_with(|| {
                        family_parses(fi)
                            .filter_map(|fp| fp.nodes.first().map(|n| (n.id, fp)))
                            .collect()
                    })
                });
                map.and_then(|m| m.get(&id).copied())
            } else {
                family_parses(fi)
                    .rev()
                    .find(|fp| fp.nodes.first().is_some_and(|n| n.id == id))
            };
            if let Some(f) = self.files.get_mut(i) {
                f.parse = parse;
            }
            if let Some(r) = self.resolved.get_mut(i) {
                *r = true;
            }
        }
    }

    /// Record every file's reading in `memo`, under its key, in walk order.
    fn close(self, memo: &mut BuildMemo) {
        let mut rows = Vec::with_capacity(self.files.len());
        for (((f, key), rec), path) in self
            .files
            .iter()
            .zip(self.keys)
            .zip(self.recs)
            .zip(self.row_paths)
        {
            if let Some(key) = key {
                rows.push(MemoRow {
                    path: path.unwrap_or_else(|| f.path.to_string()),
                    key,
                    value: rec,
                });
            }
        }
        memo.wrappers.put(rows);
    }
}

/// GA.21: compute the readings `need` lacks on the engine pool (input
/// order), each panic caught as [`Reading::Panicked`].
fn read_misses<T: Send>(
    files: &[CodeFile<'_>],
    need: &[usize],
    read: impl Fn(&CodeFile<'_>) -> Reading<T> + Sync + Send,
) -> Vec<Reading<T>> {
    crate::parallel::par_map_ordered(need, |&i| {
        files.get(i).map_or(Reading::Gated, |f| {
            crate::parallel::quiet(|| read(f)).unwrap_or(Reading::Panicked)
        })
    })
    .0
}

/// GA.21: one forwarding depth over the family `members`: for every file
/// whose record holds no reading for this frontier (`fp`), ONE pass over
/// its text finds the frontier calls it holds ([`NeedleSet`]: exactly the
/// `str::contains` set), each read with `read(file, call index)`, on the
/// pool; then every file's hits (replayed or fresh) in call-major order,
/// files in walk order: the order the per-call loop visited them, so the
/// gate admits the same candidates first.
fn forward_depth<C: Clone + Send>(
    table: &mut FileTable<'_>,
    members: &[usize],
    slot_of: fn(&mut WrapperFile) -> &mut Vec<Forward<C>>,
    depth: usize,
    calls: &[&str],
    fp: u64,
    read: impl Fn(&CodeFile<'_>, usize) -> (Vec<C>, usize) + Sync + Send,
) -> Vec<(usize, usize, ForwardRead<C>)> {
    let d = depth.saturating_sub(2);
    let need: Vec<usize> = members
        .iter()
        .copied()
        .filter(|&i| {
            table
                .recs
                .get_mut(i)
                .map(slot_of)
                .is_none_or(|v| v.get(d).is_none_or(|f| f.frontier != Some(fp)))
        })
        .collect();
    table.resolve(&need);
    let needles = NeedleSet::new(calls);
    let files = &table.files;
    let (fresh, _) = crate::parallel::par_map_ordered(&need, |&i| {
        let Some(f) = files.get(i) else {
            return Vec::new();
        };
        needles
            .present(f.source)
            .into_iter()
            .map(|c| {
                let r =
                    crate::parallel::quiet(|| read(f, c)).map_or(Reading::Panicked, Reading::Read);
                (c, r)
            })
            .collect()
    });
    for (i, hits) in need.iter().zip(fresh) {
        if let Some(v) = table.recs.get_mut(*i).map(slot_of) {
            v.truncate(d);
            v.resize_with(d, || Forward {
                frontier: None,
                hits: Vec::new(),
            });
            v.push(Forward {
                frontier: Some(fp),
                hits,
            });
        }
    }
    // `members` ascend in walk order, so (call, file index) is call-major,
    // files in walk order.
    let mut out = Vec::new();
    for &i in members {
        let Some(v) = table.recs.get_mut(i).map(slot_of) else {
            continue;
        };
        if let Some(f) = v.get(d) {
            for (c, r) in &f.hits {
                out.push((*c, i, r.clone()));
            }
        }
    }
    out.sort_by_key(|(c, i, _)| (*c, *i));
    out
}

/// GA.21: drop the forwarding readings past the depths this build ran, so a
/// record holds only what it was last read for.
fn trim_forward<C>(
    table: &mut FileTable<'_>,
    members: &[usize],
    slot_of: fn(&mut WrapperFile) -> &mut Vec<Forward<C>>,
    ran: usize,
) {
    for &i in members {
        if let Some(v) = table.recs.get_mut(i).map(slot_of) {
            v.truncate(ran);
        }
    }
}

/// The fingerprint of one depth's frontier: every call and what its sites
/// are read with, in order.
fn frontier_fp<T: std::fmt::Debug>(frontier: &[(String, T)]) -> u64 {
    let parts: Vec<String> = frontier
        .iter()
        .map(|(call, how)| format!("{call}\u{1}{how:?}"))
        .collect();
    parts_fp(parts.iter().map(String::as_str))
}

/// The Go collection wrappers (CA.4).
fn infer_collections(
    table: &mut FileTable<'_>,
    parses_by_lang: &HashMap<&'static str, Vec<FileParse>>,
    parse_errors: &mut Vec<String>,
) -> InferredWrappers {
    let mut out = InferredWrappers::default();
    let Some(go_family) = HTTP_FAMILIES.iter().position(|f| *f == [LANG]) else {
        return out;
    };
    let go = table.members(go_family);
    if go.is_empty() {
        return out;
    }
    let defined = defined_names(parses_by_lang);
    let mut seen: HashSet<String> = HashSet::new();

    // 1. DIRECT: every needle holds "ollection".
    let need: Vec<usize> = go
        .iter()
        .copied()
        .filter(|&i| table.recs.get(i).is_some_and(|r| r.go_direct.is_none()))
        .collect();
    table.resolve(&need);
    let fresh = read_misses(&table.files, &need, |f| {
        if f.source.contains("ollection") {
            Reading::Read(direct_candidates(&GoFile::of(f)))
        } else {
            Reading::Gated
        }
    });
    for (i, r) in need.iter().zip(fresh) {
        if let Some(rec) = table.recs.get_mut(*i) {
            rec.go_direct = Some(r);
        }
    }
    let mut found: Vec<Candidate> = Vec::new();
    for &i in &go {
        match table.recs.get(i).and_then(|r| r.go_direct.as_ref()) {
            Some(Reading::Read((cands, unparsed))) => {
                found.extend(cands.iter().cloned());
                out.skipped_unparsed += unparsed;
            }
            Some(Reading::Panicked) => {
                let path = table.files.get(i).map_or("", |f| f.path);
                parse_errors.push(format!("{path}: PANIC (inferred wrappers)"));
            }
            Some(Reading::Gated) | None => {}
        }
    }
    let mut frontier = admit(found, 1, &mut out, &mut seen, &defined);

    // 2. FORWARDING.
    let mut ran = 0usize;
    for depth in 2..=MAX_DEPTH {
        if frontier.is_empty() {
            break;
        }
        ran = depth - 1;
        let calls: Vec<&str> = frontier.iter().map(|(c, _)| c.as_str()).collect();
        let hits = forward_depth(
            table,
            &go,
            |r| &mut r.go_forward,
            depth,
            &calls,
            frontier_fp(&frontier),
            |f, c| {
                let (call, name_arg) = &frontier[c];
                forwarding_candidates(&GoFile::of(f), call, *name_arg)
            },
        );
        let mut found: Vec<Candidate> = Vec::new();
        for (_, i, r) in hits {
            match r {
                Reading::Read((cands, unparsed)) => {
                    found.extend(cands);
                    out.skipped_unparsed += unparsed;
                }
                Reading::Panicked => {
                    let path = table.files.get(i).map_or("", |f| f.path);
                    parse_errors.push(format!("{path}: PANIC (inferred wrappers)"));
                }
                Reading::Gated => {}
            }
        }
        frontier = admit(found, depth, &mut out, &mut seen, &defined);
    }
    trim_forward(table, &go, |r| &mut r.go_forward, ran);

    out.wrappers.sort_by(|a, b| {
        (a.decl.call.as_str(), a.def_file.as_str())
            .cmp(&(b.decl.call.as_str(), b.def_file.as_str()))
    });
    out
}

/// How many FUNCTION / METHOD nodes of the repo's Go parses carry each name.
fn defined_names<'a>(
    parses_by_lang: &'a HashMap<&'static str, Vec<FileParse>>,
) -> HashMap<&'a str, usize> {
    defined_names_in(parses_by_lang, &[LANG])
}

/// [`defined_names`] over the parses of every language in `langs`. GA.21:
/// each parse's callable names are read on the engine pool, then counted
/// here (a count does not depend on the order).
fn defined_names_in<'a>(
    parses_by_lang: &'a HashMap<&'static str, Vec<FileParse>>,
    langs: &[&str],
) -> HashMap<&'a str, usize> {
    let parses: Vec<&FileParse> = parses_by_lang
        .iter()
        .filter(|(lang, _)| langs.contains(lang))
        .flat_map(|(_, ps)| ps)
        .collect();
    let (names, _) = crate::parallel::par_map_ordered(&parses, |fp| {
        fp.nodes
            .iter()
            .filter(|n| {
                fp.nav
                    .kind_by_id
                    .get(&n.id)
                    .is_some_and(|k| *k == node_kind::FUNCTION || *k == node_kind::METHOD)
            })
            .filter_map(|n| fp.nav.name_by_id.get(&n.id).map(String::as_str))
            .collect::<Vec<&str>>()
    });
    let mut out: HashMap<&str, usize> = HashMap::new();
    for name in names.into_iter().flatten() {
        *out.entry(name).or_default() += 1;
    }
    out
}

/// The precision gate (module doc, step 3): the kept candidates join
/// `out.wrappers` at `depth`; returns their `(call, name_arg)`, the next
/// depth's frontier.
fn admit(
    found: Vec<Candidate>,
    depth: usize,
    out: &mut InferredWrappers,
    seen: &mut HashSet<String>,
    defined: &HashMap<&str, usize>,
) -> Vec<(String, usize)> {
    let mut frontier = Vec::new();
    for c in found {
        if !seen.insert(c.call.clone()) {
            continue;
        }
        if c.call.len() < MIN_CALL_LEN || defined.get(c.call.as_str()).copied().unwrap_or(0) >= 2 {
            out.skipped_ambiguous += 1;
            continue;
        }
        frontier.push((c.call.clone(), c.name_arg));
        out.wrappers.push(InferredWrapper {
            decl: entity_decl(c.call, c.name_arg),
            def_file: c.def_file,
            def_line0: c.def_line0,
            depth,
            http: None,
        });
    }
    frontier
}

/// The stanza an inferred wrapper stands for.
fn entity_decl(call: String, name_arg: usize) -> WrapperDecl {
    WrapperDecl {
        call,
        receiver: false,
        kind: "data_entity".to_string(),
        method: None,
        method_arg: None,
        path_arg: None,
        topic_arg: None,
        broker: None,
        flavor: Some(FLAVOR.to_string()),
        name_arg: Some(name_arg),
        languages: vec![LANG.to_string()],
        origin: Origin::default(),
    }
}

/// Step 1 over one file: the candidates, and the calls counted unparsed.
fn direct_candidates(f: &GoFile<'_>) -> (Vec<Candidate>, usize) {
    let calls: Vec<(usize, String)> = data_entities::collection_param_calls(f.source)
        .into_iter()
        .filter(|(at, _)| !commented_at(f.source, *at))
        .collect();
    owner_candidates(f, calls)
}

/// Step 2 over one file: the live sites of `call` whose argument at
/// `name_arg` is a bare identifier.
fn forwarding_candidates(f: &GoFile<'_>, call: &str, name_arg: usize) -> (Vec<Candidate>, usize) {
    let sites: Vec<(usize, String)> = live_call_args(f.source, call)
        .into_iter()
        .filter_map(|(at, args)| {
            let arg = args.get(name_arg)?.trim();
            is_ident(arg).then(|| (at, arg.to_string()))
        })
        .collect();
    owner_candidates(f, sites)
}

/// For each `(offset, identifier)`: when the identifier is a parameter of
/// the FUNCTION / METHOD holding the offset, that function is a candidate at
/// the parameter's index.
fn owner_candidates(f: &GoFile<'_>, calls: Vec<(usize, String)>) -> (Vec<Candidate>, usize) {
    let mut out = Vec::new();
    if calls.is_empty() {
        return (out, 0);
    }
    let Some(fp) = f.parse else {
        return (out, calls.len());
    };
    let idx = anchor::build_owner_index(&fp.nodes, &fp.nav);
    let mut unparsed = 0usize;
    for (at, ident) in calls {
        let Some(owner) = anchor::owner_of_line(&idx, anchor::line_of(f.source, at)) else {
            continue;
        };
        let Some(decl) = owner_decl(f.source, fp, owner) else {
            unparsed += 1;
            continue;
        };
        if let Some(i) = decl.params.iter().position(|p| *p == ident) {
            out.push(Candidate {
                call: decl.name,
                name_arg: i,
                def_file: f.path.to_string(),
                def_line0: decl.line0,
            });
        }
    }
    (out, unparsed)
}

/// The declaration of `owner`, read from `source` over its POSITION span:
/// the first line there that starts with `func` and declares the owner's
/// name.
fn owner_decl(source: &str, fp: &FileParse, owner: NodeId) -> Option<OwnerDecl> {
    let name = fp.nav.name_by_id.get(&owner)?;
    let node = fp.nodes.iter().find(|n| n.id == owner)?;
    let (start, end) = anchor::position_span(node)?;
    let mut offset = 0usize;
    for (i, line) in source.split_inclusive('\n').enumerate() {
        let line0 = u32::try_from(i).ok()?;
        if line0 > end {
            break;
        }
        let trimmed = line.trim_start();
        if line0 >= start
            && trimmed
                .strip_prefix("func")
                .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '('))
        {
            let at = offset + (line.len() - trimmed.len());
            if let Some((fname, params)) = go_func_signature(source, at)
                && fname == *name
            {
                return Some(OwnerDecl {
                    name: fname,
                    line0,
                    params,
                });
            }
        }
        offset += line.len();
    }
    None
}

/// `func [(<receiver>)] <name>[\[<type params>\]](<params>)` starting at
/// `at` (the `func` keyword): the name and the parameter names. `None` for a
/// function literal or a list that does not close.
fn go_func_signature(source: &str, at: usize) -> Option<(String, Vec<String>)> {
    let b = source.as_bytes();
    let skip_ws = |mut k: usize| {
        while b.get(k).is_some_and(u8::is_ascii_whitespace) {
            k += 1;
        }
        k
    };
    let mut k = skip_ws(at + "func".len());
    if b.get(k) == Some(&b'(') {
        k = skip_ws(arg_region(source, k + 1).1? + 1);
    }
    let name_start = k;
    while b
        .get(k)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
    {
        k += 1;
    }
    let name = source.get(name_start..k)?;
    if name.is_empty() {
        return None;
    }
    k = skip_ws(k);
    if b.get(k) == Some(&b'[') {
        k = skip_ws(arg_region(source, k + 1).1? + 1);
    }
    if b.get(k) != Some(&b'(') {
        return None;
    }
    let (region, close) = arg_region(source, k + 1);
    close?;
    let mut params: Vec<String> = split_args(region).into_iter().map(param_name).collect();
    // `()` and a trailing comma leave one empty last item.
    if params.last().is_some_and(String::is_empty) {
        params.pop();
    }
    Some((name.to_string(), params))
}

/// A parameter item's name: its first token, after dropping `//` comments.
fn param_name(item: &str) -> String {
    let text: String = item
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    text.trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// A bare Go identifier.
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// FH.4b: HTTP wrappers.
// ---------------------------------------------------------------------------

/// The language families an HTTP wrapper is inferred within: a wrapper's
/// forwarding sites, its name's ambiguity and its stanza's `languages` are
/// its own family's.
const HTTP_FAMILIES: &[&[&str]] = &[TS_FAMILY, &["python"], &["go"]];

/// Longest callee chain read before a client call's `(`, in bytes.
const MAX_CALLEE: usize = 256;

/// One parameter of a wrapper: its position at a call site, and the property
/// when the parameter is a destructured object (`({ path, method }) => ..`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParamRef {
    pub(crate) index: usize,
    pub(crate) key: Option<String>,
}

/// Where an inferred HTTP wrapper's site takes its method from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MethodFrom {
    /// A verb callee, or a literal `method` (upper-case).
    Fixed(String),
    /// The site's literal at this parameter (`request(method, path)`,
    /// `{ method }` handed on).
    Arg(ParamRef),
    /// The site's options object at this parameter: its literal `method`
    /// key, else `default` (the inner call's own method).
    Options { at: ParamRef, default: String },
}

/// How an inferred HTTP wrapper composes a site's ENDPOINT path:
/// `prefix + <the site's literal> + suffix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpShape {
    pub(crate) path: ParamRef,
    /// The text around the forwarded path, other holes as `${expr}`.
    pub(crate) prefix: String,
    pub(crate) suffix: String,
    pub(crate) method: MethodFrom,
}

/// One walked file of an HTTP family and its parse.
struct CodeFile<'a> {
    path: &'a str,
    source: &'a str,
    lang: &'static str,
    parse: Option<&'a FileParse>,
}

/// An HTTP wrapper before the precision gate.
#[derive(Clone)]
struct HttpCandidate {
    call: String,
    shape: HttpShape,
    def_file: String,
    def_line0: u32,
    /// FH.17: how the direct reading reached the client call.
    via: Via,
    /// GD.26: the TS CLASS the owner is a METHOD of ([`method_class`]).
    class: Option<String>,
}

/// FH.17: how a direct HTTP candidate's path reaches its client call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    /// The path argument itself (FH.4b), or a forwarding site.
    Path,
    /// A local object literal's `url` / `path` / `uri` key ([`options_hole`]).
    Options,
    /// A computed verb on a client ([`computed_verb_calls`]).
    Computed,
}

/// A callable's parameters: local name, and where a site passes it.
type Params = Vec<(String, ParamRef)>;

/// A FUNCTION / METHOD declaration read for the HTTP inference.
struct HttpOwner {
    name: String,
    line0: u32,
    params: Params,
}

/// How one client call takes its method.
enum CallMethod<'s> {
    /// The callee names it (`axios.get`, `http.Post`, `requests.put`).
    Verb(&'static str),
    /// It is this argument (`http.NewRequest(method, url, nil)`).
    Arg(usize),
    /// The options argument after the path, else the parser's own reading.
    Options(String),
    /// FH.17: the computed member of `client[<name>](..)`: the method when
    /// `<name>` is a parameter of the owner.
    Name(&'s str),
}

/// One HTTP client call in a file.
struct ClientCall<'s> {
    /// Byte offset of the callee: the owner and the comment rule read it.
    at: usize,
    args: Vec<&'s str>,
    path_arg: usize,
    method: CallMethod<'s>,
}

/// Which receiver a client needle's member is read on.
#[derive(Clone, Copy)]
enum Recv {
    /// The Go `net/http` package itself.
    GoPkg,
    /// `http` or a `*client` variable (Go).
    GoPkgOrClient,
    /// A `*client` variable (Go).
    GoClient,
    /// The Python parser's client receivers (`requests`, `httpx`, `session`,
    /// `client`, `api`, `*client`, `*session`).
    PyClient,
}

/// A Go / Python client call shape, the one the language's parser mints on
/// a literal URL, read here with any argument.
struct Needle {
    member: &'static str,
    recv: Recv,
    path_arg: usize,
    /// `Some(verb)`: the member names the method; `None`: argument
    /// `method_arg`.
    verb: Option<&'static str>,
    method_arg: usize,
}

const fn needle(member: &'static str, recv: Recv, verb: &'static str) -> Needle {
    Needle {
        member,
        recv,
        path_arg: 0,
        verb: Some(verb),
        method_arg: 0,
    }
}

const GO_NEEDLES: &[Needle] = &[
    needle("Get", Recv::GoPkgOrClient, "GET"),
    needle("Head", Recv::GoPkgOrClient, "HEAD"),
    needle("Post", Recv::GoPkgOrClient, "POST"),
    needle("PostForm", Recv::GoPkgOrClient, "POST"),
    needle("Put", Recv::GoClient, "PUT"),
    needle("Patch", Recv::GoClient, "PATCH"),
    needle("Delete", Recv::GoClient, "DELETE"),
    Needle {
        member: "NewRequest",
        recv: Recv::GoPkg,
        path_arg: 1,
        verb: None,
        method_arg: 0,
    },
    Needle {
        member: "NewRequestWithContext",
        recv: Recv::GoPkg,
        path_arg: 2,
        verb: None,
        method_arg: 1,
    },
];

const PY_NEEDLES: &[Needle] = &[
    needle("get", Recv::PyClient, "GET"),
    needle("post", Recv::PyClient, "POST"),
    needle("put", Recv::PyClient, "PUT"),
    needle("patch", Recv::PyClient, "PATCH"),
    needle("delete", Recv::PyClient, "DELETE"),
    needle("head", Recv::PyClient, "HEAD"),
    needle("options", Recv::PyClient, "OPTIONS"),
    Needle {
        member: "request",
        recv: Recv::PyClient,
        path_arg: 1,
        verb: None,
        method_arg: 0,
    },
];

impl Recv {
    fn accepts(self, recv: &str) -> bool {
        let lower = recv.trim_start_matches('_').to_ascii_lowercase();
        match self {
            Recv::GoPkg => recv == "http",
            Recv::GoPkgOrClient => recv == "http" || lower.ends_with("client"),
            Recv::GoClient => lower.ends_with("client"),
            Recv::PyClient => {
                matches!(
                    lower.as_str(),
                    "requests" | "httpx" | "http" | "session" | "client" | "api"
                ) || lower.ends_with("client")
                    || lower.ends_with("session")
            }
        }
    }
}

/// The HTTP half of [`infer_entity_wrappers`] (module doc, HTTP WRAPPERS).
fn infer_http(
    table: &mut FileTable<'_>,
    parses_by_lang: &HashMap<&'static str, Vec<FileParse>>,
    parse_errors: &mut Vec<String>,
    out: &mut InferredWrappers,
) {
    for (fi, family) in HTTP_FAMILIES.iter().enumerate() {
        let code = table.members(fi);
        if code.is_empty() {
            continue;
        }
        let defined = defined_names_in(parses_by_lang, family);
        let languages: Vec<String> = family.iter().map(|l| (*l).to_string()).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let need: Vec<usize> = code
            .iter()
            .copied()
            .filter(|&i| table.recs.get(i).is_some_and(|r| r.http_direct.is_none()))
            .collect();
        table.resolve(&need);
        let fresh = read_misses(&table.files, &need, |f| Reading::Read(direct_http(f)));
        for (i, r) in need.iter().zip(fresh) {
            if let Some(rec) = table.recs.get_mut(*i) {
                rec.http_direct = Some(r);
            }
        }
        let mut found: Vec<HttpCandidate> = Vec::new();
        for &i in &code {
            match table.recs.get(i).and_then(|r| r.http_direct.as_ref()) {
                Some(Reading::Read(d)) => {
                    found.extend(d.cands.iter().cloned());
                    out.http_unparsed += d.unparsed;
                    out.http_phantom += d.gate.phantom;
                    out.http_client_by_import += d.gate.by_import;
                }
                Some(Reading::Panicked) => {
                    let path = table.files.get(i).map_or("", |f| f.path);
                    parse_errors.push(format!("{path}: PANIC (inferred wrappers)"));
                }
                Some(Reading::Gated) | None => {}
            }
        }
        let heritage = family.contains(&"typescript");
        let mut frontier = admit_http(found, 1, out, &mut seen, &defined, &languages, heritage);
        let mut ran = 0usize;
        for depth in 2..=MAX_DEPTH {
            if frontier.is_empty() {
                break;
            }
            ran = depth - 1;
            let calls: Vec<&str> = frontier.iter().map(|(c, _)| c.as_str()).collect();
            let hits = forward_depth(
                table,
                &code,
                |r| &mut r.http_forward,
                depth,
                &calls,
                frontier_fp(&frontier),
                |f, c| {
                    let (call, shape) = &frontier[c];
                    forwarding_http(f, call, shape)
                },
            );
            let mut found: Vec<HttpCandidate> = Vec::new();
            for (_, i, r) in hits {
                match r {
                    Reading::Read((cands, unparsed)) => {
                        found.extend(cands);
                        out.http_unparsed += unparsed;
                    }
                    Reading::Panicked => {
                        let path = table.files.get(i).map_or("", |f| f.path);
                        parse_errors.push(format!("{path}: PANIC (inferred wrappers)"));
                    }
                    Reading::Gated => {}
                }
            }
            frontier = admit_http(found, depth, out, &mut seen, &defined, &languages, heritage);
        }
        trim_forward(table, &code, |r| &mut r.http_forward, ran);
        if heritage && !out.heritage.is_empty() {
            out.heritage.sort_by(|a, b| {
                (a.class.as_str(), a.def_file.as_str(), a.verb.as_str()).cmp(&(
                    b.class.as_str(),
                    b.def_file.as_str(),
                    b.verb.as_str(),
                ))
            });
            let need: Vec<usize> = code
                .iter()
                .copied()
                .filter(|&i| table.recs.get(i).is_some_and(|r| r.classes.is_none()))
                .collect();
            table.resolve(&need);
            let fresh = read_misses(&table.files, &need, |f| Reading::Read(class_record(f)));
            let mut panicked = false;
            for (i, r) in need.iter().zip(fresh) {
                match r {
                    Reading::Read(c) => {
                        if let Some(rec) = table.recs.get_mut(*i) {
                            rec.classes = Some(c);
                        }
                    }
                    Reading::Panicked | Reading::Gated => panicked = true,
                }
            }
            let joined = (!panicked)
                .then(|| {
                    let files: Vec<(&CodeFile<'_>, &ClassRecord)> = code
                        .iter()
                        .filter_map(|&i| {
                            Some((table.files.get(i)?, table.recs.get(i)?.classes.as_ref()?))
                        })
                        // A file with no class holds no scope and no base.
                        .filter(|(_, r)| !r.classes.is_empty())
                        .collect();
                    catch_unwind(AssertUnwindSafe(|| heritage_scopes(&files, &out.heritage))).ok()
                })
                .flatten();
            match joined {
                Some(scopes) => out.heritage_scopes = scopes,
                None => parse_errors.push("<heritage classes>: PANIC (inferred wrappers)".into()),
            }
        }
    }
    out.http.sort_by(|a, b| {
        (a.decl.call.as_str(), a.def_file.as_str())
            .cmp(&(b.decl.call.as_str(), b.def_file.as_str()))
    });
}

/// The HTTP precision gate: [`admit`]'s rules, plus a name that is an HTTP
/// verb is dropped (`get(path)` would mint from every `map.get("/x")`).
///
/// GD.26: in the TS family (`heritage`), a candidate that is a METHOD of a
/// CLASS and that the gate would drop for its verb name or its ambiguity
/// (not its length) is kept as a [`HeritageWrapper`] instead, one per
/// (class, file, name); it forwards nothing.
fn admit_http(
    found: Vec<HttpCandidate>,
    depth: usize,
    out: &mut InferredWrappers,
    seen: &mut HashSet<String>,
    defined: &HashMap<&str, usize>,
    languages: &[String],
    heritage: bool,
) -> Vec<(String, HttpShape)> {
    let mut frontier = Vec::new();
    for c in found {
        let dropped = HTTP_METHODS.iter().any(|m| m.eq_ignore_ascii_case(&c.call))
            || defined.get(c.call.as_str()).copied().unwrap_or(0) >= 2;
        if let Some(class) = c.class.as_ref().filter(|_| heritage)
            && dropped
            && c.call.len() >= MIN_CALL_LEN
        {
            if !out
                .heritage
                .iter()
                .any(|h| h.class == *class && h.def_file == c.def_file && h.verb == c.call)
            {
                out.heritage.push(HeritageWrapper {
                    class: class.clone(),
                    verb: c.call.clone(),
                    decl: http_decl(c.call.clone(), &c.shape, languages),
                    def_file: c.def_file,
                    def_line0: c.def_line0,
                    shape: c.shape,
                });
            }
            continue;
        }
        if !seen.insert(c.call.clone()) {
            continue;
        }
        if c.call.len() < MIN_CALL_LEN || dropped {
            out.http_ambiguous += 1;
            continue;
        }
        frontier.push((c.call.clone(), c.shape.clone()));
        match c.via {
            Via::Path => {}
            Via::Options => out.http_options_object += 1,
            Via::Computed => out.http_computed += 1,
        }
        out.http.push(InferredWrapper {
            decl: http_decl(c.call, &c.shape, languages),
            def_file: c.def_file,
            def_line0: c.def_line0,
            depth,
            http: Some(c.shape),
        });
    }
    frontier
}

/// The `kind = "http"` stanza an inferred HTTP wrapper stands for; the
/// stage reads its sites through the [`HttpShape`].
fn http_decl(call: String, shape: &HttpShape, languages: &[String]) -> WrapperDecl {
    let (method, method_arg) = match &shape.method {
        MethodFrom::Fixed(m) => (Some(m.clone()), None),
        MethodFrom::Arg(r) | MethodFrom::Options { at: r, .. } => (None, Some(r.index)),
    };
    WrapperDecl {
        call,
        receiver: false,
        kind: "http".to_string(),
        method,
        method_arg,
        path_arg: Some(shape.path.index),
        topic_arg: None,
        broker: None,
        flavor: None,
        name_arg: None,
        languages: languages.to_vec(),
        origin: Origin::default(),
    }
}

/// What step 1 found in one file.
#[derive(Clone)]
struct DirectHttp {
    cands: Vec<HttpCandidate>,
    /// Client calls whose owner's declaration did not read.
    unparsed: usize,
    /// FH.17 / FH.17b: the phantom rule's counts.
    gate: PhantomGate,
}

/// What the FH.17 phantom rule did in one file: `<unresolved>` verb calls on
/// no HTTP client, left out, and (FH.17b) those kept because their receiver
/// is imported from a client module.
#[derive(Debug, Default, Clone, Copy)]
struct PhantomGate {
    phantom: usize,
    by_import: usize,
}

/// Step 1 over one file: every client call whose path is a parameter hole of
/// its owner, directly or (FH.17) through a local options object.
fn direct_http(f: &CodeFile<'_>) -> DirectHttp {
    let (calls, gate) = client_calls(f);
    let mut out = DirectHttp {
        cands: Vec::new(),
        unparsed: 0,
        gate,
    };
    if calls.is_empty() {
        return out;
    }
    let Some(fp) = f.parse else {
        out.unparsed = calls.len();
        return out;
    };
    let idx = anchor::build_owner_index(&fp.nodes, &fp.nav);
    for call in calls {
        if commented_at(f.source, call.at) {
            continue;
        }
        let Some(owner) = anchor::owner_of_line(&idx, anchor::line_of(f.source, call.at)) else {
            continue;
        };
        let Some(decl) = http_owner(f, fp, owner) else {
            out.unparsed += 1;
            continue;
        };
        let Some(expr) = call.args.get(call.path_arg) else {
            continue;
        };
        // FH.17: the text the owner's body holds before the call, where a
        // local options object is declared.
        let body = line_offset(f.source, usize::try_from(decl.line0).unwrap_or(usize::MAX))
            .and_then(|from| f.source.get(from..call.at))
            .unwrap_or("");
        let (path, prefix, suffix, mut via) = match param_hole(expr, &decl.params) {
            Some((p, pre, suf)) => (p, pre, suf, Via::Path),
            None => match options_hole(body, expr, &decl.params) {
                Some((p, pre, suf)) => (p, pre, suf, Via::Options),
                None => continue,
            },
        };
        if matches!(call.method, CallMethod::Name(_)) {
            via = Via::Computed;
        }
        let Some(method) = call_method(&call, &decl.params, body) else {
            continue;
        };
        out.cands.push(HttpCandidate {
            call: decl.name,
            shape: HttpShape {
                path,
                prefix,
                suffix,
                method,
            },
            def_file: f.path.to_string(),
            def_line0: decl.line0,
            via,
            class: method_class(f, fp, owner),
        });
    }
    out
}

/// FH.17: the keys of an options object that carry the request's path.
const PATH_KEYS: &[&str] = &["url", "path", "uri"];

/// FH.17, options-object builders: `expr` (a client call's path argument)
/// reads a [`PATH_KEYS`] key of a local object literal (`opts.url`,
/// `opts?.path`, `opts["url"]`), or is that object itself (`axios(config)`),
/// and the object's value at that key holds the owner's parameter as
/// [`param_hole`] reads it. The object is the LAST `const` / `let` / `var
/// <name> [: T] = { .. }` declared in `body` (the owner's text before the
/// call). One hop: an object assembled from another variable is not read.
fn options_hole(body: &str, expr: &str, params: &Params) -> Option<(ParamRef, String, String)> {
    let e = expr.trim();
    let (name, keys): (&str, Vec<&str>) = match member_key(e) {
        Some((name, key)) => (name, vec![key]),
        None if !e.is_empty() && leading_ident(e) == e => (e, PATH_KEYS.to_vec()),
        None => return None,
    };
    if !keys.iter().all(|k| PATH_KEYS.contains(k)) {
        return None;
    }
    let props = split_args(local_object(body, name)?);
    keys.iter()
        .find_map(|k| prop_value(&props, k))
        .and_then(|v| param_hole(v, params))
}

/// `name.key`, `name?.key` or `name["key"]` / `name['key']`: the name and the
/// key.
fn member_key(e: &str) -> Option<(&str, &str)> {
    let name = leading_ident(e);
    if name.is_empty() {
        return None;
    }
    let rest = e.get(name.len()..)?;
    if let Some(k) = rest.strip_prefix("?.").or_else(|| rest.strip_prefix('.')) {
        return (!k.is_empty() && leading_ident(k) == k).then_some((name, k));
    }
    let inner = rest.strip_prefix('[')?.strip_suffix(']')?;
    let lit = literal(inner).filter(|l| l.quote != b':' && l.quote != b'`')?;
    (lit.body.len() + 2 == inner.trim().len()).then_some((name, lit.body))
}

/// FH.17: the inner text (between the braces) of the object literal the LAST
/// `const` / `let` / `var <name> [: T] = {` in `body` assigns.
fn local_object<'b>(body: &'b str, name: &str) -> Option<&'b str> {
    let b = body.as_bytes();
    let mut found = None;
    for (at, _) in body.match_indices(name) {
        let after = at + name.len();
        if b.get(after).is_some_and(|c| is_ident_byte(*c)) {
            continue;
        }
        let head = body.get(..at)?.trim_end();
        let declared = ["const", "let", "var"].iter().any(|kw| {
            head.strip_suffix(kw).is_some_and(|h| {
                h.is_empty() || !h.as_bytes().last().is_some_and(|c| is_ident_byte(*c))
            })
        });
        if !declared {
            continue;
        }
        let rest = body.get(after..)?;
        let eq = rest.find('=')?;
        let ty = rest.get(..eq)?.trim();
        if !(ty.is_empty() || ty.starts_with(':')) || rest.get(eq + 1..)?.starts_with(['=', '>']) {
            continue;
        }
        let value = rest.get(eq + 1..)?;
        let lead = value.len() - value.trim_start().len();
        if !value.trim_start().starts_with('{') {
            continue;
        }
        let open = after + eq + 1 + lead;
        let (inner, close) = arg_region(body, open + 1);
        if close.is_some() {
            found = Some(inner);
        }
    }
    found
}

/// Step 2 over one file: the live non-literal sites of the wrapper `call`
/// whose path is a parameter hole of the site's owner.
fn forwarding_http(f: &CodeFile<'_>, call: &str, shape: &HttpShape) -> (Vec<HttpCandidate>, usize) {
    let mut out = Vec::new();
    let sites: Vec<(usize, Vec<&str>)> = live_call_args(f.source, call)
        .into_iter()
        .filter(|(_, args)| {
            // A plain literal is a minting site; a template or f-string with
            // a hole may forward.
            arg_value(args, &shape.path)
                .is_some_and(|v| literal(v).is_none_or(|l| l.body.contains('{')))
        })
        .collect();
    if sites.is_empty() {
        return (out, 0);
    }
    let Some(fp) = f.parse else {
        return (out, sites.len());
    };
    let idx = anchor::build_owner_index(&fp.nodes, &fp.nav);
    let mut unparsed = 0usize;
    for (at, args) in sites {
        let Some(owner) = anchor::owner_of_line(&idx, anchor::line_of(f.source, at)) else {
            continue;
        };
        let Some(decl) = http_owner(f, fp, owner) else {
            unparsed += 1;
            continue;
        };
        if decl.name == call {
            continue;
        }
        let Some(v) = arg_value(&args, &shape.path) else {
            continue;
        };
        let Some((path, prefix, suffix)) = param_hole(v, &decl.params) else {
            continue;
        };
        let method = match method_at(&shape.method, &args, Some(&decl.params)) {
            MethodAt::Known(m) => MethodFrom::Fixed(m),
            MethodAt::Forward(m) => m,
            MethodAt::Unknown | MethodAt::Invalid => continue,
        };
        out.push(HttpCandidate {
            call: decl.name,
            shape: HttpShape {
                path,
                prefix: format!("{}{prefix}", shape.prefix),
                suffix: format!("{suffix}{}", shape.suffix),
                method,
            },
            def_file: f.path.to_string(),
            def_line0: decl.line0,
            via: Via::Path,
            class: method_class(f, fp, owner),
        });
    }
    (out, unparsed)
}

/// The file's HTTP client calls, in source order, and (FH.17) the TS
/// `<unresolved>` verb calls left out as phantoms: for the TS family every
/// ENDPOINT its parse holds, read back at its ENDPOINT_HIT `line` / `col`
/// (the TS parser pushes one ENDPOINT node per call site), plus its computed
/// verbs on a client ([`computed_verb_calls`]); for Go and Python the
/// [`GO_NEEDLES`] / [`PY_NEEDLES`] shapes with any argument.
fn client_calls<'s>(f: &CodeFile<'s>) -> (Vec<ClientCall<'s>>, PhantomGate) {
    let (mut out, gate) = match f.lang {
        "go" => (needle_calls(f.source, GO_NEEDLES), PhantomGate::default()),
        "python" => (needle_calls(f.source, PY_NEEDLES), PhantomGate::default()),
        _ => {
            let (mut calls, gate) = f
                .parse
                .map(|fp| endpoint_calls(f.source, fp))
                .unwrap_or_default();
            calls.extend(computed_verb_calls(f.source));
            (calls, gate)
        }
    };
    out.sort_by_key(|c| c.at);
    out.dedup_by_key(|c| c.at);
    (out, gate)
}

/// TS family: the client call at each ENDPOINT node's hit, and (FH.17) how
/// many were left out as phantoms: a verb call (`x.get(..)`) whose path the
/// parser read as `<unresolved>` and whose receiver is no HTTP client
/// ([`is_client_receiver`]) is the CH.3c receiver gate's fail-open guess (a
/// Map, a store or a cache whose type the parser could not see), not a
/// client call: it seeds no wrapper. `fetch(..)`, a non-verb member (the
/// FH.4a shape rule's `lib.fetchJson(..)`) and every read path still seed,
/// and so (FH.17b) does a verb call whose whole receiver is a name the file
/// imports from a client module ([`import_clients`]).
fn endpoint_calls<'s>(source: &'s str, fp: &FileParse) -> (Vec<ClientCall<'s>>, PhantomGate) {
    let mut out = Vec::new();
    let mut gate = PhantomGate::default();
    let mut imported: Option<Vec<&str>> = None;
    for n in fp
        .nodes
        .iter()
        .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
    {
        let unresolved = fp
            .nav
            .qname_by_id
            .get(&n.id)
            .is_some_and(|q| q.ends_with(UNRESOLVED));
        for c in n.cells.iter().filter(|c| c.kind == cell_type::ENDPOINT_HIT) {
            let CellPayload::Json(j) = &c.payload else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(j) else {
                continue;
            };
            let num = |k: &str| {
                v.get(k)
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|x| usize::try_from(x).ok())
            };
            let (Some(line), Some(col), Some(method)) = (
                num("line"),
                num("col"),
                v.get("method").and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            let Some(at) =
                line_offset(source, line.saturating_sub(1)).map(|o| o + col.saturating_sub(1))
            else {
                continue;
            };
            let Some((callee, open)) = call_head(source, at) else {
                continue;
            };
            let (receiver, member) = match callee.rsplit_once('.') {
                Some((r, m)) => (Some(r.trim_end_matches(['?', '!'])), m),
                None => (None, callee),
            };
            let member = member.trim_matches(['?', '!']);
            let method = match HTTP_METHODS.iter().find(|m| m.eq_ignore_ascii_case(member)) {
                Some(m) => CallMethod::Verb(m),
                None => CallMethod::Options(method.to_ascii_uppercase()),
            };
            if unresolved
                && matches!(method, CallMethod::Verb(_))
                && !receiver.is_some_and(|r| is_client_receiver(last_segment(r)))
            {
                let clients = imported.get_or_insert_with(|| import_clients(source));
                if receiver.is_some_and(|r| clients.contains(&r)) {
                    gate.by_import += 1;
                } else {
                    gate.phantom += 1;
                    continue;
                }
            }
            out.push(ClientCall {
                at,
                args: split_args(arg_region(source, open + 1).0),
                path_arg: 0,
                method,
            });
        }
    }
    (out, gate)
}

/// The path segment the parser writes for a client call it could not read.
const UNRESOLVED: &str = "<unresolved>";

/// The last `.`-separated identifier of a callee chain (`this.http` ->
/// `http`).
fn last_segment(chain: &str) -> &str {
    chain
        .rsplit('.')
        .next()
        .unwrap_or(chain)
        .trim_matches(['?', '!'])
}

/// FH.17: the words that make a receiver an HTTP client: the client
/// libraries the parsers name (fetch, axios, ky, got, superagent, jQuery,
/// `requests`) and the names an app gives one (`http`, `client`, `api`,
/// `rest`, `request`).
const CLIENT_WORDS: &[&str] = &[
    "http",
    "https",
    "axios",
    "client",
    "api",
    "fetch",
    "fetcher",
    "request",
    "requests",
    "rest",
    "ky",
    "got",
    "superagent",
    "xhr",
    "jquery",
];

/// FH.17: a receiver identifier names an HTTP client: one of its words
/// (split at `_` / `$` / `-` and at camel-case humps, lower-cased) is a
/// [`CLIENT_WORDS`] entry (`http`, `this.apiClient`, `restClient`,
/// `HTTPService`), or it is jQuery's `$`. `entries`, `cache`, `store`,
/// `itemsById` name none.
fn is_client_receiver(recv: &str) -> bool {
    recv == "$"
        || words(recv)
            .iter()
            .any(|w| CLIENT_WORDS.contains(&w.as_str()))
}

/// An identifier's words: split at `_` / `$` / `-` / digits and at
/// camel-case humps (`HTTPClient` -> `http`, `client`), lower-cased.
fn words(ident: &str) -> Vec<String> {
    let chars: Vec<char> = ident.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphabetic() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = i.checked_sub(1).and_then(|k| chars.get(k));
        let next = chars.get(i + 1);
        let hump = c.is_ascii_uppercase()
            && prev.is_some_and(|p| {
                p.is_ascii_lowercase()
                    || (p.is_ascii_uppercase() && next.is_some_and(char::is_ascii_lowercase))
            });
        if hump && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// FH.17b: the words of an import specifier's last path segment that make
/// the module an HTTP client's (`@/utils/request`, `./http-client`,
/// `@/lib/apiService`), a trailing `s` dropped (`services`, `requests`).
const CLIENT_MODULE_WORDS: &[&str] = &[
    "request", "http", "api", "client", "axios", "fetch", "service",
];

/// The code file extensions an import specifier may spell out.
const CODE_EXTS: &[&str] = &[
    ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".vue",
];

/// FH.17b: an import specifier names a client module: a word of its last
/// path segment, a code extension dropped (the segment before it when that
/// is `index`), is a [`CLIENT_MODULE_WORDS`] entry, as written or without a
/// trailing `s` (`@/utils/request`, `./http-client.ts`, `./user.service`).
/// `@/store`, `./cache/index.ts` and `lodash` name none.
fn is_client_specifier(spec: &str) -> bool {
    let Some(last) = module_key(spec) else {
        return false;
    };
    words(last).iter().any(|w| {
        let w = w.as_str();
        CLIENT_MODULE_WORDS.contains(&w)
            || w.strip_suffix('s')
                .is_some_and(|s| CLIENT_MODULE_WORDS.contains(&s))
    })
}

/// The module an import specifier or a file path names: its last path
/// segment with a code extension dropped, the segment before it when that is
/// `index` (`@/services/api.service` and `services/api.service.ts` ->
/// `api.service`, `./lib/index.ts` -> `lib`); `None` for an empty one.
fn module_key(spec: &str) -> Option<&str> {
    let spec = spec.split(['?', '#']).next().unwrap_or(spec);
    let mut segs = spec
        .trim_end_matches('/')
        .rsplit('/')
        .map(|s| {
            CODE_EXTS
                .iter()
                .find_map(|e| s.strip_suffix(e))
                .unwrap_or(s)
        })
        .filter(|s| !s.is_empty() && *s != "." && *s != "..");
    let last = segs.next()?;
    if last == "index" {
        return segs.next();
    }
    Some(last)
}

/// FH.17b, TS family: the local names the file binds by a default or named
/// value import (`import instance from '..'`, `import { a, b as c } from
/// '..'`, `import d, { e } from '..'`) of a client module
/// ([`is_client_specifier`]), in source order. A type-only import or
/// specifier binds no value, a namespace import (`* as ns`) is not read, an
/// import in a comment is skipped, and a name the file also declares with
/// `const` / `let` / `var` / `function` / `class` (a local that may shadow
/// it) is left out.
fn import_clients(source: &str) -> Vec<&str> {
    /// The longest `import` clause read, in bytes: a `from` further on is
    /// another statement's.
    const IMPORT_WINDOW: usize = 4096;
    let b = source.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    for (at, _) in source.match_indices("import") {
        if at > 0
            && b.get(at - 1)
                .is_some_and(|c| is_ident_byte(*c) || *c == b'.')
        {
            continue;
        }
        let rest = &source[at + "import".len()..];
        if !rest.starts_with(|c: char| c.is_whitespace() || c == '{') || commented_at(source, at) {
            continue;
        }
        let window = &rest[..rest.floor_char_boundary(IMPORT_WINDOW)];
        let Some(from) = find_word(window, "from") else {
            continue;
        };
        let clause = rest[..from].trim();
        if clause.is_empty()
            || clause.contains(['(', ';', '\'', '"', '`'])
            || word_at(clause, "type")
        {
            continue;
        }
        let after = rest[from + "from".len()..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '\'' || *c == '"') else {
            continue;
        };
        let Some(end) = after[1..].find(q) else {
            continue;
        };
        if !is_client_specifier(&after[1..1 + end]) {
            continue;
        }
        let (default, named) = match clause.find('{') {
            Some(open) => {
                let close = clause[open..].find('}').map_or(clause.len(), |c| open + c);
                (&clause[..open], &clause[open + 1..close])
            }
            None => (clause, ""),
        };
        let default = default.trim().trim_end_matches(',').trim();
        if !default.is_empty() && !default.starts_with('*') && is_js_ident(default) {
            out.push(default);
        }
        for spec in named.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if word_at(spec, "type") {
                continue;
            }
            let local = spec.rsplit_once(" as ").map_or(spec, |(_, l)| l).trim();
            if is_js_ident(local) {
                out.push(local);
            }
        }
    }
    out.retain(|name| !declared_locally(source, name));
    out
}

/// `text` starts with the keyword `kw` followed by whitespace.
fn word_at(text: &str, kw: &str) -> bool {
    text.strip_prefix(kw)
        .is_some_and(|r| r.starts_with(char::is_whitespace))
}

/// The byte offset of the first `kw` in `text` that stands as a word.
fn find_word(text: &str, kw: &str) -> Option<usize> {
    let b = text.as_bytes();
    text.match_indices(kw).map(|(i, _)| i).find(|&i| {
        (i == 0 || b.get(i - 1).is_some_and(|c| !is_ident_byte(*c)))
            && b.get(i + kw.len()).is_some_and(|c| !is_ident_byte(*c))
    })
}

/// A whole JS identifier.
fn is_js_ident(s: &str) -> bool {
    !s.is_empty() && !s.starts_with(|c: char| c.is_ascii_digit()) && s.bytes().all(is_ident_byte)
}

/// The file declares `name` itself (`const name`, `let name`, `var name`,
/// `function name`, `class name`), so a use of it may not be the import.
fn declared_locally(source: &str, name: &str) -> bool {
    ["const", "let", "var", "function", "class"]
        .iter()
        .any(|kw| {
            let b = source.as_bytes();
            source.match_indices(kw).any(|(i, _)| {
                if i > 0
                    && b.get(i - 1)
                        .is_some_and(|c| is_ident_byte(*c) || *c == b'.')
                {
                    return false;
                }
                let rest = &source[i + kw.len()..];
                let trimmed = rest.trim_start();
                trimmed.len() < rest.len()
                    && trimmed.strip_prefix(name).is_some_and(|r| {
                        !r.starts_with(|c: char| c.is_ascii() && is_ident_byte(c as u8))
                    })
            })
        })
}

/// FH.17, TS family: every live `<recv>[<name>](..)` call whose receiver
/// (the identifier before `[`) is an HTTP client ([`is_client_receiver`])
/// and whose computed member is a bare identifier (`client[method](path)`,
/// supertest's and axios's verb by value). Its path is argument 0, its
/// method the member, read as a parameter of the owner
/// ([`CallMethod::Name`]). At most `queue_topic::MAX_HITS_PER_NEEDLE`.
fn computed_verb_calls(source: &str) -> Vec<ClientCall<'_>> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    for (open_bracket, _) in source.match_indices('[') {
        if out.len() >= queue_topic::MAX_HITS_PER_NEEDLE {
            break;
        }
        let mut s = open_bracket;
        while s > 0 && b.get(s - 1).is_some_and(|c| is_ident_byte(*c)) {
            s -= 1;
        }
        let Some(recv) = source.get(s..open_bracket).filter(|r| !r.is_empty()) else {
            continue;
        };
        let Some(close) = source
            .get(open_bracket + 1..)
            .and_then(|r| r.find(']'))
            .map(|k| open_bracket + 1 + k)
        else {
            continue;
        };
        let Some(name) = source.get(open_bracket + 1..close).map(str::trim) else {
            continue;
        };
        if name.is_empty()
            || leading_ident(name) != name
            || b.get(close + 1) != Some(&b'(')
            || !is_client_receiver(recv)
            || commented_at(source, s)
        {
            continue;
        }
        out.push(ClientCall {
            at: s,
            args: split_args(arg_region(source, close + 2).0),
            path_arg: 0,
            method: CallMethod::Name(name),
        });
    }
    out
}

/// Byte offset of the start of 0-based line `line0`.
fn line_offset(source: &str, line0: usize) -> Option<usize> {
    if line0 == 0 {
        return Some(0);
    }
    source
        .match_indices('\n')
        .nth(line0 - 1)
        .map(|(i, _)| i + 1)
}

/// The callee chain starting at `at` (after an optional `await`) and the
/// offset of its `(`: identifiers joined by `.` / `?.` / `!.` (across line
/// breaks before a `.`), an optional generic list, then `(`. `None` for any
/// other shape (a call on a call's result, `axios.create().get(..)`).
fn call_head(source: &str, at: usize) -> Option<(&str, usize)> {
    let b = source.as_bytes();
    let skip_ws = |mut k: usize| {
        while b.get(k).is_some_and(u8::is_ascii_whitespace) {
            k += 1;
        }
        k
    };
    let mut k = at;
    if source.get(k..).is_some_and(|r| r.starts_with("await"))
        && b.get(k + 5).is_some_and(u8::is_ascii_whitespace)
    {
        k = skip_ws(k + 5);
    }
    let start = k;
    let limit = b.len().min(start + MAX_CALLEE);
    while k < limit {
        let c = b[k];
        if is_ident_byte(c) || matches!(c, b'.' | b'?' | b'!') {
            k += 1;
        } else if c.is_ascii_whitespace() && matches!(b.get(skip_ws(k)), Some(b'.' | b'?')) {
            k = skip_ws(k);
        } else {
            break;
        }
    }
    let callee = source.get(start..k)?;
    if callee.is_empty() {
        return None;
    }
    if b.get(k) == Some(&b'<') {
        k = skip_type_args(b, k)?;
    }
    (b.get(k) == Some(&b'(')).then_some((callee, k))
}

/// Every live `<recv>.<member>(` call of `needles` whose receiver (the last
/// identifier before the `.`) the needle accepts, at most
/// `queue_topic::MAX_HITS_PER_NEEDLE` per needle.
fn needle_calls<'s>(source: &'s str, needles: &[Needle]) -> Vec<ClientCall<'s>> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    for n in needles {
        let pat = format!(".{}(", n.member);
        let mut taken = 0usize;
        for (dot, _) in source.match_indices(pat.as_str()) {
            if taken >= queue_topic::MAX_HITS_PER_NEEDLE {
                break;
            }
            let mut s = dot;
            while s > 0 && b.get(s - 1).is_some_and(|c| is_ident_byte(*c)) {
                s -= 1;
            }
            let Some(recv) = source.get(s..dot).filter(|r| !r.is_empty()) else {
                continue;
            };
            if !n.recv.accepts(recv) || commented_at(source, s) {
                continue;
            }
            taken += 1;
            let open = dot + pat.len() - 1;
            out.push(ClientCall {
                at: s,
                args: split_args(arg_region(source, open + 1).0),
                path_arg: n.path_arg,
                method: match n.verb {
                    Some(v) => CallMethod::Verb(v),
                    None => CallMethod::Arg(n.method_arg),
                },
            });
        }
    }
    out
}

/// The declaration of `owner`, read from the walked source over its
/// POSITION span, by the file's language.
fn http_owner(f: &CodeFile<'_>, fp: &FileParse, owner: NodeId) -> Option<HttpOwner> {
    if f.lang == "go" {
        let d = owner_decl(f.source, fp, owner)?;
        let params = d
            .params
            .into_iter()
            .enumerate()
            .map(|(index, name)| (name, ParamRef { index, key: None }))
            .collect();
        return Some(HttpOwner {
            name: d.name,
            line0: d.line0,
            params,
        });
    }
    let name = fp.nav.name_by_id.get(&owner)?;
    if name.is_empty()
        || name.as_bytes().first().is_some_and(u8::is_ascii_digit)
        || !name.bytes().all(is_ident_byte)
    {
        return None;
    }
    let node = fp.nodes.iter().find(|n| n.id == owner)?;
    let (start, end) = anchor::position_span(node)?;
    let from = line_offset(f.source, usize::try_from(start).ok()?)?;
    let to = line_offset(f.source, usize::try_from(end).ok()? + 1).unwrap_or(f.source.len());
    let text = f.source.get(from..to)?;
    let b = f.source.as_bytes();
    for (k, _) in text.match_indices(name.as_str()) {
        let at = from + k;
        let after = at + name.len();
        if at
            .checked_sub(1)
            .and_then(|i| b.get(i))
            .is_some_and(|c| is_ident_byte(*c))
            || b.get(after).is_some_and(|c| is_ident_byte(*c))
        {
            continue;
        }
        let params = if f.lang == "python" {
            py_params_after(f.source, at)
        } else {
            ts_params_after(f.source, after)
        };
        if let Some(params) = params {
            return Some(HttpOwner {
                name: name.clone(),
                line0: anchor::line_of(f.source, at),
                params,
            });
        }
    }
    None
}

/// GD.26: the name of the TS CLASS that `owner` is a METHOD of (its nav
/// parent); `None` outside the TS family or for any other owner.
fn method_class(f: &CodeFile<'_>, fp: &FileParse, owner: NodeId) -> Option<String> {
    if !TS_FAMILY.contains(&f.lang) || fp.nav.kind_by_id.get(&owner) != Some(&node_kind::METHOD) {
        return None;
    }
    let class = fp.nav.parent_of.get(&owner)?;
    (fp.nav.kind_by_id.get(class) == Some(&node_kind::CLASS))
        .then(|| fp.nav.name_by_id.get(class).cloned())
        .flatten()
        .filter(|n| !n.is_empty())
}

/// GD.26: the deepest `extends` chain followed from a class to the class
/// defining a heritage wrapper.
pub(crate) const MAX_HERITAGE_DEPTH: usize = 4;

/// GD.26: one TS-family CLASS as [`heritage_scopes`] reads it.
struct ClassInfo<'a> {
    /// Index into the walked code files.
    file: usize,
    name: &'a str,
    span: (u32, u32),
    /// The `extends` clause's bare base name (its INHERITS_FROM ref).
    extends: Option<&'a str>,
    /// The names of its nav children (methods, fields, accessors).
    members: &'a HashSet<String>,
}

/// GD.26: one TS-family file's heritage reading.
struct ClassFile<'a> {
    /// The module its path names ([`module_key`]).
    key: Option<&'a str>,
    /// `(local name, imported name, specifier)` per named or default value
    /// import; a default import's imported name is its local name.
    imports: &'a [(String, String, String)],
}

/// GA.21: one file's GD.26 class record, read off its own parse alone
/// ([`class_record`]), so the memo replays it for an unchanged file.
#[derive(Default)]
struct ClassRecord {
    /// [`ClassFile::imports`], owned.
    imports: Vec<(String, String, String)>,
    /// The file's named CLASS nodes with a POSITION span, in node order.
    classes: Vec<ClassRow>,
}

/// GA.21: one CLASS of a [`ClassRecord`] ([`ClassInfo`], owned).
struct ClassRow {
    name: String,
    span: (u32, u32),
    extends: Option<String>,
    /// Looked up, never iterated.
    members: HashSet<String>,
}

/// GA.21: the GD.26 reading of one file: its value imports and its classes
/// (empty without a parse).
fn class_record(f: &CodeFile<'_>) -> ClassRecord {
    let mut rec = ClassRecord::default();
    let Some(fp) = f.parse else {
        return rec;
    };
    for imp in &fp.imports {
        if let glia_code_domain::ImportTarget::Symbol {
            module,
            name,
            alias,
            ..
        } = &imp.target
        {
            let local = alias.as_deref().unwrap_or(name.as_str());
            let imported = if name == "default" {
                local
            } else {
                name.as_str()
            };
            rec.imports
                .push((local.to_string(), imported.to_string(), module.clone()));
        }
    }
    for n in &fp.nodes {
        if fp.nav.kind_by_id.get(&n.id) != Some(&node_kind::CLASS) {
            continue;
        }
        let (Some(name), Some(span)) = (fp.nav.name_by_id.get(&n.id), anchor::position_span(n))
        else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let extends = fp.refs.iter().find_map(|r| match &r.qualifier {
            glia_code_domain::CallQualifier::Bare(b)
                if r.from == n.id && r.category == edge_category::INHERITS_FROM =>
            {
                Some(b.clone())
            }
            _ => None,
        });
        let members = fp
            .nav
            .children_of
            .get(&n.id)
            .into_iter()
            .flatten()
            .filter_map(|c| fp.nav.name_by_id.get(c).cloned())
            .collect();
        rec.classes.push(ClassRow {
            name: name.clone(),
            span,
            extends,
            members,
        });
    }
    rec
}

/// What one base chain from a class reaches for a verb.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// A heritage wrapper (index into the wrappers).
    Wrapper(usize),
    /// A member of that name that is no heritage wrapper (an override).
    Shadow,
    /// No member of that name before the chain ends (no in-repo base, or
    /// [`MAX_HERITAGE_DEPTH`]).
    End,
}

/// GD.26: the classes [`heritage_scopes`] reads, with its lookups (never
/// iterated).
struct ClassIndex<'i, 'a> {
    classes: &'i [ClassInfo<'a>],
    files: &'i [ClassFile<'a>],
    code: &'i [(&'a CodeFile<'a>, &'a ClassRecord)],
    by_name: &'i HashMap<&'a str, Vec<usize>>,
    /// (class, file, verb) -> index into the heritage wrappers.
    wrapper_at: &'i HashMap<(&'a str, &'a str, &'a str), usize>,
}

impl ClassIndex<'_, '_> {
    /// The `extends` base(s) of class `ci`: the classes of that name in its
    /// file, else the classes of the imported name in the files whose
    /// [`module_key`] is the import specifier's.
    fn bases(&self, ci: usize) -> Vec<usize> {
        let Some(c) = self.classes.get(ci) else {
            return Vec::new();
        };
        let Some(base) = c.extends else {
            return Vec::new();
        };
        let named = |n: &str| self.by_name.get(n).map_or(&[][..], Vec::as_slice);
        let local: Vec<usize> = named(base)
            .iter()
            .copied()
            .filter(|&b| b != ci && self.classes.get(b).is_some_and(|x| x.file == c.file))
            .collect();
        if !local.is_empty() {
            return local;
        }
        let Some((imported, spec)) = self.files.get(c.file).and_then(|f| {
            f.imports
                .iter()
                .find(|(l, _, _)| l == base)
                .map(|(_, i, s)| (i.as_str(), s.as_str()))
        }) else {
            return Vec::new();
        };
        let Some(key) = module_key(spec) else {
            return Vec::new();
        };
        named(imported)
            .iter()
            .copied()
            .filter(|&b| {
                b != ci
                    && self
                        .classes
                        .get(b)
                        .and_then(|x| self.files.get(x.file))
                        .is_some_and(|f| f.key == Some(key))
            })
            .collect()
    }

    /// What `this.<verb>(` in class `ci` reaches along each base chain,
    /// `depth` hops from the calling class.
    fn reach(&self, ci: usize, verb: &str, depth: usize, out: &mut Vec<Reach>) {
        let Some(c) = self.classes.get(ci) else {
            out.push(Reach::End);
            return;
        };
        if c.members.contains(verb) {
            let path = self.code.get(c.file).map_or("", |(f, _)| f.path);
            out.push(
                self.wrapper_at
                    .get(&(c.name, path, verb))
                    .map_or(Reach::Shadow, |w| Reach::Wrapper(*w)),
            );
            return;
        }
        let bs = if depth < MAX_HERITAGE_DEPTH {
            self.bases(ci)
        } else {
            Vec::new()
        };
        if bs.is_empty() {
            out.push(Reach::End);
            return;
        }
        for b in bs {
            self.reach(b, verb, depth + 1, out);
        }
    }
}

/// GD.26: every TS-family CLASS D of `code` whose `this.<verb>(` reaches a
/// heritage wrapper: D itself defines the verb as a wrapper, or D declares no
/// member of that name and its `extends` base does, through in-repo
/// intermediates up to [`MAX_HERITAGE_DEPTH`] hops. A base named N is a
/// class named N declared in D's file, else the classes named N (or, for a
/// default import, the local name) in the files whose [`module_key`] is the
/// key of the specifier D's file imports N from; several such copies all
/// count. Every chain must reach a wrapper, all of one [`HttpShape`]
/// ([`HeritageTarget::Wrapper`], the least index); a chain reaching a
/// non-wrapper member, no base or another shape beside one that reaches a
/// wrapper is [`HeritageTarget::Ambiguous`]; a class none of whose chains
/// reaches a wrapper is no scope.
fn heritage_scopes<'a>(
    code: &'a [(&'a CodeFile<'a>, &'a ClassRecord)],
    wrappers: &[HeritageWrapper],
) -> Vec<HeritageScope> {
    let mut classes: Vec<ClassInfo<'_>> = Vec::new();
    let mut files: Vec<ClassFile<'_>> = Vec::with_capacity(code.len());
    for (fi, (f, rec)) in code.iter().enumerate() {
        files.push(ClassFile {
            key: module_key(f.path),
            imports: &rec.imports,
        });
        for c in &rec.classes {
            classes.push(ClassInfo {
                file: fi,
                name: &c.name,
                span: c.span,
                extends: c.extends.as_deref(),
                members: &c.members,
            });
        }
    }
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in classes.iter().enumerate() {
        by_name.entry(c.name).or_default().push(i);
    }
    let wrapper_at: HashMap<(&str, &str, &str), usize> = wrappers
        .iter()
        .enumerate()
        .map(|(i, w)| ((w.class.as_str(), w.def_file.as_str(), w.verb.as_str()), i))
        .collect();
    let mut verbs: Vec<&str> = wrappers.iter().map(|w| w.verb.as_str()).collect();
    verbs.sort_unstable();
    verbs.dedup();

    let idx = ClassIndex {
        classes: &classes,
        files: &files,
        code,
        by_name: &by_name,
        wrapper_at: &wrapper_at,
    };
    // GA.21: a class with no `extends` reaches a wrapper only through its
    // own member, so one that defines none ends every chain: skipped, as
    // its `reach` would find nothing.
    let defines: HashSet<(&str, &str)> = wrappers
        .iter()
        .map(|w| (w.class.as_str(), w.def_file.as_str()))
        .collect();
    let mut out = Vec::new();
    for (ci, c) in classes.iter().enumerate() {
        let path = code.get(c.file).map_or("", |(f, _)| f.path);
        if c.extends.is_none() && !defines.contains(&(c.name, path)) {
            continue;
        }
        let mut targets: Vec<(String, HeritageTarget)> = Vec::new();
        for verb in &verbs {
            let mut reached = Vec::new();
            idx.reach(ci, verb, 0, &mut reached);
            let found: Vec<usize> = reached
                .iter()
                .filter_map(|r| match r {
                    Reach::Wrapper(w) => Some(*w),
                    Reach::Shadow | Reach::End => None,
                })
                .collect();
            let Some(&first) = found.iter().min() else {
                continue;
            };
            let shape = wrappers.get(first).map(|w| &w.shape);
            let one = found.len() == reached.len()
                && found
                    .iter()
                    .all(|w| wrappers.get(*w).map(|x| &x.shape) == shape);
            targets.push((
                (*verb).to_string(),
                if one {
                    HeritageTarget::Wrapper(first)
                } else {
                    HeritageTarget::Ambiguous
                },
            ));
        }
        if targets.is_empty() {
            continue;
        }
        let (start, end) = c.span;
        let inner = classes
            .iter()
            .enumerate()
            .filter(|(j, x)| {
                *j != ci
                    && x.file == c.file
                    && start <= x.span.0
                    && x.span.1 <= end
                    && x.span != c.span
            })
            .map(|(_, x)| x.span)
            .collect();
        out.push(HeritageScope {
            file: code.get(c.file).map_or("", |(f, _)| f.path).to_string(),
            start_line0: start,
            end_line0: end,
            inner,
            targets,
        });
    }
    out
}

/// The leading identifier of `s` (not starting with a digit).
fn leading_ident(s: &str) -> &str {
    if s.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return "";
    }
    let n = s.bytes().take_while(|c| is_ident_byte(*c)).count();
    s.get(..n).unwrap_or("")
}

/// TypeScript / JavaScript: the parameters declared right after the name at
/// `k`: `name(..)` / `name<T>(..)` (a function or a method), or
/// `name = [async] (..) =>` / `= [async] function [n](..)` / `= p =>` (a
/// const or a field; `name:` in an object literal alike).
fn ts_params_after(source: &str, k: usize) -> Option<Params> {
    let b = source.as_bytes();
    let skip_ws = |mut k: usize| {
        while b.get(k).is_some_and(u8::is_ascii_whitespace) {
            k += 1;
        }
        k
    };
    let word = |k: usize, w: &str| {
        source.get(k..).is_some_and(|r| r.starts_with(w))
            && !b.get(k + w.len()).is_some_and(|c| is_ident_byte(*c))
    };
    let mut k = skip_ws(k);
    if b.get(k) == Some(&b'?') {
        k = skip_ws(k + 1);
    }
    let assigned = match (b.get(k), b.get(k + 1)) {
        (Some(b'='), Some(c)) if !matches!(c, b'=' | b'>') => true,
        (Some(b':'), Some(c)) if *c != b':' => true,
        _ => false,
    };
    if assigned {
        k = skip_ws(k + 1);
        if word(k, "async") {
            k = skip_ws(k + "async".len());
        }
        if word(k, "function") {
            k = skip_ws(k + "function".len());
            if b.get(k) == Some(&b'*') {
                k = skip_ws(k + 1);
            }
            k = skip_ws(k + leading_ident(source.get(k..)?).len());
        } else if b.get(k) != Some(&b'(') && b.get(k) != Some(&b'<') {
            let p = leading_ident(source.get(k..)?);
            let rest = source.get(k + p.len()..)?.trim_start();
            return (!p.is_empty() && rest.starts_with("=>")).then(|| {
                vec![(
                    p.to_string(),
                    ParamRef {
                        index: 0,
                        key: None,
                    },
                )]
            });
        }
    }
    if b.get(k) == Some(&b'<') {
        k = skip_ws(skip_type_args(b, k)?);
    }
    if b.get(k) != Some(&b'(') {
        return None;
    }
    let (region, close) = arg_region(source, k + 1);
    close?;
    Some(ts_params(region))
}

/// A TypeScript parameter list: plain, defaulted, rest and destructured
/// (`{ path, method: m = "GET" }`) items; a `this:` item takes no position.
fn ts_params(region: &str) -> Params {
    const MODIFIERS: &[&str] = &["public", "private", "protected", "readonly", "override"];
    let mut out = Params::new();
    let mut index = 0usize;
    for item in split_args(region) {
        let mut t = item.trim();
        while let Some(m) = MODIFIERS.iter().find(|m| {
            t.strip_prefix(**m)
                .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace()))
        }) {
            t = t.get(m.len()..).unwrap_or("").trim_start();
        }
        let t = t.strip_prefix("...").unwrap_or(t).trim_start();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('{') {
            let (inner, _) = arg_region(t, 1);
            for prop in split_args(inner) {
                let p = prop.trim();
                let key = leading_ident(p);
                if key.is_empty() || p.starts_with("...") {
                    continue;
                }
                let rest = p.get(key.len()..).unwrap_or("").trim_start();
                let local = match rest.strip_prefix(':') {
                    Some(v) if !v.starts_with(':') => {
                        let v = v.trim_start();
                        let l = leading_ident(v);
                        let tail = v.get(l.len()..).unwrap_or("").trim_start();
                        if l.is_empty() || !(tail.is_empty() || tail.starts_with('=')) {
                            continue;
                        }
                        l
                    }
                    _ => key,
                };
                out.push((
                    local.to_string(),
                    ParamRef {
                        index,
                        key: Some(key.to_string()),
                    },
                ));
            }
        } else {
            let name = leading_ident(t);
            if name == "this" {
                continue;
            }
            if !name.is_empty() {
                out.push((name.to_string(), ParamRef { index, key: None }));
            }
        }
        index += 1;
    }
    out
}

/// Python: `def <name>(..)` / `async def <name>(..)` with the name at
/// `at`; a leading `self` / `cls` takes no position, `*args` / `**kw` take
/// one but no name.
fn py_params_after(source: &str, at: usize) -> Option<Params> {
    let head = source.get(..at)?.trim_end_matches([' ', '\t']);
    if !head.ends_with("def")
        || (head.len() > 3
            && head
                .as_bytes()
                .get(head.len() - 4)
                .is_some_and(|c| is_ident_byte(*c)))
    {
        return None;
    }
    let b = source.as_bytes();
    let mut k = at
        + source
            .get(at..)?
            .bytes()
            .take_while(|c| is_ident_byte(*c))
            .count();
    while b.get(k).is_some_and(|c| *c == b' ' || *c == b'\t') {
        k += 1;
    }
    if b.get(k) != Some(&b'(') {
        return None;
    }
    let (region, close) = arg_region(source, k + 1);
    close?;
    let mut names: Vec<String> = split_args(region)
        .into_iter()
        .map(|item| {
            let t = item.trim();
            if t.starts_with('*') {
                String::new()
            } else {
                leading_ident(t).to_string()
            }
        })
        .collect();
    if names.last().is_some_and(String::is_empty) {
        names.pop();
    }
    if names.first().is_some_and(|n| n == "self" || n == "cls") {
        names.remove(0);
    }
    Some(
        names
            .into_iter()
            .enumerate()
            .map(|(index, n)| (n, ParamRef { index, key: None }))
            .collect(),
    )
}

/// The parameter named `name`.
fn param_of<'p>(params: &'p Params, name: &str) -> Option<&'p ParamRef> {
    if name.is_empty() {
        return None;
    }
    params.iter().find(|(n, _)| n == name).map(|(_, r)| r)
}

/// One piece of a template, f-string or `+` concatenation.
enum Piece {
    Text(String),
    Expr(String),
}

/// The THE HOLE rule (module doc, step 2): `expr` is a parameter, or holds
/// exactly one parameter as a template hole / f-string hole / `+` piece.
/// Returns the parameter and the text around it, other holes as `${expr}`.
fn param_hole(expr: &str, params: &Params) -> Option<(ParamRef, String, String)> {
    let e = expr.trim();
    if let Some(r) = param_of(params, e) {
        return Some((r.clone(), String::new(), String::new()));
    }
    let pieces = template_pieces(e).or_else(|| concat_pieces(e))?;
    let mut hole: Option<(usize, &ParamRef)> = None;
    for (i, p) in pieces.iter().enumerate() {
        if let Piece::Expr(x) = p
            && let Some(r) = param_of(params, x.trim())
        {
            if hole.is_some() {
                return None;
            }
            hole = Some((i, r));
        }
    }
    let (i, r) = hole?;
    let render = |ps: &[Piece]| -> String {
        ps.iter()
            .map(|p| match p {
                Piece::Text(t) => t.clone(),
                Piece::Expr(x) => format!("${{{}}}", x.trim()),
            })
            .collect()
    };
    let prefix = render(pieces.get(..i)?);
    let suffix = render(pieces.get(i + 1..)?);
    if prefix.ends_with(['/', '=', '?', '&', ':']) || prefix.contains('?') {
        return None;
    }
    Some((r.clone(), prefix, suffix))
}

/// A JS template literal (`` `..${x}..` ``) or a Python f-string
/// (`f"..{x}.."`) as pieces; `None` for anything else (a triple-quoted
/// string, a `{{` escape).
fn template_pieces(e: &str) -> Option<Vec<Piece>> {
    let b = e.as_bytes();
    let (inner, python) = if b.len() >= 2 && b.first() == Some(&b'`') && b.last() == Some(&b'`') {
        (e.get(1..e.len() - 1)?, false)
    } else if b.len() >= 3
        && matches!(b.first(), Some(b'f' | b'F'))
        && matches!(b.get(1), Some(b'"' | b'\''))
        && b.last() == b.get(1)
        && !e.get(1..)?.starts_with("\"\"\"")
        && !e.get(1..)?.starts_with("'''")
    {
        (e.get(2..e.len() - 1)?, true)
    } else {
        return None;
    };
    let open = if python { "{" } else { "${" };
    if python && (inner.contains("{{") || inner.contains("}}")) {
        return None;
    }
    let mut out = Vec::new();
    let mut rest = inner;
    while let Some(i) = rest.find(open) {
        if i > 0 {
            out.push(Piece::Text(rest.get(..i)?.to_string()));
        }
        let body = rest.get(i + open.len()..)?;
        let mut depth = 1usize;
        let mut end = None;
        for (k, c) in body.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        out.push(Piece::Expr(body.get(..end)?.to_string()));
        rest = body.get(end + 1..)?;
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest.to_string()));
    }
    Some(out)
}

/// A depth-0 `+` concatenation of two or more pieces (quote-aware): a
/// literal piece is its body, anything else an expression.
fn concat_pieces(e: &str) -> Option<Vec<Piece>> {
    let b = e.as_bytes();
    let mut parts: Vec<&str> = Vec::new();
    let (mut depth, mut quote, mut start, mut i) = (0i32, None::<u8>, 0usize, 0usize);
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'+' if depth == 0 => {
                    if matches!(b.get(i + 1), Some(b'+' | b'=')) {
                        return None;
                    }
                    parts.push(e.get(start..i)?);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    parts.push(e.get(start..)?);
    if parts.len() < 2 {
        return None;
    }
    Some(
        parts
            .into_iter()
            .map(|p| match literal(p) {
                Some(l) if l.quote != b':' => Piece::Text(l.body.to_string()),
                _ => Piece::Expr(p.trim().to_string()),
            })
            .collect(),
    )
}

/// An HTTP verb, upper-case.
fn verb(s: &str) -> Option<String> {
    let m = s.trim().to_ascii_uppercase();
    HTTP_METHODS.contains(&m.as_str()).then_some(m)
}

/// The method of a direct client call (module doc, step 3); `None` when it
/// is neither fixed nor forwarded. `body` is the owner's text before the
/// call: an options argument that names a local object literal there
/// (FH.17, `fetch(opts.url, opts)`) is read as that object.
fn call_method(call: &ClientCall<'_>, params: &Params, body: &str) -> Option<MethodFrom> {
    match &call.method {
        CallMethod::Verb(v) => Some(MethodFrom::Fixed((*v).to_string())),
        CallMethod::Arg(i) => {
            let a = call.args.get(*i)?.trim();
            match literal(a) {
                Some(l) => verb(l.body).map(MethodFrom::Fixed),
                None => param_of(params, a).cloned().map(MethodFrom::Arg),
            }
        }
        CallMethod::Name(name) => param_of(params, name).cloned().map(MethodFrom::Arg),
        CallMethod::Options(default) => {
            let arg = call.args.get(call.path_arg + 1).map(|a| a.trim());
            let local = arg
                .filter(|a| {
                    !a.is_empty() && leading_ident(a) == *a && param_of(params, a).is_none()
                })
                .and_then(|a| local_object(body, a));
            match local {
                Some(inner) => options_method(Some(&format!("{{{inner}}}")), params, default),
                None => options_method(arg, params, default),
            }
        }
    }
}

/// The method an options argument gives a call whose own reading is
/// `default`: a `method` key (a literal, or a parameter handed on), a
/// parameter that is the options object or spread into it, else `default`.
/// `None` for a literal `method` that is no HTTP verb, or one that is
/// neither a literal nor a parameter.
fn options_method(arg: Option<&str>, params: &Params, default: &str) -> Option<MethodFrom> {
    let fixed = || Some(MethodFrom::Fixed(default.to_string()));
    let Some(a) = arg.map(str::trim).filter(|a| !a.is_empty()) else {
        return fixed();
    };
    if let Some(r) = param_of(params, a) {
        return Some(MethodFrom::Options {
            at: r.clone(),
            default: default.to_string(),
        });
    }
    let Some(props) = object_props(a) else {
        return fixed();
    };
    if let Some(v) = prop_value(&props, "method") {
        let v = v.trim();
        return match literal(v) {
            Some(l) => verb(l.body).map(MethodFrom::Fixed),
            None => param_of(params, v).cloned().map(MethodFrom::Arg),
        };
    }
    for p in &props {
        if let Some(s) = p.trim().strip_prefix("...")
            && let Some(r) = param_of(params, s.trim())
        {
            return Some(MethodFrom::Options {
                at: r.clone(),
                default: default.to_string(),
            });
        }
    }
    fixed()
}

/// The properties of an object literal `{ .. }`, or of the one object a
/// one-argument call wraps (`lib.json({ method })`).
fn object_props(a: &str) -> Option<Vec<&str>> {
    let a = a.trim();
    if a.starts_with('{') {
        let (inner, close) = arg_region(a, 1);
        let rest = a.get(close? + 1..)?.trim();
        return (rest.is_empty() || rest.starts_with("as ")).then(|| split_args(inner));
    }
    let open = a.find('(')?;
    let callee = a.get(..open)?;
    if callee.is_empty() || !callee.bytes().all(|c| is_ident_byte(c) || c == b'.') {
        return None;
    }
    let (inner, close) = arg_region(a, open + 1);
    if !a.get(close? + 1..)?.trim().is_empty() {
        return None;
    }
    match split_args(inner).as_slice() {
        [one] => object_props(one),
        _ => None,
    }
}

/// The value text of property `key` among `props` (`key: v`, `"key": v`,
/// or the shorthand `key`, whose value is the identifier `key`).
fn prop_value<'a>(props: &[&'a str], key: &str) -> Option<&'a str> {
    for p in props {
        let t = p.trim();
        let (k, rest) = match literal(t.split(':').next().unwrap_or("")) {
            Some(l) if t.starts_with(['"', '\'']) => {
                let n = l.body.len() + 2;
                (l.body, t.get(n..).unwrap_or("").trim_start())
            }
            _ => {
                let k = leading_ident(t);
                (k, t.get(k.len()..).unwrap_or("").trim_start())
            }
        };
        if k != key {
            continue;
        }
        if rest.is_empty() {
            return Some(t);
        }
        if let Some(v) = rest.strip_prefix(':')
            && !v.starts_with(':')
        {
            return Some(v.trim());
        }
    }
    None
}

/// The text a site passes at `r`: the argument, or its object's property.
fn arg_value<'a>(args: &[&'a str], r: &ParamRef) -> Option<&'a str> {
    let a = args.get(r.index)?.trim();
    if a.is_empty() {
        return None;
    }
    match &r.key {
        None => Some(a),
        Some(k) => prop_value(&object_props(a)?, k),
    }
}

/// What a site's method reads as.
enum MethodAt {
    Known(String),
    /// A parameter of the site's owner: the owner forwards the method.
    Forward(MethodFrom),
    /// Not literal and not forwarded.
    Unknown,
    /// A literal that is no HTTP verb.
    Invalid,
}

/// A site's method under `m`; `params` the site owner's, for a forwarding
/// read (`None` at a minting site).
fn method_at(m: &MethodFrom, args: &[&str], params: Option<&Params>) -> MethodAt {
    let none = Params::new();
    let ps = params.unwrap_or(&none);
    match m {
        MethodFrom::Fixed(s) => MethodAt::Known(s.clone()),
        MethodFrom::Arg(r) => match arg_value(args, r) {
            None => MethodAt::Unknown,
            Some(v) => match literal(v) {
                Some(l) => verb(l.body).map_or(MethodAt::Invalid, MethodAt::Known),
                None => param_of(ps, v).map_or(MethodAt::Unknown, |r| {
                    MethodAt::Forward(MethodFrom::Arg(r.clone()))
                }),
            },
        },
        MethodFrom::Options { at, default } => {
            match options_method(arg_value(args, at), ps, default) {
                Some(MethodFrom::Fixed(s)) => MethodAt::Known(s),
                Some(f) if params.is_some() => MethodAt::Forward(f),
                Some(_) => MethodAt::Unknown,
                None => match arg_value(args, at).and_then(object_props) {
                    Some(props)
                        if prop_value(&props, "method").is_some_and(|v| literal(v).is_some()) =>
                    {
                        MethodAt::Invalid
                    }
                    _ => MethodAt::Unknown,
                },
            }
        }
    }
}

/// What a minting site of an inferred HTTP stanza reads as.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HttpSite {
    NonLiteral,
    Invalid,
    /// The method and the composed path, before `http_read` normalises it.
    Path {
        method: String,
        path: String,
    },
}

/// Read one site of an inferred HTTP wrapper (module doc: the sites): the
/// path is ONE literal starting with `/` or `http(s)://`, with no
/// whitespace, every `${..}` hole right after a `/` (a Python f-string's
/// `{x}` read as `${x}`); the method under the shape's [`MethodFrom`].
pub(crate) fn read_http_site(shape: &HttpShape, args: &[&str]) -> HttpSite {
    let Some(v) = arg_value(args, &shape.path) else {
        return HttpSite::NonLiteral;
    };
    let Some(lit) = literal(v).filter(|l| l.quote != b':') else {
        return HttpSite::NonLiteral;
    };
    let fstring = v
        .find(['"', '\''])
        .and_then(|q| q.checked_sub(1))
        .and_then(|i| v.as_bytes().get(i))
        .is_some_and(|c| matches!(c, b'f' | b'F'));
    let body = if fstring {
        if lit.body.contains("{{") {
            return HttpSite::NonLiteral;
        }
        lit.body.replace('{', "${")
    } else {
        lit.body.to_string()
    };
    let path_like =
        body.starts_with('/') || body.starts_with("http://") || body.starts_with("https://");
    let holes_ok = body
        .match_indices("${")
        .all(|(i, _)| i > 0 && body.as_bytes().get(i - 1) == Some(&b'/'));
    if !path_like || !holes_ok || body.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return HttpSite::Invalid;
    }
    let method = match method_at(&shape.method, args, None) {
        MethodAt::Known(m) => m,
        MethodAt::Invalid => return HttpSite::Invalid,
        MethodAt::Forward(_) | MethodAt::Unknown => return HttpSite::NonLiteral,
    };
    HttpSite::Path {
        method,
        path: format!("{}{body}{}", shape.prefix, shape.suffix),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{GoModules, parse_one_as};

    /// Infer over `files` the way the build does: the router's module plan,
    /// one real parse per file keyed by its engine language.
    fn infer(files: &[(&str, &str)]) -> InferredWrappers {
        let repo = RepoId::from_canonical("test://infer-wrappers");
        let files: Vec<(String, String)> = files
            .iter()
            .map(|(p, s)| (p.to_string(), s.to_string()))
            .collect();
        let modules = ModuleQnames::plan(&files);
        let mut parses: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        for (path, source) in &files {
            let lang = detect_language(path).expect("a code file");
            let fp = parse_one_as(
                source,
                path,
                lang,
                repo,
                &GoModules::root_only(""),
                &modules.module_qname(path),
            )
            .expect("parses");
            parses.entry(lang).or_default().push(fp);
        }
        let mut errors = Vec::new();
        let out = infer_entity_wrappers(&parses, &files, &modules, repo, None, &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        out
    }

    /// `(call, name_arg, def_file, 1-based def line, depth)` of each wrapper.
    fn found(w: &InferredWrappers) -> Vec<(String, usize, String, u32, usize)> {
        w.wrappers
            .iter()
            .map(|w| {
                (
                    w.decl.call.clone(),
                    w.decl.name_arg.unwrap_or(usize::MAX),
                    w.def_file.clone(),
                    w.def_line0 + 1,
                    w.depth,
                )
            })
            .collect()
    }

    const GROUPED: &str = "package repositories\n\nimport \"go.mongodb.org/mongo-driver/mongo\"\n\n// NewCollection constructs a generic collection wrapper.\nfunc NewCollection[T any](client *mongo.Client, database, name string) *Collection[T] {\n\treturn &Collection[T]{col: client.Database(database).Collection(name)}\n}\n";

    #[test]
    fn go_grouped_params() {
        let w = infer(&[("repositories/collection.go", GROUPED)]);
        assert_eq!(
            found(&w),
            [(
                "NewCollection".to_string(),
                2,
                "repositories/collection.go".to_string(),
                6,
                1
            )]
        );
        let d = &w.wrappers[0].decl;
        assert_eq!(
            (
                d.kind.as_str(),
                d.flavor.as_deref(),
                d.languages.as_slice(),
                d.receiver
            ),
            (
                "data_entity",
                Some("nosql"),
                ["go".to_string()].as_slice(),
                false
            )
        );
        assert_eq!(
            (
                w.direct(),
                w.forwarding(),
                w.skipped_ambiguous,
                w.skipped_unparsed
            ),
            (1, 0, 0, 0)
        );
    }

    #[test]
    fn go_method_receiver_is_not_a_param() {
        let src = "package store\n\ntype Repo struct{ db *mongo.Database }\n\nfunc (r *Repo) coll(name string) *mongo.Collection {\n\treturn r.db.Collection(name)\n}\n";
        let w = infer(&[("store/repo.go", src)]);
        assert_eq!(
            found(&w),
            [("coll".to_string(), 0, "store/repo.go".to_string(), 5, 1)]
        );
    }

    #[test]
    fn ident_not_a_param_is_skipped() {
        // A package const names no parameter; a commented driver call is
        // not a call.
        let src = "package store\n\nconst tableName = \"users\"\n\nfunc Users(c *mongo.Client, db string) *mongo.Collection {\n\treturn c.Database(db).Collection(tableName)\n}\n\nfunc Rooms(c *mongo.Client, name string) *mongo.Collection {\n\t// return c.Database(\"app\").Collection(name)\n\treturn nil\n}\n";
        let w = infer(&[("store/users.go", src)]);
        assert!(w.wrappers.is_empty(), "{:?}", found(&w));
        assert_eq!((w.skipped_ambiguous, w.skipped_unparsed), (0, 0));
    }

    #[test]
    fn forwarding_depth_two() {
        let named = "package repositories\n\n// NewNamedCollection forwards a caller-chosen name.\nfunc NewNamedCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {\n\treturn NewCollection[T](client, database, name)\n}\n\n// Third hop: the name moves to the first slot.\nfunc Named(\n\tname string, // the collection\n\tclient *mongo.Client,\n) *Collection[User] {\n\treturn NewNamedCollection[User](client, \"app\", name)\n}\n\nfunc Literal(client *mongo.Client) *Collection[User] {\n\treturn NewNamedCollection[User](client, \"app\", \"users\")\n}\n";
        let w = infer(&[
            ("repositories/collection.go", GROUPED),
            ("repositories/named.go", named),
        ]);
        assert_eq!(
            found(&w),
            [
                (
                    "Named".to_string(),
                    0,
                    "repositories/named.go".to_string(),
                    9,
                    3
                ),
                (
                    "NewCollection".to_string(),
                    2,
                    "repositories/collection.go".to_string(),
                    6,
                    1
                ),
                (
                    "NewNamedCollection".to_string(),
                    2,
                    "repositories/named.go".to_string(),
                    4,
                    2
                ),
            ]
        );
        assert_eq!((w.direct(), w.forwarding()), (1, 2));
    }

    #[test]
    fn forwarding_stops_at_max_depth() {
        let chain = "package r\n\nfunc HopTwo(name string) { NewCollection[T](c, d, name) }\n\nfunc HopThree(name string) { HopTwo(name) }\n\nfunc HopFour(name string) { HopThree(name) }\n";
        let w = infer(&[("r/collection.go", GROUPED), ("r/chain.go", chain)]);
        let calls: Vec<String> = found(&w).into_iter().map(|f| f.0).collect();
        assert_eq!(
            calls,
            ["HopThree", "HopTwo", "NewCollection"],
            "HopFour is depth 4"
        );
    }

    #[test]
    fn ambiguous_name_is_dropped() {
        let other = "package other\n\nfunc NewCollection(name string) *Thing {\n\treturn &Thing{name: name}\n}\n";
        let w = infer(&[
            ("repositories/collection.go", GROUPED),
            ("other/thing.go", other),
        ]);
        assert!(w.wrappers.is_empty(), "{:?}", found(&w));
        assert_eq!(w.skipped_ambiguous, 1);
        // A name under MIN_CALL_LEN bytes is dropped too.
        let short = "package r\n\nfunc cl(c *mongo.Database, name string) *mongo.Collection {\n\treturn c.Collection(name)\n}\n";
        let w = infer(&[("r/short.go", short)]);
        assert!(w.wrappers.is_empty(), "{:?}", found(&w));
        assert_eq!(w.skipped_ambiguous, 1);
    }

    #[test]
    fn non_go_files_are_not_inferred() {
        let ts = "export function coll(name: string) {\n  return db.collection(name);\n}\n\nexport const users = () => coll('users');\n";
        let w = infer(&[("web/db.ts", ts)]);
        assert!(w.wrappers.is_empty(), "{:?}", found(&w));
        assert_eq!((w.skipped_ambiguous, w.skipped_unparsed), (0, 0));
    }

    #[test]
    fn signatures_read_receivers_type_params_and_groups() {
        let sig = |s: &str| go_func_signature(s, 0);
        assert_eq!(
            sig(
                "func (r *Repo[T]) Get[K comparable](ctx context.Context, a, b K, rest ...string) error {"
            ),
            Some((
                "Get".to_string(),
                vec!["ctx".into(), "a".into(), "b".into(), "rest".into()]
            ))
        );
        assert_eq!(sig("func F() {"), Some(("F".to_string(), vec![])));
        assert_eq!(sig("func(name string) {"), None, "a func literal");
        assert_eq!(sig("func F(a int"), None, "unclosed");
        assert!(is_ident("tableName") && is_ident("_x1"));
        assert!(!is_ident("1x") && !is_ident("a.b") && !is_ident("\"x\"") && !is_ident(""));
    }

    // -----------------------------------------------------------------------
    // FH.4b: HTTP wrappers.
    // -----------------------------------------------------------------------

    /// One line per inferred HTTP wrapper:
    /// `<call> path=<i>[.<key>] "<prefix>"+P+"<suffix>" method=<m> depth=<d> @<file>:<line>`.
    fn http_found(w: &InferredWrappers) -> Vec<String> {
        let r = |p: &ParamRef| match &p.key {
            Some(k) => format!("{}.{k}", p.index),
            None => p.index.to_string(),
        };
        w.http
            .iter()
            .map(|w| {
                let s = w.http.as_ref().expect("an http shape");
                let m = match &s.method {
                    MethodFrom::Fixed(m) => m.clone(),
                    MethodFrom::Arg(a) => format!("arg{}", r(a)),
                    MethodFrom::Options { at, default } => format!("opts{}|{default}", r(at)),
                };
                format!(
                    "{} path={} {:?}+P+{:?} method={m} depth={} @{}:{}",
                    w.decl.call,
                    r(&s.path),
                    s.prefix,
                    s.suffix,
                    w.depth,
                    w.def_file,
                    w.def_line0 + 1
                )
            })
            .collect()
    }

    const FIX_HTTP: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/ts-http-wrapper-forward/web/src/lib/http.ts"
    );
    const FIX_VERBS: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/ts-http-wrapper-forward/web/src/lib/verbs.ts"
    );
    const FIX_USERS: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/ts-http-wrapper-forward/web/src/features/users.ts"
    );

    #[test]
    fn ts_fetch_wrapper_forwards_two_levels() {
        let w = infer(&[
            ("src/lib/http.ts", FIX_HTTP),
            ("src/lib/verbs.ts", FIX_VERBS),
            ("src/features/users.ts", FIX_USERS),
        ]);
        assert_eq!(
            http_found(&w),
            [
                r#"api path=0 "${API_BASE}"+P+"" method=opts1|GET depth=1 @src/lib/http.ts:5"#,
                r#"getJson path=0 "${API_BASE}"+P+"" method=GET depth=2 @src/lib/verbs.ts:4"#,
                r#"lookup path=0 "${API_BASE}"+P+"" method=GET depth=3 @src/features/users.ts:20"#,
                r#"readResource path=0 "${API_BASE}/v1"+P+"" method=GET depth=3 @src/lib/verbs.ts:13"#,
                r#"sendJson path=1 "${API_BASE}"+P+"" method=arg0 depth=2 @src/lib/verbs.ts:8"#,
            ]
        );
        assert_eq!((w.http_ambiguous, w.http_unparsed), (0, 0));
        assert!(w.wrappers.is_empty(), "no collection wrapper");
        let api = &w.http[0].decl;
        assert_eq!(
            (
                api.kind.as_str(),
                api.path_arg,
                api.method_arg,
                api.languages.len()
            ),
            ("http", Some(0), Some(1), TS_FAMILY.len())
        );
    }

    #[test]
    fn ts_holes_destructuring_and_precision() {
        let src = r#"import axios from "axios";
const REST = "/rest";
export function getUser(id: string) { return fetch(`/users/${id}`); }
export function get(path: string) { return axios.get(path); }
export const makeRestApiRequest = async ({ path, method: verb = "GET" }: Args) => {
  return fetch(`${REST}${path}?x=1`, { method: verb, headers: {} });
};
export class Api {
  constructor(private http: HttpClient) {}
  load(url: string, body: unknown) { return this.http.post<Item>(`${REST}/v2${url}`, body); }
}
"#;
        let w = infer(&[("src/rest.ts", src)]);
        assert_eq!(
            http_found(&w),
            [
                r#"load path=0 "${REST}/v2"+P+"" method=POST depth=1 @src/rest.ts:10"#,
                r#"makeRestApiRequest path=0.path "${REST}"+P+"?x=1" method=arg0.method depth=1 @src/rest.ts:5"#,
            ]
        );
        // `get` is an HTTP verb; `getUser`'s hole follows a `/`.
        assert_eq!(w.http_ambiguous, 1);
    }

    const FIX_REST: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/ts-http-wrapper-precision/web/src/lib/rest.ts"
    );
    const FIX_CACHE: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/ts-http-wrapper-precision/web/src/lib/cache.ts"
    );
    const FIX_A: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/ts-http-wrapper-precision/web/src/features/a.ts"
    );

    /// FH.17: an options-object builder is followed (its `opts.url` read
    /// through the local object literal, the method through the same object);
    /// a computed verb on a client (`client[method](..)`) forwards the method
    /// parameter; a verb call on a receiver that is no HTTP client
    /// (`this.entries.get(key)`, the parser's `<unresolved>`) seeds nothing,
    /// counted `phantom_dropped`, while `fetch`, a client-named receiver and a
    /// non-verb member still seed.
    #[test]
    fn ts_options_objects_computed_verbs_and_phantoms() {
        let sup = r#"import request from "supertest";
export const makeRestApiRequest = ({ method, path }: Params) => {
  const client = request(`http://localhost:${PORT}`);
  return client[method](`/rest${path}`).set("Authorization", "x");
};
export function dispatch(type: string, payload: unknown) {
  return handlers[type](payload);
}
export class Remote {
  private apiClient: any;
  private store: any;
  fetchOne(url: string) { return this.apiClient.get(url); }
  readOne(key: string) { return this.store.get(key); }
}
"#;
        let w = infer(&[
            ("src/lib/rest.ts", FIX_REST),
            ("src/lib/cache.ts", FIX_CACHE),
            ("src/features/a.ts", FIX_A),
            ("src/sup.ts", sup),
        ]);
        assert_eq!(
            http_found(&w),
            [
                r#"fetchOne path=0 ""+P+"" method=GET depth=1 @src/sup.ts:12"#,
                r#"makeRequest path=0.path "/api"+P+"" method=GET depth=1 @src/lib/rest.ts:4"#,
                r#"makeRestApiRequest path=0.path "/rest"+P+"" method=arg0.method depth=1 @src/sup.ts:2"#,
            ]
        );
        // `lookup`, `drop` and `readOne`: verb calls on no HTTP client.
        assert_eq!(w.http_phantom, 3);
        assert_eq!((w.http_options_object, w.http_computed), (1, 1));
    }

    /// FH.17b: a verb call whose receiver is a default or named import from
    /// a module whose specifier's last path words name a client
    /// (`@/utils/request`, `../services/api/index`, `./http-client.ts`) is a
    /// client call although the receiver's own name (`instance`, `svc`) is
    /// none; a member receiver (`this.cache`, though `cache` is imported from
    /// `./request`), an import of another module (`@/store`), a type-only
    /// import and a name the file redeclares (`entries`) stay phantoms.
    #[test]
    fn ts_receivers_imported_from_client_modules() {
        let src = r#"import instance from '@/utils/request';
import { service as svc, other } from "../services/api/index";
import type { Shape } from "./http-client.ts";
import store from "@/store";
import { cache, entries } from "./request";
export function getThing(url: string) { return instance.get(url); }
export const send = (path: string, body: unknown) => svc.post(path, body);
export function readOther(key: string) { return other.delete(key); }
export function readStore(key: string) { return store.get(key); }
export class Repo {
  private cache: any;
  readOne(id: string) { return this.cache.get(id); }
}
export function typed(path: string) { return Shape.get(path); }
export function local(path: string) {
  const entries = new Map<string, string>();
  return entries.get(path);
}
export function useThing() { return getThing("/x"); }
"#;
        let w = infer(&[("src/api/user.ts", src)]);
        assert_eq!(
            http_found(&w),
            [
                r#"getThing path=0 ""+P+"" method=GET depth=1 @src/api/user.ts:6"#,
                r#"readOther path=0 ""+P+"" method=DELETE depth=1 @src/api/user.ts:8"#,
                r#"send path=0 ""+P+"" method=POST depth=1 @src/api/user.ts:7"#,
            ]
        );
        // `readStore`, `readOne`, `typed` and `local`: no client by import.
        assert_eq!(w.http_phantom, 4);
        assert_eq!(w.http_client_by_import, 3);
    }

    #[test]
    fn import_specifier_client_words() {
        for (spec, client) in [
            ("@/utils/request", true),
            ("../services/api/index", true),
            ("./http-client.ts", true),
            ("~/lib/fetcher", false),
            ("@/lib/apiService", true),
            ("./requests.js", true),
            ("@/store", false),
            ("./user.service", true),
            ("../a/index.js", false),
            ("./cache/index.ts", false),
            ("lodash", false),
        ] {
            assert_eq!(is_client_specifier(spec), client, "{spec}");
        }
    }

    #[test]
    fn python_wrappers_through_self_and_fstrings() {
        let src = r#"import requests

BASE = "http://svc/api"

class Client:
    def _request(self, method, path, **kw):
        return requests.request(method, f"{self.base}{path}", **kw)

    def get_json(self, path):
        return self._request("GET", path).json()

def fetch_url(path):
    return requests.get(BASE + path)

def users(c):
    return c.get_json("/users")
"#;
        let w = infer(&[("client.py", src)]);
        assert_eq!(
            http_found(&w),
            [
                r#"_request path=1 "${self.base}"+P+"" method=arg0 depth=1 @client.py:6"#,
                r#"fetch_url path=0 "${BASE}"+P+"" method=GET depth=1 @client.py:12"#,
                r#"get_json path=0 "${self.base}"+P+"" method=GET depth=2 @client.py:9"#,
            ]
        );
    }

    #[test]
    fn go_wrappers_over_net_http() {
        let src = "package client\n\nimport \"net/http\"\n\ntype Client struct{ base string; hc *http.Client }\n\nfunc (c *Client) send(method, path string) (*http.Response, error) {\n\treq, err := http.NewRequest(method, c.base+path, nil)\n\tif err != nil {\n\t\treturn nil, err\n\t}\n\treturn c.hc.Do(req)\n}\n\nfunc getURL(path string) (*http.Response, error) {\n\treturn http.Get(\"http://svc\" + path)\n}\n\nfunc (c *Client) Users() { c.send(\"GET\", \"/users\") }\n\nfunc (c *Client) do(path string) { http.Get(path) }\n";
        let w = infer(&[("client/client.go", src)]);
        assert_eq!(
            http_found(&w),
            [
                r#"getURL path=0 "http://svc"+P+"" method=GET depth=1 @client/client.go:15"#,
                r#"send path=1 "${c.base}"+P+"" method=arg0 depth=1 @client/client.go:7"#,
            ]
        );
        // `do` is under MIN_CALL_LEN bytes.
        assert_eq!(w.http_ambiguous, 1);
        assert!(w.wrappers.is_empty());
    }

    #[test]
    fn http_sites_read_one_path_literal_and_a_method() {
        let shape = |method: MethodFrom| HttpShape {
            path: ParamRef {
                index: 0,
                key: None,
            },
            prefix: "${API_BASE}".to_string(),
            suffix: String::new(),
            method,
        };
        let opts = shape(MethodFrom::Options {
            at: ParamRef {
                index: 1,
                key: None,
            },
            default: "GET".to_string(),
        });
        let path = |m: &str, p: &str| HttpSite::Path {
            method: m.to_string(),
            path: p.to_string(),
        };
        assert_eq!(
            read_http_site(&opts, &["'/users'"]),
            path("GET", "${API_BASE}/users")
        );
        assert_eq!(
            read_http_site(&opts, &["`/users/${id}`", "{ method: \"DELETE\" }"]),
            path("DELETE", "${API_BASE}/users/${id}")
        );
        assert_eq!(
            read_http_site(&opts, &["'/x'", "lib.json({ ...body, method: 'PUT' })"]),
            path("PUT", "${API_BASE}/x")
        );
        assert_eq!(
            read_http_site(&opts, &["'/x'", "{ method: 'FETCH' }"]),
            HttpSite::Invalid
        );
        assert_eq!(
            read_http_site(&opts, &["'/x'", "{ method: m }"]),
            HttpSite::NonLiteral
        );
        assert_eq!(read_http_site(&opts, &["key"]), HttpSite::NonLiteral);
        assert_eq!(read_http_site(&opts, &["'cache key'"]), HttpSite::Invalid);
        assert_eq!(read_http_site(&opts, &["'users'"]), HttpSite::Invalid);
        assert_eq!(read_http_site(&opts, &["`/u${id}`"]), HttpSite::Invalid);
        let fixed = shape(MethodFrom::Fixed("GET".to_string()));
        assert_eq!(
            read_http_site(&fixed, &["f\"/users/{uid}\""]),
            path("GET", "${API_BASE}/users/${uid}")
        );
        let keyed = HttpShape {
            path: ParamRef {
                index: 0,
                key: Some("path".to_string()),
            },
            prefix: "/rest".to_string(),
            suffix: String::new(),
            method: MethodFrom::Arg(ParamRef {
                index: 0,
                key: Some("method".to_string()),
            }),
        };
        assert_eq!(
            read_http_site(&keyed, &["{ path: '/metadata', method: 'POST' }"]),
            path("POST", "/rest/metadata")
        );
        assert_eq!(
            read_http_site(&keyed, &["{ path: '/metadata' }"]),
            HttpSite::NonLiteral
        );
    }

    /// The fixture end to end: the wrapper chain's endpoints pair the Go
    /// routes, carry the inferred ORIGIN and the `wrapper_call` EVIDENCE, and
    /// replace the FH.4a shape rule's unprefixed reading.
    #[test]
    fn fixture_wrapper_chain_builds() {
        use glia_code_domain::evidence::Evidence;
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bench/substrate-gap/fixtures/ts-http-wrapper-forward");
        let root = std::env::temp_dir().join(format!("glia_fh4b_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for rel in [
            "web/src/lib/http.ts",
            "web/src/lib/verbs.ts",
            "web/src/features/users.ts",
            "server/main.go",
        ] {
            let dest = root.join(rel);
            std::fs::create_dir_all(dest.parent().expect("a parent")).expect("mkdir");
            std::fs::copy(fx.join(rel), &dest).expect("copy");
        }
        let repos: Vec<String> = ["web", "server"]
            .iter()
            .map(|d| root.join(d).to_string_lossy().into_owned())
            .collect();
        let built = crate::generate_many_opts(&repos, false, &crate::BuildOptions::default());
        std::fs::remove_dir_all(&root).ok();
        let r = built.expect("build");
        let qname = |id: NodeId| {
            r.merged
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
                .unwrap_or_default()
        };
        let mut endpoints: Vec<String> = r
            .merged
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
                    .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
            })
            .collect();
        endpoints.sort();
        endpoints.dedup();
        assert_eq!(
            endpoints,
            [
                "endpoint:DELETE:/api/users/${…}",
                "endpoint:GET:/api${…}",
                "endpoint:GET:/api/users",
                "endpoint:GET:/api/v1/teams",
                "endpoint:POST:/api/users",
            ]
        );
        let paired: Vec<String> = r
            .merged
            .cross_edges
            .iter()
            .filter(|e| e.category == glia_code_domain::edge_category::HTTP_CALLS)
            .map(|e| qname(e.from))
            .collect();
        assert_eq!(paired.len(), 4, "{paired:?}");
        let teams = r
            .merged
            .graphs
            .iter()
            .find_map(|g| {
                g.nodes.iter().find(|n| {
                    g.nav.qname_by_id.get(&n.id).map(String::as_str)
                        == Some("endpoint:GET:/api/v1/teams")
                })
            })
            .expect("the teams endpoint");
        let origin: Vec<&str> = teams
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::ORIGIN)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(j) => Some(j.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            origin,
            [
                r#"{"provenance":"inferred:wrapper","rule":"inferred:readResource","def":"src/lib/verbs.ts:13"}"#
            ]
        );
        let ev = r
            .merged
            .graphs
            .iter()
            .flat_map(|g| g.edges.iter())
            .find(|e| {
                qname(e.to) == "endpoint:GET:/api/v1/teams"
                    && qname(e.from).ends_with("::listTeams")
            })
            .and_then(Evidence::of)
            .expect("listTeams CALLS the teams endpoint");
        assert_eq!(
            (ev.emitter.as_str(), ev.rule.as_deref(), ev.line),
            ("pass:inferred_wrapper", Some("wrapper_call"), Some(16))
        );
    }

    /// FH.17, the `ts-http-wrapper-precision` fixture end to end: the
    /// options-object wrapper's site pairs the Go route; the Map-like methods
    /// mint nothing; the Go test helper's site in `api_test.go` mints its
    /// ENDPOINT with ORIGIN `test_fixture` and a `"test_site":true` hit, and
    /// the http pass pairs it with nothing although the route exists.
    #[test]
    fn fixture_precision_builds() {
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bench/substrate-gap/fixtures/ts-http-wrapper-precision");
        let root = std::env::temp_dir().join(format!("glia_fh17_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for rel in [
            "web/src/lib/rest.ts",
            "web/src/lib/cache.ts",
            "web/src/features/a.ts",
            "server/main.go",
            "server/api_test.go",
        ] {
            let dest = root.join(rel);
            std::fs::create_dir_all(dest.parent().expect("a parent")).expect("mkdir");
            std::fs::copy(fx.join(rel), &dest).expect("copy");
        }
        let repos: Vec<String> = ["web", "server"]
            .iter()
            .map(|d| root.join(d).to_string_lossy().into_owned())
            .collect();
        let built = crate::generate_many_opts(&repos, false, &crate::BuildOptions::default());
        std::fs::remove_dir_all(&root).ok();
        let r = built.expect("build");
        let qname = |id: NodeId| {
            r.merged
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
                .unwrap_or_default()
        };
        let mut endpoints: Vec<String> = r
            .merged
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
                    .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
            })
            .collect();
        endpoints.sort();
        endpoints.dedup();
        assert_eq!(
            endpoints,
            [
                "endpoint:DELETE:<unresolved>",
                "endpoint:GET:/api/items",
                "endpoint:GET:/api/stats",
                "endpoint:GET:<unresolved>",
            ]
        );
        let paired: Vec<(String, String)> = r
            .merged
            .cross_edges
            .iter()
            .filter(|e| e.category == glia_code_domain::edge_category::HTTP_CALLS)
            .map(|e| (qname(e.from), qname(e.to)))
            .collect();
        assert_eq!(
            paired,
            [(
                "endpoint:GET:/api/items".to_string(),
                "GET /api/items".to_string()
            )]
        );
        let stats = r
            .merged
            .graphs
            .iter()
            .find_map(|g| {
                g.nodes.iter().find(|n| {
                    g.nav.qname_by_id.get(&n.id).map(String::as_str)
                        == Some("endpoint:GET:/api/stats")
                })
            })
            .expect("the test site's endpoint");
        let json = |kind| -> Vec<&str> {
            stats
                .cells
                .iter()
                .filter(|c| c.kind == kind)
                .filter_map(|c| match &c.payload {
                    CellPayload::Json(j) => Some(j.as_str()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            json(cell_type::ORIGIN),
            [
                r#"{"provenance":"test_fixture","rule":"inferred:PerformRequest","def":"api_test.go:11"}"#
            ]
        );
        let hits = json(cell_type::ENDPOINT_HIT);
        assert!(
            hits.len() == 1 && hits[0].ends_with(r#","test_site":true}"#),
            "{hits:?}"
        );
    }

    /// GD.26: the heritage wrappers as `<class>.<verb> @<file>:<1-based
    /// line> path=<index> <method>`.
    fn heritage_found(w: &InferredWrappers) -> Vec<String> {
        w.heritage
            .iter()
            .map(|h| {
                format!(
                    "{}.{} @{}:{} path={} {:?}",
                    h.class,
                    h.verb,
                    h.def_file,
                    h.def_line0 + 1,
                    h.shape.path.index,
                    h.shape.method
                )
            })
            .collect()
    }

    /// GD.26: the heritage scopes as `<file>:<1-based start>-<end>
    /// <verb>=<wrapper index | ?>..`.
    fn scopes_found(w: &InferredWrappers) -> Vec<String> {
        w.heritage_scopes
            .iter()
            .map(|sc| {
                let targets: Vec<String> = sc
                    .targets
                    .iter()
                    .map(|(v, t)| match t {
                        HeritageTarget::Wrapper(i) => format!("{v}=W{i}"),
                        HeritageTarget::Ambiguous => format!("{v}=?"),
                    })
                    .collect();
                format!(
                    "{}:{}-{} {}",
                    sc.file,
                    sc.start_line0 + 1,
                    sc.end_line0 + 1,
                    targets.join(",")
                )
            })
            .collect()
    }

    const HERITAGE_BASE: &str = r#"import axios from "axios";
export abstract class APIService {
  private axiosInstance: any;
  get(url: string, params = {}) {
    return this.axiosInstance.get(url, { ...params });
  }
  post(url: string, data = {}) {
    return this.axiosInstance.post(url, data);
  }
}
export class LocalChild extends APIService {
  a() { return this.get("/api/local/"); }
}
"#;

    /// GD.26: an HTTP-verb member of a class that hands its parameter to a
    /// client call is a heritage wrapper, not an ambiguous name; its scopes
    /// are the class itself, a same-file subclass, subclasses importing it by
    /// a specifier whose last segment is its file's stem (relative or
    /// aliased), and their in-repo subclasses (a same-file intermediate, then
    /// an imported one). An override shadows the verb (`get` there reaches
    /// nothing; `post` still does); a base imported from a package, and a
    /// Map-like member whose call is a phantom (no client receiver), reach
    /// nothing.
    #[test]
    fn ts_heritage_wrappers_and_their_scopes() {
        let view = r#"import { APIService } from "./api.service";
export class ViewService extends APIService {
  load(id: string) { return this.get(`/api/views/${id}/`); }
}
"#;
        let live = r#"import { APIService } from "@/services/api.service";
export class LiveService extends APIService {}
export class DeepService extends LiveService {
  x() { return this.post("/api/deep/", {}); }
}
"#;
        let deeper = r#"import { DeepService } from "./live.service";
export class Deeper extends DeepService {
  y() { return super.get("/api/deeper/"); }
}
"#;
        let cached = r#"import { APIService } from "./api.service";
export class CachedService extends APIService {
  private store: any;
  get(url: string) { return this.store.get(url); }
  z() { return this.post("/api/cached/", {}); }
}
"#;
        let orphan = r#"import { APIService } from "@vendor/sdk";
export class OrphanService extends APIService {
  w() { return this.get("/api/orphans/"); }
}
"#;
        let kv = r#"export class KV {
  private entries: any;
  get(key: string) { return this.entries.get(key); }
}
export class Sub extends KV {
  v() { return this.get("/not/http"); }
}
"#;
        let w = infer(&[
            ("src/services/api.service.ts", HERITAGE_BASE),
            ("src/services/view.service.ts", view),
            ("src/services/live.service.ts", live),
            ("src/services/deeper.ts", deeper),
            ("src/services/cached.service.ts", cached),
            ("src/legacy/orphan.ts", orphan),
            ("src/lib/kv.ts", kv),
        ]);
        assert_eq!(
            heritage_found(&w),
            [
                r#"APIService.get @src/services/api.service.ts:4 path=0 Fixed("GET")"#,
                r#"APIService.post @src/services/api.service.ts:7 path=0 Fixed("POST")"#,
            ]
        );
        assert!(w.http.is_empty(), "no plain wrapper: {:?}", w.http);
        assert_eq!(
            w.http_ambiguous, 0,
            "the verb members are heritage wrappers"
        );
        // KV.get's `this.entries.get(key)` and CachedService.get's
        // `this.store.get(url)`: verb calls on no HTTP client.
        assert_eq!(w.http_phantom, 2);
        assert_eq!(
            scopes_found(&w),
            [
                "src/services/api.service.ts:2-10 get=W0,post=W1",
                "src/services/api.service.ts:11-13 get=W0,post=W1",
                "src/services/view.service.ts:2-4 get=W0,post=W1",
                "src/services/live.service.ts:2-2 get=W0,post=W1",
                "src/services/live.service.ts:3-5 get=W0,post=W1",
                "src/services/deeper.ts:2-4 get=W0,post=W1",
                "src/services/cached.service.ts:2-6 post=W1",
            ]
        );
    }

    /// GD.26: several same-named base classes whose files share the
    /// specifier's stem all count: one wrapper shape across them reaches the
    /// least wrapper; two shapes, or a copy that does not define the verb, is
    /// `ambiguous_base`.
    #[test]
    fn ts_heritage_copies_agree_or_are_ambiguous() {
        let copy = |prefix: &str| {
            format!(
                "export class APIService {{\n  private http: any;\n  get(url: string) {{ return this.http.get({prefix}url); }}\n  put(url: string) {{ return this.http.put(url); }}\n}}\n"
            )
        };
        let (a, b) = (copy(""), copy("\"/v2\" + "));
        let bare = "export class APIService {\n  private http: any;\n}\n";
        let user = r#"import { APIService } from "@/api.service";
export class UserService extends APIService {
  u() { return this.get("/api/users/"); }
}
"#;
        let w = infer(&[
            ("app/a/api.service.ts", &a),
            ("app/b/api.service.ts", &b),
            ("app/user.service.ts", user),
        ]);
        assert_eq!(
            scopes_found(&w),
            [
                "app/a/api.service.ts:1-5 get=W0,put=W1",
                "app/b/api.service.ts:1-5 get=W2,put=W3",
                // `get`: "/v2" + url differs from url; `put` agrees, and
                // reaches the least of the two.
                "app/user.service.ts:2-4 get=?,put=W1",
            ]
        );
        let w = infer(&[
            ("app/a/api.service.ts", &a),
            ("app/c/api.service.ts", bare),
            ("app/user.service.ts", user),
        ]);
        assert_eq!(
            scopes_found(&w),
            [
                "app/a/api.service.ts:1-5 get=W0,put=W1",
                "app/user.service.ts:2-4 get=?,put=?",
            ]
        );
    }

    /// The walked files, their module plan and their parses by language.
    type Parsed = (
        Vec<(String, String)>,
        ModuleQnames,
        HashMap<&'static str, Vec<FileParse>>,
    );

    /// GA.21: the parses of `files` the way [`infer`] makes them.
    fn parsed(files: &[(&str, &str)]) -> Parsed {
        let repo = RepoId::from_canonical("test://infer-wrappers");
        let files: Vec<(String, String)> = files
            .iter()
            .map(|(p, s)| (p.to_string(), s.to_string()))
            .collect();
        let modules = ModuleQnames::plan(&files);
        let mut parses: HashMap<&'static str, Vec<FileParse>> = HashMap::new();
        for (path, source) in &files {
            let lang = detect_language(path).expect("a code file");
            let fp = parse_one_as(
                source,
                path,
                lang,
                repo,
                &GoModules::root_only(""),
                &modules.module_qname(path),
            )
            .expect("parses");
            parses.entry(lang).or_default().push(fp);
        }
        (files, modules, parses)
    }

    /// GA.21: [`infer_entity_wrappers`] over `files` with `memo` (or none),
    /// its result and errors rendered for comparison.
    fn infer_memo(files: &[(&str, &str)], memo: Option<&mut BuildMemo>) -> String {
        let repo = RepoId::from_canonical("test://infer-wrappers");
        let (files, modules, parses) = parsed(files);
        let hashes: Vec<u64> = files
            .iter()
            .map(|(_, s)| crate::cache::content_hash(s))
            .collect();
        let mut errors = Vec::new();
        let out = infer_entity_wrappers(
            &parses,
            &files,
            &modules,
            repo,
            memo.map(|m| (hashes.as_slice(), m)),
            &mut errors,
        );
        format!("{out:?}\n{errors:?}")
    }

    const MEMO_HTTP: &str =
        "export async function api(path: string) {\n  return fetch(`/api${path}`);\n}\n";
    const MEMO_VERBS: &str = "import { api } from \"./http\";\n\nexport function getJson(path: string) {\n  return api(path);\n}\n\nexport function readResource(path: string) {\n  return getJson(`/v1${path}`);\n}\n";
    const MEMO_USERS: &str = "import { getJson } from \"../lib/verbs\";\n\nexport function listUsers() {\n  return getJson(\"/users\");\n}\n";
    const MEMO_BASE: &str = "import axios from \"axios\";\n\nexport class APIService {\n  private axiosInstance = axios.create({});\n\n  get(url: string) {\n    return this.axiosInstance.get(url);\n  }\n}\n";
    const MEMO_VIEW: &str = "import { APIService } from \"./api.service\";\n\nexport class ViewService extends APIService {\n  async fetchView() {\n    return this.get(\"/api/views/\");\n  }\n}\n";

    fn memo_files() -> Vec<(&'static str, &'static str)> {
        vec![
            ("repositories/collection.go", GROUPED),
            ("web/lib/http.ts", MEMO_HTTP),
            ("web/lib/verbs.ts", MEMO_VERBS),
            ("web/features/users.ts", MEMO_USERS),
            ("web/services/api.service.ts", MEMO_BASE),
            ("web/services/view.service.ts", MEMO_VIEW),
        ]
    }

    #[test]
    fn the_memo_replays_what_a_fresh_inference_reads() {
        let files = memo_files();
        let fresh = infer_memo(&files, None);
        assert!(
            fresh.contains("call: \"getJson\"")
                && fresh.contains("call: \"readResource\"")
                && fresh.contains("class: \"APIService\"")
                && fresh.contains("call: \"NewCollection\""),
            "the fixture holds direct, forwarding, heritage and Go wrappers: {fresh}"
        );
        let mut memo = BuildMemo::default();
        assert_eq!(infer_memo(&files, Some(&mut memo)), fresh, "a cold memo");
        let w = memo.wrapper_stats;
        assert_eq!((w.wrappers_hit, w.wrappers_miss), (0, 6));
        assert_eq!(memo.wrappers.len(), 6, "one record per inference file");
        assert_eq!(infer_memo(&files, Some(&mut memo)), fresh, "a warm memo");
        let w = memo.wrapper_stats;
        assert_eq!((w.wrappers_hit, w.wrappers_miss), (6, 0));

        // An edited file is the one miss, and the replay equals a fresh run
        // over the edited tree: `send` joins the frontier, so every
        // unchanged file's depth-2 reading is retaken for the new frontier.
        let http = format!(
            "{MEMO_HTTP}\nexport async function send(path: string) {{\n  return fetch(`/api${{path}}`, {{ method: \"POST\" }});\n}}\n"
        );
        let verbs = format!(
            "{MEMO_VERBS}\nexport function postJson(path: string) {{\n  return send(path);\n}}\n"
        );
        let mut edited: Vec<(&str, &str)> = files.clone();
        edited[2].1 = &verbs;
        assert_eq!(
            infer_memo(&edited, Some(&mut memo)),
            infer_memo(&edited, None)
        );
        assert_eq!(memo.wrapper_stats.wrappers_miss, 1);
        edited[1].1 = &http;
        let want = infer_memo(&edited, None);
        assert!(want.contains("call: \"postJson\""), "{want}");
        assert_eq!(infer_memo(&edited, Some(&mut memo)), want, "edited");
        assert_eq!(
            (
                memo.wrapper_stats.wrappers_hit,
                memo.wrapper_stats.wrappers_miss
            ),
            (5, 1)
        );

        // A removed file: the rest replays, equal to a fresh run.
        let mut removed = edited.clone();
        removed.remove(4);
        let want = infer_memo(&removed, None);
        assert!(!want.contains("class: \"APIService\""), "{want}");
        assert_eq!(infer_memo(&removed, Some(&mut memo)), want, "removed");
        assert_eq!(memo.wrapper_stats.wrappers_hit, 5);
    }

    #[test]
    fn a_moved_module_id_misses() {
        let files = memo_files();
        let (files, modules, parses) = parsed(&files);
        let hashes: Vec<u64> = files
            .iter()
            .map(|(_, s)| crate::cache::content_hash(s))
            .collect();
        let mut memo = BuildMemo::default();
        let run = |repo: &str, memo: &mut BuildMemo| {
            infer_entity_wrappers(
                &parses,
                &files,
                &modules,
                RepoId::from_canonical(repo),
                Some((hashes.as_slice(), memo)),
                &mut Vec::new(),
            );
            (
                memo.wrapper_stats.wrappers_hit,
                memo.wrapper_stats.wrappers_miss,
            )
        };
        assert_eq!(run("test://infer-wrappers", &mut memo), (0, 6));
        assert_eq!(run("test://infer-wrappers", &mut memo), (6, 0));
        assert_eq!(
            run("test://elsewhere", &mut memo),
            (0, 6),
            "every MODULE id moved: every key misses"
        );
    }
}
