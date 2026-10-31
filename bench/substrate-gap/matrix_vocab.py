#!/usr/bin/env python3
"""Machine-readable contract for the 17-language x 30-mechanism coverage matrix.

The review (dev-notes/review-2026-09-15-coverage-and-issues.md:127-143) states the
matrix as a markdown code block: a human reads a fixture, judges it, and writes a
glyph. This module exists so a cell verdict is DERIVED instead. For each mechanism
it pins down four things (plus one opt-in flag) a fixture author would otherwise
re-invent:

  kinds        which node kinds count as EXTRACTION, as ordered any-of groups.
               The first group with any node present wins and names `via`, so a
               mechanism that routes through an alternative registry (mqtt/redis
               through EVENT_* rather than QUEUE_*) records WHICH path fired
               instead of silently grading as if it were the intended one.
  literal      the identifying literal that must survive into the node name/qname.
               Extraction without the literal is the review's `partial`, not `full`.
  categories   which edge categories count as ROUTING (any-of).
  anchor       optional, default False. True marks an ANCHOR mechanism: its node
               is an edge-less anchor by design (PROJECT, like REGION), so it has
               no routing vocabulary (`categories` is empty) and matrix.derive()
               grades it on extract + literal + forbid, with route not-applicable
               rather than a cap. Opt-in per mechanism: every other column keeps
               the rule that an unrouted cell is not proven.
  role_cells   optional, default absent. Node-kind NAMES that also prove
               extraction when a fixture asserts them as a ROLE cell
               (`expect_cells {cell: ROLE, contains: <name>}`). LB.3's build-time
               fold removes a framework-role overlay (SERVICE, COMPONENT, ...) and
               leaves its declaration with its own kind plus a ROLE cell
               `{"roles":["SERVICE"]}`, so after the fold a role mechanism's kind
               group matches only standalone overlays. matrix.derive() counts a
               found ROLE row naming one of these as extraction AND literal (the
               row's `node` is matched on identity), and treats its role as a
               present kind when naming `via` -- the graph crate's `roles_in`
               model: a folded node plays its role kind. Every entry must also
               sit in one of the mechanism's `kinds` groups, so `via` resolves.
               Independent of `anchor`: the two flags never interact.
  cross_repo   whether the column is about a cross-service flow. It decides the
               scaffold's `dirs`: True gives ["client", "server"], False gives
               ["."]. It does NOT mean a single dir cannot show the edge: every
               cross-graph resolver runs in both generate() and generate_many(),
               and the stack resolvers (HTTP, gRPC, RPC, queue, GraphQL, WS,
               EventBus, CLI) pair inside one repo. Only the pairwise SHARES_*
               resolvers need two separate RepoIds -- REPO_PAIRWISE_CATEGORIES
               below, pinned by test_matrix.py. Two dirs stay the default for
               the realistic shape and the precise counts; AUTHORING.md step 1
               has the measured table.

Every kind and category name here is checked against the LOCKED code-domain
registries via glia_py's decode tables (`--selftest`), so the vocabulary
cannot drift from code-domain/src/lib.rs. Only SHIPPED registry entries may be
named: a reserved-but-unemitted id is absent from the installed wheel's tables and
will fail the self-test.

NOT APPLICABLE -- `NOT_APPLICABLE` below lists the (language, mechanism) cells
the language or runtime cannot express at all: an on-chain Solidity contract has
no network, broker or process access; Terraform declares no functions, types or
CLI. Such a cell reads `n/a` (glyph `-`), not `unknown`, so the unknown count
keeps meaning "work left". This reverses the earlier recorded non-decision (no
list, such cells stay `unknown` forever) -- James 2026-09-30, CF.13a. Two rules
keep the list from re-importing the judgement this harness exists to delete:

  structural only  an entry says the mechanism CANNOT be expressed. A cell whose
                   language merely lacks a client library today (ruby / php /
                   dart / elixir x azure_sb) is never listed and stays `unknown`:
                   a client can appear, and a wrong n/a hides a measurable cell,
                   while a wrong `unknown` costs nothing.
  falsifiable      a fixture that claims a listed cell is a CELL ERROR
                   (matrix.apply_not_applicable), and scaffold.py refuses to stamp
                   one. Proving a cell applicable therefore forces deleting its
                   entry in the same commit as the fixture.

The list is module level, NOT a MECHANISMS field, so the scoring ruler's digest
(matrix_emit.vocabulary_digest) does not move with it; it has its own
(matrix_emit.na_digest).
"""

import sys

# ---------------------------------------------------------------------------
# Languages -- the 16 review rows at review:128-143, in review order, then
# `kotlin` (CF.13b). The 17th row is NOT a review row: Kotlin has its own
# parser (parsers/code/kotlin) and only shares the JVM graph build with java,
# and rows are per language parser (scala and java are separate rows for the
# same reason). It is APPENDED so every review row, the column geometry and the
# review-order diff stay put. The 18th, `lua` (0.5.5, the Lua parser from glia
# PR #3), is appended after it for the same reason.
# ---------------------------------------------------------------------------

LANGUAGES = [
    "python", "go", "typescript", "java", "csharp", "ruby", "php", "swift",
    "c_cpp", "scala", "clojure", "dart", "elixir", "rust", "solidity", "terraform",
    "kotlin", "lua",
]

# Spelling variants only. Composite legacy values (`typescript+go`, `proto+go`)
# are DELIBERATELY not aliased: a fixture spanning two languages must name them
# per-assertion via `cells`, because one row cannot own a two-language result.
LANGUAGE_ALIASES = {
    "c++": "c_cpp", "cpp": "c_cpp", "c#": "csharp", "cs": "csharp",
    "ts": "typescript", "js": "typescript", "javascript": "typescript",
    "tsx": "typescript", "kt": "kotlin", "kts": "kotlin",
}

# ---------------------------------------------------------------------------
# Mechanisms -- the 30 review columns at review:127, in review order.
# `label` is the review's abbreviation so a rendered header diffs against the
# review's own block character-for-character.
# ---------------------------------------------------------------------------

_QUEUE = ["QUEUE_PRODUCER", "QUEUE_CONSUMER"]
_EVENT = ["EVENT_EMITTER", "EVENT_HANDLER"]


def _broker(mid, label, literal, note, via_first="queue"):
    """A message-broker mechanism: queue registry with an eventbus fallback.

    via_first="eventbus" flips the resolution order for brokers whose MEASURED
    path is the generic emitter/handler needles rather than a queue framework.
    """
    queue_first = via_first == "queue"
    return {
        "id": mid, "label": label, "family": "messaging",
        "kinds": [_QUEUE, _EVENT] if queue_first else [_EVENT, _QUEUE],
        "via_labels": ["queue", "eventbus"] if queue_first else ["eventbus", "queue"],
        "literal": literal,
        "categories": ["QUEUE_FLOWS", "EVENT_FLOWS"],
        "cross_repo": True, "note": note,
    }


_TOPIC = "topic/queue name (e.g. 'orders') in the node name and `queue_*:<topic>` qname"
_TAG = ("queues.rs:139/:170 falls back to framework_tag() when extract_topic_near "
        "finds no string literal -- a node named after the framework, not the "
        "topic, is the PARTIAL signal: extracted but literal lost")
_GENERIC = ("no QueueFramework variant exists (queues.rs:11-23); the only path is "
            "eventbus.rs:9-27 generic needles, which capture no broker identity")

MECHANISMS = [
    {"id": "http_client", "label": "http_cl", "family": "http",
     "kinds": [["ENDPOINT"]], "via_labels": ["http"],
     "literal": "the request path template (e.g. '/orders/{id}') in the ENDPOINT name/qname",
     "categories": ["HTTP_CALLS"], "cross_repo": True,
     "note": "caller side; HttpStackResolver pairs ENDPOINT to ROUTE across repos"},
    {"id": "http_server", "label": "http_sr", "family": "http",
     "kinds": [["ROUTE"]], "via_labels": ["http"],
     "literal": "the route path template in the ROUTE name/qname; a missing leading '/' is a partial",
     "categories": ["HANDLED_BY", "HTTP_CALLS"], "cross_repo": True,
     "note": "HANDLED_BY is intra-repo (route->handler); HTTP_CALLS is the cross-repo pairing"},

    _broker("kafka", "kafka", _TOPIC, "QueueFramework::Kafka; " + _TAG),
    _broker("amqp", "amqp", _TOPIC, "QueueFramework::RabbitMQ (channel.publish/consume); " + _TAG),
    _broker("sqs_sns", "sqs/sns", _TOPIC, _GENERIC, via_first="eventbus"),
    _broker("pubsub", "pubsub", _TOPIC, _GENERIC, via_first="eventbus"),
    _broker("azure_sb", "azure_s", _TOPIC, _GENERIC, via_first="eventbus"),
    _broker("nats", "nats", _TOPIC, "QueueFramework::Nats (nc.publish/nc.Publish); " + _TAG),
    _broker("redis", "redis", _TOPIC,
            "measured path is eventbus (.publish(/.subscribe(); QueueFramework::RedisList "
            "also exists but only for LPUSH/BLPOP list shapes",
            via_first="eventbus"),
    _broker("mqtt", "mqtt", _TOPIC, "measured path is eventbus; " + _GENERIC, via_first="eventbus"),
    _broker("taskq", "taskq", "task name in the node name/qname",
            "Celery/Dramatiq/BullMQ/Sidekiq/Oban; " + _TAG),

    {"id": "grpc", "label": "grpc", "family": "rpc",
     "kinds": [["GRPC_CLIENT", "GRPC_SERVICE"]], "via_labels": ["grpc"],
     "literal": "the service (and ideally method) name in the node name/qname",
     "categories": ["GRPC_CALLS"], "cross_repo": True,
     "note": "service-level only today; RPC_PROCEDURE (reserved node_kind 48) is unemitted"},
    {"id": "graphql", "label": "graphql", "family": "rpc",
     "kinds": [["GRAPHQL_OPERATION", "GRAPHQL_RESOLVER"]], "via_labels": ["graphql"],
     "literal": "the operation / field name in the node name/qname",
     "categories": ["GRAPHQL_CALLS"], "cross_repo": True, "note": ""},
    {"id": "ws", "label": "ws", "family": "streaming",
     "kinds": [["WS_CLIENT", "WS_HANDLER"]], "via_labels": ["websocket"],
     "literal": "the channel / event name in the node name/qname",
     "categories": ["WS_CONNECTS"], "cross_repo": True, "note": ""},
    {"id": "eventbus", "label": "eventbu", "family": "messaging",
     "kinds": [_EVENT], "via_labels": ["eventbus"],
     "literal": "the event name in the node name/qname",
     "categories": ["EVENT_FLOWS"], "cross_repo": True,
     "note": "eventbus.rs:9-27 EMITTER_PATTERNS/HANDLER_PATTERNS; the in-process bus proper"},

    {"id": "db", "label": "db", "family": "data",
     "kinds": [["DATA_ENTITY", "DATABASE"]], "via_labels": ["db"],
     "literal": "the table/collection name, as `data_entity:<flavor>:<name>` in the qname",
     "categories": ["ACCESSES_DATA", "SHARES_DATA_ENTITY"], "cross_repo": False,
     "note": "ACCESSES_DATA is intra-repo; SHARES_DATA_ENTITY needs two dirs to appear"},
    {"id": "migrations", "label": "migr", "family": "data",
     "kinds": [["DATA_ENTITY"]], "via_labels": ["db"],
     "literal": "the migrated table name in the DATA_ENTITY qname",
     "categories": ["ACCESSES_DATA"], "cross_repo": False,
     "note": "no dedicated kind: A13.9 routes migration .sql (Flyway / golang-migrate / "
             "Liquibase / migrations dirs) and reads migration DSLs (Alembic, Django, Rails, "
             "knex / Sequelize, Laravel, EF Core); the file's MODULE ACCESSES_DATA each table"},

    {"id": "config", "label": "config", "family": "config",
     "kinds": [["CONFIG_KEY"]], "via_labels": ["config"],
     "literal": "the env-var name, as `config:env:<NAME>` in the qname",
     "categories": ["READS_CONFIG", "DEFINES_CONFIG", "SHARES_CONFIG"], "cross_repo": True,
     "note": "SHARES_CONFIG is the cross-repo pairing; READS/DEFINES are intra-repo"},
    {"id": "secrets", "label": "secrets", "family": "config",
     "kinds": [["CONFIG_KEY"]], "via_labels": ["config"],
     "literal": "the secret's key name in the CONFIG_KEY qname",
     "categories": ["READS_CONFIG", "DEFINES_CONFIG", "SHARES_CONFIG"], "cross_repo": True,
     "note": "A13.8: a secrets-manager ref is `config:secret:<provider>/<ref>` (vault / aws_sm / "
             "gcp_sm / azure_kv from code, k8s from secretKeyRef / secretRef reads and kind: Secret "
             "defines); an env-delivered secret stays `config:env:` with the redacted ENV cell. "
             "Out of scope: .NET IConfiguration[\"Section:Key\"] + appsettings.json (csharp/secrets)"},
    {"id": "flags", "label": "flags", "family": "config",
     "kinds": [["CONFIG_KEY"]], "via_labels": ["config"],
     "literal": "the feature-flag key in the CONFIG_KEY qname",
     "categories": ["READS_CONFIG", "DEFINES_CONFIG", "SHARES_CONFIG"], "cross_repo": True,
     "note": "A13.8: an SDK check (LaunchDarkly, OpenFeature, Unleash, Flagsmith, Split) reads "
             "`config:flag:<key>`, gated on the file naming its provider; a Flipt `flags:` list "
             "defines it; the qname carries no provider so both sides pair (SHARES_CONFIG)"},

    {"id": "cron", "label": "cron", "family": "schedule",
     "kinds": [["CRON_JOB"]], "via_labels": ["cron"],
     "literal": "the schedule expression, as `cron:<schedule>` in the qname",
     "categories": ["SCHEDULES", "SHARES_CRON_SCHEDULE"], "cross_repo": True, "note": ""},

    {"id": "cli_def", "label": "cli_def", "family": "cli",
     "kinds": [["CLI_COMMAND"]], "via_labels": ["cli"],
     "literal": "the command name in the CLI_COMMAND name/qname",
     "categories": ["CLI_INVOKES"], "cross_repo": True, "note": "definition side"},
    {"id": "cli_inv", "label": "cli_inv", "family": "cli",
     "kinds": [["CLI_INVOCATION"]], "via_labels": ["cli"],
     "literal": "the invoked command name in the CLI_INVOCATION name/qname",
     "categories": ["CLI_INVOKES"], "cross_repo": True, "note": "invocation side"},

    {"id": "calls", "label": "calls", "family": "intra",
     "kinds": [["FUNCTION", "METHOD"]], "via_labels": ["call"],
     "literal": "the callee's qname (`Module::Class::method`), resolved not textual",
     "categories": ["CALLS"], "cross_repo": False, "note": ""},
    {"id": "imports", "label": "imports", "family": "intra",
     "kinds": [["MODULE", "PACKAGE", "PACKAGE_DEP"]], "via_labels": ["import"],
     "literal": "the imported module path or `package:<ecosystem>:<name>` qname",
     "categories": ["IMPORTS", "DEPENDS_ON"], "cross_repo": False,
     "note": "IMPORTS is source-level; DEPENDS_ON is manifest-level (PACKAGE_DEP)"},
    {"id": "injects", "label": "injects", "family": "intra",
     "kinds": [["SERVICE", "CLASS", "INTERFACE"]], "via_labels": ["di"],
     "literal": "the injected type's name in the target node's name/qname",
     "categories": ["INJECTS"], "cross_repo": False, "note": ""},
    {"id": "impl", "label": "impl", "family": "intra",
     "kinds": [["INTERFACE", "CLASS", "STRUCT"]], "via_labels": ["type"],
     "literal": "the interface / base type name in the target node's name/qname",
     "categories": ["IMPLEMENTS", "INHERITS_FROM"], "cross_repo": False,
     "note": "IMPLEMENTS (cat 32) split from INHERITS_FROM (cat 20) in glia-v5 G12.5"},
    {"id": "tests", "label": "tests", "family": "intra",
     "kinds": [["FUNCTION", "METHOD"]], "via_labels": ["test"],
     "literal": "the tested symbol's qname on the TESTS edge target",
     "categories": ["TESTS"], "cross_repo": False, "note": ""},

    {"id": "service", "label": "service", "family": "topology",
     "kinds": [["SERVICE", "REGION"]], "via_labels": ["service"],
     "role_cells": ["SERVICE"],
     "literal": "the service name in the node name/qname",
     "categories": ["HTTP_CALLS", "GRPC_CALLS", "QUEUE_FLOWS", "GRAPHQL_CALLS",
                    "WS_CONNECTS", "EVENT_FLOWS", "SHARES_SCHEMA", "SHARES_DATA_ENTITY",
                    "SHARES_CONFIG", "SHARES_INFRA_REF",
                    "SHARES_CRON_SCHEDULE"],
     "cross_repo": True,
     "note": "the roll-up column: any cross-service routing edge at all. Extraction "
             "is a SERVICE/REGION node OR a declaration (CLASS / STRUCT / PACKAGE / "
             "FUNCTION) carrying LB.3's ROLE cell naming SERVICE: the fold removes "
             "every SERVICE twin that has a same-qname declaration"},
    {"id": "subproject", "label": "subproj", "family": "topology",
     "kinds": [["PROJECT"]], "via_labels": ["project"],
     "literal": "the sub-project dir in the PROJECT qname `project:<rel_path>`",
     "categories": [], "anchor": True, "cross_repo": False,
     "note": "PROJECT (A8.5) is an edge-less anchor; membership is by longest path "
             "prefix (glia arch / --scope), so the column grades on the anchor, its "
             "literal and its precision guards"},
]

_BY_ID = {m["id"]: m for m in MECHANISMS}
MECHANISM_IDS = [m["id"] for m in MECHANISMS]

# ---------------------------------------------------------------------------
# Use patterns (0.5.5) -- the matrix's second axis. The 30 mechanisms are how
# services talk to each other (a web-service shape: HTTP, brokers, RPC, env
# config). Much code is not a service: it is embedded in a host (game engines,
# editors, window managers, nginx, Redis), wired by event names, or run as a
# script. James 2026-10-31: "the matrix ... doesn't deal with language use
# patterns breakdown like lua is in game scripting for glue work or processing
# scripts or Linux stuff". Each pattern is graded per language by the SAME
# machinery as a mechanism (matrix.derive), filed under patterns/<lang>/<id>/,
# rendered as its own table, and kept out of the 30-column totals. The keys use
# today's node / edge / cell vocabulary, so a pattern glia does not read yet
# grades `none` or `partial` (blindness stated) until 0.5.6's host-to-plugin
# graph fills it. `proof_cells`: a cell that proves the routing where no edge
# can (a host-called function is an entry, not an edge's endpoint).
PATTERNS = [
    {"id": "host_command", "label": "h_cmd", "family": "host",
     "kinds": [["CLI_COMMAND"]], "via_labels": ["host_command"],
     "literal": "the command name the script registers with its host, in the CLI_COMMAND name/qname",
     "categories": ["HANDLED_BY"], "cross_repo": False,
     "note": "Neovim nvim_create_user_command, a Maki register_tool, a VS Code registerCommand: "
             "the host's command -> the script's handler"},
    {"id": "host_event", "label": "h_evt", "family": "host",
     "kinds": [["EVENT_HANDLER"]], "via_labels": ["host_event"],
     "literal": "the host event name in the EVENT_HANDLER name/qname",
     "categories": ["HANDLED_BY"], "cross_repo": False,
     "note": "Neovim autocmds, WoW RegisterEvent + OnEvent, AwesomeWM connect_signal: a host "
             "event name -> the script's callback"},
    {"id": "host_callback", "label": "h_cb", "family": "host",
     "kinds": [["FUNCTION", "METHOD"]], "via_labels": ["host_callback"],
     "literal": "the callback name the host calls by convention (`love.update`)",
     "categories": [], "proof_cells": ["ENTRYPOINT"], "cross_repo": False,
     "note": "LOVE love.load / update / draw, Defold update(self, dt): the host calls the "
             "function by its name, so it is an entry (ENTRYPOINT cell), never dead"},
    {"id": "config_table", "label": "cfg_tbl", "family": "host",
     "kinds": [["CONFIG_KEY"]], "via_labels": ["config_table"],
     "literal": "the setting's key in the CONFIG_KEY qname (`config:<host>:<key>`)",
     "categories": ["DEFINES_CONFIG"], "cross_repo": False,
     "note": "configuration as code the host reads: WezTerm's returned table, vim.opt"},
    {"id": "embedded_script", "label": "embed", "family": "host",
     "kinds": [["MODULE"]], "via_labels": ["embedded_script"],
     "literal": "the script's MODULE, as the target of its host's reference",
     "categories": ["HANDLED_BY", "USES"], "cross_repo": False,
     "note": "another language's code or config runs the script: nginx content_by_lua_file "
             "(a location ROUTE HANDLED_BY the module), a Redis EVAL / register_script"},
    {"id": "script_entry", "label": "script", "family": "host",
     "kinds": [["MODULE"]], "via_labels": ["script_entry"],
     "literal": "the script's MODULE",
     "categories": [], "proof_cells": ["ENTRYPOINT"], "cross_repo": False,
     "note": "a standalone script run from a shell (`#!/usr/bin/env lua`, `arg`): its "
             "MODULE is an entry"},
    # 0.5.6 (GH.12): a process another file starts. Not script_entry: the
    # evidence is the deployment unit (cross-file, cross-language, like
    # embedded_script) and the target may be a compiled entry (a Go main
    # package, a Rust bin, a Java main class, a .NET Program) where a `main`
    # FUNCTION is already a named entry and the gain is the HANDLED_BY link.
    {"id": "process_entry", "label": "proc", "family": "host",
     "kinds": [["INFRA_RESOURCE"]], "via_labels": ["process_entry"],
     "literal": "the started file's MODULE / main FUNCTION / main METHOD as the HANDLED_BY "
                "target of the starting INFRA_RESOURCE",
     "categories": ["HANDLED_BY"], "proof_cells": ["ENTRYPOINT"], "cross_repo": False,
     "note": "Dockerfile ENTRYPOINT / CMD, compose command, k8s command / args, Procfile, "
             "systemd ExecStart: the host that starts the process -> the code it starts"},
]
_PATTERN_BY_ID = {p["id"]: p for p in PATTERNS}
PATTERN_IDS = [p["id"] for p in PATTERNS]

# The cross-graph categories that appear ONLY when the build has two separate
# RepoIds: their resolvers pair a shared name across repos and skip same-repo
# pairs (graph/src/resolvers/mod.rs emit_cross_repo_pairs, and the equivalent
# check in config / cron / iac / shared_schema / db). Every other
# cross-graph category pairs inside one repo. Module level, NOT a MECHANISMS
# field, so the vocabulary digest does not move. test_matrix.py
# test_single_dir_pairing_matches_the_vocabulary holds real builds to it.
# Edge category 31 SHARES_DEPENDENCY (registered, never reused) is in neither
# list: no resolver emits it since 0.5.2 removed PackageResolver (DD.16); a
# shared package is a SAME_AS identity class now.
REPO_PAIRWISE_CATEGORIES = frozenset({
    "SHARES_SCHEMA", "SHARES_DATA_ENTITY", "SHARES_DATA_SOURCE", "SHARES_CONFIG",
    "SHARES_INFRA_REF", "SHARES_CRON_SCHEDULE",
})

# The cell type a `role_cells` mechanism reads (LB.3a, cell_type ROLE). Module
# level for the same reason as above: the digest covers MECHANISMS only. The
# self-test holds it to the locked cell_type registry.
ROLE_CELL = "ROLE"


# ---------------------------------------------------------------------------
# Not applicable -- cell label -> (class, reason). See the module docstring:
# structural only, and falsified by any fixture that claims the cell. Written
# sorted; the self-test and test_matrix.py hold every row to a canonical
# LANGUAGES row and MECHANISM_IDS column. Consumers iterate it sorted, so an emit
# is byte-identical across processes. A cell whose language merely lacks a
# client library (ruby / php / dart / elixir azure_sb) is deliberately absent.
# ---------------------------------------------------------------------------

NA_LEVEL = "n/a"
NA_STRUCTURAL = "structural"

_SOLIDITY_ONCHAIN = (
    "an on-chain Solidity contract has no network, broker, filesystem, environment "
    "or process access; the off-chain tooling around it (hardhat / foundry scripts, "
    "subgraphs) is TypeScript / YAML and scores on those rows")

NOT_APPLICABLE = {
    "solidity/amqp": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/azure_sb": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/cli_def": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/cli_inv": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/config": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/cron": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/db": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/flags": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/graphql": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/grpc": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/http_client": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/http_server": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/kafka": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/migrations": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/mqtt": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/nats": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/pubsub": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/redis": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/secrets": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/sqs_sns": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/taskq": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "solidity/ws": (NA_STRUCTURAL, _SOLIDITY_ONCHAIN),
    "terraform/calls": (NA_STRUCTURAL,
                        "Terraform has no user-defined functions; built-in and provider "
                        "functions are not resolved callee qnames, and a module block is "
                        "the imports column"),
    "terraform/cli_def": (NA_STRUCTURAL, "Terraform declares no command-line interface"),
    "terraform/impl": (NA_STRUCTURAL,
                       "Terraform declares no types, so nothing implements or inherits"),
    "terraform/injects": (NA_STRUCTURAL,
                          "Terraform declares no types, so nothing is injected"),
    "terraform/redis": (NA_STRUCTURAL,
                        "no Terraform construct publishes to or subscribes on a Redis "
                        "channel or list; ElastiCache resources declare the store only"),
    "terraform/taskq": (NA_STRUCTURAL,
                        "no Terraform construct names, enqueues or processes a task; a "
                        "Cloud Tasks queue resource declares the queue only"),
}


# ---------------------------------------------------------------------------
# Chain cells (FH.10) -- where mechanisms CHAIN. Real repos break where one
# mechanism hands to another (a route prefix read through a constant, a client
# that calls a wrapper that calls the endpoint, a client reaching a route through
# a workspace SDK package, a single-file component calling a service), which no
# per-mechanism column can show. Each chain is one row `chain/<id>`, graded by the
# outlier cohort (bench/cohort/cells.py tags the rows that cross it) and, on the
# fixture side, by generated repos (FH.11); until then its fixture grade is
# `unknown`. Module level, NOT a MECHANISMS field, so the scoring ruler's digest
# does not move; it has its own (matrix_emit.chains_digest). Order is render order.
# ---------------------------------------------------------------------------

CHAINS = [
    {"id": "const_prefix", "label": "prefix through a constant",
     "description": "a route prefix, base URL or path segment held in a constant or enum"},
    {"id": "client_wrapper_endpoint", "label": "client -> wrapper -> endpoint",
     "description": "the client calls a request wrapper that issues the HTTP call"},
    {"id": "workspace_sdk_route", "label": "client -> workspace package -> SDK -> route",
     "description": "the client imports a workspace SDK / client package that reaches the "
                    "route"},
    {"id": "sfc_service_endpoint", "label": "SFC -> service -> endpoint",
     "description": "a single-file component calls a service that issues the call"},
]
CHAIN_IDS = [c["id"] for c in CHAINS]


# ---------------------------------------------------------------------------
# Accessors
# ---------------------------------------------------------------------------

def normalize_language(name):
    """Map a language spelling onto one of the LANGUAGES matrix rows.

    Raises ValueError naming every row on anything else, so a typo cannot
    silently mint a new row in the matrix.
    """
    key = str(name).strip().lower()
    key = LANGUAGE_ALIASES.get(key, key)
    if key not in LANGUAGES:
        raise ValueError(
            f"unknown language {name!r} (normalized {key!r}); "
            f"the {len(LANGUAGES)} matrix rows are: {', '.join(LANGUAGES)}"
        )
    return key


def mechanism(mid):
    """Look up a mechanism, or a use pattern (PATTERNS), by id, raising with
    every valid id on a miss. The grading reads both alike."""
    key = str(mid).strip()
    m = _BY_ID.get(key) or _PATTERN_BY_ID.get(key)
    if m is None:
        raise ValueError(
            f"unknown mechanism {mid!r}; "
            f"the {len(MECHANISMS)} matrix columns are: {', '.join(MECHANISM_IDS)}; "
            f"the {len(PATTERNS)} use patterns are: {', '.join(PATTERN_IDS)}"
        )
    return m


def is_pattern(mid):
    """True for a use-pattern id (PATTERNS), false for a mechanism."""
    return str(mid).strip() in _PATTERN_BY_ID


def not_applicable(lang, mech):
    """The reason `<lang>/<mech>` is NOT_APPLICABLE, or None when it is not listed.

    Takes the canonical row and column ids (what matrix.parse_cell returns); an
    alias spelling is simply not listed, so it reads None.
    """
    entry = NOT_APPLICABLE.get(f"{lang}/{mech}")
    return entry[1] if entry else None


def resolve_via(mid, present_kinds):
    """Name which extraction path actually fired, given the kinds observed.

    Returns the `via_labels` entry for the first `kinds` group with ANY member in
    `present_kinds`, or None when nothing was extracted. This is what stops a
    mechanism that routed through its alternative registry (mqtt through EVENT_*
    rather than QUEUE_*) from grading as if the intended path had fired.
    """
    have = set(present_kinds)
    m = mechanism(mid)
    for group, label in zip(m["kinds"], m["via_labels"]):
        if have.intersection(group):
            return label
    return None


# ---------------------------------------------------------------------------
# Self-test -- checks the vocabulary against the LOCKED code-domain registries.
# ---------------------------------------------------------------------------

def _selftest():
    import glia_py as rg

    expected_ids = [
        "http_client", "http_server", "kafka", "amqp", "sqs_sns", "pubsub",
        "azure_sb", "nats", "redis", "mqtt", "taskq", "grpc", "graphql", "ws",
        "eventbus", "db", "migrations", "config", "secrets", "flags", "cron",
        "cli_def", "cli_inv", "calls", "imports", "injects", "impl", "tests",
        "service", "subproject",
    ]
    assert MECHANISM_IDS == expected_ids, (
        f"mechanism order drifted from review:127\n  got {MECHANISM_IDS}\n  want {expected_ids}"
    )
    assert len(MECHANISMS) == 30, f"expected 30 mechanisms, got {len(MECHANISMS)}"
    assert len(LANGUAGES) == 18, f"expected 18 languages, got {len(LANGUAGES)}"
    assert len(set(MECHANISM_IDS)) == 30, "duplicate mechanism id"
    assert len(set(LANGUAGES)) == 18, "duplicate language row"

    known_kinds = set(dict(rg.kind_names()).values())
    known_cats = set(dict(rg.category_names()).values())
    known_cells = set(dict(rg.cell_type_names()).values())
    assert ROLE_CELL in known_cells, (
        f"ROLE_CELL {ROLE_CELL!r} is not in the locked cell_type registry "
        f"(glia_py.cell_type_names())")
    kinds_seen, cats_seen = set(), set()
    anchors = roled = 0
    for m in MECHANISMS:
        assert len(m["via_labels"]) == len(m["kinds"]), (
            f"{m['id']}: {len(m['via_labels'])} via_labels for {len(m['kinds'])} kind groups"
        )
        anchor = m.get("anchor", False)
        assert isinstance(anchor, bool), f"{m['id']}: anchor must be a bool"
        # An anchor mechanism has no routing vocabulary BY DEFINITION; every other
        # mechanism must name at least one routing category.
        assert m["kinds"] and (m["categories"] or anchor), (
            f"{m['id']}: empty kinds, or empty categories on a non-anchor mechanism"
        )
        assert not (anchor and m["categories"]), (
            f"{m['id']}: an anchor mechanism grades route as not-applicable, so it "
            f"must not also declare routing categories"
        )
        anchors += anchor
        assert m["literal"], f"{m['id']}: no identifying literal declared"
        assert isinstance(m["cross_repo"], bool), f"{m['id']}: cross_repo must be a bool"
        for group in m["kinds"]:
            for k in group:
                if k not in known_kinds:
                    raise ValueError(
                        f"{m['id']}: node kind {k!r} is not in the locked code-domain "
                        f"registry (glia_py.kind_names()); only SHIPPED kinds "
                        f"may be named in the vocabulary"
                    )
                kinds_seen.add(k)
        for c in m["categories"]:
            if c not in known_cats:
                raise ValueError(
                    f"{m['id']}: edge category {c!r} is not in the locked code-domain "
                    f"registry (glia_py.category_names())"
                )
            cats_seen.add(c)
        roles = m.get("role_cells", [])
        assert isinstance(roles, list), f"{m['id']}: role_cells must be a list"
        roled += bool(roles)
        for r in roles:
            # ROLE payloads name node kinds (`{"roles":["SERVICE"]}`), so a
            # role_cells entry is a node-kind NAME, never a free string.
            if r not in known_kinds:
                raise ValueError(
                    f"{m['id']}: role_cells entry {r!r} is not a node kind in the "
                    f"locked code-domain registry (glia_py.kind_names()); ROLE "
                    f"payloads name kinds"
                )
            assert any(r in group for group in m["kinds"]), (
                f"{m['id']}: role_cells entry {r!r} is in none of its kinds groups, "
                f"so a role-proven cell could not name its `via`"
            )

    assert normalize_language("c++") == "c_cpp"
    assert normalize_language("TS") == "typescript"
    assert normalize_language("kt") == "kotlin"
    assert normalize_language("kts") == "kotlin"
    # `kotlin+java` stays composite like the others: the JVM family builds one
    # graph, but a fixture spanning both names each row per assertion via `cells`.
    for bad in ("kotlin+java", "typescript+go", "proto+go"):
        try:
            normalize_language(bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"normalize_language({bad!r}) should have raised")
    try:
        mechanism("sqs")
    except ValueError:
        pass
    else:
        raise AssertionError("mechanism('sqs') should have raised")
    assert resolve_via("mqtt", ["EVENT_EMITTER"]) == "eventbus"
    assert resolve_via("mqtt", ["QUEUE_PRODUCER"]) == "queue"
    assert resolve_via("mqtt", ["CLASS"]) is None

    assert list(NOT_APPLICABLE) == sorted(NOT_APPLICABLE), "NOT_APPLICABLE must be written sorted"
    for label, entry in NOT_APPLICABLE.items():
        lang, sep, mech = label.partition("/")
        assert sep and normalize_language(lang) == lang, (
            f"NOT_APPLICABLE {label!r}: the row must be a canonical LANGUAGES id")
        assert mech in MECHANISM_IDS, (
            f"NOT_APPLICABLE {label!r}: {mech!r} is not a MECHANISM_IDS column")
        assert isinstance(entry, tuple) and len(entry) == 2, (
            f"NOT_APPLICABLE {label!r}: an entry is (class, reason)")
        cls, reason = entry
        assert cls == NA_STRUCTURAL, (
            f"NOT_APPLICABLE {label!r}: class {cls!r}; only {NA_STRUCTURAL!r} is listed "
            f"(a missing client library is not a reason: that cell stays unknown)")
        assert isinstance(reason, str) and len(reason) >= 20, (
            f"NOT_APPLICABLE {label!r}: the reason must say why, in >= 20 chars")
        assert not_applicable(lang, mech) == reason
    assert len(set(CHAIN_IDS)) == len(CHAINS), "duplicate chain id"
    for ch in CHAINS:
        assert set(ch) == {"id", "label", "description"}, f"chain {ch!r}: id, label, description"
        assert ch["id"] not in MECHANISM_IDS, f"chain {ch['id']!r} shadows a mechanism column"
    # the unknown anchor test_matrix.py measures must stay measurable
    assert "clojure/kafka" not in NOT_APPLICABLE
    assert not_applicable("clojure", "kafka") is None

    print(
        f"[matrix] vocab: {len(MECHANISMS)} mechanisms, {len(LANGUAGES)} languages, "
        f"{len(kinds_seen)} kinds referenced, {len(cats_seen)} categories referenced, "
        f"anchor={anchors} role_cells={roled} not_applicable={len(NOT_APPLICABLE)} "
        f"chains={len(CHAINS)}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    if "--selftest" in sys.argv[1:]:
        try:
            _selftest()
        except (AssertionError, ValueError) as exc:
            print(f"[matrix] vocab SELFTEST FAILED: {exc}", file=sys.stderr)
            sys.exit(1)
        sys.exit(0)
    print(__doc__)
