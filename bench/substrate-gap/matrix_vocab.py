#!/usr/bin/env python3
"""Machine-readable contract for the 16-language x 30-mechanism coverage matrix.

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
  cross_repo   whether the routing edge is emitted by a cross-repo resolver.
               This decides the fixture's `dirs`: grade.py:122-126 calls
               rg.generate() for a single dir and rg.generate_many() for several,
               and engine/src/build.rs:85 runs run_all_resolvers in BOTH cases --
               but a single-repo run only ever has one RepoId to pair, so a
               cross-repo edge can never appear. cross_repo True therefore means
               the fixture MUST use dirs ["client", "server"] (or similar);
               False means dirs ["."] is correct and two dirs would be noise.

Every kind and category name here is checked against the LOCKED code-domain
registries via repo_graph_py's decode tables (`--selftest`), so the vocabulary
cannot drift from code-domain/src/lib.rs. Only SHIPPED registry entries may be
named: a reserved-but-unemitted id is absent from the installed wheel's tables and
will fail the self-test.

DELIBERATE NON-DECISION -- there is NO not-applicable list. Cells like
terraform x calls or solidity x grpc are not enumerated as n/a. `unknown` already
carries exactly the "we make no claim" semantics, and an n/a list would re-import
the human judgement this harness exists to delete. Such cells simply stay
`unknown` forever, because nobody ever authors a fixture for them.
"""

import sys

# ---------------------------------------------------------------------------
# Languages -- the 16 review rows at review:128-143, in review order.
# ---------------------------------------------------------------------------

LANGUAGES = [
    "python", "go", "typescript", "java", "csharp", "ruby", "php", "swift",
    "c_cpp", "scala", "clojure", "dart", "elixir", "rust", "solidity", "terraform",
]

# Spelling variants only. Composite legacy values (`typescript+go`, `proto+go`)
# are DELIBERATELY not aliased: a fixture spanning two languages must name them
# per-assertion via `cells`, because one row cannot own a two-language result.
LANGUAGE_ALIASES = {
    "c++": "c_cpp", "cpp": "c_cpp", "c#": "csharp", "cs": "csharp",
    "ts": "typescript", "js": "typescript", "javascript": "typescript",
    "tsx": "typescript",
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
     "note": "no dedicated kind exists; review rank 15 proposes migration routing -- "
             "a fixture here is EXPECTED to read none"},

    {"id": "config", "label": "config", "family": "config",
     "kinds": [["CONFIG_KEY"]], "via_labels": ["config"],
     "literal": "the env-var name, as `config:env:<NAME>` in the qname",
     "categories": ["READS_CONFIG", "DEFINES_CONFIG", "SHARES_CONFIG"], "cross_repo": True,
     "note": "SHARES_CONFIG is the cross-repo pairing; READS/DEFINES are intra-repo"},
    {"id": "secrets", "label": "secrets", "family": "config",
     "kinds": [["CONFIG_KEY"]], "via_labels": ["config"],
     "literal": "the secret's key name in the CONFIG_KEY qname",
     "categories": ["READS_CONFIG", "DEFINES_CONFIG", "SHARES_CONFIG"], "cross_repo": True,
     "note": "no secrets-specific flavor; secrets are indistinguishable from env config today"},
    {"id": "flags", "label": "flags", "family": "config",
     "kinds": [["CONFIG_KEY"]], "via_labels": ["config"],
     "literal": "the feature-flag key in the CONFIG_KEY qname",
     "categories": ["READS_CONFIG", "DEFINES_CONFIG", "SHARES_CONFIG"], "cross_repo": True,
     "note": "no flag-provider needles; only flags that arrive as env vars are visible"},

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
     "literal": "the service name in the node name/qname",
     "categories": ["HTTP_CALLS", "GRPC_CALLS", "QUEUE_FLOWS", "GRAPHQL_CALLS",
                    "WS_CONNECTS", "EVENT_FLOWS", "SHARES_SCHEMA", "SHARES_DATA_ENTITY",
                    "SHARES_CONFIG", "SHARES_DEPENDENCY", "SHARES_INFRA_REF",
                    "SHARES_CRON_SCHEDULE"],
     "cross_repo": True,
     "note": "the roll-up column: any cross-service routing edge at all"},
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
# Accessors
# ---------------------------------------------------------------------------

def normalize_language(name):
    """Map a language spelling onto one of the 16 matrix rows.

    Raises ValueError naming all 16 rows on anything else, so a typo cannot
    silently mint a 17th row in the matrix.
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
    """Look up a mechanism by id, raising with all 30 valid ids on a miss."""
    m = _BY_ID.get(str(mid).strip())
    if m is None:
        raise ValueError(
            f"unknown mechanism {mid!r}; "
            f"the {len(MECHANISMS)} matrix columns are: {', '.join(MECHANISM_IDS)}"
        )
    return m


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
    import repo_graph_py as rg

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
    assert len(LANGUAGES) == 16, f"expected 16 languages, got {len(LANGUAGES)}"
    assert len(set(MECHANISM_IDS)) == 30, "duplicate mechanism id"
    assert len(set(LANGUAGES)) == 16, "duplicate language row"

    known_kinds = set(dict(rg.kind_names()).values())
    known_cats = set(dict(rg.category_names()).values())
    kinds_seen, cats_seen = set(), set()
    anchors = 0
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
                        f"registry (repo_graph_py.kind_names()); only SHIPPED kinds "
                        f"may be named in the vocabulary"
                    )
                kinds_seen.add(k)
        for c in m["categories"]:
            if c not in known_cats:
                raise ValueError(
                    f"{m['id']}: edge category {c!r} is not in the locked code-domain "
                    f"registry (repo_graph_py.category_names())"
                )
            cats_seen.add(c)

    assert normalize_language("c++") == "c_cpp"
    assert normalize_language("TS") == "typescript"
    for bad in ("kotlin", "typescript+go", "proto+go"):
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

    print(
        f"[matrix] vocab: {len(MECHANISMS)} mechanisms, {len(LANGUAGES)} languages, "
        f"{len(kinds_seen)} kinds referenced, {len(cats_seen)} categories referenced, "
        f"anchor={anchors}",
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
