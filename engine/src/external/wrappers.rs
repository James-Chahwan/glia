//! The overlay wrapper stage (LF.2e): every `.glia/overlay.toml`
//! `[[wrapper]]` stanza turns the call sites of a hand-rolled HTTP / queue
//! wrapper into the sink node a direct call would have minted. The commonest
//! client blind spot is `request('GET', '/users')` over
//! `function request(method, path) { return fetch(path, { method }) }`: the
//! extractors see only the inner `fetch(path)` (`endpoint:GET:<unresolved>`),
//! while the identity lives at every call site of `request`.
//!
//! A stanza names the callee (`call`, a bare or dotted name: `request`,
//! `api.request`, `ApiClient.request`) and where the identity sits:
//! - `kind = "http"`: `method_arg` / `path_arg` (or a fixed `method`), or
//!   `receiver = true`, a client object whose HTTP-verb members ARE the calls
//!   (`api.get('/orders')`: the method is the member name, matched
//!   case-insensitively so C#'s `Get(` and Dart's `get(` both read; the path
//!   is argument `path_arg`, default 0);
//! - `kind = "queue_producer"` / `"queue_consumer"`: `topic_arg`, and an
//!   optional `broker` (a queue family tag: `kafka`, `nats`, ...);
//! - `kind = "data_entity"` (LG.3d): `name_arg` and a `flavor` (`sql` /
//!   `nosql` / `graph`, default `nosql`), a project-local table / collection
//!   constructor such as quokka's `NewCollection[T](client, db, "rooms")`,
//!   whose driver call (`.Collection(name)`) the data-entity extractor sees
//!   only with a variable.
//!
//! SITES. A hit is `<call>(` (receiver: `<call>.<verb>(`), with an optional
//! generic argument list, `<...>` or `[...]`, directly before the paren
//! (`api.get<Order[]>(`, `NewCollection[chatModels.ChatPreview](`), where
//! the byte before `<call>` is not an identifier byte (`xrequest(` does not
//! match; `this.request(` / `this.api.get(` do) and, for the plain form, the
//! name ends at the paren (`requestId(` does not match). Per needle (one per
//! plain stanza, one per verb for a receiver) at most
//! `queue_topic::MAX_HITS_PER_NEEDLE` hits are read, so a generated file
//! cannot blow up the pass. Then, in this order:
//! - a DEFINITION is not a site: the identifier token right before the
//!   callee is `function` / `def` / `fn` / `func` / `fun`; or the text after
//!   the closing paren opens a body (`{`) or a return annotation (`:`) and
//!   the argument list is typed (a depth-0 `:` before any quote,
//!   `request(method: string, ...): Promise<X> {`); or a body opens right
//!   after an argument list that holds no quote at all
//!   (`public Response Request(string m, string p) {`);
//! - a COMMENTED call is a site, never minted (`skipped_comment`): its line,
//!   trimmed, starts with `//`, `#`, `*`, `--` or `/*`, or an in-line `//`,
//!   an unclosed `/*` or a whitespace-delimited `# ` precedes it outside
//!   quotes;
//! - the arguments are read out of the parenthesised region (quote- and
//!   escape-aware, at most `queue_topic::MAX_REGION` bytes), split on depth-0
//!   commas. An identity argument must be exactly ONE string literal (an
//!   optional `name=` / `name:` label and an `f` / `r` / `b` / `u` / `$` /
//!   `@` prefix are allowed; a Ruby / Elixir `:symbol` for a method or a
//!   topic or an entity name). Anything else (`request(v, p)`,
//!   `BASE + '/users'`, a `{placeholder}` in a topic or entity name) carries
//!   no identity and mints nothing (`skipped_nonliteral`). A literal method
//!   that is not an HTTP verb, an empty path, a topic `fold_topic` rejects,
//!   or an entity name that is empty, longer than [`MAX_ENTITY_NAME`] bytes
//!   or holds whitespace, a control character or `/` is `skipped_invalid`.
//!   The extractor's noise-word gate is NOT applied to an entity name: the
//!   stanza is an explicit declaration.
//!
//! WHY NOT `queue_topic::scan`. The packet named it as the argument reader;
//! it is the TOPIC reader: `fold_topic` trims trailing `)` / `]` / `}` and
//! folds a URL to its last segment, so `` `/orders/${id}` `` came back as
//! `/orders/${id` and `https://api.x/v1/users` as `users`. This module reads
//! the literal verbatim and hands a topic to `queue_topic::fold_topic`, so a
//! queue wrapper's topic folds exactly like the queue scanner's (ARN / URL /
//! GCP path), and an HTTP path keeps its identity.
//!
//! MINTING, per site with an identity, anchored to the innermost METHOD /
//! FUNCTION holding the site (`anchor::build_owner_index` +
//! `owner_of_line`; the file's MODULE when none):
//! - http: the ENDPOINT `code_domain::endpoint::push_client_endpoint_with`
//!   mints for `(method, path)` (a `${x}` path becomes `${…}` with the
//!   verbatim literal as the ENDPOINT_HIT `template`, the TypeScript shape, so
//!   the endpoint fold resolves it through the const table; host / query
//!   stripped as for every client call, the literal kept as `raw`), plus the
//!   CALLS edge from the owner;
//! - queue: the node the queue scanner mints (`queue_producer:<topic>` /
//!   `queue_consumer:<topic>`, POSITION at the first site, a CODE cell
//!   `{"framework":<broker|"wrapper">,["family":<broker>,]"sites":[...]}`),
//!   the module CONTAINS edge and the owner edge (`owner -USES-> producer`,
//!   `consumer -HANDLED_BY-> owner`);
//! - data_entity: the DATA_ENTITY `data_entity:<flavor>:<name>`, the qname
//!   (and nav name, and module parent) the data-entity extractor mints, so a
//!   wrapper site and a driver call collapse onto one node and `DbResolver` /
//!   SHARES_DATA_ENTITY pair it; POSITION at its first site in the file, and
//!   one `owner -ACCESSES_DATA-> entity` edge per owner (function-level; no
//!   module edge is added, and an extractor edge is left as it is). An
//!   owner edge the parse already holds (the extractor's module edge for a
//!   module-level site, a second site in one function) is a `duplicate`; a
//!   node it already holds is re-used and keeps its cells.
//!
//! INFERRED STANZAS (CA.4). Besides the overlay's stanzas, the stage takes
//! the Go collection wrappers `infer_wrappers::infer_entity_wrappers` found
//! in the code (a function handing its own parameter to the driver's
//! `.Collection(name)`, or to another such function): each is a
//! `kind = "data_entity"`, `flavor = "nosql"`, `languages = ["go"]` stanza
//! at the parameter's index, read and minted exactly like an overlay one.
//! Stanzas run in a fixed order, the overlay's first, then the inferred ones
//! sorted by (call, definition file). An overlay stanza naming the same
//! `call` shadows the inferred one (`shadowed_by_overlay`), so a repo whose
//! overlay already declares the wrapper builds byte-identical. The inference
//! is code-derived, not overlay: it runs under `--no-overlay` too.
//!
//! A minted node takes the stanza's confidence (`Origin::confidence`: `llm`
//! Weak, `human` Medium) and an ORIGIN cell
//! `{"provenance":"overlay:<llm|human>","rule":"wrapper#<n>"}` (`<n>`: the
//! stanza's 1-based position among the file's `[[wrapper]]` stanzas, the
//! loader-dropped ones counted, as `edge#<n>`); a minted edge carries EVIDENCE
//! emitter [`EMITTER`], rule `wrapper#<n>`, at the site (basis `site`).
//! An inferred stanza's node is Medium with the ORIGIN cell
//! `{"provenance":"inferred:wrapper","rule":"inferred:<call>","def":"<file>:<1-based line>"}`
//! (the wrapper's `func` line); FH.17: an inferred HTTP stanza's node minted
//! at a site in test code (`passes::is_test_file`, or the repo's declared
//! `[walk] tests` when the build reads its overlay file) writes provenance
//! `test_fixture` there, and its ENDPOINT_HIT ends `"test_site":true`, which
//! the `http` pass pairs with nothing. Its edge carries EVIDENCE emitter
//! [`INFERRED_EMITTER`] (stage `pass`: a rule inference, tiered DERIVED),
//! rule `inferred:<call>`, at the site. A
//! sink the file's parse already holds for the SAME identity at the SAME line
//! (an extractor caught the call, or two stanzas match one call) is a
//! `duplicate` and adds nothing; one it holds at another line is re-used (the
//! site adds only its edge, the node keeps its extracted cells).
//!
//! WHERE IT RUNS. `build::grafts::apply_post_cache`: [`WrapperPass::scan`]
//! first; the http half BEFORE the endpoint fold (so a `${X}` wrapper path
//! folds like any other) and the queue half after the LA.4 queue const fold
//! (which rebuilds a file's queue nodes from the queue scan and would drop a
//! wrapper node minted earlier); the data_entity sites mint in that late
//! (`Phase::Queue`) half too. All above the LB.8 owner pass, so minted
//! nodes are owner-qualified, and above the A16.4 IMPORTS filter, which
//! rewrites the raw IMPORTS cell they take here. Post-cache, so cached parses
//! get it too and the cache never holds a wrapper node. The overlay's
//! stanzas only when the build applies the overlay: `--no-overlay` drops them
//! (the edge stage prints the `[overlay] disabled` line) and keeps the
//! inferred ones. Each file's scan and each file's mint run under
//! `catch_unwind`; a panic pushes `<path>: PANIC (overlay wrappers)`.
//!
//! Markers (the fired_on lines), counting each stanza source's own sites:
//! once per repo whose overlay keeps a `[[wrapper]]` stanza,
//!   `[overlay] wrappers repo=<label> stanzas=<s> sites=<n> minted=<m> duplicate=<d> skipped_nonliteral=<x> skipped_comment=<c> skipped_invalid=<i> (http=<h> queue_producer=<p> queue_consumer=<q> data_entity=<e> receiver=<r>)`
//! and once per repo where the inference found a candidate wrapper,
//!   `[wrappers] inferred repo=<label> wrappers=<w> (direct=<d> forwarding=<f>) sites=<n> minted=<m> duplicate=<u> skipped_nonliteral=<x> skipped_comment=<c> skipped_invalid=<i> shadowed_by_overlay=<s> skipped_ambiguous=<a> skipped_unparsed=<p>`
//! and once per repo where the HTTP inference found a candidate (FH.4b, FH.17),
//!   `[http-wrappers] wrappers=<w> sites=<s> minted=<m> ambiguous=<a> test_sites=<t> phantom_dropped=<p> client_by_import=<i> options_object=<o> computed_verb=<c> repo=<label>`
//! (`wrappers` counts the kept inferred wrappers, shadowed ones included;
//! see `infer_wrappers` for the last three); GD.26 adds, before ` repo=`,
//! when the inference kept a heritage wrapper (a TS class's HTTP-verb member,
//! `infer_wrappers::HeritageWrapper`) or read a heritage site,
//!   ` heritage=<classes> heritage_sites=<s> heritage_minted=<m> ambiguous_base=<a> routes_released=<r>`
//! (`heritage` the distinct (class, file) pairs holding one; `heritage_sites`
//! / `heritage_minted` the `this.<verb>(` / `super.<verb>(` sites
//! [`scan_heritage`] read, counted like any stanza's; `ambiguous_base` the
//! live calls whose base chains disagree; `routes_released` the Express-scan
//! ROUTEs [`release_heritage_routes`] dropped), and `test_sites` counts
//! the heritage ones too. A heritage stanza is never shadowed by an overlay
//! stanza (it has no plain sites); its edges carry rule
//! [`HERITAGE_WRAPPER_RULE`] and its nodes the ORIGIN rule
//! `inferred:<class>.<verb>`. In both,
//! `sites = minted + duplicate + skipped_*`; `minted` counts sites that added
//! a sink (a node, or an edge to one), split by kind; `receiver` counts the
//! http ones read through a client receiver; `skipped_invalid` also counts
//! identity sites in a file with no parse to hang a node on.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use glia_code_domain::endpoint::{self, ClientEndpoint, HitExtras};
use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::glia_config::{
    HTTP_METHODS, LoadedConfig, OVERLAY_FILE, Origin, TestPaths, WrapperDecl, WrapperKind,
};
use glia_code_domain::{
    FileParse, GRAPH_TYPE, attach_imports_cell, cell_type, edge_category, node_kind,
};
use glia_code_extractors::{anchor, queue_topic};
use glia_core::{
    Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId,
};

use super::infer_wrappers::{
    HeritageScope, HeritageTarget, HttpShape, HttpSite, InferredWrappers, read_http_site,
};
use crate::extract::detect_language;
use crate::route::ModuleQnames;

/// The EVIDENCE emitter of every edge an overlay stanza adds: stage
/// `overlay`, component the `[[wrapper]]` section.
pub(crate) const EMITTER: &str = "overlay:wrapper";

/// The EVIDENCE emitter of every edge an inferred stanza adds (CA.4): stage
/// `pass`, a rule inference rather than a read site or a declaration.
pub(crate) const INFERRED_EMITTER: &str = "pass:inferred_wrapper";

/// The ORIGIN provenance of every node an inferred stanza mints (CA.4).
pub(crate) const INFERRED_PROVENANCE: &str = "inferred:wrapper";

/// The EVIDENCE rule of every edge an inferred HTTP stanza adds (FH.4b).
pub(crate) const WRAPPER_CALL_RULE: &str = "wrapper_call";

/// GD.26: the EVIDENCE rule of every edge a heritage wrapper's site adds (a
/// `this.<verb>(` call reaching a class's HTTP wrapper member).
pub(crate) const HERITAGE_WRAPPER_RULE: &str = "heritage_wrapper_call";

/// FH.17: the ORIGIN provenance of a node an inferred HTTP stanza mints at a
/// site in test code (the provenance stamp's own word for test code).
const TEST_PROVENANCE: &str = "test_fixture";

/// FH.17: the key appended last to the ENDPOINT_HIT of a node an inferred
/// HTTP stanza mints at a site in test code. The `http` resolver
/// (`glia_graph`'s `resolvers/http.rs`, its `TEST_SITE_MARK`) leaves an
/// ENDPOINT every hit of which carries it out of pairing, as CG.4a's
/// `"external":true` does for a third-party call.
const TEST_SITE_MARK: &str = r#""test_site":true"#;

/// The EVIDENCE rule of the TypeScript parser's FH.4a shape-rule CALLS edge.
const CALL_SHAPE_RULE: &str = "call_shape";

/// Identifier tokens that make `<token> <call>(` a definition.
const DEF_KEYWORDS: &[&str] = &["function", "def", "fn", "func", "fun"];

/// A line whose trimmed text starts with one of these is a comment.
const COMMENT_STARTS: &[&str] = &["//", "#", "*", "--", "/*"];

/// Longest `<...>` / `[...]` generic argument list stepped over between a
/// callee and `(`.
const MAX_TYPE_ARGS: usize = 256;

/// Longest data_entity name, in bytes (the stanza-id bound).
const MAX_ENTITY_NAME: usize = 128;

/// Call sites recorded in a queue node's CODE cell (the queue scanner's cap).
const MAX_SITES: usize = 16;

/// Engine languages that hold no call sites.
const NOT_CODE: &[&str] = &["proto", "graphql", "avro"];

/// Which half of the stage a [`WrapperPass::mint`] call runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Http,
    Queue,
}

/// Where a [`Stanza`] came from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StanzaSource {
    /// A kept `[[wrapper]]` stanza of the overlay; `ordinal` is 1-based,
    /// counting the loader-dropped stanzas.
    Overlay { ordinal: usize },
    /// A wrapper the code declares (CA.4): the function is defined at 0-based
    /// line `def_line0` of the walked file `def_file`.
    Inferred { def_file: String, def_line0: u32 },
}

/// One stanza the stage reads: a kept `[[wrapper]]` stanza (borrowed from the
/// loaded config) or an inferred one (owned).
struct Stanza<'c> {
    decl: Cow<'c, WrapperDecl>,
    kind: WrapperKind,
    source: StanzaSource,
    /// FH.4b: an inferred HTTP wrapper's composition (its sites read through
    /// `infer_wrappers::read_http_site`); `None` for every other stanza.
    shape: Option<HttpShape>,
    /// GD.26: a heritage wrapper's class: its sites are the `this.<verb>(` /
    /// `super.<verb>(` calls its [`HeritageScope`]s reach, never a plain
    /// `<verb>(` hit.
    heritage: Option<String>,
}

impl Stanza<'_> {
    /// `wrapper#<n>` for an overlay stanza, `inferred:<call>` for an inferred
    /// one: the EVIDENCE rule and the ORIGIN `rule`.
    fn rule(&self) -> String {
        match &self.source {
            StanzaSource::Overlay { ordinal } => format!("wrapper#{ordinal}"),
            StanzaSource::Inferred { .. } => match &self.heritage {
                Some(class) => format!("inferred:{class}.{}", self.decl.call),
                None => format!("inferred:{}", self.decl.call),
            },
        }
    }

    fn is_inferred(&self) -> bool {
        matches!(self.source, StanzaSource::Inferred { .. })
    }

    /// The confidence of every node and edge the stanza mints: the overlay
    /// origin's (`llm` Weak, `human` Medium), Medium for an inferred
    /// collection wrapper, Weak for an inferred HTTP one (FH.4b: a heuristic
    /// until a resolver pairs it).
    fn confidence(&self) -> Confidence {
        match self.source {
            StanzaSource::Overlay { .. } => self.decl.origin.confidence(),
            StanzaSource::Inferred { .. } if self.shape.is_some() => Confidence::Weak,
            StanzaSource::Inferred { .. } => Confidence::Medium,
        }
    }

    /// The ORIGIN cell of a node the stanza mints (see the module doc);
    /// `test` (FH.17: an inferred HTTP stanza's site in test code) writes the
    /// provenance `test_fixture` in place of `inferred:wrapper`.
    fn origin_cell(&self, test: bool) -> Cell {
        match &self.source {
            StanzaSource::Overlay { .. } => origin_cell(self.decl.origin, &self.rule()),
            StanzaSource::Inferred {
                def_file,
                def_line0,
            } => Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(format!(
                    r#"{{"provenance":"{}","rule":{},"def":{}}}"#,
                    if test {
                        TEST_PROVENANCE
                    } else {
                        INFERRED_PROVENANCE
                    },
                    json_str(&self.rule()),
                    json_str(&format!("{def_file}:{}", u64::from(*def_line0) + 1))
                )),
            },
        }
    }

    /// FH.17: an inferred HTTP stanza, whose sites in test code mint
    /// test-site endpoints.
    fn is_inferred_http(&self) -> bool {
        self.is_inferred() && self.shape.is_some()
    }

    /// The EVIDENCE of an edge the stanza mints at 0-based `line0` of `path`.
    fn evidence(&self, path: &str, line0: u32) -> Evidence {
        let emitter = match self.source {
            StanzaSource::Overlay { .. } => EMITTER,
            StanzaSource::Inferred { .. } => INFERRED_EMITTER,
        };
        let rule = if self.heritage.is_some() {
            HERITAGE_WRAPPER_RULE.to_string()
        } else if self.shape.is_some() {
            WRAPPER_CALL_RULE.to_string()
        } else {
            self.rule()
        };
        Evidence::emitter(emitter).rule(rule).at(path, line0)
    }

    /// The data_entity sites mint in the late half: the grafts call only
    /// these two phases, and nothing between them touches a DATA_ENTITY.
    fn phase(&self) -> Phase {
        match self.kind {
            WrapperKind::Http => Phase::Http,
            WrapperKind::QueueProducer | WrapperKind::QueueConsumer | WrapperKind::DataEntity => {
                Phase::Queue
            }
        }
    }

    /// `languages` empty = every language; else an exact engine name.
    fn applies_to(&self, lang: &str) -> bool {
        self.decl.languages.is_empty() || self.decl.languages.iter().any(|l| l == lang)
    }
}

/// What one call site reads as.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Read {
    Comment,
    NonLiteral,
    Invalid,
    /// `path` canonical; `raw` the literal it was normalised from (only when
    /// normalising changed it); `template` the verbatim `${x}` literal.
    Http {
        method: String,
        path: String,
        raw: Option<String>,
        template: Option<String>,
    },
    Queue {
        topic: String,
    },
    /// A data_entity name, verbatim.
    Entity {
        name: String,
    },
}

/// One call site of one stanza.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Site {
    /// Index into [`WrapperPass::stanzas`].
    stanza: usize,
    offset: usize,
    /// 0-based, the POSITION convention.
    line0: u32,
    /// 1-based byte column, the ENDPOINT_HIT convention.
    col1: usize,
    receiver: bool,
    read: Read,
}

/// The sites of one walked file.
struct FileSites {
    /// Index into the walked `files`.
    file: usize,
    lang: &'static str,
    sites: Vec<Site>,
}

/// The counts of one stanza source's marker (`[overlay] wrappers` or
/// `[wrappers] inferred`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapperTally {
    pub(crate) stanzas: usize,
    pub(crate) sites: usize,
    pub(crate) minted: usize,
    pub(crate) duplicate: usize,
    pub(crate) skipped_nonliteral: usize,
    pub(crate) skipped_comment: usize,
    pub(crate) skipped_invalid: usize,
    pub(crate) http: usize,
    pub(crate) queue_producer: usize,
    pub(crate) queue_consumer: usize,
    pub(crate) data_entity: usize,
    pub(crate) receiver: usize,
    /// FH.17: minted sites of an inferred HTTP stanza in test code.
    pub(crate) test_sites: usize,
    /// GD.26: Express-scan ROUTEs released because every registration row
    /// is a heritage site ([`release_heritage_routes`]).
    pub(crate) routes_released: usize,
}

impl WrapperTally {
    fn count_read(&mut self, read: &Read) {
        self.sites += 1;
        match read {
            Read::Comment => self.skipped_comment += 1,
            Read::NonLiteral => self.skipped_nonliteral += 1,
            Read::Invalid => self.skipped_invalid += 1,
            Read::Http { .. } | Read::Queue { .. } | Read::Entity { .. } => {}
        }
    }

    fn add_mint(&mut self, m: WrapperTally) {
        self.minted += m.minted;
        self.duplicate += m.duplicate;
        self.skipped_invalid += m.skipped_invalid;
        self.http += m.http;
        self.queue_producer += m.queue_producer;
        self.queue_consumer += m.queue_consumer;
        self.data_entity += m.data_entity;
        self.receiver += m.receiver;
        self.test_sites += m.test_sites;
        self.routes_released += m.routes_released;
    }
}

/// One [`WrapperTally`] per stanza source, so each marker counts only its
/// own stanzas' sites.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Tallies {
    overlay: WrapperTally,
    inferred: WrapperTally,
    /// FH.4b: the inferred HTTP wrappers' sites (`[http-wrappers]`).
    http: WrapperTally,
    /// GD.26: the heritage wrappers' sites (`[http-wrappers] .. heritage_*`).
    heritage: WrapperTally,
}

impl Tallies {
    fn of(&mut self, st: &Stanza<'_>) -> &mut WrapperTally {
        if st.heritage.is_some() {
            &mut self.heritage
        } else if st.shape.is_some() {
            &mut self.http
        } else if st.is_inferred() {
            &mut self.inferred
        } else {
            &mut self.overlay
        }
    }

    fn add_mint(&mut self, m: Tallies) {
        self.overlay.add_mint(m.overlay);
        self.inferred.add_mint(m.inferred);
        self.http.add_mint(m.http);
        self.heritage.add_mint(m.heritage);
    }
}

/// What the CA.4 inference reported beyond its stanzas: the counts of the
/// `[wrappers] inferred` marker that no site produces.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct InferenceCounts {
    /// Kept inferred wrappers, shadowed ones included.
    wrappers: usize,
    direct: usize,
    forwarding: usize,
    shadowed_by_overlay: usize,
    skipped_ambiguous: usize,
    skipped_unparsed: usize,
    /// FH.4b: kept inferred HTTP wrappers (shadowed ones included), those
    /// the precision gate dropped, and client calls whose owner did not read.
    http_wrappers: usize,
    http_ambiguous: usize,
    http_unparsed: usize,
    /// FH.17: `<unresolved>` verb calls on no HTTP client that seeded
    /// nothing, and kept wrappers read through a local options object or a
    /// computed verb (`infer_wrappers`).
    http_phantom: usize,
    /// FH.17b: `<unresolved>` verb calls kept because their receiver is
    /// imported from a client module (`infer_wrappers`).
    http_client_by_import: usize,
    http_options_object: usize,
    http_computed: usize,
    /// GD.26: the distinct (class, file) pairs holding a heritage wrapper,
    /// and the heritage sites whose base chains disagree (`ambiguous_base`).
    heritage_classes: usize,
    heritage_ambiguous: usize,
}

impl InferenceCounts {
    /// The inference found a candidate collection wrapper (kept or skipped).
    fn any(&self) -> bool {
        self.wrappers + self.skipped_ambiguous + self.skipped_unparsed > 0
    }

    /// The inference found a candidate HTTP wrapper (kept or skipped).
    fn http_any(&self) -> bool {
        self.http_wrappers
            + self.http_ambiguous
            + self.http_unparsed
            + self.http_phantom
            + self.http_client_by_import
            + self.heritage_classes
            > 0
    }
}

/// One repo's wrapper stage: the scanned sites, minted in two phases.
pub(crate) struct WrapperPass<'c> {
    stanzas: Vec<Stanza<'c>>,
    files: Vec<FileSites>,
    tally: Tallies,
    /// The overlay keeps a `[[wrapper]]` stanza: the `[overlay] wrappers`
    /// marker prints.
    overlay_declared: bool,
    inference: InferenceCounts,
    /// FH.17: the repo's declared `[walk] tests` (CJ.4), read from the
    /// overlay file the build applies; `None` under `--no-overlay`, where
    /// only the path rule classes a site as test code.
    tests: Option<TestPaths>,
}

impl<'c> WrapperPass<'c> {
    /// Read every call site of `config`'s `[[wrapper]]` stanzas and of the
    /// `inferred` wrappers no overlay stanza shadows in `files`. Changes no
    /// graph. `None` when neither source holds a stanza (the caller passes
    /// `None` for `config` on a build without the overlay) and the inference
    /// found no candidate.
    pub(crate) fn scan(
        config: Option<&'c LoadedConfig>,
        mut inferred: InferredWrappers,
        files: &[(String, String)],
        parse_errors: &mut Vec<String>,
    ) -> Option<Self> {
        let tests = config.and_then(|c| c.config.walk.tests_matcher());
        let cfg = config.filter(|c| !c.config.wrapper.is_empty());
        let mut stanzas: Vec<Stanza<'c>> = Vec::new();
        if let Some(cfg) = cfg {
            let rejected = rejected_wrapper_lines(cfg);
            stanzas.extend(
                cfg.config
                    .wrapper
                    .iter()
                    .enumerate()
                    .filter_map(|(kept, s)| {
                        let line = cfg.line_of(s.span());
                        let ordinal = 1 + kept + rejected.iter().filter(|l| **l < line).count();
                        Some(Stanza {
                            decl: Cow::Borrowed(s.get_ref()),
                            kind: s.get_ref().wrapper_kind()?,
                            source: StanzaSource::Overlay { ordinal },
                            shape: None,
                            heritage: None,
                        })
                    }),
            );
        }
        let overlay_stanzas = stanzas.len();
        let mut inference = InferenceCounts {
            wrappers: inferred.wrappers.len(),
            direct: inferred.direct(),
            forwarding: inferred.forwarding(),
            skipped_ambiguous: inferred.skipped_ambiguous,
            skipped_unparsed: inferred.skipped_unparsed,
            http_wrappers: inferred.http.len(),
            http_ambiguous: inferred.http_ambiguous,
            http_unparsed: inferred.http_unparsed,
            http_phantom: inferred.http_phantom,
            http_client_by_import: inferred.http_client_by_import,
            http_options_object: inferred.http_options_object,
            http_computed: inferred.http_computed,
            heritage_classes: {
                let mut keys: Vec<(&str, &str)> = inferred
                    .heritage
                    .iter()
                    .map(|h| (h.class.as_str(), h.def_file.as_str()))
                    .collect();
                keys.sort_unstable();
                keys.dedup();
                keys.len()
            },
            ..InferenceCounts::default()
        };
        let heritage = std::mem::take(&mut inferred.heritage);
        let scopes = std::mem::take(&mut inferred.heritage_scopes);
        for w in inferred.wrappers.into_iter().chain(inferred.http) {
            if stanzas
                .iter()
                .take(overlay_stanzas)
                .any(|s| s.decl.call == w.decl.call)
            {
                if w.http.is_none() {
                    inference.shadowed_by_overlay += 1;
                }
                continue;
            }
            let Some(kind) = w.decl.wrapper_kind() else {
                continue;
            };
            stanzas.push(Stanza {
                decl: Cow::Owned(w.decl),
                kind,
                source: StanzaSource::Inferred {
                    def_file: w.def_file,
                    def_line0: w.def_line0,
                },
                shape: w.http,
                heritage: None,
            });
        }
        // GD.26: the heritage wrappers follow, in their sorted order, so a
        // scope's `HeritageTarget::Wrapper(i)` is stanza `heritage_base + i`.
        let heritage_base = stanzas.len();
        for h in heritage {
            stanzas.push(Stanza {
                decl: Cow::Owned(h.decl),
                kind: WrapperKind::Http,
                source: StanzaSource::Inferred {
                    def_file: h.def_file,
                    def_line0: h.def_line0,
                },
                shape: Some(h.shape),
                heritage: Some(h.class),
            });
        }
        if cfg.is_none() && !inference.any() && !inference.http_any() {
            return None;
        }
        let mut pass = WrapperPass {
            tally: Tallies {
                overlay: WrapperTally {
                    stanzas: overlay_stanzas,
                    ..WrapperTally::default()
                },
                inferred: WrapperTally {
                    stanzas: stanzas.len() - overlay_stanzas,
                    ..WrapperTally::default()
                },
                http: WrapperTally::default(),
                heritage: WrapperTally::default(),
            },
            stanzas,
            files: Vec::new(),
            overlay_declared: cfg.is_some(),
            inference,
            tests,
        };
        // Looked up by path, never iterated.
        let mut scopes_of: HashMap<&str, Vec<&HeritageScope>> = HashMap::new();
        for sc in &scopes {
            scopes_of.entry(sc.file.as_str()).or_default().push(sc);
        }
        for (i, (path, source)) in files.iter().enumerate() {
            let Some(lang) = detect_language(path) else {
                continue;
            };
            let file_scopes = scopes_of.get(path.as_str()).map_or(&[][..], Vec::as_slice);
            if NOT_CODE.contains(&lang)
                || (file_scopes.is_empty()
                    && !pass.stanzas.iter().any(|s| {
                        s.heritage.is_none()
                            && s.applies_to(lang)
                            && source.contains(s.decl.call.as_str())
                    }))
            {
                continue;
            }
            let read = catch_unwind(AssertUnwindSafe(|| {
                let mut sites = scan_file(source, lang, &pass.stanzas);
                let (heritage_sites, ambiguous) =
                    scan_heritage(source, file_scopes, heritage_base, &pass.stanzas);
                sites.extend(heritage_sites);
                sites.sort_by_key(|s| (s.offset, s.stanza));
                (sites, ambiguous)
            }));
            match read {
                Ok((sites, ambiguous)) => {
                    pass.inference.heritage_ambiguous += ambiguous;
                    if sites.is_empty() {
                        continue;
                    }
                    for s in &sites {
                        if let Some(st) = pass.stanzas.get(s.stanza) {
                            pass.tally.of(st).count_read(&s.read);
                        }
                    }
                    pass.files.push(FileSites {
                        file: i,
                        lang,
                        sites,
                    });
                }
                Err(_) => parse_errors.push(format!("{path}: PANIC (overlay wrappers)")),
            }
        }
        Some(pass)
    }

    /// Mint the `phase` half's sites onto their files' parses (found the
    /// `apply_rpc_needles` way: the parse whose first node is the file's
    /// MODULE in the LB.9b plan `modules`).
    pub(crate) fn mint(
        &mut self,
        phase: Phase,
        parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
        files: &[(String, String)],
        repo: RepoId,
        modules: &ModuleQnames,
        parse_errors: &mut Vec<String>,
    ) {
        for fs in &self.files {
            let sites: Vec<&Site> = fs
                .sites
                .iter()
                .filter(|s| {
                    self.stanzas
                        .get(s.stanza)
                        .is_some_and(|st| st.phase() == phase)
                        && matches!(
                            s.read,
                            Read::Http { .. } | Read::Queue { .. } | Read::Entity { .. }
                        )
                })
                .collect();
            if sites.is_empty() {
                continue;
            }
            let Some((path, _)) = files.get(fs.file) else {
                continue;
            };
            let module_id = modules.module_id(path, repo);
            let fp = parses_by_lang.get_mut(fs.lang).and_then(|ps| {
                ps.iter_mut()
                    .find(|fp| fp.nodes.first().is_some_and(|n| n.id == module_id))
            });
            let Some(fp) = fp else {
                // The file failed to parse: no module to hang a sink on.
                for s in &sites {
                    if let Some(st) = self.stanzas.get(s.stanza) {
                        self.tally.of(st).skipped_invalid += 1;
                    }
                }
                continue;
            };
            let at = FileAt {
                path,
                lang: fs.lang,
                module_id,
                repo,
                test: crate::passes::is_test_file(path)
                    || self.tests.as_ref().is_some_and(|m| m.matches(path)),
            };
            let stanzas = &self.stanzas;
            let minted = catch_unwind(AssertUnwindSafe(|| match phase {
                Phase::Http => mint_http(fp, &sites, stanzas, &at),
                Phase::Queue => {
                    let mut t = mint_queue(fp, &sites, stanzas, &at);
                    t.add_mint(mint_entities(fp, &sites, stanzas, &at));
                    t
                }
            }));
            match minted {
                Ok(m) => self.tally.add_mint(m),
                Err(_) => parse_errors.push(format!("{path}: PANIC (overlay wrappers)")),
            }
        }
    }

    /// The fired_on markers (see the module doc): `[overlay] wrappers` when
    /// the overlay keeps a `[[wrapper]]` stanza, `[wrappers] inferred` when
    /// the inference found a candidate.
    pub(crate) fn report(&self, repo_label: &str) {
        if self.overlay_declared {
            let t = &self.tally.overlay;
            eprintln!(
                "[overlay] wrappers repo={repo_label} stanzas={} sites={} minted={} duplicate={} skipped_nonliteral={} skipped_comment={} skipped_invalid={} (http={} queue_producer={} queue_consumer={} data_entity={} receiver={})",
                t.stanzas,
                t.sites,
                t.minted,
                t.duplicate,
                t.skipped_nonliteral,
                t.skipped_comment,
                t.skipped_invalid,
                t.http,
                t.queue_producer,
                t.queue_consumer,
                t.data_entity,
                t.receiver
            );
        }
        let inf = &self.inference;
        if inf.any() {
            let t = &self.tally.inferred;
            eprintln!(
                "[wrappers] inferred repo={repo_label} wrappers={} (direct={} forwarding={}) sites={} minted={} duplicate={} skipped_nonliteral={} skipped_comment={} skipped_invalid={} shadowed_by_overlay={} skipped_ambiguous={} skipped_unparsed={}",
                inf.wrappers,
                inf.direct,
                inf.forwarding,
                t.sites,
                t.minted,
                t.duplicate,
                t.skipped_nonliteral,
                t.skipped_comment,
                t.skipped_invalid,
                inf.shadowed_by_overlay,
                inf.skipped_ambiguous,
                inf.skipped_unparsed
            );
        }
        if inf.http_any() {
            let t = &self.tally.http;
            let h = &self.tally.heritage;
            // GD.26: the heritage fields only where a heritage wrapper or
            // site exists, so every other repo's line is unchanged.
            let heritage = if inf.heritage_classes + h.sites + inf.heritage_ambiguous > 0 {
                format!(
                    " heritage={} heritage_sites={} heritage_minted={} ambiguous_base={} routes_released={}",
                    inf.heritage_classes,
                    h.sites,
                    h.minted,
                    inf.heritage_ambiguous,
                    h.routes_released
                )
            } else {
                String::new()
            };
            eprintln!(
                "[http-wrappers] wrappers={} sites={} minted={} ambiguous={} test_sites={} phantom_dropped={} client_by_import={} options_object={} computed_verb={}{heritage} repo={repo_label}",
                inf.http_wrappers,
                t.sites,
                t.minted,
                inf.http_ambiguous,
                t.test_sites + h.test_sites,
                inf.http_phantom,
                inf.http_client_by_import,
                inf.http_options_object,
                inf.http_computed
            );
        }
    }
}

/// The 1-based lines of the `[[wrapper]]` stanzas the loader dropped, read
/// from its errors (`.glia/overlay.toml:<line>: [[wrapper]] ...`), so
/// `wrapper#<n>` is the number a reader counts in the file.
fn rejected_wrapper_lines(cfg: &LoadedConfig) -> Vec<u32> {
    let prefix = format!("{OVERLAY_FILE}:");
    cfg.errors
        .iter()
        .filter_map(|e| {
            let (line, msg) = e.strip_prefix(prefix.as_str())?.split_once(": ")?;
            msg.starts_with("[[wrapper]] ")
                .then(|| line.parse().ok())
                .flatten()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Scanning: text only, no graph.
// ---------------------------------------------------------------------------

/// Every site of every stanza that applies to `lang`, in source order. A
/// heritage stanza has no plain sites ([`scan_heritage`] reads them).
fn scan_file(source: &str, lang: &str, stanzas: &[Stanza<'_>]) -> Vec<Site> {
    let mut out = Vec::new();
    for (si, st) in stanzas.iter().enumerate() {
        if !st.applies_to(lang) || st.heritage.is_some() || !source.contains(st.decl.call.as_str())
        {
            continue;
        }
        for hit in call_hits(source, &st.decl.call, st.decl.receiver) {
            if let Some(site) = read_site(source, &hit, si, st) {
                out.push(site);
            }
        }
    }
    out.sort_by_key(|s| (s.offset, s.stanza));
    out
}

/// GD.26: the heritage sites of one file: every `this.<verb>(` /
/// `super.<verb>(` call (the plain site rules: generic list allowed, not a
/// definition; a commented call is a site, never minted) on a line of a
/// [`HeritageScope`] that is no nested class's, read through the wrapper
/// the scope's verb reaches (stanza `base + i`). A live call whose verb is
/// [`HeritageTarget::Ambiguous`] there is no site, counted (the second value).
fn scan_heritage(
    source: &str,
    scopes: &[&HeritageScope],
    base: usize,
    stanzas: &[Stanza<'_>],
) -> (Vec<Site>, usize) {
    let mut out = Vec::new();
    let mut ambiguous = 0usize;
    for sc in scopes {
        for (verb, target) in &sc.targets {
            for recv in ["this", "super"] {
                for hit in call_hits(source, &format!("{recv}.{verb}"), false) {
                    let line = anchor::line_of(source, hit.start);
                    if line < sc.start_line0
                        || line > sc.end_line0
                        || sc.inner.iter().any(|(s, e)| *s <= line && line <= *e)
                    {
                        continue;
                    }
                    match *target {
                        HeritageTarget::Wrapper(i) => {
                            let si = base + i;
                            if let Some(site) = stanzas
                                .get(si)
                                .and_then(|st| read_site(source, &hit, si, st))
                            {
                                out.push(site);
                            }
                        }
                        HeritageTarget::Ambiguous => {
                            let prefix = line_prefix(source, hit.start).1;
                            let (region, close) = arg_region(source, hit.open + 1);
                            let after = close.and_then(|c| source.get(c + 1..)).unwrap_or("");
                            if !is_definition(prefix, region, after) && !is_commented(prefix) {
                                ambiguous += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    (out, ambiguous)
}

pub(super) fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// One call-shaped occurrence of a stanza's callee.
struct Hit {
    /// Byte offset of the callee (the receiver, for the receiver form).
    start: usize,
    /// Byte offset of the `(`.
    open: usize,
    /// The receiver form's verb, upper-case (an [`HTTP_METHODS`] entry).
    verb: Option<&'static str>,
}

/// The call-shaped occurrences of `call(` (receiver: `call.<verb>(`), at most
/// `MAX_HITS_PER_NEEDLE` per needle.
fn call_hits(source: &str, call: &str, receiver: bool) -> Vec<Hit> {
    let b = source.as_bytes();
    let needle = if receiver {
        format!("{call}.")
    } else {
        call.to_string()
    };
    let mut out = Vec::new();
    if call.is_empty() {
        return out;
    }
    // Slot 0: the plain needle; slot 1 + i: the receiver's HTTP_METHODS[i].
    let mut taken = vec![0usize; HTTP_METHODS.len() + 1];
    for (start, _) in source.match_indices(needle.as_str()) {
        if start
            .checked_sub(1)
            .and_then(|i| b.get(i))
            .is_some_and(|c| is_ident_byte(*c))
        {
            continue;
        }
        let mut at = start + needle.len();
        let mut slot = 0usize;
        let verb = if receiver {
            let len = b
                .get(at..)
                .map_or(0, |r| r.iter().take_while(|c| is_ident_byte(**c)).count());
            let Some(word) = source.get(at..at + len) else {
                continue;
            };
            let Some(i) = HTTP_METHODS
                .iter()
                .position(|m| m.eq_ignore_ascii_case(word))
            else {
                continue;
            };
            at += len;
            slot = i + 1;
            HTTP_METHODS.get(i).copied()
        } else {
            None
        };
        if matches!(b.get(at), Some(b'<' | b'[')) {
            match skip_type_args(b, at) {
                Some(end) => at = end,
                None => continue,
            }
        }
        if b.get(at) != Some(&b'(') {
            continue;
        }
        let Some(n) = taken.get_mut(slot) else {
            continue;
        };
        if *n >= queue_topic::MAX_HITS_PER_NEEDLE {
            continue;
        }
        *n += 1;
        out.push(Hit {
            start,
            open: at,
            verb,
        });
    }
    out
}

/// The byte after a balanced generic argument list starting at `at`: `<...>`
/// (TypeScript / Java / C# / Dart) or `[...]` (Go, Scala), nested lists of
/// either kind matched pairwise (`<Order[]>`, `[map[string]int]`), ASCII, on
/// one line and within [`MAX_TYPE_ARGS`] bytes. `None` for anything that is
/// not a generic argument list (a comparison, an arrow type, a mismatched or
/// unclosed bracket).
pub(super) fn skip_type_args(b: &[u8], at: usize) -> Option<usize> {
    let mut open: Vec<u8> = Vec::new();
    for (k, &c) in b.get(at..)?.iter().enumerate().take(MAX_TYPE_ARGS) {
        match c {
            b'<' | b'[' => open.push(c),
            b'>' | b']' => {
                let want = if c == b'>' { b'<' } else { b'[' };
                if open.pop()? != want {
                    return None;
                }
                if open.is_empty() {
                    return Some(at + k + 1);
                }
            }
            b'\n' | b'\r' | b';' | b'(' | b')' | b'=' => return None,
            _ if !c.is_ascii() => return None,
            _ => {}
        }
    }
    None
}

/// The start of the line holding byte `at`, and that line's text up to `at`.
fn line_prefix(source: &str, at: usize) -> (usize, &str) {
    let line_start = source
        .get(..at)
        .and_then(|s| s.rfind('\n'))
        .map_or(0, |i| i + 1);
    (line_start, source.get(line_start..at).unwrap_or(""))
}

/// CA.4: the text before byte `at` on its line reads as a comment (the
/// COMMENTED rule of the module doc).
pub(super) fn commented_at(source: &str, at: usize) -> bool {
    is_commented(line_prefix(source, at).1)
}

/// CA.4: every live `call(` site in `source` (the plain form, generic list
/// allowed; not a definition, not commented), as the callee's byte offset
/// and its depth-0 split arguments: the inference's forwarding reader, the
/// same site rules a stanza's scan applies.
pub(super) fn live_call_args<'s>(source: &'s str, call: &str) -> Vec<(usize, Vec<&'s str>)> {
    call_hits(source, call, false)
        .into_iter()
        .filter_map(|hit| {
            let prefix = line_prefix(source, hit.start).1;
            let (region, close) = arg_region(source, hit.open + 1);
            let after = close.and_then(|c| source.get(c + 1..)).unwrap_or("");
            (!is_definition(prefix, region, after) && !is_commented(prefix))
                .then(|| (hit.start, split_args(region)))
        })
        .collect()
}

/// The site at `hit`, or `None` when the hit is a definition.
fn read_site(source: &str, hit: &Hit, stanza: usize, st: &Stanza<'_>) -> Option<Site> {
    let (line_start, prefix) = line_prefix(source, hit.start);
    let (region, close) = arg_region(source, hit.open + 1);
    let after = close.and_then(|c| source.get(c + 1..)).unwrap_or("");
    if is_definition(prefix, region, after) {
        return None;
    }
    let read = if is_commented(prefix) {
        Read::Comment
    } else {
        read_args(region, hit.verb, st)
    };
    Some(Site {
        stanza,
        offset: hit.start,
        line0: anchor::line_of(source, hit.start),
        col1: hit.start - line_start + 1,
        receiver: hit.verb.is_some(),
        read,
    })
}

/// The argument region after the `(` at `from - 1` and the offset of its
/// closing `)` (`None` when it is not closed within `MAX_REGION` bytes):
/// quote- and escape-aware, nested brackets counted.
pub(super) fn arg_region(source: &str, from: usize) -> (&str, Option<usize>) {
    let rest = source.get(from..).unwrap_or("");
    let b = rest.as_bytes();
    let limit = b.len().min(queue_topic::MAX_REGION);
    let (mut depth, mut quote, mut i) = (1i32, None::<u8>, 0usize);
    while i < limit {
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
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        // `c` is ASCII, so `i` is a char boundary.
                        return (rest.get(..i).unwrap_or(""), Some(from + i));
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    (queue_topic::clip(rest, limit), None)
}

/// See the module doc's DEFINITION rule.
pub(super) fn is_definition(prefix: &str, region: &str, after: &str) -> bool {
    let token = prefix
        .trim_end()
        .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .next()
        .unwrap_or("");
    if DEF_KEYWORDS.contains(&token) {
        return true;
    }
    let after = after.trim_start();
    let opens_body = after.starts_with('{');
    if (opens_body || after.starts_with(':')) && typed_params(region) {
        return true;
    }
    opens_body && !region.bytes().any(|c| matches!(c, b'\'' | b'"' | b'`'))
}

/// A depth-0 `:` (not `::`) before the first quote: a typed parameter list.
fn typed_params(region: &str) -> bool {
    let b = region.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => return false,
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            b':' if depth == 0 => {
                if b.get(i + 1) == Some(&b':') {
                    i += 2;
                    continue;
                }
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// See the module doc's COMMENTED rule; `prefix` is the line up to the call.
fn is_commented(prefix: &str) -> bool {
    let head = prefix.trim_start();
    if COMMENT_STARTS.iter().any(|c| head.starts_with(c)) {
        return true;
    }
    let b = prefix.as_bytes();
    let (mut quote, mut block, mut i) = (None::<u8>, false, 0usize);
    while i < b.len() {
        let c = b[i];
        if block {
            if c == b'*' && b.get(i + 1) == Some(&b'/') {
                block = false;
                i += 2;
                continue;
            }
        } else if let Some(q) = quote {
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
                b'/' if b.get(i + 1) == Some(&b'/') => return true,
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    block = true;
                    i += 2;
                    continue;
                }
                b'#' if i
                    .checked_sub(1)
                    .and_then(|p| b.get(p))
                    .is_none_or(u8::is_ascii_whitespace)
                    && b.get(i + 1).is_none_or(u8::is_ascii_whitespace) =>
                {
                    return true;
                }
                _ => {}
            }
        }
        i += 1;
    }
    block
}

/// The identity of one non-commented site.
fn read_args(region: &str, verb: Option<&'static str>, st: &Stanza<'_>) -> Read {
    let args = split_args(region);
    let arg = |i: Option<usize>| i.and_then(|i| args.get(i)).and_then(|a| literal(a));
    if let Some(shape) = &st.shape {
        return match read_http_site(shape, &args) {
            HttpSite::NonLiteral => Read::NonLiteral,
            HttpSite::Invalid => Read::Invalid,
            HttpSite::Path { method, path } => http_read(method, &path),
        };
    }
    match st.kind {
        WrapperKind::Http => {
            let method = match (verb, &st.decl.method, st.decl.method_arg) {
                (Some(v), _, _) => v.to_string(),
                (None, Some(m), _) => m.to_ascii_uppercase(),
                (None, None, i) => match arg(i) {
                    None => return Read::NonLiteral,
                    Some(l) => {
                        let m = l.body.to_ascii_uppercase();
                        if !HTTP_METHODS.contains(&m.as_str()) {
                            return Read::Invalid;
                        }
                        m
                    }
                },
            };
            let lit = match arg(st.decl.path_arg_index()) {
                Some(l) if l.quote != b':' => l,
                _ => return Read::NonLiteral,
            };
            http_read(method, lit.body)
        }
        WrapperKind::QueueProducer | WrapperKind::QueueConsumer => {
            let Some(lit) = arg(st.decl.topic_arg) else {
                return Read::NonLiteral;
            };
            // `${region}` / f"{region}": a placeholder names no topic.
            if lit.body.contains('{') {
                return Read::NonLiteral;
            }
            match queue_topic::fold_topic(lit.body) {
                Some((topic, _)) => Read::Queue { topic },
                None => Read::Invalid,
            }
        }
        WrapperKind::DataEntity => match arg(st.decl.name_arg) {
            None => Read::NonLiteral,
            Some(lit) => entity_read(lit.body),
        },
    }
}

/// A data_entity name literal: a `{placeholder}` names no entity; the name
/// is kept verbatim when it is 1..=[`MAX_ENTITY_NAME`] bytes with no
/// whitespace, control character or `/`.
fn entity_read(body: &str) -> Read {
    if body.contains('{') {
        return Read::NonLiteral;
    }
    if body.is_empty()
        || body.len() > MAX_ENTITY_NAME
        || body
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/')
    {
        return Read::Invalid;
    }
    Read::Entity {
        name: body.to_string(),
    }
}

/// An HTTP path literal as the TypeScript parser keys it: `${x}` becomes
/// `${…}` (the verbatim literal kept as `template`), then host / query
/// stripping and the canonical leading `/`, with the pre-normalisation path
/// kept as `raw` when either changed it.
fn http_read(method: String, body: &str) -> Read {
    if body.trim().is_empty() || body.contains(['\n', '\r']) {
        return Read::Invalid;
    }
    let (path, template) = if body.contains("${") {
        match placeholder_path(body) {
            Some(p) => (p, Some(body.to_string())),
            None => return Read::Invalid,
        }
    } else {
        (body.to_string(), None)
    };
    let (norm, changed) = endpoint::normalise_client_path(&path);
    let canonical = endpoint::canonical_http_path(&norm).into_owned();
    let raw = (changed || canonical != norm).then_some(path);
    Read::Http {
        method,
        path: canonical,
        raw,
        template,
    }
}

/// `body` with every balanced `${...}` replaced by `${…}`; `None` when one is
/// not closed.
fn placeholder_path(body: &str) -> Option<String> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("${") {
        out.push_str(rest.get(..i)?);
        let inner = rest.get(i + 2..)?;
        let mut depth = 1usize;
        let mut end = None;
        for (k, c) in inner.char_indices() {
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
        out.push_str("${…}");
        rest = inner.get(end? + 1..)?;
    }
    out.push_str(rest);
    Some(out)
}

/// Split an argument region on depth-0 commas (quote- and escape-aware).
pub(super) fn split_args(region: &str) -> Vec<&str> {
    let b = region.as_bytes();
    let mut out = Vec::new();
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
                b',' if depth == 0 => {
                    out.push(region.get(start..i).unwrap_or(""));
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    out.push(region.get(start..).unwrap_or(""));
    out
}

/// One literal argument: its body (between the quotes, verbatim) and its
/// quote byte (`:` for a symbol).
pub(super) struct Lit<'a> {
    pub(super) body: &'a str,
    pub(super) quote: u8,
}

/// `arg` when it is exactly one literal, after an optional `name=` /
/// `name:` label: a quoted string with an optional one-letter prefix, or a
/// `:symbol`.
pub(super) fn literal(arg: &str) -> Option<Lit<'_>> {
    let a = strip_label(arg.trim());
    let b = a.as_bytes();
    if b.first() == Some(&b':') && b.len() > 1 && b.get(1..)?.iter().all(|c| is_ident_byte(*c)) {
        return Some(Lit {
            body: a.get(1..)?,
            quote: b':',
        });
    }
    let i = usize::from(
        matches!(
            b.first(),
            Some(b'f' | b'r' | b'b' | b'u' | b'F' | b'R' | b'B' | b'U' | b'$' | b'@')
        ) && matches!(b.get(1), Some(b'\'' | b'"')),
    );
    let q = *b.get(i)?;
    if !matches!(q, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut j = i + 1;
    while let Some(&c) = b.get(j) {
        if c == b'\\' {
            j += 2;
            continue;
        }
        if c == q {
            break;
        }
        j += 1;
    }
    // Unterminated, or more follows the literal (`'/a' + id`).
    if j + 1 != b.len() {
        return None;
    }
    Some(Lit {
        body: a.get(i + 1..j)?,
        quote: q,
    })
}

/// `method="GET"` / `method: "GET"` -> `"GET"`; anything else unchanged.
fn strip_label(a: &str) -> &str {
    let b = a.as_bytes();
    let n = b.iter().take_while(|c| is_ident_byte(**c)).count();
    if n == 0 || b.first().is_some_and(u8::is_ascii_digit) {
        return a;
    }
    let rest = a.get(n..).unwrap_or("").trim_start();
    match rest.as_bytes() {
        [b'=', b'=' | b'>', ..] | [b':', b':', ..] => a,
        [b'=' | b':', ..] => rest.get(1..).unwrap_or("").trim_start(),
        _ => a,
    }
}

// ---------------------------------------------------------------------------
// Minting: one file's parse at a time.
// ---------------------------------------------------------------------------

/// The file a mint runs over.
struct FileAt<'a> {
    path: &'a str,
    lang: &'static str,
    module_id: NodeId,
    repo: RepoId,
    /// FH.17: the file is test code: the provenance stamp's path rule
    /// (`passes::is_test_file`: `_test.go`, `.spec.ts`, `test_*.py`, a test
    /// directory segment ..) or the repo's declared `[walk] tests`.
    test: bool,
}

/// `s` as a JSON string literal, quotes included.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn origin_cell(origin: Origin, rule: &str) -> Cell {
    Cell {
        kind: cell_type::ORIGIN,
        payload: CellPayload::Json(format!(
            r#"{{"provenance":"{}","rule":"{rule}"}}"#,
            origin.provenance()
        )),
    }
}

/// `nodes` onto `fp`, with the raw IMPORTS cell the router gave every other
/// node of a language-parser parse (`filter_imports_cells` rewrites it later).
fn graft_nodes(fp: &mut FileParse, nodes: Vec<Node>, lang: &str) {
    if nodes.is_empty() {
        return;
    }
    let carries_imports = fp
        .nodes
        .iter()
        .any(|n| n.cells.iter().any(|c| c.kind == cell_type::IMPORTS));
    if !carries_imports {
        fp.nodes.extend(nodes);
        return;
    }
    let mut extra = FileParse {
        nodes,
        imports: fp.imports.clone(),
        ..Default::default()
    };
    attach_imports_cell(&mut extra, lang);
    fp.nodes.extend(extra.nodes);
}

/// The `line` of every ENDPOINT_HIT on `n`.
fn hit_lines(n: &Node) -> impl Iterator<Item = usize> + '_ {
    n.cells
        .iter()
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => serde_json::from_str::<serde_json::Value>(j)
                .ok()?
                .get("line")?
                .as_u64()
                .and_then(|l| usize::try_from(l).ok()),
            _ => None,
        })
}

/// Append `"template":<t>` to the ENDPOINT_HIT payload of a freshly minted
/// node: after `raw` / `host`, the TypeScript field order.
fn add_template(n: &mut Node, template: &str) {
    let Ok(t) = serde_json::to_string(template) else {
        return;
    };
    for c in n
        .cells
        .iter_mut()
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
    {
        if let CellPayload::Json(j) = &mut c.payload
            && j.ends_with('}')
        {
            j.pop();
            j.push_str(&format!(r#","template":{t}}}"#));
        }
    }
}

/// FH.17: append [`TEST_SITE_MARK`] last to the ENDPOINT_HIT of a node an
/// inferred HTTP stanza minted at a site in test code.
fn mark_test_site(n: &mut Node) {
    for c in n
        .cells
        .iter_mut()
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
    {
        if let CellPayload::Json(j) = &mut c.payload
            && j.ends_with('}')
        {
            j.pop();
            j.push(',');
            j.push_str(TEST_SITE_MARK);
            j.push('}');
        }
    }
}

fn mint_http(
    fp: &mut FileParse,
    sites: &[&Site],
    stanzas: &[Stanza<'_>],
    at: &FileAt<'_>,
) -> Tallies {
    let mut t = Tallies::default();
    t.heritage.routes_released = release_heritage_routes(fp, sites, stanzas);
    let released = release_shape_reads(fp, sites);
    let idx = anchor::build_owner_index(&fp.nodes, &fp.nav);
    let endpoints: Vec<&Node> = fp
        .nodes
        .iter()
        .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
        .collect();
    // An ENDPOINT the parse already holds is re-used, never pushed twice.
    let mut seen: HashSet<NodeId> = endpoints.iter().map(|n| n.id).collect();
    let mut held: HashSet<(NodeId, usize)> = endpoints
        .iter()
        .flat_map(|n| hit_lines(n).map(move |l| (n.id, l)))
        .filter(|k| !released.contains(k))
        .collect();
    let mut fresh: Vec<Node> = Vec::new();
    for site in sites {
        let (
            Some(st),
            Read::Http {
                method,
                path,
                raw,
                template,
            },
        ) = (stanzas.get(site.stanza), &site.read)
        else {
            continue;
        };
        let id = endpoint::endpoint_id(at.repo, method, path);
        let line1 = usize::try_from(site.line0)
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        if !held.insert((id, line1)) {
            t.of(st).duplicate += 1;
            continue;
        }
        let owner = anchor::owner_of_line(&idx, site.line0).unwrap_or(at.module_id);
        let ep = ClientEndpoint {
            method: method.clone(),
            path: path.clone(),
            file: at.path.to_string(),
            line: line1,
            col: site.col1,
            confidence: st.confidence(),
        };
        let pushed = fresh.len();
        let test = at.test && st.is_inferred_http();
        let extras = HitExtras {
            raw: raw.as_deref(),
            host: None,
        };
        endpoint::push_client_endpoint_with(
            at.repo,
            &ep,
            extras,
            owner,
            &mut fresh,
            &mut fp.edges,
            &mut fp.nav,
            &mut seen,
        );
        if let Some(e) = fp.edges.last_mut() {
            evidence::attach(e, st.evidence(at.path, site.line0));
        }
        if fresh.len() > pushed
            && let Some(n) = fresh.last_mut()
        {
            if let Some(tpl) = template {
                add_template(n, tpl);
            }
            if test {
                mark_test_site(n);
            }
            n.cells.push(st.origin_cell(test));
        }
        let t = t.of(st);
        t.minted += 1;
        t.http += 1;
        t.receiver += usize::from(site.receiver);
        t.test_sites += usize::from(test);
    }
    graft_nodes(fp, fresh, at.lang);
    t
}

/// GD.26: a heritage site is an outbound call through the class's HTTP
/// client wrapper, so the Express scan's reading of the same call as a
/// server registration (`this.post('/api/x', data)` read like
/// `app.post('/x', handler)`) is a phantom ROUTE: a ROUTE of the file's parse
/// EVERY POSITION row of which is the line of a heritage site this file reads
/// as a path goes, with its nav entries, its refs and any edge naming it. A
/// ROUTE with one other registration row stays whole. Returns how many went.
fn release_heritage_routes(fp: &mut FileParse, sites: &[&Site], stanzas: &[Stanza<'_>]) -> usize {
    let lines: HashSet<u32> = sites
        .iter()
        .filter(|s| {
            matches!(s.read, Read::Http { .. })
                && stanzas
                    .get(s.stanza)
                    .is_some_and(|st| st.heritage.is_some())
        })
        .map(|s| s.line0)
        .collect();
    if lines.is_empty() {
        return 0;
    }
    let phantom: HashSet<NodeId> = fp
        .nodes
        .iter()
        .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE))
        .filter(|n| {
            let rows: Vec<Option<u64>> = n
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::POSITION)
                .map(|c| match &c.payload {
                    CellPayload::Json(j) | CellPayload::Text(j) => {
                        serde_json::from_str::<serde_json::Value>(j)
                            .ok()
                            .and_then(|v| v.get("start_line").and_then(serde_json::Value::as_u64))
                    }
                    CellPayload::Bytes(_) => None,
                })
                .collect();
            !rows.is_empty()
                && rows.iter().all(|r| {
                    r.and_then(|l| u32::try_from(l).ok())
                        .is_some_and(|l| lines.contains(&l))
                })
        })
        .map(|n| n.id)
        .collect();
    if phantom.is_empty() {
        return 0;
    }
    fp.nodes.retain(|n| !phantom.contains(&n.id));
    fp.edges
        .retain(|e| !phantom.contains(&e.from) && !phantom.contains(&e.to));
    fp.refs.retain(|r| !phantom.contains(&r.from));
    for id in &phantom {
        fp.nav.name_by_id.remove(id);
        fp.nav.qname_by_id.remove(id);
        fp.nav.kind_by_id.remove(id);
        if let Some(parent) = fp.nav.parent_of.remove(id)
            && let Some(kids) = fp.nav.children_of.get_mut(&parent)
        {
            kids.retain(|k| k != id);
        }
        fp.nav.children_of.remove(id);
    }
    phantom.len()
}

/// FH.4a's handoff: a call the shape rule already read (a CALLS edge to an
/// ENDPOINT with EVIDENCE rule `call_shape`) on the line of a site this
/// stage reads gives way to the stanza's reading: the edge goes, and its
/// ENDPOINT with it when no other edge of the file names it (every node of
/// that id, and its nav entry). Returns the `(endpoint, 1-based line)` pairs
/// whose node stays, so the site is not taken for a duplicate of it.
fn release_shape_reads(fp: &mut FileParse, sites: &[&Site]) -> HashSet<(NodeId, usize)> {
    let lines: HashSet<u32> = sites
        .iter()
        .filter(|s| matches!(s.read, Read::Http { .. }))
        .map(|s| s.line0)
        .collect();
    let mut released: HashSet<(NodeId, usize)> = HashSet::new();
    if lines.is_empty() {
        return released;
    }
    let mut dropped: Vec<(NodeId, u32)> = Vec::new();
    let kinds = &fp.nav.kind_by_id;
    fp.edges.retain(|e| {
        let shape_line = (e.category == edge_category::CALLS
            && kinds.get(&e.to) == Some(&node_kind::ENDPOINT))
        .then(|| Evidence::of(e))
        .flatten()
        .filter(|ev| ev.rule.as_deref() == Some(CALL_SHAPE_RULE))
        .and_then(|ev| ev.line)
        .filter(|l| lines.contains(l));
        if let Some(l) = shape_line {
            dropped.push((e.to, l));
        }
        shape_line.is_none()
    });
    for (id, line0) in dropped {
        if fp.edges.iter().any(|e| e.to == id || e.from == id) {
            let line1 = usize::try_from(line0)
                .unwrap_or(usize::MAX)
                .saturating_add(1);
            released.insert((id, line1));
            continue;
        }
        fp.nodes.retain(|n| n.id != id);
        fp.nav.name_by_id.remove(&id);
        fp.nav.qname_by_id.remove(&id);
        fp.nav.kind_by_id.remove(&id);
    }
    released
}

/// The 0-based site lines a queue node records: its POSITION start and its
/// CODE cell's `sites`.
fn queue_lines(n: &Node) -> Vec<u32> {
    let mut out: Vec<u32> = anchor::position_span(n)
        .map(|(s, _)| s)
        .into_iter()
        .collect();
    for c in n.cells.iter().filter(|c| c.kind == cell_type::CODE) {
        let CellPayload::Json(j) = &c.payload else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(j) else {
            continue;
        };
        for s in v
            .get("sites")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(l) = s
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .and_then(|l| u32::try_from(l).ok())
            {
                out.push(l);
            }
        }
    }
    out
}

/// One queue node this stage mints in a file, before it becomes a `Node`.
struct QueuePending {
    id: NodeId,
    kind: NodeKindId,
    qname: String,
    topic: String,
    /// The stanza of the first site.
    stanza: usize,
    lines: Vec<u32>,
}

fn mint_queue(
    fp: &mut FileParse,
    sites: &[&Site],
    stanzas: &[Stanza<'_>],
    at: &FileAt<'_>,
) -> Tallies {
    let mut t = Tallies::default();
    let idx = anchor::build_owner_index(&fp.nodes, &fp.nav);
    let existing: HashSet<NodeId> = fp.nodes.iter().map(|n| n.id).collect();
    let mut held: HashSet<(NodeId, u32)> =
        fp.nodes
            .iter()
            .filter(|n| {
                fp.nav.kind_by_id.get(&n.id).is_some_and(|k| {
                    *k == node_kind::QUEUE_PRODUCER || *k == node_kind::QUEUE_CONSUMER
                })
            })
            .flat_map(|n| queue_lines(n).into_iter().map(move |l| (n.id, l)))
            .collect();
    let mut edge_keys: HashSet<(NodeId, NodeId, EdgeCategoryId)> = fp
        .edges
        .iter()
        .map(|e| (e.from, e.to, e.category))
        .collect();
    let mut pending: Vec<QueuePending> = Vec::new();
    let mut owner_edges: Vec<Edge> = Vec::new();
    for site in sites {
        let (Some(st), Read::Queue { topic }) = (stanzas.get(site.stanza), &site.read) else {
            continue;
        };
        let (kind, prefix) = match st.kind {
            WrapperKind::QueueProducer => (node_kind::QUEUE_PRODUCER, "queue_producer:"),
            WrapperKind::QueueConsumer => (node_kind::QUEUE_CONSUMER, "queue_consumer:"),
            WrapperKind::Http | WrapperKind::DataEntity => continue,
        };
        let qname = format!("{prefix}{topic}");
        let id = NodeId::from_parts(GRAPH_TYPE, at.repo, kind, &qname);
        if !held.insert((id, site.line0)) {
            t.of(st).duplicate += 1;
            continue;
        }
        if !existing.contains(&id) {
            match pending.iter_mut().find(|p| p.id == id) {
                Some(p) => p.lines.push(site.line0),
                None => pending.push(QueuePending {
                    id,
                    kind,
                    qname,
                    topic: topic.clone(),
                    stanza: site.stanza,
                    lines: vec![site.line0],
                }),
            }
        }
        if let Some(owner) = anchor::owner_of_line(&idx, site.line0)
            && let Some(mut e) = anchor::owner_edge(kind, id, owner)
            && edge_keys.insert((e.from, e.to, e.category))
        {
            e.confidence = st.confidence();
            evidence::attach(&mut e, st.evidence(at.path, site.line0));
            owner_edges.push(e);
        }
        let t = t.of(st);
        t.minted += 1;
        match kind {
            k if k == node_kind::QUEUE_PRODUCER => t.queue_producer += 1,
            _ => t.queue_consumer += 1,
        }
    }

    let file = queue_topic::escape_json(at.path);
    let mut fresh: Vec<Node> = Vec::with_capacity(pending.len());
    for p in pending {
        let Some(st) = stanzas.get(p.stanza) else {
            continue;
        };
        let first = p.lines.first().copied().unwrap_or(0);
        let sites = p
            .lines
            .iter()
            .take(MAX_SITES)
            .map(|l| format!(r#"{{"file":"{file}","line":{l}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let code = match st.decl.broker.as_deref() {
            Some(b) => format!(
                r#"{{"framework":"{}","family":"{}","sites":[{sites}]}}"#,
                queue_topic::escape_json(b),
                queue_topic::escape_json(&b.to_ascii_lowercase())
            ),
            None => format!(r#"{{"framework":"wrapper","sites":[{sites}]}}"#),
        };
        fresh.push(Node {
            id: p.id,
            repo: at.repo,
            confidence: st.confidence(),
            cells: vec![
                anchor::position_cell(at.path, first),
                Cell {
                    kind: cell_type::CODE,
                    payload: CellPayload::Json(code),
                },
                st.origin_cell(false),
            ],
        });
        fp.nav
            .record(p.id, &p.topic, &p.qname, p.kind, Some(at.module_id));
        if edge_keys.insert((at.module_id, p.id, edge_category::CONTAINS)) {
            let mut e = Edge {
                from: at.module_id,
                to: p.id,
                category: edge_category::CONTAINS,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            };
            evidence::attach(&mut e, st.evidence(at.path, first));
            fp.edges.push(e);
        }
    }
    fp.edges.extend(owner_edges);
    graft_nodes(fp, fresh, at.lang);
    t
}

/// The data_entity half (LG.3d): per site, the DATA_ENTITY the data-entity
/// extractor would mint for `(flavor, name)` and the ACCESSES_DATA edge from
/// the innermost METHOD / FUNCTION holding the site (the module when none).
fn mint_entities(
    fp: &mut FileParse,
    sites: &[&Site],
    stanzas: &[Stanza<'_>],
    at: &FileAt<'_>,
) -> Tallies {
    let mut t = Tallies::default();
    let idx = anchor::build_owner_index(&fp.nodes, &fp.nav);
    let mut known: HashSet<NodeId> = fp.nodes.iter().map(|n| n.id).collect();
    let mut owned: HashSet<(NodeId, NodeId)> = fp
        .edges
        .iter()
        .filter(|e| e.category == edge_category::ACCESSES_DATA)
        .map(|e| (e.from, e.to))
        .collect();
    let mut fresh: Vec<Node> = Vec::new();
    for site in sites {
        let (Some(st), Read::Entity { name }) = (stanzas.get(site.stanza), &site.read) else {
            continue;
        };
        // The loader drops a stanza whose flavor is not one of the three.
        let Some(flavor) = st.decl.entity_flavor() else {
            t.of(st).skipped_invalid += 1;
            continue;
        };
        let qname = format!("data_entity:{flavor}:{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, at.repo, node_kind::DATA_ENTITY, &qname);
        let owner = anchor::owner_of_line(&idx, site.line0).unwrap_or(at.module_id);
        if !owned.insert((owner, id)) {
            t.of(st).duplicate += 1;
            continue;
        }
        if known.insert(id) {
            fresh.push(Node {
                id,
                repo: at.repo,
                confidence: st.confidence(),
                cells: vec![
                    anchor::position_cell(at.path, site.line0),
                    st.origin_cell(false),
                ],
            });
            fp.nav
                .record(id, name, &qname, node_kind::DATA_ENTITY, Some(at.module_id));
        }
        let mut e = Edge::new(owner, id, edge_category::ACCESSES_DATA, st.confidence());
        evidence::attach(&mut e, st.evidence(at.path, site.line0));
        fp.edges.push(e);
        let t = t.of(st);
        t.minted += 1;
        t.data_entity += 1;
    }
    graft_nodes(fp, fresh, at.lang);
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::glia_config::parse_str;

    fn sites(overlay: &str, source: &str, lang: &str) -> Vec<Site> {
        let cfg = parse_str(overlay);
        assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
        let ext = match lang {
            "python" => "py",
            "go" => "go",
            _ => "ts",
        };
        let files = vec![(format!("src/a.{ext}"), source.to_string())];
        let mut errors = Vec::new();
        let pass = WrapperPass::scan(Some(&cfg), InferredWrappers::default(), &files, &mut errors)
            .expect("a wrapper stanza");
        assert!(errors.is_empty(), "{errors:?}");
        pass.files.into_iter().flat_map(|f| f.sites).collect()
    }

    const REQUEST: &str = "version = 1\n[[wrapper]]\ncall = \"request\"\nkind = \"http\"\nmethod_arg = 0\npath_arg = 1\n";

    fn http(method: &str, path: &str) -> Read {
        Read::Http {
            method: method.into(),
            path: path.into(),
            raw: None,
            template: None,
        }
    }

    #[test]
    fn reads_literals_and_skips_the_rest() {
        let src = "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\
                   export async function a() { return request('GET', '/users'); }\n\
                   // request('GET', '/legacy');\n\
                   f(); // request('GET', '/inline')\n\
                   /* request('GET', '/block') */\n\
                    * request('GET', '/jsdoc')\n\
                   const x = request(v, p);\n\
                   const y = request('GET', BASE + '/x');\n\
                   const z = request('FETCH', '/z');\n\
                   xrequest('GET', '/no'); requestId('GET', '/no');\n\
                   this.request(method=\"post\", path=\"/kw\");\n\
                   request<User>(\"DELETE\", `/users/${id}`);\n";
        let got: Vec<Read> = sites(REQUEST, src, "typescript")
            .into_iter()
            .map(|s| s.read)
            .collect();
        assert_eq!(
            got,
            [
                http("GET", "/users"),
                Read::Comment,
                Read::Comment,
                Read::Comment,
                Read::Comment,
                Read::NonLiteral,
                Read::NonLiteral,
                Read::Invalid,
                http("POST", "/kw"),
                Read::Http {
                    method: "DELETE".into(),
                    path: "/users/${…}".into(),
                    raw: None,
                    template: Some("/users/${id}".into()),
                },
            ]
        );
    }

    #[test]
    fn definitions_are_not_sites() {
        let src = "function request(method = 'GET', path = '/def') {}\n\
                   class C {\n  request(method: string = 'PUT', path: string = '/typed'): Promise<void> {\n  }\n\
                   public Response request(String m, String p) {\n  }\n}\n\
                   def request(method=\"GET\", path=\"/py\"):\n    pass\n\
                   fun request(method: String, path: String) = 1\n";
        assert!(sites(REQUEST, src, "typescript").is_empty());
    }

    #[test]
    fn receiver_form_reads_the_verb_and_the_path() {
        let overlay =
            "version = 1\n[[wrapper]]\ncall = \"api\"\nkind = \"http\"\nreceiver = true\n";
        let src = "api.get('/orders');\napi.delete('/orders/1');\napiClient.get('/x');\nthis.api.Get<Order>(\"/cs\");\napi.getUsers('/no');\napi.post(body);\n";
        let got: Vec<(bool, Read)> = sites(overlay, src, "typescript")
            .into_iter()
            .map(|s| (s.receiver, s.read))
            .collect();
        assert_eq!(
            got,
            [
                (true, http("GET", "/orders")),
                (true, http("DELETE", "/orders/1")),
                (true, http("GET", "/cs")),
                (true, Read::NonLiteral),
            ]
        );
    }

    #[test]
    fn absolute_urls_keep_their_path_and_raw() {
        let src = "request('GET', 'https://api.example.com/v1/users?page=2');\n";
        let got: Vec<Read> = sites(REQUEST, src, "typescript")
            .into_iter()
            .map(|s| s.read)
            .collect();
        assert_eq!(
            got,
            [Read::Http {
                method: "GET".into(),
                path: "/v1/users".into(),
                raw: Some("https://api.example.com/v1/users?page=2".into()),
                template: None,
            }]
        );
    }

    #[test]
    fn queue_topics_fold_like_the_queue_scanner() {
        let overlay = "version = 1\n[[wrapper]]\ncall = \"publish\"\nkind = \"queue_producer\"\ntopic_arg = 0\nbroker = \"sns\"\nlanguages = [\"python\"]\n";
        let src = "publish(\"orders\", body)\npublish('arn:aws:sns:us-east-1:123456789012:payments', b)\npublish(f\"{region}.x\", b)\n# publish(\"old\", b)\n";
        let got: Vec<Read> = sites(overlay, src, "python")
            .into_iter()
            .map(|s| s.read)
            .collect();
        assert_eq!(
            got,
            [
                Read::Queue {
                    topic: "orders".into()
                },
                Read::Queue {
                    topic: "payments".into()
                },
                Read::NonLiteral,
                Read::Comment,
            ]
        );
        // The language filter: a TypeScript file is not scanned.
        let cfg = parse_str(overlay);
        let files = vec![("src/a.ts".to_string(), src.to_string())];
        let pass = WrapperPass::scan(
            Some(&cfg),
            InferredWrappers::default(),
            &files,
            &mut Vec::new(),
        )
        .expect("stanza");
        assert!(pass.files.is_empty());
    }

    #[test]
    fn hits_per_needle_are_capped() {
        let src = "request('GET', '/a');\n".repeat(queue_topic::MAX_HITS_PER_NEEDLE + 5);
        assert_eq!(
            sites(REQUEST, &src, "typescript").len(),
            queue_topic::MAX_HITS_PER_NEEDLE
        );
    }

    #[test]
    fn multibyte_text_never_panics() {
        let src = "const s = 'é'; request('GET', '/ü/é');\n// é request('GET', '/x')\nrequest('GET', `/a/${ '}' }/b`);\nrequest('GET', '/end";
        let got: Vec<Read> = sites(REQUEST, src, "typescript")
            .into_iter()
            .map(|s| s.read)
            .collect();
        assert_eq!(got.first(), Some(&http("GET", "/ü/é")));
        assert_eq!(got.get(1), Some(&Read::Comment));
    }

    const COLLECTION: &str = "version = 1\n[[wrapper]]\ncall = \"NewCollection\"\nkind = \"data_entity\"\nname_arg = 2\n";

    fn entity(name: &str) -> Read {
        Read::Entity { name: name.into() }
    }

    #[test]
    fn entity_names_are_read_verbatim_and_checked() {
        let long = "x".repeat(MAX_ENTITY_NAME + 1);
        let src = format!(
            "package r\n\n\
             func NewCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {{\n\treturn nil\n}}\n\
             func A() {{ NewCollection[ChatPreview](c, d, \"chat_previews\") }}\n\
             func B() {{ NewCollection[map[string]int](c, d, `raw.maps`) }}\n\
             // NewCollection[ChatPreview](c, d, \"legacy_previews\")\n\
             func C(name string) {{ NewCollection[T](c, d, name) }}\n\
             func D() {{ NewCollection[T](c, d, \"a b\"); NewCollection[T](c, d, \"a/b\"); NewCollection[T](c, d, \"\") }}\n\
             func E() {{ NewCollection[T](c, d, \"{long}\"); NewCollection[T](c, d, \"rooms_{{id}}\") }}\n\
             func F() {{ NewCollection(c, d, \"plain\"); NewCollection [T](c, d, \"spaced\"); NewCollection[T>(c, d, \"mismatched\") }}\n"
        );
        let got: Vec<Read> = sites(COLLECTION, &src, "go")
            .into_iter()
            .map(|s| s.read)
            .collect();
        assert_eq!(
            got,
            [
                entity("chat_previews"),
                entity("raw.maps"),
                Read::Comment,
                Read::NonLiteral,
                Read::Invalid,
                Read::Invalid,
                Read::Invalid,
                Read::Invalid,
                Read::NonLiteral,
                entity("plain"),
            ]
        );
    }

    #[test]
    fn generic_lists_are_ascii_bounded_and_balanced() {
        let b = |s: &str| skip_type_args(s.as_bytes(), 0);
        assert_eq!(b("[ChatPreview]("), Some(13));
        assert_eq!(b("<Map<string, Order[]>>("), Some(22));
        assert_eq!(b("[map[string]int]("), Some(16));
        for bad in ["[T>(", "<T](", "[T", "<T\n>(", "<Ü>(", "[a=b]("] {
            assert_eq!(b(bad), None, "{bad:?}");
        }
        // A comparison closes its `<...>` run, but no `(` follows directly.
        assert!(call_hits("ok = less<b && c > (d);", "less", false).is_empty());
        assert!(call_hits("ok = less < b && c > (d);", "less", false).is_empty());
        let long = format!("<{}>(", "T".repeat(MAX_TYPE_ARGS));
        assert_eq!(b(&long), None, "longer than MAX_TYPE_ARGS");
        // Every kind gets the tolerance: a TypeScript generic http helper.
        let src = "export async function f() { return request<User[]>('GET', '/users'); }\n";
        let got: Vec<Read> = sites(REQUEST, src, "typescript")
            .into_iter()
            .map(|s| s.read)
            .collect();
        assert_eq!(got, [http("GET", "/users")]);
    }

    #[test]
    fn rejected_stanzas_keep_their_ordinal() {
        let overlay = "version = 1\n[[wrapper]]\ncall = \"a\"\nkind = \"nope\"\n[[wrapper]]\ncall = \"request\"\nkind = \"http\"\nmethod = \"get\"\npath_arg = 0\n";
        let cfg = parse_str(overlay);
        assert_eq!(cfg.errors.len(), 1, "{:?}", cfg.errors);
        let files = vec![("src/a.ts".to_string(), "request('/x');\n".to_string())];
        let pass = WrapperPass::scan(
            Some(&cfg),
            InferredWrappers::default(),
            &files,
            &mut Vec::new(),
        )
        .expect("stanza");
        assert_eq!(
            pass.stanzas.first().map(Stanza::rule).as_deref(),
            Some("wrapper#2")
        );
        assert_eq!(pass.files[0].sites[0].read, http("GET", "/x"));
    }
}
