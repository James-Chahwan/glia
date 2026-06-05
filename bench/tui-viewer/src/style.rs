//! kind → glyph + tier classification + score → intensity mapping.

use repo_graph_code_domain::node_kind::*;
use repo_graph_core::NodeKindId;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tier {
    Entry,
    Service,
    Handler,
    Data,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Entry => "ENTRY",
            Tier::Service => "SERVICE",
            Tier::Handler => "HANDLER",
            Tier::Data => "DATA",
        }
    }
}

pub fn tier_of(k: NodeKindId) -> Tier {
    match k {
        k if k == ROUTE
            || k == GRPC_SERVICE
            || k == QUEUE_CONSUMER
            || k == GRAPHQL_RESOLVER
            || k == WS_HANDLER
            || k == EVENT_HANDLER
            || k == CLI_COMMAND
            || k == CRON_JOB => Tier::Entry,
        k if k == MODULE || k == PACKAGE => Tier::Service,
        k if k == ENDPOINT
            || k == GRPC_CLIENT
            || k == QUEUE_PRODUCER
            || k == GRAPHQL_OPERATION
            || k == WS_CLIENT
            || k == EVENT_EMITTER
            || k == CLI_INVOCATION
            || k == DATABASE
            || k == CACHE
            || k == BLOB_STORE
            || k == SEARCH_INDEX
            || k == EMAIL_SERVICE
            || k == DATA_ENTITY
            || k == CONFIG_KEY
            || k == INFRA_RESOURCE
            || k == PACKAGE_DEP => Tier::Data,
        _ => Tier::Handler,
    }
}

pub fn glyph(k: NodeKindId) -> char {
    match k {
        k if k == MODULE => '◇',
        k if k == PACKAGE => '◈',
        k if k == FUNCTION || k == METHOD => 'ƒ',
        k if k == CLASS || k == STRUCT => '□',
        k if k == ROUTE
            || k == GRPC_SERVICE
            || k == QUEUE_CONSUMER
            || k == GRAPHQL_RESOLVER
            || k == WS_HANDLER
            || k == EVENT_HANDLER
            || k == CLI_COMMAND => '⟁',
        k if k == CRON_JOB => '⏲',
        k if k == INTERFACE => '◊',
        k if k == ENUM => '▣',
        k if k == ENDPOINT
            || k == GRPC_CLIENT
            || k == QUEUE_PRODUCER
            || k == GRAPHQL_OPERATION
            || k == WS_CLIENT
            || k == EVENT_EMITTER
            || k == CLI_INVOCATION => '↗',
        k if k == DATABASE => '⊟',
        k if k == CACHE => '⊠',
        k if k == BLOB_STORE => '⬢',
        k if k == SEARCH_INDEX => '⊙',
        k if k == EMAIL_SERVICE => '✉',
        k if k == COMPONENT => '⬡',
        k if k == HOOK => '⤴',
        k if k == SERVICE => '⚙',
        k if k == DIRECTIVE => '▾',
        k if k == PIPE => '▸',
        k if k == GUARD => '⛊',
        k if k == COMPOSABLE => '◉',
        k if k == ATTRIBUTE => '⌗',
        k if k == DATA_ENTITY => '⊞',
        k if k == CONFIG_KEY => '⚿',
        k if k == INFRA_RESOURCE => '☁',
        k if k == PACKAGE_DEP => '⊕',
        _ => '●',
    }
}

pub fn kind_label(k: NodeKindId) -> &'static str {
    match k {
        k if k == MODULE => "module",
        k if k == CLASS => "class",
        k if k == FUNCTION => "function",
        k if k == METHOD => "method",
        k if k == ROUTE => "route",
        k if k == PACKAGE => "package",
        k if k == INTERFACE => "interface",
        k if k == STRUCT => "struct",
        k if k == ENDPOINT => "endpoint",
        k if k == ENUM => "enum",
        k if k == GRPC_SERVICE => "grpc_service",
        k if k == GRPC_CLIENT => "grpc_client",
        k if k == QUEUE_CONSUMER => "queue_consumer",
        k if k == QUEUE_PRODUCER => "queue_producer",
        k if k == GRAPHQL_RESOLVER => "graphql_resolver",
        k if k == GRAPHQL_OPERATION => "graphql_operation",
        k if k == WS_HANDLER => "ws_handler",
        k if k == WS_CLIENT => "ws_client",
        k if k == EVENT_HANDLER => "event_handler",
        k if k == EVENT_EMITTER => "event_emitter",
        k if k == CLI_COMMAND => "cli_command",
        k if k == CLI_INVOCATION => "cli_invocation",
        k if k == DATABASE => "database",
        k if k == CACHE => "cache",
        k if k == BLOB_STORE => "blob_store",
        k if k == SEARCH_INDEX => "search_index",
        k if k == EMAIL_SERVICE => "email_service",
        k if k == COMPONENT => "component",
        k if k == HOOK => "hook",
        k if k == SERVICE => "service",
        k if k == DIRECTIVE => "directive",
        k if k == PIPE => "pipe",
        k if k == GUARD => "guard",
        k if k == COMPOSABLE => "composable",
        k if k == ATTRIBUTE => "attribute",
        k if k == DATA_ENTITY => "data_entity",
        k if k == CRON_JOB => "cron_job",
        k if k == CONFIG_KEY => "config_key",
        k if k == INFRA_RESOURCE => "infra_resource",
        k if k == PACKAGE_DEP => "package_dep",
        _ => "?",
    }
}

/// Map a normalised PPR score (0.0..=1.0) to a "heat" character.
/// Position over hue (per the cambridge-intelligence rule) — denser glyphs
/// stand in for higher activation.
pub fn heat_glyph(score: f64) -> char {
    if score <= 0.0 {
        ' '
    } else if score < 0.05 {
        '·'
    } else if score < 0.15 {
        '∘'
    } else if score < 0.35 {
        '◌'
    } else if score < 0.6 {
        '◍'
    } else {
        '●'
    }
}
