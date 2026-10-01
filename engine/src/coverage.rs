// ============================================================================
// P2 coverage / blind-spot signaling (handoff v6) — turn silent blind spots
// into declared ones so the graph+grep fallback is deliberate, not lucky.
// ============================================================================

use glia_code_domain::edge_category;
use glia_core::CellPayload;
use glia_graph::MergedGraph;

/// A known extraction limitation for a language/edge-category. Advisory: an
/// agent that sees this should verify that dimension with grep rather than trust
/// a silent absence. `language == "*"` applies to every repo.
#[derive(serde::Serialize, Clone, Copy)]
#[non_exhaustive]
pub struct CoverageCaveat {
    /// Language the caveat applies to (`"*"` = universal), matched to files present.
    pub language: &'static str,
    /// Edge category (or `"*"`) whose extraction is partial.
    pub edge_category: &'static str,
    /// What glia does NOT catch here.
    pub note: &'static str,
    /// The grep-shaped fallback to confirm completeness.
    pub verify: &'static str,
}

/// Declared coverage caveats — derived from the `bench/substrate-gap` eval +
/// documented residuals (`BLINDSPOTS.md`). Update when the matrix changes. The
/// universal (`"*"`) entries are the load-bearing honesty: static analysis
/// cannot see dynamic dispatch / string-built targets, so completeness there is
/// never guaranteed.
static COVERAGE_CAVEATS: &[CoverageCaveat] = &[
    CoverageCaveat {
        language: "*",
        edge_category: "CALLS",
        note: "calls through reflection, dynamic dispatch, or higher-order indirection are not resolved",
        verify: "grep the callee name",
    },
    CoverageCaveat {
        language: "*",
        edge_category: "HTTP_CALLS",
        note: "URLs built dynamically (string concat / variables / base-url config) may not pair to a route",
        verify: "grep the path literal or base URL",
    },
    // A2.6: until these rows, no QUEUE_FLOWS caveat existed, so a silent queue
    // blind spot was undeclared.
    CoverageCaveat {
        language: "*",
        edge_category: "QUEUE_FLOWS",
        // LA.4 (A11.7): literal constants now fold through the repo const
        // table after the parse cache; this names what stays blind.
        note: "topics passed as parameters, runtime variables or env vars - and constants that are ambiguous, or lower-case bindings in another file - are extracted as an unresolved framework tag and never paired; literal constants the repo table resolves are folded",
        verify: "grep the topic constant or env var name",
    },
    CoverageCaveat {
        language: "*",
        edge_category: "QUEUE_FLOWS",
        note: "GCP Pub/Sub subscriptions are named independently of their topic, so publisher and subscriber pair only when both name the topic",
        verify: "check the subscription-to-topic binding in IaC",
    },
    CoverageCaveat {
        language: "*",
        edge_category: "QUEUE_FLOWS",
        note: "SNS→SQS fan-out is declared in infrastructure, not code — a producer to an SNS topic will not pair with the SQS consumers it feeds",
        verify: "grep the SNS subscription in terraform/CDK",
    },
    // A10.8: the walk admits a `.json` contract by content sniff under a size
    // cap (`walk::JSON_CONTRACT_CAP`), so a large generated spec is skipped.
    CoverageCaveat {
        language: "*",
        edge_category: "DOCUMENTS",
        note: "contract JSON (OpenAPI/Swagger, AsyncAPI, Pact) over 512 KB is not read, and one whose format key is outside its first and last 8 KB is not recognised",
        verify: "look for large swagger.json / openapi.json / pact files and read their paths by hand",
    },
    // CJ.2: the repo markdown `docs::include_doc` admits, and the doc linker's
    // backtick-only mention rule, so a `docs-for` absence says what was read.
    CoverageCaveat {
        language: "*",
        edge_category: "DOCUMENTS",
        note: "repo markdown is ingested only from: the well-known files (README*.md, ARCHITECTURE.md, CHANGELOG.md, CONTRIBUTING.md, CODE_OF_CONDUCT.md, CLAUDE.md, AGENTS.md, CODE_RULES.md, any case) at the repo root and at each PROJECT root, a docs/ tree at the repo root or at a PROJECT root, a top-level .ai/ tree, ADR directories (adr / adrs / decisions under doc / docs / architecture), SDD feature docs (features/<feature>/*.md and spec-kit specs/<NNN-slug>/ at the repo root or a PROJECT root) and synced external docs (`glia docs sync`); other markdown - a README inside a source directory, notes beside code - is not read. A doc section DOCUMENTS a symbol only when it names it in a single-backtick span (`ChatOps.EnsureUserReadyForChat`, `create_order()`); a plain-prose mention, a fenced code block or a link is not read, and one section links at most 25 symbols",
        verify: "grep the symbol name across the repo's *.md files, and read the doc that names it in plain prose",
    },
    // LA.27 (James's call): SDL inside code is read only from GraphQL-marked
    // literals, so unmarked SDL is a declared recall gap, not a silent one.
    // CB.1 names `.graphqls`; CB.24 adds the client base-URL narrowing rule.
    CoverageCaveat {
        language: "*",
        edge_category: "GRAPHQL_CALLS",
        note: "GraphQL SDL embedded in code is read only from a GraphQL-marked literal: a gql / graphql tag or call, a buildSchema / MustParseSchema / ParseSchema / from_definition argument, a /* GraphQL */ or #graphql literal, a GRAPHQL / GQL heredoc, or a literal bound to a variable or key named typeDefs / type_defs. SDL kept unmarked in a differently named variable (a plain template in `const schema = ...`, a Go string passed to MustParseSchema by name) is not read: its root types and fields mint no GRAPHQL_RESOLVER, so its clients' operations pair with nothing. `.graphql` / `.graphqls` / `.gql` files are read whole. A client's base URL narrows its project's operations to one server only when it is one literal absolute URL (Apollo `uri`, HttpLink / createHttpLink, urql createClient / new Client, graphql-request GraphQLClient) whose host names a nested project or an IaC service; a relative, env-var or interpolated URL leaves an operation paired with every server that declares the field.",
        verify: "grep 'type Query {' / 'type Mutation {' outside .graphql / .graphqls / .gql files and read the variable that holds it; for an operation paired with several servers, read the client's uri / url",
    },
    // LA.18b: a path-less upgrade handler (gorilla, nhooyr, raw ASP.NET) pairs
    // only through the routes that reach its upgrading function, and a client
    // whose URL the extractor could not read pairs nothing. CB.21: the host
    // narrowing a client's literal URL allows.
    CoverageCaveat {
        language: "*",
        edge_category: "WS_CONNECTS",
        note: "a WebSocket upgrade whose route is registered more than one call away from the upgrading function, or through a router glia does not extract, stays unpaired; a client whose URL has no static path is not paired; a client matching several same-path handlers narrows to one service only when its URL's static head carries a literal host naming a nested project or an IaC service (compose / Kubernetes service, deployment, statefulset, image), otherwise it pairs with all of them",
        verify: "grep the upgrade call and the route registration; for a client paired with several handlers, read its URL",
    },
    // CB.21: a gRPC stub's dial host narrows same-named services, read only
    // from a literal target.
    CoverageCaveat {
        language: "*",
        edge_category: "GRPC_CALLS",
        note: "a gRPC client stub whose service several projects declare narrows to one only when its dial target is a literal (the constructor's own argument, or the nearest dial above it in the same function: grpc.Dial / DialContext / NewClient, grpc(.aio).*_channel, forTarget / forAddress, GrpcChannel.ForAddress, tonic from_static, ClientChannel) whose host names a nested project or an IaC service; a target read from config or env, or a unix: / xds: target, names no host, so the stub stays unnarrowed: paired with every such service, or with none when the pick is ambiguous",
        verify: "grep the dial call and the address it is given",
    },
    // LA.17: Connect / Twirp procedures come from the build's .proto set and
    // calls from the bound client variable, both read per file. CB.24: a tRPC
    // link URL narrows like a GraphQL client's.
    CoverageCaveat {
        language: "*",
        edge_category: "RPC_CALLS",
        note: "Connect/Twirp procedures and calls are read only when the service's .proto is in the build; a client stored in one file and called from another, and connect-node / non-Go Twirp code, are not extracted; a procedure whose implementing type lives in another file than its registration is contained by the module, not HANDLED_BY the method; a tRPC call narrows to one server's procedure only when its link's `url` (httpBatchLink / httpLink / httpBatchStreamLink) is one literal absolute URL whose host names a nested project or an IaC service, otherwise it pairs with every same-named procedure",
        verify: "grep New<Service>Client / createClient(<Service> and the method name; for a tRPC call paired twice, read the link's url",
    },
    // LA.33: a consumer is HANDLED_BY its callback only for these client
    // shapes; every other consumer keeps LE.4c's subscribing-function edge.
    CoverageCaveat {
        language: "*",
        edge_category: "HANDLED_BY",
        note: "a queue consumer is HANDLED_BY its callback only for kafkajs eachMessage/eachBatch, amqplib consume, BullMQ Worker, nats subscribe (JS callback, Go Subscribe/QueueSubscribe) and pika basic_consume, and only when the callback is a name, a member, this/self.method or a one-call inline function; other consumers are HANDLED_BY the function that subscribes",
        verify: "grep the consumer call and read its callback argument",
    },
    CoverageCaveat {
        language: "python",
        edge_category: "HTTP_CALLS",
        note: "requests / httpx / aiohttp clients are extracted; urllib / http.client are not",
        verify: "grep urllib / http.client",
    },
    CoverageCaveat {
        language: "python",
        edge_category: "HANDLED_BY",
        note: "Flask typed route converters (e.g. <int:id>) may not normalize to the client's path param",
        verify: "check the @app.route decorator vs the caller path",
    },
    CoverageCaveat {
        language: "typescript",
        edge_category: "HTTP_CALLS",
        note: "fetch/axios at component or hook scope are extracted; calls hidden behind custom wrappers may be missed",
        verify: "grep fetch / axios / the wrapper name",
    },
    // LA.6e: page flow (`glia pages`) judges only the links the LA.6c
    // extractor can read, so the navigations it counts but cannot judge
    // (`dynamic_skipped` in the `[nav-links]` marker) are declared here.
    CoverageCaveat {
        language: "typescript",
        edge_category: "NAVIGATES_TO",
        note: "navigations built from variables (navigate([path]), navigateByUrl(url), router.push(url)), guard redirects (router.createUrlTree / parseUrl), Vue named-route pushes and Angular relative navigations are not resolved, so a page reached only through one is listed unlinked; lazily loaded child tables in another file match only by path suffix (Weak); plain <a href> links bind but are never reported dead",
        verify: "grep -rnE 'navigate\\(|navigateByUrl|createUrlTree|parseUrl|router\\.push|routerLink' src",
    },
    // LA.1b: with crate paths (LA.1a), macro-argument calls (LA.2), inline
    // mods (LA.3) and `use` trees (LA.1b) resolved, the Rust CALLS gap left
    // was a method call on a typed local or field. LA.35a binds those whose
    // type the parser can read, LA.35b those whose method sits in an `impl`
    // in another file than its type; the row keeps the receivers no rule types.
    CoverageCaveat {
        language: "rust",
        edge_category: "CALLS",
        note: "a method call on a value (`x.m()`) binds through the type of a struct field, a parameter, or a `let` with a type annotation or a `T::new()` / `T::default()` / `T::from(..)` / `T { .. }` / enum-variant initialiser (`&`, `Arc`, `Rc`, `Box` peeled); it stays unresolved when the receiver's type comes from a function's return value (`let r = make()`), a chain (`a.b().m()`) or a container (`Option<T>`, `Vec<T>`), is generic, `dyn` or `impl Trait`, when the name is rebound to another type in the same fn, or when the method is a trait's default body the type does not override (a `trait`'s own fns have no node)",
        verify: "grep the method name",
    },
    // LD.7c: the heritage rows `glia implementors` carries, measured at HEAD.
    // Rust `impl Trait for Type` is IMPLEMENTS; its supertraits are not.
    CoverageCaveat {
        language: "rust",
        edge_category: "INHERITS_FROM",
        note: "supertraits (`trait A: B`) are not extracted, and a trait's own method declarations are not nodes, so a trait has no supertype and a trait method no implementors (`impl Trait for Type` IS extracted as IMPLEMENTS)",
        verify: "grep the trait bound (`trait .*: <Name>`)",
    },
    // LD.7b: Go satisfaction is inferred, so its edges are Medium (DERIVED).
    // CA.3b: gated on signatures and, for a one-method set or a test side,
    // package reachability.
    CoverageCaveat {
        language: "go",
        edge_category: "IMPLEMENTS",
        note: "implicit interface satisfaction is inferred (Medium, DERIVED) from method names and, where the parser read both, signatures (parameter and result types; package qualifiers, parameter names and a type alias are not resolved); a one-method interface, or a pair with a side in a _test.go file, pairs only across an import path (same package, a transitive import either way, or a package importing both); an import of the repository-root package is not recorded, so a root-package side is assumed reachable; a method on a generic type and an interface with type parameters are matched by name only; pointer and value receivers are merged; methods promoted through an embedded struct field are not seen; an interface embedding one that does not bind is skipped as open; constraint type terms are ignored",
        verify: "check the method signatures and receivers against the interface",
    },
    // CB.11 / CB.20 / CB.23: struct-held routers and group mounts, measured
    // on fixtures/go-route-struct-routers and go-route-mounts.
    CoverageCaveat {
        language: "go",
        edge_category: "HANDLED_BY",
        note: "a Go route takes its group's prefix when the group is built from a string literal (`g := r.Group(\"/api\")`), held in a struct field the package assigns it to, or passed through a parameter or a struct field by a call the build has resolved (one ROUTE per mount); a prefix held in a constant or a variable or built at run time (fmt.Sprintf, os.Getenv), a group returned by a function, and a group passed through a call that is not resolved (a function value, a loop over registrars) are not read, so the route keeps its local path and pairs with a client only through the HTTP resolver's prefix folds",
        verify: "grep .Group( / .Route( / .Mount( and follow the group to its registrations",
    },
    CoverageCaveat {
        language: "php",
        edge_category: "IMPLEMENTS",
        note: "`class X implements I` clauses are not extracted: no PHP class has an IMPLEMENTS edge",
        verify: "grep 'implements' for the interface name",
    },
    CoverageCaveat {
        language: "php",
        edge_category: "INHERITS_FROM",
        note: "`class X extends Base` and `interface A extends B` clauses are not extracted, and traits (`use T;` in a class) are not nodes: no PHP type has an INHERITS_FROM edge",
        verify: "grep 'extends' / 'use' for the type or trait name",
    },
    // CB.10 / CB.18: the Swift call scope, measured on fixtures/swift-members
    // and swift-implicit-self plus a receiver probe of each shape below.
    CoverageCaveat {
        language: "swift",
        edge_category: "CALLS",
        note: "Swift calls bind: a bare call inside a type to the type's own member (implicit self, across its extensions in other files) before a same-named free function, else to a function of the same file; a static call or construction (`T.m()`, `T()`) on a type declared in the same module; a method call through a stored property's declared type, optional chaining included (init, deinit, subscripts and computed properties are METHOD nodes). Not bound: a receiver typed by a parameter or a local (`let r: Repo = ..`, `let r = Repo()`), a function's return value or a chain (`make().load()`), a protocol-typed receiver (a protocol's requirements are not nodes), a closure-held function (`let f = { .. }; f()`), a free function declared in another file of the module, and anything declared in another module (an `import`ed target)",
        verify: "grep the callee name across *.swift",
    },
    CoverageCaveat {
        language: "swift",
        edge_category: "IMPLEMENTS",
        note: "protocol conformance (`class X: P`, `struct X: P`, `extension X: P`) is not extracted: no Swift type has an IMPLEMENTS edge",
        verify: "grep ': <Protocol>' in type and extension declarations",
    },
    CoverageCaveat {
        language: "swift",
        edge_category: "INHERITS_FROM",
        note: "class inheritance (`class Child: Base`) and protocol inheritance (`protocol A: B`) are not extracted: no Swift type has an INHERITS_FROM edge",
        verify: "grep ': <Base>' in class and protocol declarations",
    },
    CoverageCaveat {
        language: "dart",
        edge_category: "HTTP_CALLS",
        note: "dio / http client verbs are extracted; other HTTP libraries are not",
        verify: "grep the HTTP client class name",
    },
    // CB.9 / CB.17: constructors, operators and extension members are nodes;
    // measured on fixtures/dart-members, dart-extensions-imports and a probe
    // of each receiver shape below.
    CoverageCaveat {
        language: "dart",
        edge_category: "CALLS",
        note: "Dart constructors, factories, operators, abstract members and extension members are METHOD nodes. Calls bind: a bare call inside a class to its own member (implicit this) before a top-level function of the library; a call through a typed field (`final Repo repo;`, `final _r = Repo()`); a same-file type's constructions and statics; and, through an import with a `show` list or an `as` prefix, the imported library's functions, constructors and statics. Not bound: a bare call, construction or static call into a library pulled in by a plain `import 'x.dart';` (no `show` / `as`), a receiver typed by a parameter or a local, a function's return value or a chain (`make().value()`), a `dynamic` receiver, an extension-method call and an operator expression (`a + b`) at the call site, and a closure-held function",
        verify: "grep the callee name across *.dart",
    },
    // CB.19 / CB.22 / CB.25: the C/C++ residual, measured on fixtures/
    // cpp-declarations, cpp-include-paths and cpp-call-scope plus a probe of
    // each shape below. Keyed "c_cpp", the detect_language tag.
    CoverageCaveat {
        language: "c_cpp",
        edge_category: "CALLS",
        note: "C/C++ calls resolve by name, and a qname carries no signature, so an overload set is one node. A bare call in a member binds the class's own (or an inherited) member first; `Type::m()` / `ns::f()` bind through C++ name lookup; a call on a receiver binds through the declared type of a field, parameter or typed local (`using` applied), and a virtual call binds that static type's member, never an override. A function in another file is reached only through an include glia resolves (see IMPORTS); one reached through a header prototype binds when exactly one non-static definition of it sits in a .c / .cc / .cpp / .cxx file, or, among several, the one beside the declaring header. Not bound: a call through a function pointer or a variable holding a lambda, a template-dependent call (`t.f()` on a template parameter), a macro-expanded call, a call inside a lambda body, and a receiver declared `auto` or reached through a chain or a function's return (`make().f()`)",
        verify: "grep the callee name across the .c / .cc / .cpp / .h / .hpp files",
    },
    CoverageCaveat {
        language: "c_cpp",
        edge_category: "IMPORTS",
        note: "a quoted include resolves against the includer's directory, then the repo's search roots; an angle include through the search roots only. The roots: the -I / -isystem / -iquote dirs of a compile_commands.json at the repo root or in its depth-1 build* / out / cmake-build-* dirs (and the same under each CMake project root), CMake include_directories / target_include_directories in the CMakeLists.txt beside the walked files (an argument holding a generator expression `$<..>`, or a variable other than the list's own dir and the project / source root, is skipped), and include/ under the repo root and each project root; a root outside the repo, or holding no walked C/C++ file, is dropped. Other build systems (Bazel, Meson, Make CFLAGS, autotools) are not read, and a macro-named include (`#include HDR`) records nothing. A system or third-party header is no node: its first path segment is listed in the MODULE's IMPORTS cell",
        verify: "grep '#include' in the file and read the build's -I flags (compile_commands.json, CMakeLists.txt, Makefile, BUILD)",
    },
    // A14.1 / A14.2 — the Kotlin rows. `.kt` has its own parser since A14.2
    // (`parsers/code/kotlin`, one JVM graph with Java), so these describe the
    // residual that parser leaves, measured on bench/substrate-gap/fixtures/
    // kotlin-{entities,spring,ktor,retrofit,flip-guard}. A14.6 rewrote them
    // once the Kotlin chain (calls + heritage A14.3, Spring / JPA A14.4, Ktor
    // A14.5, HTTP clients + Android A14.6) had landed: each row names only
    // what that chain still does not extract. CA.6a narrowed the CALLS row:
    // typed parameters and typed / constructor-initialised locals bind. CA.6b
    // narrowed the HTTP_CALLS and `*` rows: Ktor-client verb calls are
    // client ENDPOINTs.
    CoverageCaveat {
        language: "kotlin",
        edge_category: "*",
        note: "Kotlin is extracted by a dedicated parser: declarations and imports, calls and supertypes, Spring / Micronaut / JAX-RS annotation routes with HANDLED_BY, stereotype / @Inject constructor and field INJECTS, JPA @Entity DATA_ENTITY and repository ACCESSES_DATA, Ktor routes, Retrofit / RestTemplate / WebClient / Ktor-client client ENDPOINTs, and Android components (a class extending an Android framework type in a file importing android / androidx, or carrying @AndroidEntryPoint / @HiltViewModel / @HiltAndroidApp, is an entrypoint). Not read: `.kts` scripts (Gradle KTS) are never parsed; a class extending the app's own base (`: BaseActivity()`), WorkManager workers and @Composable functions are not classified as entrypoints.",
        verify: "grep the symbol across *.kt; for an unclassified Android screen, grep its superclass chain",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "CALLS",
        note: "Kotlin calls are extracted from declared function bodies; a call on a variable binds when the variable is a typed property, a typed parameter, a typed `val` / `var` or one initialised by a constructor call (`val r = Repo()`); a local initialised any other way, a lambda parameter, bare calls in an INTERFACE default method, inherited methods called bare, calls inside a lambda with receiver (`with(x) { m() }`, `apply { }`), calls outside a function body (init blocks, secondary-constructor bodies, property initializers / accessors, default argument values) and a same-package call into another file with no import stay unresolved",
        verify: "grep the callee name across *.kt",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "IMPORTS",
        note: "Kotlin `import a.b.C` / `a.b.C as D` binds to the in-repo declaration only when its simple name is unique in the repo's Java + Kotlin graph (or the file layout mirrors the package); a simple name declared twice leaves the import unbound. `import a.b.*` binds only a file module named for the package's last segment (`b.kt`), and only when that name is unique",
        verify: "grep '^import' in the .kt file",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "INHERITS_FROM",
        note: "Kotlin supertypes (`: Base()` INHERITS_FROM, `: Iface` IMPLEMENTS) are extracted by simple name and bind through an import, the same file, or a repo-unique type name: a supertype whose simple name is declared twice without an import stays unbound, and a library supertype (AppCompatActivity, JpaRepository) has no node to bind",
        verify: "grep the supertype name across *.kt",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "HANDLED_BY",
        note: "Javalin `app.get(\"/path\", h)` and WebFlux `.GET(\"/path\", h)` ROUTEs come from a text scan with no HANDLED_BY; a Ktor route is HANDLED_BY the declared function its lambda sits in (not the lambda), and a `fun Route.x()` installed under `route(\"/api\")` elsewhere is named without that prefix",
        verify: "grep for routing { / route(\" / .get(\" in *.kt",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "HTTP_CALLS",
        note: "Kotlin client ENDPOINTs come from Retrofit interface methods, Spring RestTemplate / WebClient / RestClient calls and Ktor-client verb calls (`client.get(url)`, `client.request(url) { method = HttpMethod.Post }`, in a file importing io.ktor.client) with a literal or templated URL. OkHttp, java.net.http and Fuel calls emit no ENDPOINT; a Ktor builder call without a positional URL (`client.get { url(..) }`), a Ktor `request` whose `method =` is not an `HttpMethod.<X>` constant, and a URL held in a variable are not read; the Retrofit builder's `baseUrl(...)` is not read, so a Retrofit path is taken as root-relative and a base URL carrying a path prefix (`baseUrl(\"https://x/api/\")`) pairs only through the resolver's route-prefix fallback (Medium), or not at all for a prefix outside its API-prefix list",
        verify: "grep OkHttpClient / HttpClient( / Request.Builder / baseUrl( / url( across *.kt",
    },
];

/// One coverage note surfaced for a repo: a caveat that applies because the repo
/// contains that language. `Clone` so an absence answer (LD.8a,
/// `crate::absence::Absence::caveats`) can carry the rows it depended on.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct CoverageNote {
    pub language: &'static str,
    pub edge_category: &'static str,
    pub note: &'static str,
    pub verify: &'static str,
    /// How many edges of this category the graph actually holds (0 = extra
    /// reason to grep: either none exist or extraction missed them).
    pub edges_found: usize,
}

/// **coverage** (P2): for the languages actually present in the repo, the known
/// extraction caveats + how many edges of each flagged category were found —
/// so an agent falls back to grep deliberately where glia is known-partial
/// instead of trusting a silent blind spot. One call.
pub fn coverage_report(merged: &MergedGraph) -> Vec<CoverageNote> {
    let langs = languages_present(merged);
    if langs.contains("kotlin") {
        eprintln!("[coverage] kotlin: residual rows declared for calls/heritage/imports/routes/http clients");
    }
    // CB.26: a C/C++ repo is told its CALLS / IMPORTS residual.
    if langs.contains("c_cpp") {
        eprintln!("[coverage] c_cpp: residual rows declared for calls/imports");
    }
    notes(merged, |c| c.language == "*" || langs.contains(c.language))
}

/// The caveat rows an answer that depended on `mechanisms` must carry (LD.8a
/// absence answers): the [`COVERAGE_CAVEATS`] rows whose `edge_category` is one
/// of `mechanisms` — or `*`, a row that declares every category of its
/// language partial — and whose `language` is `*` or one of `languages`.
/// `languages = None` means every language present in the graph, the same
/// inference [`coverage_report`] makes. Table order is kept; `edges_found` is
/// counted as in [`coverage_report`]. An answer that depended on no edge
/// (`mechanisms` empty) carries no caveat row.
pub(crate) fn caveats_for(
    merged: &MergedGraph,
    mechanisms: &[&str],
    languages: Option<&[&str]>,
) -> Vec<CoverageNote> {
    if mechanisms.is_empty() {
        return Vec::new();
    }
    let langs: std::collections::HashSet<&str> = match languages {
        Some(l) => l.iter().copied().collect(),
        None => languages_present(merged),
    };
    notes(merged, |c| {
        (c.language == "*" || langs.contains(c.language))
            && (c.edge_category == "*" || mechanisms.contains(&c.edge_category))
    })
}

/// The [`COVERAGE_CAVEATS`] rows `keep` admits, in table order, each with its
/// category's edge count.
fn notes(merged: &MergedGraph, keep: impl Fn(&CoverageCaveat) -> bool) -> Vec<CoverageNote> {
    let counts = edge_category_counts(merged);
    COVERAGE_CAVEATS
        .iter()
        .filter(|c| keep(c))
        .map(|c| CoverageNote {
            language: c.language,
            edge_category: c.edge_category,
            note: c.note,
            verify: c.verify,
            edges_found: *counts.get(c.edge_category).unwrap_or(&0),
        })
        .collect()
}

/// Languages present in the repo, inferred from POSITION-cell file extensions.
fn languages_present(merged: &MergedGraph) -> std::collections::HashSet<&'static str> {
    let mut langs = std::collections::HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            for c in &n.cells {
                if c.kind != glia_code_domain::cell_type::POSITION {
                    continue;
                }
                if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                        if let Some(file) = v.get("file").and_then(|f| f.as_str()) {
                            if let Some(lang) = ext_to_language(file) {
                                langs.insert(lang);
                            }
                        }
                    }
                }
            }
        }
    }
    langs
}

/// Map a file path's extension to the analyzer language name used in caveats.
pub(crate) fn ext_to_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    Some(match ext {
        "py" => "python",
        "go" => "go",
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => "typescript",
        "dart" => "dart",
        "rs" => "rust",
        "java" => "java",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "scala" | "sc" => "scala",
        "sol" => "solidity",
        // CB.26: every extension `detect_language` sends to the C/C++ parser
        // (CB.1 added the `.hh` / `.hxx` / `.inl` / `.ipp` / `.tpp` set), under
        // its tag, so a caveat keyed "c_cpp" reaches a C/C++ repo.
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "inl" | "ipp" | "tpp" => "c_cpp",
        // A14.1: `.kt` only. `.kts` never passes the walk's read gate
        // (`detect_language`), so an arm for it could never fire.
        "kt" => "kotlin",
        _ => return None,
    })
}

/// Count edges per category name across the merged graph (intra + cross).
fn edge_category_counts(merged: &MergedGraph) -> std::collections::HashMap<&'static str, usize> {
    let mut counts = std::collections::HashMap::new();
    for e in merged.all_edges() {
        *counts.entry(edge_category::name(e.category)).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_flows_caveats_are_universal() {
        // A2.6: a repo with no recognisable language still gets the three
        // QUEUE_FLOWS notes, and each names a category that really exists.
        let report = coverage_report(&MergedGraph::new(Vec::new()));
        let queue: Vec<_> = report
            .iter()
            .filter(|n| n.edge_category == "QUEUE_FLOWS")
            .collect();
        assert_eq!(queue.len(), 3);
        assert!(
            queue
                .iter()
                .all(|n| n.language == "*" && n.edges_found == 0)
        );
        assert!(queue.iter().any(|n| n.note.contains("Pub/Sub")));
        assert!(queue.iter().any(|n| n.note.contains("SNS")));
        assert_eq!(
            edge_category::name(edge_category::QUEUE_FLOWS),
            "QUEUE_FLOWS",
            "edges_found is keyed by this spelling"
        );
    }

    #[test]
    fn documents_scope_caveat_is_universal() {
        // CJ.2: every repo is told which markdown is ingested and that a
        // mention is read only from a single-backtick span; the row sits
        // right after the contract-JSON one.
        let report = coverage_report(&MergedGraph::new(Vec::new()));
        let docs: Vec<_> = report
            .iter()
            .filter(|n| n.language == "*" && n.edge_category == "DOCUMENTS")
            .collect();
        assert_eq!(docs.len(), 2, "{:?}", docs.iter().map(|n| n.note).collect::<Vec<_>>());
        assert!(docs[0].note.starts_with("contract JSON"), "{}", docs[0].note);
        let note = docs[1].note;
        for needle in [
            "features/<feature>/*.md",
            "specs/<NNN-slug>/",
            "docs/ tree at the repo root or at a PROJECT root",
            "single-backtick",
            "README*.md",
            "at most 25 symbols",
        ] {
            assert!(note.contains(needle), "{needle}: {note}");
        }
        assert_eq!(docs[1].edges_found, 0);
        assert_eq!(
            edge_category::name(edge_category::DOCUMENTS),
            "DOCUMENTS",
            "edges_found is keyed by this spelling"
        );
    }

    #[test]
    fn embedded_sdl_caveat_names_the_variable_rule() {
        // LA.27: code-mode SDL is read only from marked literals, and among
        // bare variables only from typeDefs / type_defs; the row says so for
        // every repo, and names a category edges_found can count.
        let report = coverage_report(&MergedGraph::new(Vec::new()));
        let sdl: Vec<_> = report
            .iter()
            .filter(|n| n.edge_category == "GRAPHQL_CALLS")
            .collect();
        assert_eq!(sdl.len(), 1);
        assert_eq!((sdl[0].language, sdl[0].edges_found), ("*", 0));
        assert!(sdl[0].note.contains("typeDefs / type_defs"));
        assert!(sdl[0].note.contains("differently named variable"));
        assert!(sdl[0].note.contains("is not read"));
        assert_eq!(
            edge_category::name(edge_category::GRAPHQL_CALLS),
            "GRAPHQL_CALLS"
        );
    }

    #[test]
    fn ws_connects_caveat_is_universal() {
        // LA.18b: generic upgrade handlers pair only through their routes, so
        // an unreachable route is a declared recall gap on every repo.
        let report = coverage_report(&MergedGraph::new(Vec::new()));
        let ws: Vec<_> = report
            .iter()
            .filter(|n| n.edge_category == "WS_CONNECTS")
            .collect();
        assert_eq!(ws.len(), 1);
        assert_eq!((ws[0].language, ws[0].edges_found), ("*", 0));
        assert!(ws[0].note.contains("more than one call away"));
        assert!(ws[0].note.contains("no static path"));
        assert_eq!(
            edge_category::name(edge_category::WS_CONNECTS),
            "WS_CONNECTS",
            "edges_found is keyed by this spelling"
        );
    }

    #[test]
    fn rpc_calls_caveat_is_universal() {
        // LA.17: Connect / Twirp read only the build's .proto services and the
        // file-local client binding; the row says so for every repo.
        let report = coverage_report(&MergedGraph::new(Vec::new()));
        let rpc: Vec<_> = report
            .iter()
            .filter(|n| n.edge_category == "RPC_CALLS")
            .collect();
        assert_eq!(rpc.len(), 1);
        assert_eq!((rpc[0].language, rpc[0].edges_found), ("*", 0));
        assert!(rpc[0].note.contains(".proto is in the build"));
        assert!(rpc[0].note.contains("called from another"));
        assert_eq!(
            edge_category::name(edge_category::RPC_CALLS),
            "RPC_CALLS",
            "edges_found is keyed by this spelling"
        );
    }

    #[test]
    fn queue_callback_caveat_is_universal() {
        // LA.33: every repo is told which consumer shapes bind their callback,
        // and that the rest stay HANDLED_BY the subscribing function.
        let report = coverage_report(&MergedGraph::new(Vec::new()));
        let rows: Vec<_> = report
            .iter()
            .filter(|n| n.edge_category == "HANDLED_BY" && n.language == "*")
            .collect();
        assert_eq!(rows.len(), 1);
        let note = rows[0].note;
        for client in ["kafkajs", "amqplib", "BullMQ", "nats", "pika"] {
            assert!(note.contains(client), "{client}: {note}");
        }
        assert!(note.contains("the function that subscribes"), "{note}");
        assert_eq!(rows[0].edges_found, 0);
        assert_eq!(
            edge_category::name(edge_category::HANDLED_BY),
            "HANDLED_BY",
            "edges_found is keyed by this spelling"
        );
    }

    /// One MODULE node whose POSITION cell names `file` — all
    /// `languages_present` reads, so no parse is needed.
    fn graph_with_file(file: &str) -> MergedGraph {
        use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
        use glia_core::{Cell, Confidence, Node, NodeId, RepoId};
        use glia_graph::{RepoGraph, SymbolTable};
        let repo = RepoId::from_canonical("test://coverage");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "Sample");
        let g = RepoGraph {
            repo,
            nodes: vec![Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::POSITION,
                    payload: CellPayload::Json(format!(
                        r#"{{"file":"{file}","start_line":0,"end_line":2}}"#
                    )),
                }],
            }],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        MergedGraph::new(vec![g])
    }

    #[test]
    fn navigates_to_caveat_lists_for_typescript_repos_only() {
        // LA.6e: a TS repo's page flow declares the navigations it cannot
        // judge; a repo without TS gets no such row.
        let report = coverage_report(&graph_with_file("src/app/app.component.ts"));
        let nav: Vec<_> = report
            .iter()
            .filter(|n| n.edge_category == "NAVIGATES_TO")
            .collect();
        assert_eq!(nav.len(), 1);
        assert_eq!((nav[0].language, nav[0].edges_found), ("typescript", 0));
        assert!(nav[0].note.contains("navigate([path])"));
        assert!(nav[0].note.contains("never reported dead"));
        assert!(nav[0].note.contains("createUrlTree"), "guard redirects are declared");
        assert!(nav[0].verify.contains("routerLink") && nav[0].verify.contains("createUrlTree"));
        assert_eq!(
            edge_category::name(edge_category::NAVIGATES_TO),
            "NAVIGATES_TO",
            "edges_found is keyed by this spelling"
        );
        let go = coverage_report(&graph_with_file("main.go"));
        assert!(go.iter().all(|n| n.edge_category != "NAVIGATES_TO"));
    }

    #[test]
    fn rust_calls_caveat_names_the_receiver_gap() {
        // LA.1b: a Rust repo states the one CALLS gap LA.1-LA.3 leave, which
        // LA.35a narrows to the untyped receivers; a repo without Rust gets
        // no such row.
        let report = coverage_report(&graph_with_file("src/lib.rs"));
        let rust: Vec<_> = report
            .iter()
            .filter(|n| n.language == "rust" && n.edge_category == "CALLS")
            .collect();
        assert_eq!(rust.len(), 1);
        assert_eq!((rust[0].edge_category, rust[0].edges_found), ("CALLS", 0));
        let note = rust[0].note;
        // LA.35a: typed receivers bind; the row names the receivers that do not.
        assert!(!note.contains("not implemented"), "{note}");
        assert!(note.contains("a function's return value"), "{note}");
        // LA.35b: an `impl` in another file than its type binds through the hook.
        assert!(!note.contains("in another file than its type"), "{note}");
        assert!(note.contains("a trait's default body"), "{note}");
        assert_eq!(rust[0].verify, "grep the method name");
        let go = coverage_report(&graph_with_file("main.go"));
        assert!(go.iter().all(|n| n.language != "rust"));
        // Both reports share the universal rows; each adds its own language's.
        let own = |r: &[CoverageNote], lang: &str| r.iter().filter(|n| n.language == lang).count();
        assert_eq!(
            go.len() - own(&go, "go"),
            report.len() - own(&report, "rust")
        );
    }

    #[test]
    fn heritage_caveats_name_the_blind_languages() {
        // LD.7c: an implementors answer over Go, PHP, Swift or Rust carries the
        // heritage rows its language needs; Java, whose heritage is extracted,
        // gets none.
        let rows = |file: &str| -> Vec<(&'static str, &'static str)> {
            let mut r: Vec<_> = coverage_report(&graph_with_file(file))
                .iter()
                .filter(|n| matches!(n.edge_category, "IMPLEMENTS" | "INHERITS_FROM"))
                .map(|n| (n.language, n.edge_category))
                .collect();
            r.sort_unstable();
            r
        };
        assert_eq!(rows("main.go"), [("go", "IMPLEMENTS")]);
        assert_eq!(
            rows("Repo.php"),
            [("php", "IMPLEMENTS"), ("php", "INHERITS_FROM")]
        );
        assert_eq!(
            rows("Repo.swift"),
            [("swift", "IMPLEMENTS"), ("swift", "INHERITS_FROM")]
        );
        assert_eq!(rows("src/lib.rs"), [("rust", "INHERITS_FROM")]);
        assert!(rows("Repo.java").is_empty());
        let go = coverage_report(&graph_with_file("main.go"));
        let go_row = go.iter().find(|n| n.language == "go").map(|n| n.note);
        assert!(
            go_row.is_some_and(|n| n.contains("DERIVED") && n.contains("pointer and value receivers")),
            "{go_row:?}"
        );
        // CA.3b: the row says signatures are compared and what they miss.
        assert!(
            go_row.is_some_and(|n| n.contains("signatures (parameter and result types")
                && n.contains("a type alias are not resolved")),
            "{go_row:?}"
        );
    }

    #[test]
    fn kotlin_reports_as_blind_spot() {
        // A14.1 / A14.2 / A14.6: a Kotlin repo must SAY which dimensions to
        // grep rather than answer an empty blast radius. Since A14.6 each row
        // names the residual the full Kotlin chain leaves, not a missing pass.
        let report = coverage_report(&graph_with_file("src/Sample.kt"));
        let kotlin: Vec<_> = report.iter().filter(|n| n.language == "kotlin").collect();
        let mut cats: Vec<_> = kotlin.iter().map(|n| n.edge_category).collect();
        cats.sort_unstable();
        assert_eq!(
            cats,
            ["*", "CALLS", "HANDLED_BY", "HTTP_CALLS", "IMPORTS", "INHERITS_FROM"],
            "the Kotlin residual rows, one per category"
        );
        let star = kotlin.iter().find(|n| n.edge_category == "*").map(|n| n.note);
        assert!(
            star.is_some_and(|n| !n.contains("no Kotlin parser") && !n.contains("ungraphed")),
            "the parser landed: the `*` row describes its residual, not a missing parser"
        );
        let note = |cat: &str| {
            kotlin
                .iter()
                .find(|n| n.edge_category == cat)
                .map(|n| n.note)
                .unwrap_or_default()
        };
        // The rows describe residuals, never the pre-A14.3..A14.6 absences.
        assert!(!note("CALLS").contains("no CALLS are extracted"));
        assert!(!note("INHERITS_FROM").contains("not extracted yet"));
        assert!(!note("HTTP_CALLS").contains("emit no ENDPOINT, so"));
        // The residuals the correction names.
        // CA.6a: a typed parameter / `val` binds; an untyped lambda parameter
        // is the residual.
        assert!(note("CALLS").contains("typed parameter"));
        assert!(note("CALLS").contains("lambda parameter"));
        assert!(note("CALLS").contains("INTERFACE default method"));
        assert!(note("HTTP_CALLS").contains("Retrofit") && note("HTTP_CALLS").contains("OkHttp"));
        // CA.6b: Ktor-client verb calls are read; OkHttp stays the residual.
        assert!(note("HTTP_CALLS").contains("Ktor-client verb calls"));
        assert!(!note("HTTP_CALLS").contains("Ktor-client (`HttpClient().get(...)`)"));
        assert!(note("*").contains("Ktor-client client ENDPOINTs"));
        assert!(note("*").contains("`.kts` scripts"));
        assert!(kotlin.iter().all(|n| n.edges_found == 0));
        // Every non-`*` row names a category that exists, so edges_found can count it.
        for n in kotlin.iter().filter(|n| n.edge_category != "*") {
            assert!(
                edge_category::ALL
                    .iter()
                    .any(|(_, name)| *name == n.edge_category),
                "{} is not an edge category",
                n.edge_category
            );
        }
        // Control: the same graph over a .java file gets no Kotlin row.
        let java = coverage_report(&graph_with_file("src/Sample.java"));
        assert!(java.iter().all(|n| n.language != "kotlin"));
        assert_eq!(java.len(), report.len() - kotlin.len());
        // `.kts` never reaches the walk, so it maps to no language.
        assert_eq!(ext_to_language("build.gradle.kts"), None);
        assert_eq!(ext_to_language("app/Main.kt"), Some("kotlin"));
    }

    /// The `(language, edge_category)` rows a repo holding only `file` gets
    /// for `lang`, sorted.
    fn lang_rows(file: &str, lang: &str) -> Vec<(&'static str, &'static str)> {
        let mut r: Vec<_> = coverage_report(&graph_with_file(file))
            .iter()
            .filter(|n| n.language == lang)
            .map(|n| (n.language, n.edge_category))
            .collect();
        r.sort_unstable();
        r
    }

    /// The note of the `lang` row for `category` in a repo holding only `file`.
    fn row_note(file: &str, lang: &str, category: &str) -> &'static str {
        coverage_report(&graph_with_file(file))
            .iter()
            .find(|n| n.language == lang && n.edge_category == category)
            .map(|n| n.note)
            .unwrap_or_default()
    }

    #[test]
    fn c_cpp_files_get_their_caveats() {
        // CB.26: every extension detect_language routes to the C/C++ parser
        // maps to its tag, so a caveat keyed "c_cpp" reaches a C/C++ repo.
        for ext in ["c", "h", "cc", "cpp", "cxx", "hh", "hpp", "hxx", "inl", "ipp", "tpp"] {
            let path = format!("src/a.{ext}");
            assert_eq!(crate::extract::detect_language(&path), Some("c_cpp"), "{path}");
            assert_eq!(ext_to_language(&path), Some("c_cpp"), "{path}");
        }
        let want = [("c_cpp", "CALLS"), ("c_cpp", "IMPORTS")];
        assert_eq!(lang_rows("src/a.cpp", "c_cpp"), want);
        assert_eq!(lang_rows("include/a.hh", "c_cpp"), want);
        assert_eq!(lang_rows("lib/codec.c", "c_cpp"), want);
        // Control: a repo with no C/C++ file gets no c_cpp row.
        assert!(lang_rows("main.go", "c_cpp").is_empty());
        // CALLS names the residual CB.19 / CB.25 leave, measured.
        let calls = row_note("src/a.cpp", "c_cpp", "CALLS");
        for gap in [
            "overload set is one node",
            "function pointer",
            "never an override",
            "template parameter",
            "macro",
            "lambda body",
            "`auto`",
            "exactly one non-static definition",
            "only through an include",
        ] {
            assert!(calls.contains(gap), "{gap}: {calls}");
        }
        // IMPORTS names the search roots CB.22 reads and what it does not.
        let imports = row_note("include/a.hh", "c_cpp", "IMPORTS");
        for part in [
            "includer's directory",
            "compile_commands.json",
            "target_include_directories",
            "include/",
            "generator expression",
            "Bazel",
            "Meson",
            "Make",
        ] {
            assert!(imports.contains(part), "{part}: {imports}");
        }
    }

    #[test]
    fn swift_and_dart_calls_caveats_name_the_cb_residual() {
        // CB.10 / CB.18: members, extension members and same-module statics
        // bind; the row names the receivers that still do not.
        assert!(lang_rows("Sources/App/Cart.swift", "swift").contains(&("swift", "CALLS")));
        let swift = row_note("Sources/App/Cart.swift", "swift", "CALLS");
        for part in [
            "extension",
            "stored property",
            "protocol",
            "closure",
            "a parameter or a local",
            "another file",
            "another module",
        ] {
            assert!(swift.contains(part), "{part}: {swift}");
        }
        // CB.9 / CB.17: constructors, operators and extension members are
        // nodes; the row names the plain-import and receiver residual.
        assert!(lang_rows("lib/app.dart", "dart").contains(&("dart", "CALLS")));
        let dart = row_note("lib/app.dart", "dart", "CALLS");
        for part in [
            "constructors",
            "operators",
            "extension",
            "`show`",
            "`as`",
            "plain `import",
            "`dynamic`",
            "a parameter or a local",
        ] {
            assert!(dart.contains(part), "{part}: {dart}");
        }
        // Control: neither row reaches a repo without the language.
        assert!(lang_rows("main.go", "swift").is_empty());
        assert!(lang_rows("main.go", "dart").is_empty());
    }

    #[test]
    fn go_route_mount_caveat_names_the_unread_prefixes() {
        // CB.11 / CB.20 / CB.23: a literal group reaches routes through
        // parameters and fields when the passing call binds; the row says so.
        assert!(lang_rows("main.go", "go").contains(&("go", "HANDLED_BY")));
        let note = row_note("main.go", "go", "HANDLED_BY");
        for part in ["struct field", "string literal", "resolved", "constant", "function value"] {
            assert!(note.contains(part), "{part}: {note}");
        }
        assert!(lang_rows("src/lib.rs", "go").is_empty());
    }

    #[test]
    fn host_narrowing_caveats_are_universal() {
        // CB.21 / CB.24: every channel resolver narrows same-key targets only
        // on a literal host naming a project or an IaC service; each `*` row
        // of those mechanisms says so on every repo.
        let report = coverage_report(&MergedGraph::new(Vec::new()));
        let note = |cat: &str| -> Vec<&'static str> {
            report
                .iter()
                .filter(|n| n.language == "*" && n.edge_category == cat)
                .map(|n| n.note)
                .collect()
        };
        let gql = note("GRAPHQL_CALLS");
        assert_eq!(gql.len(), 1);
        assert!(gql[0].contains("literal absolute URL"), "{}", gql[0]);
        // CB.1's handoff: `.graphqls` stays named in both note and verify.
        assert!(gql[0].contains(".graphqls"), "{}", gql[0]);
        let gql_verify = report
            .iter()
            .find(|n| n.edge_category == "GRAPHQL_CALLS")
            .map(|n| n.verify)
            .unwrap_or_default();
        assert!(gql_verify.contains(".graphqls"), "{gql_verify}");
        let ws = note("WS_CONNECTS");
        assert_eq!(ws.len(), 1);
        assert!(ws[0].contains("literal") && ws[0].contains("IaC"), "{}", ws[0]);
        let rpc = note("RPC_CALLS");
        assert_eq!(rpc.len(), 1);
        assert!(rpc[0].contains("tRPC") && rpc[0].contains("literal absolute URL"), "{}", rpc[0]);
        let grpc = note("GRPC_CALLS");
        assert_eq!(grpc.len(), 1, "the gRPC row is keyed by its own mechanism");
        assert!(grpc[0].contains("dial target") && grpc[0].contains("IaC"), "{}", grpc[0]);
        assert_eq!(
            edge_category::name(edge_category::GRPC_CALLS),
            "GRPC_CALLS",
            "edges_found is keyed by this spelling"
        );
    }
}
