// ============================================================================
// P2 coverage / blind-spot signaling (handoff v6) — turn silent blind spots
// into declared ones so the graph+grep fallback is deliberate, not lucky.
// ============================================================================

use repo_graph_code_domain::edge_category;
use repo_graph_core::CellPayload;
use repo_graph_graph::MergedGraph;

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
    // LA.27 (James's call): SDL inside code is read only from GraphQL-marked
    // literals, so unmarked SDL is a declared recall gap, not a silent one.
    CoverageCaveat {
        language: "*",
        edge_category: "GRAPHQL_CALLS",
        note: "GraphQL SDL embedded in code is read only from a GraphQL-marked literal: a gql / graphql tag or call, a buildSchema / MustParseSchema / ParseSchema / from_definition argument, a /* GraphQL */ or #graphql literal, a GRAPHQL / GQL heredoc, or a literal bound to a variable or key named typeDefs / type_defs. SDL kept unmarked in a differently named variable (a plain template in `const schema = ...`, a Go string passed to MustParseSchema by name) is not read: its root types and fields mint no GRAPHQL_RESOLVER, so its clients' operations pair with nothing. `.graphql` / `.gql` files are read whole.",
        verify: "grep 'type Query {' / 'type Mutation {' outside .graphql / .gql files and read the variable that holds it",
    },
    // LA.18b: a path-less upgrade handler (gorilla, nhooyr, raw ASP.NET) pairs
    // only through the routes that reach its upgrading function, and a client
    // whose URL the extractor could not read pairs nothing.
    CoverageCaveat {
        language: "*",
        edge_category: "WS_CONNECTS",
        note: "a WebSocket upgrade whose route is registered more than one call away from the upgrading function, or through a router glia does not extract, stays unpaired; a client whose URL has no static path is not paired",
        verify: "grep the upgrade call and the route registration",
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
    CoverageCaveat {
        language: "dart",
        edge_category: "HTTP_CALLS",
        note: "dio / http client verbs are extracted; other HTTP libraries are not",
        verify: "grep the HTTP client class name",
    },
    // A14.1 / A14.2 — the Kotlin rows. `.kt` has its own parser since A14.2
    // (`parsers/code/kotlin`, one JVM graph with Java), so these describe the
    // residual that parser leaves, measured on bench/substrate-gap/fixtures/
    // kotlin-{entities,spring,ktor,retrofit,flip-guard}. The Kotlin packets
    // after A14.2 (calls + heritage, Spring / JPA annotations, Ktor handlers,
    // HTTP clients) each rewrite the row they close.
    CoverageCaveat {
        language: "kotlin",
        edge_category: "*",
        note: "Kotlin declarations are extracted by a dedicated parser: classes, interfaces, enums, objects (companion members attach to their class), member and top-level / extension functions, non-trivial properties and imports. Framework needles are the gap: Spring stereotype and mapping annotations, constructor / field INJECTS, JPA `@Entity` DATA_ENTITY and repository ACCESSES_DATA are not read from Kotlin. `.kts` scripts (Gradle KTS) are never parsed.",
        verify: "grep the annotation (@RestController, @GetMapping, @Entity, @Autowired) across *.kt",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "CALLS",
        note: "no CALLS are extracted from Kotlin yet: its functions and methods are nodes, but no call site inside them is recorded, so a Kotlin caller never appears in a blast radius or trace",
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
        note: "Kotlin `: Base()` / `: Iface` supertype lists are not extracted yet — no INHERITS_FROM or IMPLEMENTS edge exists for any Kotlin type",
        verify: "grep the supertype name across *.kt",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "HANDLED_BY",
        note: "Ktor `get(\"/path\") { }`, Javalin `app.get(\"/path\", h)` and WebFlux `.GET(\"/path\", h)` ROUTE nodes are emitted by a text scan with no handler and no HANDLED_BY edge; Spring `@RequestMapping` / `@GetMapping` ROUTEs are not extracted from Kotlin at all",
        verify: "grep for routing { / @GetMapping / .get(\" in *.kt",
    },
    CoverageCaveat {
        language: "kotlin",
        edge_category: "HTTP_CALLS",
        note: "Kotlin HTTP clients (RestTemplate, WebClient, Retrofit interfaces) emit no ENDPOINT, so a Kotlin caller never pairs to the route it calls",
        verify: "grep getForObject / .uri( / @GET( across *.kt",
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
        eprintln!("[coverage] kotlin: parser is new — calls/heritage/framework needles may be partial");
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
                if c.kind != repo_graph_code_domain::cell_type::POSITION {
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

    /// One MODULE node whose POSITION cell names `file` — all
    /// `languages_present` reads, so no parse is needed.
    fn graph_with_file(file: &str) -> MergedGraph {
        use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
        use repo_graph_core::{Cell, Confidence, Node, NodeId, RepoId};
        use repo_graph_graph::{RepoGraph, SymbolTable};
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
    fn kotlin_reports_as_blind_spot() {
        // A14.1 / A14.2: Kotlin has a parser now, but its calls, heritage and
        // framework needles are still missing, so a Kotlin repo must SAY which
        // dimensions to grep rather than answer an empty blast radius.
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
}
