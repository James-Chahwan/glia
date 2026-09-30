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
//! wrappers around the other needles are out of scope (every stanza carries
//! `languages = ["go"]`).
//!
//! Output sorted by (call, definition file). Each file's reading runs under
//! `catch_unwind`; a panic pushes `<path>: PANIC (inferred wrappers)`. The
//! `[wrappers] inferred` marker is `WrapperPass::report`'s.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use glia_code_domain::glia_config::{Origin, WrapperDecl};
use glia_code_domain::{FileParse, node_kind};
use glia_code_extractors::{anchor, data_entities};
use glia_core::{NodeId, RepoId};

use super::wrappers::{arg_region, commented_at, live_call_args, split_args};
use crate::extract::detect_language;
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

/// A wrapper before the precision gate.
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

/// Infer the repo's Go collection wrappers (see the module doc). Reads the
/// parses and the walked sources; changes nothing.
pub(crate) fn infer_entity_wrappers(
    parses_by_lang: &HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    modules: &ModuleQnames,
    repo: RepoId,
    parse_errors: &mut Vec<String>,
) -> InferredWrappers {
    let mut out = InferredWrappers::default();
    let parses: HashMap<NodeId, &FileParse> = parses_by_lang
        .get(LANG)
        .map(|ps| {
            ps.iter()
                .filter_map(|fp| fp.nodes.first().map(|n| (n.id, fp)))
                .collect()
        })
        .unwrap_or_default();
    let go: Vec<GoFile<'_>> = files
        .iter()
        .filter(|(path, _)| detect_language(path) == Some(LANG))
        .map(|(path, source)| GoFile {
            path,
            source,
            parse: parses.get(&modules.module_id(path, repo)).copied(),
        })
        .collect();
    if go.is_empty() {
        return out;
    }
    let defined = defined_names(parses_by_lang);
    let mut seen: HashSet<String> = HashSet::new();

    // 1. DIRECT: every needle holds "ollection".
    let mut found: Vec<Candidate> = Vec::new();
    for f in go.iter().filter(|f| f.source.contains("ollection")) {
        match catch_unwind(AssertUnwindSafe(|| direct_candidates(f))) {
            Ok((cands, unparsed)) => {
                found.extend(cands);
                out.skipped_unparsed += unparsed;
            }
            Err(_) => parse_errors.push(format!("{}: PANIC (inferred wrappers)", f.path)),
        }
    }
    let mut frontier = admit(found, 1, &mut out, &mut seen, &defined);

    // 2. FORWARDING.
    for depth in 2..=MAX_DEPTH {
        if frontier.is_empty() {
            break;
        }
        let mut found: Vec<Candidate> = Vec::new();
        for (call, name_arg) in &frontier {
            for f in go.iter().filter(|f| f.source.contains(call.as_str())) {
                match catch_unwind(AssertUnwindSafe(|| {
                    forwarding_candidates(f, call, *name_arg)
                })) {
                    Ok((cands, unparsed)) => {
                        found.extend(cands);
                        out.skipped_unparsed += unparsed;
                    }
                    Err(_) => parse_errors.push(format!("{}: PANIC (inferred wrappers)", f.path)),
                }
            }
        }
        frontier = admit(found, depth, &mut out, &mut seen, &defined);
    }

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
    let mut out: HashMap<&str, usize> = HashMap::new();
    for fp in parses_by_lang.get(LANG).into_iter().flatten() {
        for n in &fp.nodes {
            let callable = fp
                .nav
                .kind_by_id
                .get(&n.id)
                .is_some_and(|k| *k == node_kind::FUNCTION || *k == node_kind::METHOD);
            if let Some(name) = fp.nav.name_by_id.get(&n.id).filter(|_| callable) {
                *out.entry(name.as_str()).or_default() += 1;
            }
        }
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
        let out = infer_entity_wrappers(&parses, &files, &modules, repo, &mut errors);
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
}
