//! The code domain's profile tables (`CODE_TABLES`): the per-domain dials the
//! domain-agnostic layers read.
//!
//! Module slot declared by L0.1 so its owner edits only this file. Filled by
//! LD.14a; LD.14b, LD.6 and LE.4d extend it.
//!
//! LD.14a: [`CODE_TABLES`] is the code domain's [`DomainTables`] — the data
//! half of the engine's `profile::CODE_PROFILE`, here beside the ids it names
//! so the graph, store and projection crates (which cannot depend on the
//! engine) read the same tables. Every list is the hardcoding it replaced,
//! with the same members in the same order:
//! - `entry`: the entrypoint predicate in `engine::answers` (LD.6 then
//!   reconciled it with the repo-graph wrapper's `ENTRY_KINDS` and added the
//!   inbound handler kinds both lacked, so it is the one entrypoint set);
//! - `carry_edges`: the blast carry list in `graph::blast`;
//! - `activation_weights` / `activation_presets`: the activation default and
//!   preset functions in `graph::activation`.
//!
//! LD.14b switched every consumer to these tables and deleted the originals:
//! liveness, cross-stack trace and blast radius (`MergedGraph::blast_radius`
//! takes `&CODE_TABLES`), signal resolution, `PyGraph.activate`, and the
//! store / projection research harnesses. The engine's
//! `profile::tests::code_profile_matches_head_tables` pins each list to the
//! original's values as literals. `effect_sinks` had no hardcoding: LD.14a
//! seeded it with four categories.
//!
//! LE.4d: `effect_sinks` is the table `effects` classifies with, one
//! [`EffectSink`] per (target kind, reaching category) class: the data kinds
//! over ACCESSES_DATA, every OUTBOUND marker the anchor pass
//! (`extractors::anchor`) ties to the function that owns it (QUEUE_PRODUCER,
//! EVENT_EMITTER, GRPC_CLIENT, RPC_CALL, WS_CLIENT, GRAPHQL_OPERATION, reached
//! over USES; RPC_CALL also over CALLS) and the outbound HTTP ENDPOINT
//! (reached over the CALLS the HTTP client extractor anchors, or USES).
//!
//! CD.1c: `community_weights` is the integer table communities and split
//! cuts group by, kept apart from the PPR `activation_weights` so a ranking
//! retune never moves community structure. Code that calls, handles or
//! messages other code binds hardest; DEFINES glues a symbol to its module
//! at 1; every edge that would glue unrelated code through a shared hub (a
//! PROJECT's CONTAINS, a package, a doc, a test, a shared schema / config /
//! entity, git history) is left out, so weighs 0.
//!
//! DD.3: `identity_rules` is the code domain's ways two nodes are one
//! thing, one [`IdentityRule`] per [`same_as::ALL_RULES`] name in that order
//! (the identity resolver's eight [`same_as::RULES`], then DT.1h's
//! `http_model` and DT.1k's `graphql_model`, which the models resolver
//! writes):
//! the kinds each pairs, the key they share, the tier (FACT, DERIVED or
//! HEURISTIC) and whether a class is contracted in walks. Three consumers
//! read the rows: the identity resolver (which kinds a rule pairs), the tier
//! of a SAME_AS edge, and the answers' collapse (FACT and DERIVED classes
//! collapse, HEURISTIC never). SAME_AS itself weighs 0 in PPR and is in no
//! carry or community list: identity reaches a walk or a ranking only through
//! the quotient (`glia_activation::algo::classes`, DD.1), never as an edge.
//!
//! DT.5d: [`INVOKED_MEMBERS`] is the code domain's table of members a
//! framework or the runtime invokes on an instance of their owner, with no
//! source line spelling the call: Angular's lifecycle hooks, the
//! ControlValueAccessor / Validator, PipeTransform, HttpInterceptor and
//! route-guard members, and each language's constructor. Liveness reads it
//! as a walk-local hop from a live owner to such a member (engine
//! `answers::invoked_members`), and `glia gaps` withholds such a member's
//! dead_symbol row while its owner has an incoming carry edge. Neither
//! stores an edge. [`invoked_member`] is the one matcher.
//!
//! DT.5l: the entry rule's second named row makes a Go `func init()` an
//! entry, read through [`node_ext`] (the extension of a node's POSITION
//! file, the one reader every entry caller and the invoked matcher use).
//! [`INVOKED_MEMBERS`] gains signature-checked Go runtime-interface rows
//! (`go_runtime`: `error`, `fmt.Stringer`, the marshalers, `sql.Scanner`,
//! `driver.Valuer`, `http.Handler`, `sort.Interface`), the UIKit / AppKit /
//! SwiftUI app-delegate row (`apple_app`) and the external-base rows
//! (`transloco`, Angular's ErrorHandler / Resolve / TitleStrategy /
//! RouteReuseStrategy, `dio`), whose owner must extend or implement a named
//! base of a named library ([`ExternalBase`]). [`invoked_member_at`] is the
//! matcher that reads those gates; [`invoked_member`] never matches such a
//! row.

use glia_activation::profile::{
    ActivationPreset, DomainTables, EffectSink, EntryRule, IdentityRule, IdentityTier, NamedEntry,
    Registries,
};
use glia_core::{Cell, NodeKindId};

use crate::{GRAPH_TYPE, cell_type, edge_category as ec, node_kind as nk, same_as};

/// The definitions the SCIP build stage binds a definition row to
/// (`engine/src/external/scip.rs` `DEF_KINDS`, in its order): what a SCIP
/// symbol can name on either side of a `scip_symbol` class.
const SCIP_DEF_KINDS: &[NodeKindId] = &[
    nk::FUNCTION,
    nk::METHOD,
    nk::CLASS,
    nk::STRUCT,
    nk::INTERFACE,
    nk::ENUM,
    nk::ATTRIBUTE,
    nk::STATE_VAR,
];

/// The declarations a vendored copy is matched by (`body_hash`,
/// `path_suffix`): a file's MODULE and the code declared in it.
const VENDORED_KINDS: &[NodeKindId] = &[
    nk::MODULE,
    nk::FUNCTION,
    nk::METHOD,
    nk::CLASS,
    nk::STRUCT,
    nk::INTERFACE,
    nk::ENUM,
];

/// The code domain's tables.
pub const CODE_TABLES: DomainTables = DomainTables {
    graph_type: GRAPH_TYPE,
    registries: Registries {
        node_kinds: nk::ALL,
        edge_categories: ec::ALL,
        cell_types: cell_type::ALL,
    },
    // The ONE entrypoint set (LD.6): liveness seeds from it, the engine's
    // `profile::entry_kinds()` lists it, and the dense-text `*` sigil reads
    // it. Every kind is an externally triggered inbound handler: routes, gRPC
    // services and servers, Connect / Twirp procedures, queue consumers,
    // GraphQL resolvers, WS / event handlers, CLI commands, cron jobs and
    // framework components; so are `main` and `test*` / `Test*` functions and
    // methods. A COMPONENT role (LB.3a's fold: an Angular `@Component` is a
    // CLASS carrying ROLE COMPONENT) is an entry exactly as the COMPONENT
    // kind is (LB.3b); the other roles never were. DT.5l: so is a Go
    // `func init()`, which the runtime calls when its package is linked; the
    // row is limited to `.go` files, so a TS or Python function named `init`
    // stays an ordinary name (and a Go METHOD named `init` is no FUNCTION).
    // The entry readers pass a node's extension through [`node_ext`].
    entry: EntryRule {
        kinds: &[
            nk::ROUTE,
            nk::GRPC_SERVICE,
            nk::GRPC_SERVER,
            nk::RPC_PROCEDURE,
            nk::QUEUE_CONSUMER,
            nk::GRAPHQL_RESOLVER,
            nk::WS_HANDLER,
            nk::EVENT_HANDLER,
            nk::CLI_COMMAND,
            nk::CRON_JOB,
            nk::COMPONENT,
        ],
        roles: &[nk::COMPONENT],
        named: &[
            NamedEntry {
                kinds: &[nk::FUNCTION, nk::METHOD],
                exact: &["main"],
                prefixes: &["test", "Test"],
                exts: &[],
            },
            NamedEntry { kinds: &[nk::FUNCTION], exact: &["init"], prefixes: &[], exts: &["go"] },
        ],
    },
    // The semantic edges blast radius and liveness follow. Structural
    // containment / import / define edges stay out (every node would reach
    // everything), and so does SHARES_DATA_SOURCE: a shared database is an
    // operational fact, not a code dependency (see the guard test below).
    carry_edges: &[
        ec::CALLS,
        ec::USES,
        ec::HTTP_CALLS,
        ec::GRPC_CALLS,
        ec::RPC_CALLS,
        ec::GRAPHQL_CALLS,
        ec::QUEUE_FLOWS,
        ec::WS_CONNECTS,
        ec::EVENT_FLOWS,
        ec::CLI_INVOKES,
        // LA.6a: renaming or removing a route breaks every page that links to
        // it, and page flow becomes traceable end to end.
        ec::NAVIGATES_TO,
        ec::HANDLED_BY,
        ec::INJECTS,
        ec::ACCESSES_DATA,
        ec::TESTS,
        ec::DOCUMENTS,
        ec::IMPLEMENTS,
        ec::INHERITS_FROM,
        ec::RETURNS_TYPE,
        ec::SHARES_SCHEMA,
        ec::SHARES_DATA_ENTITY,
        ec::INFRA_REFERENCES,
        ec::DEPENDS_ON,
        ec::SCHEDULES,
        ec::READS_CONFIG,
        ec::DEFINES_CONFIG,
    ],
    // LE.4d: what a function does to the outside world, as (target kind,
    // reaching category). A category alone misfires: USES also links a
    // function to symbols that are no effect, and a producer with no consumer
    // in the build has no QUEUE_FLOWS edge. The flow edges past a sink
    // (QUEUE_FLOWS, HTTP_CALLS, ...) name its receivers; they are not sinks.
    effect_sinks: &[
        EffectSink {
            class: "db",
            kinds: &[nk::DATA_ENTITY, nk::DATABASE, nk::CACHE, nk::BLOB_STORE, nk::SEARCH_INDEX],
            via: &[ec::ACCESSES_DATA],
        },
        EffectSink { class: "email", kinds: &[nk::EMAIL_SERVICE], via: &[ec::ACCESSES_DATA] },
        EffectSink { class: "queue_produce", kinds: &[nk::QUEUE_PRODUCER], via: &[ec::USES] },
        EffectSink { class: "http_call", kinds: &[nk::ENDPOINT], via: &[ec::CALLS, ec::USES] },
        EffectSink { class: "event_emit", kinds: &[nk::EVENT_EMITTER], via: &[ec::USES] },
        EffectSink {
            class: "rpc_call",
            kinds: &[nk::GRPC_CLIENT, nk::RPC_CALL],
            via: &[ec::USES, ec::CALLS],
        },
        EffectSink { class: "ws_send", kinds: &[nk::WS_CLIENT], via: &[ec::USES] },
        EffectSink { class: "graphql_op", kinds: &[nk::GRAPHQL_OPERATION], via: &[ec::USES] },
    ],
    // `calls` and cross-stack calls highest, flows and handlers next, imports
    // medium, structural edges (`contains`, `defines`) lowest; any category
    // not listed weighs 1.0. CO_CHANGES (LF.5b) weighs 0: a git-history
    // heuristic, it must not reshape any ranking (a 0 edge is skipped
    // outright, so scores are those of the graph without it). SAME_AS (DD.3)
    // weighs 0 too: identity reaches a ranking only through the quotient, and
    // a SAME_AS star at 1.0 would conduct across every repo sharing a package.
    activation_weights: &[
        (ec::CALLS, 5.0),
        (ec::HTTP_CALLS, 5.0),
        (ec::GRPC_CALLS, 5.0),
        (ec::RPC_CALLS, 5.0),
        (ec::GRAPHQL_CALLS, 5.0),
        (ec::QUEUE_FLOWS, 4.0),
        (ec::WS_CONNECTS, 4.0),
        (ec::EVENT_FLOWS, 4.0),
        (ec::CLI_INVOKES, 3.0),
        (ec::NAVIGATES_TO, 3.0),
        (ec::HANDLED_BY, 4.0),
        (ec::IMPORTS, 3.0),
        (ec::USES, 3.0),
        (ec::SHARES_SCHEMA, 2.0),
        (ec::TESTS, 2.0),
        (ec::INJECTS, 2.0),
        (ec::DEFINES, 1.0),
        (ec::CONTAINS, 1.0),
        (ec::DOCUMENTS, 0.5),
        (ec::CO_CHANGES, 0.0),
        (ec::SAME_AS, 0.0),
    ],
    // Task-tuned lenses over the base weights (WP-F / GR-5); any other name,
    // `"default"` included, is the base.
    activation_presets: &[
        // What buggy code actually touches: calls, data / config access, tests.
        ActivationPreset {
            name: "repair",
            overrides: &[
                (ec::CALLS, 8.0),
                (ec::USES, 6.0),
                (ec::ACCESSES_DATA, 6.0),
                (ec::READS_CONFIG, 5.0),
                (ec::IMPORTS, 5.0),
                (ec::TESTS, 4.0),
            ],
        },
        // What a reviewer reasons over: calls, tests, implements / inherits,
        // return types.
        ActivationPreset {
            name: "review",
            overrides: &[
                (ec::CALLS, 6.0),
                (ec::TESTS, 6.0),
                (ec::IMPLEMENTS, 5.0),
                (ec::INHERITS_FROM, 5.0),
                (ec::RETURNS_TYPE, 4.0),
            ],
        },
        // The high-level shape: entry points, modules, containment, docs.
        ActivationPreset {
            name: "onboard",
            overrides: &[
                (ec::CONTAINS, 5.0),
                (ec::IMPORTS, 5.0),
                (ec::HANDLED_BY, 6.0),
                (ec::DEFINES, 3.0),
                (ec::DOCUMENTS, 3.0),
            ],
        },
        // CC.10a: what much depends on, for one global PageRank (hotspots).
        // Every module DEFINES / CONTAINS what it declares, a heavily tested
        // function is not more depended on and a doc is no dependant, so
        // none of them makes code central.
        ActivationPreset {
            name: "centrality",
            overrides: &[
                (ec::DEFINES, 0.0),
                (ec::CONTAINS, 0.0),
                (ec::DOCUMENTS, 0.0),
                (ec::TESTS, 0.0),
            ],
        },
    ],
    // CD.1c: every category of `ec::ALL` in its order, weighted or left out
    // (weight 0) with the reason. The guard test below pins the left-out set.
    community_weights: &[
        (ec::DEFINES, 1),
        // CONTAINS out: a PROJECT / directory contains everything under it.
        (ec::IMPORTS, 1),
        (ec::CALLS, 4),
        (ec::USES, 2),
        // DOCUMENTS out: one doc names many unrelated symbols.
        // TESTS out: a test suite touches whatever it tests, across modules.
        (ec::INJECTS, 3),
        (ec::HANDLED_BY, 4),
        (ec::HTTP_CALLS, 3),
        (ec::GRPC_CALLS, 3),
        (ec::QUEUE_FLOWS, 3),
        (ec::GRAPHQL_CALLS, 3),
        (ec::WS_CONNECTS, 3),
        (ec::EVENT_FLOWS, 3),
        // SHARES_SCHEMA out: a shared schema pairs services, not their code.
        (ec::CLI_INVOKES, 2),
        (ec::ACCESSES_DATA, 2),
        (ec::HAS_ATTRIBUTE, 1),
        (ec::INHERITS_FROM, 2),
        (ec::RETURNS_TYPE, 1),
        // SHARES_DATA_ENTITY out: a shared table is an operational fact.
        (ec::SCHEDULES, 2),
        // SHARES_CRON_SCHEDULE out: one schedule string, no code link.
        (ec::READS_CONFIG, 1),
        (ec::DEFINES_CONFIG, 1),
        // SHARES_CONFIG out: one config key read in two places, no code link.
        (ec::INFRA_REFERENCES, 1),
        // SHARES_INFRA_REF out: one infra name in two places, no code link.
        // DEPENDS_ON out: a package manifest depends on whole libraries.
        // SHARES_DEPENDENCY out: two manifests naming one library.
        (ec::IMPLEMENTS, 2),
        // SHARES_DATA_SOURCE out: a shared database, as for blast radius.
        (ec::RPC_CALLS, 3),
        (ec::NAVIGATES_TO, 2),
        // CO_CHANGES out: git history is a heuristic, never structure.
        // SAME_AS out: identity reaches a walk only through the quotient (DD.1).
    ],
    // DD.3: one row per `same_as::ALL_RULES` name, in that order (the guard test
    // below pins it). `walk` is a ranking-visible dial: a class contracted in
    // a walk joins its members' neighbourhoods into one blast radius.
    identity_rules: &[
        // A class of one external package spans every repo depending on it:
        // it collapses in rows, but walking it would join every dependant of
        // `react` in every repo into one blast radius.
        IdentityRule {
            name: same_as::RULE_PACKAGE_NAME,
            from: &[nk::PACKAGE_DEP],
            to: &[nk::PACKAGE_DEP],
            key: "package:<eco>:<name>, ecosystem-normalised",
            tier: IdentityTier::Fact,
            walk: false,
        },
        // A PROJECT has no edges, so contracting it with the dependencies
        // naming it fans out only to the library's real consumers.
        IdentityRule {
            name: same_as::RULE_MANIFEST_NAME,
            from: &[nk::PACKAGE_DEP],
            to: &[nk::PROJECT],
            key: "dependency name == manifest label (per ecosystem)",
            tier: IdentityTier::Fact,
            walk: true,
        },
        IdentityRule {
            name: same_as::RULE_MESSAGE_FQNAME,
            from: &[nk::MESSAGE_TYPE],
            to: &[nk::MESSAGE_TYPE],
            key: "message:<flavor>:<fq name>",
            tier: IdentityTier::Fact,
            walk: true,
        },
        // A contract op is paired from its ORIGIN by `same_as::contract_op_key`.
        IdentityRule {
            name: same_as::RULE_CONTRACT_OP,
            from: &[nk::DOC_SECTION],
            to: &[nk::DOC_SECTION],
            key: "METHOD + normalised path, or action + channel, under one project",
            tier: IdentityTier::Derived,
            walk: true,
        },
        IdentityRule {
            name: same_as::RULE_CONTRACT_ROUTE,
            from: &[nk::DOC_SECTION],
            to: &[nk::DOC_SECTION],
            key: "the one ROUTE the op's path matches",
            tier: IdentityTier::Derived,
            walk: true,
        },
        IdentityRule {
            name: same_as::RULE_SCIP_SYMBOL,
            from: SCIP_DEF_KINDS,
            to: SCIP_DEF_KINDS,
            key: "SCIP scheme manager package descriptor",
            tier: IdentityTier::Fact,
            walk: true,
        },
        // Vendored copies are guesses: they never collapse and never walk.
        IdentityRule {
            name: same_as::RULE_BODY_HASH,
            from: VENDORED_KINDS,
            to: VENDORED_KINDS,
            key: "path-suffix files, name chain, byte-identical body",
            tier: IdentityTier::Heuristic,
            walk: false,
        },
        IdentityRule {
            name: same_as::RULE_PATH_SUFFIX,
            from: VENDORED_KINDS,
            to: VENDORED_KINDS,
            key: "path-suffix files under a confirmed vendoring offset, body differs",
            tier: IdentityTier::Heuristic,
            walk: false,
        },
        // DT.1h: written by the models resolver, not the identity one. A
        // client model (a TS / Dart INTERFACE or CLASS) and the server STRUCT
        // the exchanges carry: DERIVED (a resolver paired the exchange, the
        // field names confirm it), and contracted in walks, so a client
        // model's blast radius reaches the handlers writing its struct.
        IdentityRule {
            name: same_as::RULE_HTTP_MODEL,
            from: &[nk::INTERFACE, nk::CLASS],
            to: &[nk::STRUCT],
            key: "one HTTP exchange's client and server models, JSON field names confirm",
            tier: IdentityTier::Derived,
            walk: true,
        },
        // DT.1k: written by the models resolver. A GraphQL client call's TS
        // result model (an INTERFACE / CLASS) or gqlgen's generated Go STRUCT
        // and the SDL type the call's root field returns (a MESSAGE_TYPE
        // `message:graphql:<T>`, the class's center): DERIVED (the graphql
        // resolver paired the operation, the field names confirm it), and
        // contracted in walks like `http_model`.
        IdentityRule {
            name: same_as::RULE_GRAPHQL_MODEL,
            from: &[nk::INTERFACE, nk::CLASS, nk::STRUCT],
            to: &[nk::MESSAGE_TYPE],
            key: "a GraphQL result model or generated model and the schema type it returns",
            tier: IdentityTier::Derived,
            walk: true,
        },
    ],
};

/// DT.5l: a base type a library declares, which an [`InvokedMembers`] row's
/// owner must extend or implement: the owner's file imports `library` (its
/// IMPORTS cell lists it exactly) and the owner's heritage clause names
/// `name`, unbound in the repo (no in-repo type of that name took the ref).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalBase {
    pub library: &'static str,
    pub name: &'static str,
}

/// DT.5d: one row of [`INVOKED_MEMBERS`]: METHODs a framework or the runtime
/// calls on an instance of their owner, so a live owner makes them live.
///
/// A METHOD matches when its file's extension is one of `exts` (compared
/// ignoring ASCII case), its owner carries one of `owner_roles` (its kind
/// when that is a role kind, or a ROLE cell; empty = any owner), and its name
/// is one of `names`, or (`named_like_owner`) exactly its owner's name, the
/// constructor of Dart, Java, C# and C++.
///
/// DT.5l adds three gates, each unchecked when empty, read only by
/// [`Self::matches_at`]: `sigs`, the CA.3a-normalised signatures the member
/// must have (`()(string)`); `siblings`, the (name, signature) pairs the
/// owner must also declare (`sort.Interface` is Len + Less + Swap, and
/// `errors.Unwrap` calls an `Unwrap` only on an error); `owner_bases`, the
/// [`ExternalBase`]s of which the owner must extend or implement one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvokedMembers {
    /// What invokes the member: `angular`, `constructor`, `flutter`,
    /// `go_runtime`, `apple_app`, `transloco` or `dio`.
    pub framework: &'static str,
    pub owner_roles: &'static [NodeKindId],
    pub exts: &'static [&'static str],
    pub names: &'static [&'static str],
    pub named_like_owner: bool,
    pub sigs: &'static [&'static str],
    pub siblings: &'static [(&'static str, &'static str)],
    pub owner_bases: &'static [ExternalBase],
}

impl InvokedMembers {
    /// Does this row match `member_name` in a file of extension `ext`, owned
    /// by `owner_name` carrying `owner_roles`? The form with nothing to read
    /// a signature, a sibling or a base from: a row with `sigs`, `siblings`
    /// or `owner_bases` never matches here.
    pub fn matches(
        &self,
        owner_roles: &[NodeKindId],
        owner_name: &str,
        member_name: &str,
        ext: &str,
    ) -> bool {
        self.matches_at(owner_roles, owner_name, member_name, ext, &|| None, &|_| None, &|_| false)
    }

    /// DT.5l: [`Self::matches`] with the three readers the new gates need:
    /// `sig` gives the member's signature, `sibling_sig(n)` the signature of
    /// the owner's METHOD named `n`, and `has_base(b)` whether the owner
    /// extends or implements `b`. The cheap gates run first ([`Self::gates`]:
    /// name, extension, roles, bases); `sig` is called at most once, and the
    /// siblings only once the member's own signature matched. Every sibling's
    /// signature must equal its pair's.
    #[allow(clippy::too_many_arguments)]
    pub fn matches_at(
        &self,
        owner_roles: &[NodeKindId],
        owner_name: &str,
        member_name: &str,
        ext: &str,
        sig: &dyn Fn() -> Option<String>,
        sibling_sig: &dyn Fn(&str) -> Option<String>,
        has_base: &dyn Fn(&ExternalBase) -> bool,
    ) -> bool {
        self.gates(owner_roles, owner_name, member_name, ext, has_base)
            && self.signature_holds(sig, sibling_sig)
    }

    /// The cheap half of [`Self::matches_at`]: the name ([`Self::names_member`]),
    /// the extension, the owner's roles and (`owner_bases`) its base. What is
    /// left, when this passes, is the signature and sibling check.
    pub fn gates(
        &self,
        owner_roles: &[NodeKindId],
        owner_name: &str,
        member_name: &str,
        ext: &str,
        has_base: &dyn Fn(&ExternalBase) -> bool,
    ) -> bool {
        self.names_member(owner_name, member_name)
            && self.exts.iter().any(|e| e.eq_ignore_ascii_case(ext))
            && (self.owner_roles.is_empty()
                || self.owner_roles.iter().any(|r| owner_roles.contains(r)))
            && (self.owner_bases.is_empty() || self.owner_bases.iter().any(has_base))
    }

    /// Does this row check a signature or a sibling (a row [`Self::matches`]
    /// can never match)?
    pub fn checks_signature(&self) -> bool {
        !self.sigs.is_empty() || !self.siblings.is_empty()
    }

    /// The signature half of [`Self::matches_at`]: the member's own
    /// signature is one of `sigs` (`sig` read once, only when `sigs` is
    /// non-empty), then every sibling's is its pair's.
    fn signature_holds(
        &self,
        sig: &dyn Fn() -> Option<String>,
        sibling_sig: &dyn Fn(&str) -> Option<String>,
    ) -> bool {
        if !self.sigs.is_empty() && !sig().is_some_and(|s| self.sigs.contains(&s.as_str())) {
            return false;
        }
        self.siblings.iter().all(|(n, want)| sibling_sig(n).is_some_and(|s| s == *want))
    }

    /// The name half of [`Self::matches`], cheap enough to test before the
    /// file and the owner's roles are read.
    pub fn names_member(&self, owner_name: &str, member_name: &str) -> bool {
        !member_name.is_empty()
            && (self.names.contains(&member_name)
                || (self.named_like_owner && member_name == owner_name))
    }
}

/// A DT.5d row: a framework or the runtime invokes `names` (or the member
/// named like its owner) on an owner of `owner_roles`, in `exts` files.
const fn row(
    framework: &'static str,
    owner_roles: &'static [NodeKindId],
    exts: &'static [&'static str],
    names: &'static [&'static str],
    named_like_owner: bool,
) -> InvokedMembers {
    InvokedMembers {
        framework,
        owner_roles,
        exts,
        names,
        named_like_owner,
        sigs: &[],
        siblings: &[],
        owner_bases: &[],
    }
}

/// A DT.5l `go_runtime` row: the Go runtime or the standard library calls
/// `names` through an interface the member satisfies, so its signature must
/// be one of `sigs` and its owner must declare `siblings`.
const fn go(
    names: &'static [&'static str],
    sigs: &'static [&'static str],
    siblings: &'static [(&'static str, &'static str)],
) -> InvokedMembers {
    InvokedMembers {
        framework: "go_runtime",
        owner_roles: &[],
        exts: &["go"],
        names,
        named_like_owner: false,
        sigs,
        siblings,
        owner_bases: &[],
    }
}

/// A DT.5l external-base row: `framework`'s library calls `names` on an owner
/// extending or implementing one of `bases`, in `exts` files.
const fn based(
    framework: &'static str,
    exts: &'static [&'static str],
    bases: &'static [ExternalBase],
    names: &'static [&'static str],
) -> InvokedMembers {
    InvokedMembers {
        framework,
        owner_roles: &[],
        exts,
        names,
        named_like_owner: false,
        sigs: &[],
        siblings: &[],
        owner_bases: bases,
    }
}

const fn base(library: &'static str, name: &'static str) -> ExternalBase {
    ExternalBase { library, name }
}

/// The CA.3a signatures the go rows name, each once.
const SIG_STRING: &str = "()(string)";
const SIG_LEN: &str = "()(int)";
const SIG_LESS: &str = "(int,int)(bool)";
const SIG_SWAP: &str = "(int,int)()";

/// The TypeScript / JavaScript extensions. The walk routes `.ts` / `.tsx` /
/// `.js` / `.jsx` to the TS family parser today; `.mts` / `.cts` / `.mjs` /
/// `.cjs` (like Python's `.pyi`) match once a parser reads them.
const JS_FAMILY: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
/// An Angular member is a TypeScript member.
const TS_ONLY: &[&str] = &["ts"];
/// The languages whose constructor is the METHOD named like its class: Dart
/// (CB.9), Java, C#, and every file the C / C++ parser reads (a header's
/// in-class constructor, an out-of-line `Foo::Foo` in a source or a
/// `.inl` / `.ipp` / `.tpp` a header includes).
const CTOR_LIKE_OWNER: &[&str] = &[
    "dart", "java", "cs", "cpp", "cc", "cxx", "hpp", "hh", "h", "hxx", "inl", "ipp", "tpp",
];

/// DT.5d: the framework- and runtime-invoked members, first match wins.
///
/// - Angular calls a lifecycle hook on every instance it creates of a
///   component, directive, pipe, service or guard; the forms API calls a
///   ControlValueAccessor's / Validator's members on the component or
///   directive bound as `NG_VALUE_ACCESSOR` / `NG_VALIDATORS`; the template
///   compiler calls a pipe's `transform`; the HTTP client calls an
///   interceptor's `intercept` (a NestJS interceptor's too, an
///   `@Injectable` SERVICE in both); the router calls a guard's
///   `canActivate` family. `resolve` and `handleError` are too generic a
///   name for a role gate: DT.5l's heritage rows below read them.
/// - DI or `new` runs the constructor of every instance it makes: TS / JS
///   `constructor`, Python `__init__`, Ruby `initialize`, PHP `__construct`,
///   Swift `init`, and the METHOD named like its class in Dart (CB.9's
///   `<T>::<T>`), Java, C# and C++ (a constructor declared only in a header
///   included).
/// - DT.5e: Flutter calls a widget's `build` / `createState`, a State's
///   `initState` / `build` / `dispose` and the rest of its lifecycle, and an
///   InheritedWidget's `updateShouldNotify`, on every Dart class the parser
///   tags a COMPONENT (a widget / State subclass in a Flutter file).
/// - DT.5l `go_runtime`: the Go runtime and standard library call a method
///   through an interface its type satisfies by signature alone: `Error` /
///   `String` (`error`, `fmt.Stringer`), `Unwrap` beside an `Error`
///   (`errors.Unwrap` / `Is` / `As`), the JSON / text / BSON marshalers and
///   unmarshalers, `Scan` (`sql.Scanner`), `Value` (`driver.Valuer`),
///   `ServeHTTP` (`http.Handler`), and `Len` / `Less` / `Swap` only together
///   (`sort.Interface`). The signature, re-read from the METHOD's CODE text,
///   keeps the look-alikes out (`Scan() bool`, `Value() int64`).
/// - DT.5l `apple_app`: UIKit / AppKit call the app-delegate callbacks (the
///   `application(_:..)` overloads fold onto one METHOD) and SwiftUI an
///   `App`'s `body` on the `@main` type, which the Swift parser tags
///   COMPONENT.
/// - DT.5l external bases: Transloco calls its loader's `getTranslation`, a
///   missing handler's `handle`, an interceptor's `preSaveTranslation*`, a
///   transpiler's `transpile` and a fallback strategy's `getNextLangs`;
///   Angular an ErrorHandler's `handleError`, a Resolve's `resolve`, a
///   TitleStrategy's `updateTitle` and a RouteReuseStrategy's members; dio an
///   interceptor's `onRequest` / `onResponse` / `onError`. The owner must
///   extend or implement the library's base: its heritage ref left unbound
///   and its file importing the library (the engine's `has_base`).
///
/// engine/tests/dead_framework.rs builds a one-file graph per constructor
/// language and asserts its parser mints the constructor METHOD this table
/// names; engine/tests/dead_runtime.rs covers the DT.5l rows.
pub const INVOKED_MEMBERS: &[InvokedMembers] = &[
    row(
        "angular",
        &[nk::COMPONENT, nk::DIRECTIVE, nk::PIPE, nk::SERVICE, nk::GUARD],
        TS_ONLY,
        &[
            "ngOnInit",
            "ngOnChanges",
            "ngDoCheck",
            "ngAfterContentInit",
            "ngAfterContentChecked",
            "ngAfterViewInit",
            "ngAfterViewChecked",
            "ngOnDestroy",
        ],
        false,
    ),
    row(
        "angular",
        &[nk::COMPONENT, nk::DIRECTIVE],
        TS_ONLY,
        &[
            "writeValue",
            "registerOnChange",
            "registerOnTouched",
            "setDisabledState",
            "validate",
            "registerOnValidatorChange",
        ],
        false,
    ),
    row("angular", &[nk::PIPE], TS_ONLY, &["transform"], false),
    row("angular", &[nk::SERVICE], TS_ONLY, &["intercept"], false),
    row(
        "angular",
        &[nk::SERVICE, nk::GUARD],
        TS_ONLY,
        &["canActivate", "canActivateChild", "canDeactivate", "canMatch", "canLoad"],
        false,
    ),
    row("constructor", &[], JS_FAMILY, &["constructor"], false),
    row("constructor", &[], &["py", "pyi"], &["__init__"], false),
    row("constructor", &[], &["rb"], &["initialize"], false),
    row("constructor", &[], &["php"], &["__construct"], false),
    row("constructor", &[], &["swift"], &["init"], false),
    row("constructor", &[], CTOR_LIKE_OWNER, &[], true),
    row(
        "flutter",
        &[nk::COMPONENT],
        &["dart"],
        &[
            "build",
            "createState",
            "initState",
            "dispose",
            "didChangeDependencies",
            "didUpdateWidget",
            "deactivate",
            "activate",
            "reassemble",
            "updateShouldNotify",
        ],
        false,
    ),
    go(&["Error", "String"], &[SIG_STRING], &[]),
    go(&["Unwrap"], &["()(error)", "()([]error)"], &[("Error", SIG_STRING)]),
    go(&["MarshalJSON", "MarshalText", "MarshalBSON"], &["()([]byte,error)"], &[]),
    go(&["UnmarshalJSON", "UnmarshalText", "UnmarshalBSON"], &["([]byte)(error)"], &[]),
    go(&["Scan"], &["(any)(error)"], &[]),
    go(&["Value"], &["()(Value,error)"], &[]),
    go(&["ServeHTTP"], &["(ResponseWriter,*Request)()"], &[]),
    go(&["Len"], &[SIG_LEN], &[("Less", SIG_LESS), ("Swap", SIG_SWAP)]),
    go(&["Less"], &[SIG_LESS], &[("Len", SIG_LEN), ("Swap", SIG_SWAP)]),
    go(&["Swap"], &[SIG_SWAP], &[("Len", SIG_LEN), ("Less", SIG_LESS)]),
    row(
        "apple_app",
        &[nk::COMPONENT],
        &["swift"],
        &[
            "application",
            "applicationDidBecomeActive",
            "applicationWillResignActive",
            "applicationDidEnterBackground",
            "applicationWillEnterForeground",
            "applicationWillTerminate",
            "applicationDidReceiveMemoryWarning",
            "applicationSignificantTimeChange",
            "applicationWillFinishLaunching",
            "applicationDidFinishLaunching",
            "applicationShouldTerminate",
            "applicationShouldTerminateAfterLastWindowClosed",
            "applicationShouldHandleReopen",
            "body",
        ],
        false,
    ),
    based("transloco", TS_ONLY, &transloco("TranslocoLoader"), &["getTranslation"]),
    based("transloco", TS_ONLY, &transloco("TranslocoMissingHandler"), &["handle"]),
    based(
        "transloco",
        TS_ONLY,
        &transloco("TranslocoInterceptor"),
        &["preSaveTranslation", "preSaveTranslationKey"],
    ),
    based("transloco", TS_ONLY, &transloco("TranslocoTranspiler"), &["transpile"]),
    based("transloco", TS_ONLY, &transloco("TranslocoFallbackStrategy"), &["getNextLangs"]),
    based("angular", TS_ONLY, &[base("@angular/core", "ErrorHandler")], &["handleError"]),
    based("angular", TS_ONLY, &[base("@angular/router", "Resolve")], &["resolve"]),
    based("angular", TS_ONLY, &[base("@angular/router", "TitleStrategy")], &["updateTitle"]),
    based(
        "angular",
        TS_ONLY,
        &[
            base("@angular/router", "RouteReuseStrategy"),
            base("@angular/router", "BaseRouteReuseStrategy"),
        ],
        &["shouldDetach", "store", "shouldAttach", "retrieve", "shouldReuseRoute"],
    ),
    based(
        "dio",
        &["dart"],
        &[base("dio", "Interceptor"), base("dio", "QueuedInterceptor"), base("dio", "InterceptorsWrapper")],
        &["onRequest", "onResponse", "onError"],
    ),
];

/// Transloco's base `name`, from either of its package names (`@jsverse`
/// since v7, `@ngneat` before).
const fn transloco(name: &'static str) -> [ExternalBase; 2] {
    [base("@jsverse/transloco", name), base("@ngneat/transloco", name)]
}

/// DT.5d: the framework tag (`angular`, `constructor`, `flutter`, ...) of the
/// first [`INVOKED_MEMBERS`] row matching a METHOD called `member_name`, in a
/// file of extension `ext` (no dot), owned by `owner_name` carrying
/// `owner_roles`; `None` when none does. A row with `sigs`, `siblings` or
/// `owner_bases` never matches here: [`invoked_member_at`] reads them.
pub fn invoked_member(
    owner_roles: &[NodeKindId],
    owner_name: &str,
    member_name: &str,
    ext: &str,
) -> Option<&'static str> {
    INVOKED_MEMBERS
        .iter()
        .find(|r| r.matches(owner_roles, owner_name, member_name, ext))
        .map(|r| r.framework)
}

/// DT.5l: [`invoked_member`] through [`InvokedMembers::matches_at`], first
/// match wins. `sig` is called at most once over every row (the result is
/// kept for the next row that needs it).
#[allow(clippy::too_many_arguments)]
pub fn invoked_member_at(
    owner_roles: &[NodeKindId],
    owner_name: &str,
    member_name: &str,
    ext: &str,
    sig: &dyn Fn() -> Option<String>,
    sibling_sig: &dyn Fn(&str) -> Option<String>,
    has_base: &dyn Fn(&ExternalBase) -> bool,
) -> Option<&'static str> {
    let memo: std::cell::OnceCell<Option<String>> = std::cell::OnceCell::new();
    let once = || memo.get_or_init(sig).clone();
    INVOKED_MEMBERS
        .iter()
        .find(|r| {
            r.matches_at(owner_roles, owner_name, member_name, ext, &once, sibling_sig, has_base)
        })
        .map(|r| r.framework)
}

/// DT.5l: the extension (no dot, as written) of the file a node's POSITION
/// cell names (`glia_code_domain::evidence::locate`, so an HTTP node's span
/// fallback too); `None` with no file or a file with no extension. The one
/// reader every entry caller (`EntryRule::is_entry_at`) and the invoked
/// matcher use.
pub fn node_ext(cells: &[Cell]) -> Option<String> {
    let (file, _) = crate::evidence::locate(cells)?;
    std::path::Path::new(&file).extension().and_then(|e| e.to_str()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DT.5d: every row matches its positive case, and the role, extension
    /// and name gates keep the look-alikes out.
    #[test]
    fn invoked_members_match_their_rows() {
        let comp: &[NodeKindId] = &[nk::COMPONENT];
        let svc: &[NodeKindId] = &[nk::SERVICE];
        let none: &[NodeKindId] = &[];
        let m = invoked_member;
        // (a) lifecycle hooks, on every Angular role.
        for role in [nk::COMPONENT, nk::DIRECTIVE, nk::PIPE, nk::SERVICE, nk::GUARD] {
            for hook in INVOKED_MEMBERS[0].names {
                assert_eq!(m(&[role], "Owner", hook, "ts"), Some("angular"), "{hook}");
            }
        }
        assert_eq!(m(comp, "Page", "ngOnDestroyLater", "ts"), None, "a hook look-alike");
        assert_eq!(m(none, "Plain", "ngOnInit", "ts"), None, "no Angular role");
        assert_eq!(m(comp, "Page", "ngOnInit", "js"), None, "Angular is TypeScript");
        // (b) ControlValueAccessor / Validator members.
        for member in INVOKED_MEMBERS[1].names {
            assert_eq!(m(comp, "AmountInput", member, "ts"), Some("angular"), "{member}");
            assert_eq!(m(&[nk::DIRECTIVE], "D", member, "ts"), Some("angular"), "{member}");
            assert_eq!(m(svc, "S", member, "ts"), None, "{member} on a SERVICE");
        }
        // (c) PipeTransform.
        assert_eq!(m(&[nk::PIPE], "MoneyPipe", "transform", "ts"), Some("angular"));
        assert_eq!(m(svc, "Mapper", "transform", "ts"), None, "transform on a SERVICE");
        assert_eq!(m(comp, "Page", "transform", "ts"), None);
        // (d) HttpInterceptor (and NestJS interceptors).
        assert_eq!(m(svc, "AuthInterceptor", "intercept", "ts"), Some("angular"));
        assert_eq!(m(comp, "Page", "intercept", "ts"), None);
        // (e) route guards.
        for member in INVOKED_MEMBERS[4].names {
            assert_eq!(m(&[nk::GUARD], "AuthGuard", member, "ts"), Some("angular"), "{member}");
            assert_eq!(m(svc, "AuthGuard", member, "ts"), Some("angular"), "{member}");
            assert_eq!(m(&[nk::PIPE], "P", member, "ts"), None, "{member} on a PIPE");
        }
        // (f)-(j) constructors by name, any owner, gated by extension.
        for ext in JS_FAMILY {
            assert_eq!(m(none, "Repo", "constructor", ext), Some("constructor"), "{ext}");
        }
        assert_eq!(m(svc, "OrderService", "constructor", "ts"), Some("constructor"));
        assert_eq!(m(none, "Repo", "constructor", "go"), None, "a `constructor` in a .go file");
        assert_eq!(m(none, "Repo", "constructor", "py"), None);
        for (ext, name) in [
            ("py", "__init__"),
            ("pyi", "__init__"),
            ("rb", "initialize"),
            ("php", "__construct"),
            ("swift", "init"),
        ] {
            assert_eq!(m(none, "Worker", name, ext), Some("constructor"), "{ext} {name}");
        }
        assert_eq!(m(none, "Worker", "__init__", "rb"), None);
        assert_eq!(m(none, "Worker", "initialize", "py"), None);
        assert_eq!(m(none, "Worker", "init", "ts"), None);
        // (k) the METHOD named like its owner.
        for ext in CTOR_LIKE_OWNER {
            assert_eq!(m(none, "Friend", "Friend", ext), Some("constructor"), "{ext}");
            assert_eq!(m(none, "Friend", "fromJson", ext), None, "{ext}: a factory");
        }
        assert_eq!(m(none, "Friend", "Friend", "ts"), None, "TS names its constructor");
        assert_eq!(m(none, "Friend", "Friend", "py"), None);
        assert_eq!(m(none, "", "", "dart"), None, "an empty name matches nothing");
        // The extension compares ignoring ASCII case; the name exactly.
        assert_eq!(m(none, "Repo", "constructor", "TS"), Some("constructor"));
        assert_eq!(m(none, "Repo", "Constructor", "ts"), None);
        // The first match wins: a component's constructor is a constructor.
        assert_eq!(m(comp, "Page", "constructor", "ts"), Some("constructor"));
        // (l) DT.5e: Flutter's lifecycle, on a Dart COMPONENT only.
        for member in INVOKED_MEMBERS[11].names {
            assert_eq!(m(comp, "CardPage", member, "dart"), Some("flutter"), "{member}");
            assert_eq!(m(none, "CartModel", member, "dart"), None, "{member} on a plain class");
            assert_eq!(m(svc, "Repo", member, "dart"), None, "{member} on a SERVICE");
            assert_eq!(m(comp, "Page", member, "ts"), None, "{member} in a .ts file");
        }
        assert_eq!(m(comp, "CardPage", "onTap", "dart"), None, "a plain member");
        assert_eq!(m(comp, "CardPage", "Build", "dart"), None, "the name compares exactly");
        // A widget's constructor is still the constructor row's.
        assert_eq!(m(comp, "CardPage", "CardPage", "dart"), Some("constructor"));
    }

    /// The table's shape: Angular rows are TypeScript only and role- or
    /// heritage-gated, constructor rows take any owner, go_runtime rows are
    /// signature-checked Go, apple_app is the Swift COMPONENT, transloco and
    /// dio are heritage-gated; `names_member` is the name half.
    #[test]
    fn invoked_members_table_shape() {
        let frameworks: Vec<&str> = INVOKED_MEMBERS.iter().map(|r| r.framework).collect();
        let mut want = vec!["angular"; 5];
        want.extend(["constructor"; 6]);
        want.push("flutter");
        want.extend(["go_runtime"; 10]);
        want.push("apple_app");
        want.extend(["transloco"; 5]);
        want.extend(["angular"; 4]);
        want.push("dio");
        assert_eq!(frameworks, want);
        for r in INVOKED_MEMBERS {
            assert!(!r.names.is_empty() || r.named_like_owner, "{}", r.framework);
            match r.framework {
                "angular" => {
                    assert_eq!(r.exts, TS_ONLY);
                    assert!(!r.owner_roles.is_empty() || !r.owner_bases.is_empty());
                    assert!(!r.names.is_empty() && !r.named_like_owner);
                    assert!(!r.checks_signature());
                }
                "flutter" => {
                    assert_eq!(r.exts, ["dart"]);
                    assert_eq!(r.owner_roles, [nk::COMPONENT]);
                    assert!(!r.names.is_empty() && !r.named_like_owner);
                }
                "go_runtime" => {
                    assert_eq!(r.exts, ["go"]);
                    assert!(!r.sigs.is_empty());
                    assert!(r.owner_roles.is_empty() && r.owner_bases.is_empty());
                    assert!(!r.named_like_owner);
                }
                "apple_app" => {
                    assert_eq!(r.exts, ["swift"]);
                    assert_eq!(r.owner_roles, [nk::COMPONENT]);
                    assert!(r.owner_bases.is_empty() && !r.checks_signature());
                }
                "transloco" | "dio" => {
                    assert!(!r.owner_bases.is_empty() && r.owner_roles.is_empty());
                    assert!(!r.checks_signature() && !r.named_like_owner);
                }
                "constructor" => {
                    assert!(r.owner_roles.is_empty());
                    assert!(r.names.is_empty() == r.named_like_owner, "names or named_like_owner");
                    assert!(r.owner_bases.is_empty() && !r.checks_signature());
                }
                other => panic!("an unknown framework tag {other:?}"),
            }
            for e in r.exts {
                assert!(!e.starts_with('.') && e.to_ascii_lowercase() == *e, "{e}");
            }
        }
        let ctor = INVOKED_MEMBERS[10];
        assert!(ctor.names_member("Foo", "Foo") && !ctor.names_member("Foo", "foo"));
        assert!(!INVOKED_MEMBERS[0].names_member("Page", "Page"));
    }

    /// DT.5l: the rows with a signature, sibling or base gate and their
    /// invariants: signature rows are Go; every sibling's signature is one a
    /// row lists; every base names a library and a type; the framework tags
    /// first appear in the `[invoked]` marker's order.
    #[test]
    fn dt5l_rows_hold_their_invariants() {
        let all_sigs: Vec<&str> = INVOKED_MEMBERS.iter().flat_map(|r| r.sigs.iter().copied()).collect();
        for r in INVOKED_MEMBERS {
            if r.checks_signature() {
                assert_eq!(r.exts, ["go"], "{:?}", r.names);
            }
            for (n, sig) in r.siblings {
                assert!(all_sigs.contains(sig), "{n} {sig}");
                let row = INVOKED_MEMBERS.iter().find(|x| x.names.contains(n));
                assert!(row.is_some_and(|x| x.sigs.contains(sig)), "{n}'s own row lists {sig}");
            }
            for b in r.owner_bases {
                assert!(!b.library.is_empty() && !b.name.is_empty(), "{b:?}");
            }
        }
        let mut order: Vec<&str> = Vec::new();
        for r in INVOKED_MEMBERS {
            if !order.contains(&r.framework) {
                order.push(r.framework);
            }
        }
        assert_eq!(
            order,
            ["angular", "constructor", "flutter", "go_runtime", "apple_app", "transloco", "dio"]
        );
    }

    /// DT.5l: what `matches_at` reads. A signature reader, a sibling reader
    /// and a base predicate, each from a fixed table.
    fn at(
        roles: &[NodeKindId],
        member: &str,
        ext: &str,
        sig: Option<&str>,
        siblings: &[(&str, &str)],
        bases: &[ExternalBase],
    ) -> Option<&'static str> {
        let sig = sig.map(str::to_string);
        let sib: Vec<(String, String)> =
            siblings.iter().map(|(n, s)| (n.to_string(), s.to_string())).collect();
        let bases = bases.to_vec();
        invoked_member_at(
            roles,
            "Owner",
            member,
            ext,
            &|| sig.clone(),
            &|n: &str| sib.iter().find(|(m, _)| m == n).map(|(_, s)| s.clone()),
            &|b: &ExternalBase| bases.contains(b),
        )
    }

    #[test]
    fn go_runtime_rows_check_signatures_and_siblings() {
        let none: &[NodeKindId] = &[];
        for (member, sig) in [
            ("String", "()(string)"),
            ("Error", "()(string)"),
            ("MarshalJSON", "()([]byte,error)"),
            ("MarshalText", "()([]byte,error)"),
            ("UnmarshalJSON", "([]byte)(error)"),
            ("Scan", "(any)(error)"),
            ("Value", "()(Value,error)"),
            ("ServeHTTP", "(ResponseWriter,*Request)()"),
        ] {
            assert_eq!(at(none, member, "go", Some(sig), &[], &[]), Some("go_runtime"), "{member}");
        }
        assert_eq!(at(none, "Scan", "go", Some("()(bool)"), &[], &[]), None, "Scan() bool");
        assert_eq!(at(none, "Value", "go", Some("()(int64)"), &[], &[]), None);
        assert_eq!(at(none, "String", "go", Some("(string)(string)"), &[], &[]), None);
        assert_eq!(at(none, "String", "go", None, &[], &[]), None, "an unreadable signature");
        assert_eq!(at(none, "String", "ts", Some("()(string)"), &[], &[]), None, "a .ts file");
        assert_eq!(at(none, "Len", "go", Some("(int,int)(bool)"), &[], &[]), None);
        assert_eq!(at(none, "Stringify", "go", Some("()(string)"), &[], &[]), None);

        // sort.Interface is all three.
        let sorted: &[(&str, &str)] = &[("Less", "(int,int)(bool)"), ("Swap", "(int,int)()")];
        assert_eq!(at(none, "Len", "go", Some("()(int)"), sorted, &[]), Some("go_runtime"));
        assert_eq!(at(none, "Len", "go", Some("()(int)"), &[], &[]), None, "a lone Len");
        let wrong: &[(&str, &str)] = &[("Less", "(int,int)(int)"), ("Swap", "(int,int)()")];
        assert_eq!(at(none, "Len", "go", Some("()(int)"), wrong, &[]), None);
        let full: &[(&str, &str)] =
            &[("Len", "()(int)"), ("Less", "(int,int)(bool)"), ("Swap", "(int,int)()")];
        assert_eq!(at(none, "Less", "go", Some("(int,int)(bool)"), full, &[]), Some("go_runtime"));
        assert_eq!(at(none, "Swap", "go", Some("(int,int)()"), full, &[]), Some("go_runtime"));
        // An Unwrap is an error's.
        let err: &[(&str, &str)] = &[("Error", "()(string)")];
        assert_eq!(at(none, "Unwrap", "go", Some("()(error)"), err, &[]), Some("go_runtime"));
        assert_eq!(at(none, "Unwrap", "go", Some("()([]error)"), err, &[]), Some("go_runtime"));
        assert_eq!(at(none, "Unwrap", "go", Some("()(error)"), &[], &[]), None, "no Error");

        // The signature reader runs at most once, and never past a cheap gate.
        let calls = std::cell::Cell::new(0);
        let count = || {
            calls.set(calls.get() + 1);
            Some("()(string)".to_string())
        };
        let m = |member: &str, ext: &str| {
            invoked_member_at(none, "O", member, ext, &count, &|_| None, &|_| false)
        };
        assert_eq!(m("String", "go"), Some("go_runtime"));
        assert_eq!(calls.get(), 1);
        assert_eq!(m("String", "ts"), None);
        assert_eq!(m("Stringify", "go"), None);
        assert_eq!(m("constructor", "ts"), Some("constructor"));
        assert_eq!(calls.get(), 1, "a failed cheap gate reads no signature");
        assert_eq!(m("Scan", "go"), None);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn apple_app_and_external_base_rows() {
        let comp: &[NodeKindId] = &[nk::COMPONENT];
        let none: &[NodeKindId] = &[];
        assert_eq!(at(comp, "application", "swift", None, &[], &[]), Some("apple_app"));
        assert_eq!(at(comp, "body", "swift", None, &[], &[]), Some("apple_app"));
        assert_eq!(at(none, "application", "swift", None, &[], &[]), None, "no @main role");
        assert_eq!(at(comp, "application", "ts", None, &[], &[]), None);
        assert_eq!(at(comp, "applicationLater", "swift", None, &[], &[]), None);
        // A Swift constructor is still the constructor row's.
        assert_eq!(at(comp, "init", "swift", None, &[], &[]), Some("constructor"));

        let loader = base("@jsverse/transloco", "TranslocoLoader");
        assert_eq!(at(none, "getTranslation", "ts", None, &[], &[loader]), Some("transloco"));
        let old = base("@ngneat/transloco", "TranslocoLoader");
        assert_eq!(at(none, "getTranslation", "ts", None, &[], &[old]), Some("transloco"));
        assert_eq!(at(none, "getTranslation", "ts", None, &[], &[]), None, "no base");
        let other = base("@jsverse/transloco", "TranslocoMissingHandler");
        assert_eq!(at(none, "getTranslation", "ts", None, &[], &[other]), None, "another base");
        assert_eq!(at(none, "handle", "ts", None, &[], &[other]), Some("transloco"));
        assert_eq!(at(none, "getTranslation", "dart", None, &[], &[loader]), None);

        let handler = base("@angular/core", "ErrorHandler");
        assert_eq!(at(none, "handleError", "ts", None, &[], &[handler]), Some("angular"));
        assert_eq!(at(none, "handleError", "ts", None, &[], &[]), None);
        let reuse = base("@angular/router", "BaseRouteReuseStrategy");
        assert_eq!(at(none, "shouldReuseRoute", "ts", None, &[], &[reuse]), Some("angular"));
        let resolve = base("@angular/router", "Resolve");
        assert_eq!(at(none, "resolve", "ts", None, &[], &[resolve]), Some("angular"));

        let dio = base("dio", "Interceptor");
        assert_eq!(at(none, "onRequest", "dart", None, &[], &[dio]), Some("dio"));
        assert_eq!(at(none, "onError", "dart", None, &[], &[base("dio", "QueuedInterceptor")]), Some("dio"));
        assert_eq!(at(none, "onRequest", "ts", None, &[], &[dio]), None, "dio is Dart");
        assert_eq!(at(none, "onRequest", "dart", None, &[], &[]), None);
    }

    /// The 4-argument matcher never returns a row with a signature, sibling
    /// or base gate, whatever the member's name; DT.5d's rows are unchanged.
    #[test]
    fn invoked_member_never_reads_a_dt5l_gate() {
        let roles: &[NodeKindId] = &[nk::COMPONENT];
        for r in INVOKED_MEMBERS.iter().filter(|r| r.checks_signature() || !r.owner_bases.is_empty()) {
            for name in r.names {
                for ext in r.exts {
                    let tag = invoked_member(roles, "Owner", name, ext);
                    assert!(
                        tag.is_none_or(|t| !["go_runtime", "transloco", "dio"].contains(&t)),
                        "{name} {ext}: {tag:?}"
                    );
                    assert!(!r.matches(roles, "Owner", name, ext), "{name} {ext}");
                }
            }
        }
        assert_eq!(invoked_member(roles, "AppDelegate", "application", "swift"), Some("apple_app"));
    }

    /// DT.5l: Go `init` is an entry in a .go file only, and node_ext reads
    /// the POSITION file's extension.
    #[test]
    fn go_init_entry_and_node_ext() {
        use glia_core::CellPayload;
        let e = &CODE_TABLES.entry;
        let f = Some(nk::FUNCTION);
        assert!(e.is_entry_at(f, "init", &[], || Some("go".into())));
        assert!(!e.is_entry_at(f, "init", &[], || Some("ts".into())));
        assert!(!e.is_entry_at(f, "init", &[], || Some("py".into())));
        assert!(!e.is_entry_at(Some(nk::METHOD), "init", &[], || Some("go".into())));
        assert!(!e.is_entry(f, "init", &[]), "the extension-blind form never matches init");
        assert!(e.is_entry(f, "main", &[]) && e.is_entry_at(f, "main", &[], || None));
        assert!(e.is_entry_at(f, "TestX", &[], || Some("ts".into())));

        let pos = |file: &str| Cell {
            kind: crate::cell_type::POSITION,
            payload: CellPayload::Json(format!("{{\"file\":\"{file}\",\"start_line\":3}}")),
        };
        assert_eq!(node_ext(&[pos("a/b.go")]).as_deref(), Some("go"));
        assert_eq!(node_ext(&[pos("src/x.component.TS")]).as_deref(), Some("TS"));
        assert_eq!(node_ext(&[pos("Makefile")]), None);
        assert_eq!(node_ext(&[]), None);
    }

    #[test]
    fn code_tables_validate() {
        assert_eq!(CODE_TABLES.validate(), Ok(()));
        assert_eq!(CODE_TABLES.graph_type, "code");
        assert_eq!(CODE_TABLES.registries.kind_name(nk::ROUTE), Some("ROUTE"));
        assert_eq!(CODE_TABLES.registries.category_name(ec::NAVIGATES_TO), Some("NAVIGATES_TO"));
        assert_eq!(CODE_TABLES.registries.cell_name(cell_type::ORIGIN), Some("ORIGIN"));
    }

    /// LE.4d: a sink is the (kind, category) pair. The USES a function has on
    /// a queue producer is an effect; the same USES on a plain function, or a
    /// CONTAINS / QUEUE_FLOWS into the producer, is not.
    #[test]
    fn effect_sinks_are_kind_and_category() {
        let class = |k, c| CODE_TABLES.effect_sink(k, c).map(|(_, s)| s.class);
        assert_eq!(class(nk::DATA_ENTITY, ec::ACCESSES_DATA), Some("db"));
        assert_eq!(class(nk::QUEUE_PRODUCER, ec::USES), Some("queue_produce"));
        assert_eq!(class(nk::ENDPOINT, ec::CALLS), Some("http_call"));
        assert_eq!(class(nk::EVENT_EMITTER, ec::USES), Some("event_emit"));
        assert_eq!(class(nk::RPC_CALL, ec::CALLS), Some("rpc_call"));
        assert_eq!(class(nk::FUNCTION, ec::USES), None);
        assert_eq!(class(nk::QUEUE_PRODUCER, ec::CONTAINS), None);
        assert_eq!(class(nk::QUEUE_CONSUMER, ec::QUEUE_FLOWS), None);
        assert_eq!(class(nk::DATA_ENTITY, ec::USES), None);
    }

    #[test]
    fn shares_data_source_is_not_carried() {
        // REGRESSION GUARD. A shared Postgres is an operational fact, not a
        // code dependency: carrying it would fan every blast radius across
        // every service in the stack. Do not "fix" this by adding the row.
        assert!(
            !CODE_TABLES.carries(ec::SHARES_DATA_SOURCE),
            "SHARES_DATA_SOURCE must stay OUT of CODE_TABLES.carry_edges"
        );
    }

    #[test]
    fn co_changes_is_heuristic_everywhere() {
        // REGRESSION GUARD (LF.5b). Files that change together are coupled,
        // not linked: CO_CHANGES must never carry a blast radius or liveness,
        // and must weigh 0 in every activation preset (an unlisted category
        // would weigh 1.0 and reshape every ranking in a repo with history).
        assert!(!CODE_TABLES.carries(ec::CO_CHANGES), "CO_CHANGES must stay OUT of carry_edges");
        assert!(!CODE_TABLES.effect_sinks.iter().any(|s| s.via.contains(&ec::CO_CHANGES)));
        for preset in [None, Some("repair"), Some("review"), Some("onboard"), Some("centrality")] {
            let w = CODE_TABLES.activation_config(preset).edge_weights.get(&ec::CO_CHANGES).copied();
            assert_eq!(w, Some(0.0), "preset {preset:?}");
        }
    }

    #[test]
    fn same_as_never_conducts() {
        // REGRESSION GUARD (DD.3). SAME_AS joins an identity class's members
        // to its star center: one npm package in every repo, hundreds of
        // vendored candidates. Identity reaches a walk or a ranking only
        // through the quotient, so the edge carries no blast radius or
        // liveness, is no effect, groups no community and weighs 0 in every
        // activation preset (an unlisted category would weigh 1.0).
        assert!(!CODE_TABLES.carries(ec::SAME_AS), "SAME_AS must stay OUT of carry_edges");
        assert!(!CODE_TABLES.effect_sinks.iter().any(|s| s.via.contains(&ec::SAME_AS)));
        assert_eq!(CODE_TABLES.community_weight(ec::SAME_AS), 0);
        for preset in [None, Some("repair"), Some("review"), Some("onboard"), Some("centrality")] {
            let w = CODE_TABLES.activation_config(preset).edge_weights.get(&ec::SAME_AS).copied();
            assert_eq!(w, Some(0.0), "preset {preset:?}");
        }
    }

    /// DD.3: one identity rule per `same_as` rule name, in its order, with
    /// the tiers and walk dials 0.5.2 ruled (Q17 / Q18): packages and
    /// vendored copies never walk, vendored copies are HEURISTIC.
    #[test]
    fn identity_rules_cover_every_rule() {
        use IdentityTier::{Derived, Fact, Heuristic};
        assert_eq!(CODE_TABLES.validate(), Ok(()));
        let rules = CODE_TABLES.identity_rules;
        let names: Vec<&str> = rules.iter().map(|r| r.name).collect();
        assert_eq!(names, same_as::ALL_RULES);
        let table: Vec<(&str, IdentityTier, bool)> =
            rules.iter().map(|r| (r.name, r.tier, r.walk)).collect();
        assert_eq!(
            table,
            [
                ("package_name", Fact, false),
                ("manifest_name", Fact, true),
                ("message_fqname", Fact, true),
                ("contract_op", Derived, true),
                ("contract_route", Derived, true),
                ("scip_symbol", Fact, true),
                ("body_hash", Heuristic, false),
                ("path_suffix", Heuristic, false),
                ("http_model", Derived, true),
                ("graphql_model", Derived, true),
            ]
        );

        let rule = |name: &str| CODE_TABLES.identity_rule(name);
        let pairs = |name: &str| rule(name).map(|r| (r.from, r.to));
        const DEP: &[NodeKindId] = &[nk::PACKAGE_DEP];
        const PROJECT: &[NodeKindId] = &[nk::PROJECT];
        const MESSAGE: &[NodeKindId] = &[nk::MESSAGE_TYPE];
        const DOC: &[NodeKindId] = &[nk::DOC_SECTION];
        assert_eq!(pairs("package_name"), Some((DEP, DEP)));
        assert_eq!(pairs("manifest_name"), Some((DEP, PROJECT)));
        assert_eq!(pairs("message_fqname"), Some((MESSAGE, MESSAGE)));
        for name in ["contract_op", "contract_route"] {
            assert_eq!(pairs(name), Some((DOC, DOC)), "{name}");
        }
        assert_eq!(pairs("scip_symbol"), Some((SCIP_DEF_KINDS, SCIP_DEF_KINDS)));
        assert_eq!(SCIP_DEF_KINDS.len(), 8);
        for name in ["body_hash", "path_suffix"] {
            assert_eq!(pairs(name), Some((VENDORED_KINDS, VENDORED_KINDS)), "{name}");
            assert!(rule(name).is_some_and(|r| !r.tier.collapses()), "{name} never collapses");
        }
        assert_eq!(VENDORED_KINDS[0], nk::MODULE);
        assert_eq!(&VENDORED_KINDS[1..], &SCIP_DEF_KINDS[..6], "a file plus its declared code");
        const CLIENT: &[NodeKindId] = &[nk::INTERFACE, nk::CLASS];
        const SERVER: &[NodeKindId] = &[nk::STRUCT];
        assert_eq!(pairs("http_model"), Some((CLIENT, SERVER)));
        assert!(rule("http_model").is_some_and(|r| r.tier.collapses()), "http_model collapses");
        const GQL_MEMBER: &[NodeKindId] = &[nk::INTERFACE, nk::CLASS, nk::STRUCT];
        assert_eq!(pairs("graphql_model"), Some((GQL_MEMBER, MESSAGE)));
        assert!(
            rule("graphql_model").is_some_and(|r| r.tier.collapses()),
            "graphql_model collapses"
        );

        // Every key is a line, the rule names are the bare EVIDENCE rules.
        for r in rules {
            assert!(!r.key.is_empty() && !r.key.contains('\n'), "{}", r.name);
            let diverged = same_as::evidence_rule(r.name, true);
            assert_eq!(same_as::split_evidence_rule(&diverged), (r.name, true), "{diverged}");
        }
        assert!(rule("package_name/diverged").is_none());
    }

    /// CD.1c: communities group by what code does to code. Structure
    /// (CONTAINS, DEPENDS_ON), docs, tests, git history and every SHARES_*
    /// pairing weigh 0 - each would glue unrelated code through one hub - and
    /// exactly those: every other registered category is listed, so a new
    /// category must be weighed or added here.
    #[test]
    fn community_weights_exclude_structure_history_and_docs() {
        assert_eq!(CODE_TABLES.validate(), Ok(()));
        let w = |c| CODE_TABLES.community_weight(c);
        for c in [ec::CONTAINS, ec::DOCUMENTS, ec::TESTS, ec::DEPENDS_ON, ec::CO_CHANGES] {
            assert_eq!(w(c), 0, "{} is left out", ec::name(c));
        }
        let shares: Vec<_> =
            ec::ALL.iter().filter(|(_, n)| n.starts_with("SHARES_")).map(|(c, _)| *c).collect();
        assert_eq!(shares.len(), 7, "{shares:?}");
        for c in &shares {
            assert_eq!(w(*c), 0, "{} is left out", ec::name(*c));
        }
        for c in [ec::CALLS, ec::HANDLED_BY, ec::DEFINES, ec::HTTP_CALLS, ec::QUEUE_FLOWS] {
            assert!(w(c) > 0, "{} groups", ec::name(c));
        }
        assert!(w(ec::CALLS) > w(ec::DEFINES), "a call binds harder than a definition");

        let left_out: Vec<&str> =
            ec::ALL.iter().filter(|(c, _)| w(*c) == 0).map(|(_, n)| *n).collect();
        assert_eq!(
            left_out,
            [
                "CONTAINS",
                "DOCUMENTS",
                "TESTS",
                "SHARES_SCHEMA",
                "SHARES_DATA_ENTITY",
                "SHARES_CRON_SCHEDULE",
                "SHARES_CONFIG",
                "SHARES_INFRA_REF",
                "DEPENDS_ON",
                "SHARES_DEPENDENCY",
                "SHARES_DATA_SOURCE",
                "CO_CHANGES",
                "SAME_AS",
            ]
        );
        assert_eq!(CODE_TABLES.community_weights.len() + left_out.len(), ec::ALL.len());
        // The table is laid out in `ec::ALL` order.
        let listed: Vec<_> = CODE_TABLES.community_weights.iter().map(|(c, _)| *c).collect();
        let in_all: Vec<_> =
            ec::ALL.iter().map(|(c, _)| *c).filter(|c| listed.contains(c)).collect();
        assert_eq!(listed, in_all);
    }

    /// The presets are real lenses (moved from `graph::activation`, LD.14b):
    /// `repair` weighs CALLS above the base, `onboard` weighs CONTAINS above
    /// it, and `"default"` or a name no preset has is exactly the base.
    #[test]
    fn activation_presets_shift_weights() {
        let base = CODE_TABLES.activation_config(None);
        let repair = CODE_TABLES.activation_config(Some("repair"));
        let onboard = CODE_TABLES.activation_config(Some("onboard"));
        assert!(repair.edge_weights[&ec::CALLS] > base.edge_weights[&ec::CALLS]);
        assert!(onboard.edge_weights[&ec::CONTAINS] > base.edge_weights[&ec::CONTAINS]);
        assert_ne!(repair.edge_weights, onboard.edge_weights, "the presets differ");
        // CC.10a: `centrality` zeroes structure, tests and docs, and keeps
        // every other base weight.
        let centrality = CODE_TABLES.activation_config(Some("centrality"));
        for c in [ec::DEFINES, ec::CONTAINS, ec::DOCUMENTS, ec::TESTS] {
            assert_eq!(centrality.edge_weights[&c], 0.0, "category {}", c.0);
        }
        for (c, w) in &base.edge_weights {
            if ![ec::DEFINES, ec::CONTAINS, ec::DOCUMENTS, ec::TESTS].contains(c) {
                assert_eq!(centrality.edge_weights[c], *w, "category {}", c.0);
            }
        }
        for name in ["default", "nonsense"] {
            assert_eq!(
                CODE_TABLES.activation_config(Some(name)).edge_weights,
                base.edge_weights,
                "preset {name:?} is the base"
            );
        }
    }
}
