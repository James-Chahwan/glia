use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

pub struct QueueConsumer {
    pub from: NodeId,
    pub framework: QueueFramework,
    pub identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueFramework {
    Celery,
    Dramatiq,
    BullMQ,
    Sidekiq,
    Oban,
    Nats,
    RabbitMQ,
    Kafka,
    /// Raw Redis lists used as a queue (LPUSH/RPUSH ↔ BLPOP/BRPOP). Common in
    /// polyglot demos like dockersamples/example-voting-app and any "ad-hoc
    /// worker queue without a framework" pattern.
    RedisList,
}

pub fn extract_queue_consumers(source: &str, from: NodeId) -> Vec<QueueConsumer> {
    let mut consumers = Vec::new();
    for (pattern, framework, signals) in CONSUMER_PATTERNS {
        if source.contains(pattern) && signals_present(source, signals) {
            consumers.push(QueueConsumer {
                from,
                framework: framework.clone(),
                identifier: pattern.to_string(),
            });
        }
    }
    consumers
}

/// (needle, framework, framework-presence signals). The signals list gates
/// emission: if NONE of the substrings appears in the same source file, the
/// pattern is skipped — this stops e.g. Express `res.send('Hello')` from
/// being mis-classified as a Dramatiq producer. An empty signals list means
/// the needle is unique enough to stand alone (Sidekiq's `perform_async`,
/// `Oban.insert`, etc.).
const CONSUMER_PATTERNS: &[(&str, QueueFramework, &[&str])] = &[
    ("@celery.task", QueueFramework::Celery, &[]),
    ("@shared_task", QueueFramework::Celery, &[]),
    ("@dramatiq.actor", QueueFramework::Dramatiq, &[]),
    // `new Worker(` is a common JS shape (web workers, BullMQ, etc.); require
    // BullMQ presence.
    ("new Worker(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"]),
    ("BullModule", QueueFramework::BullMQ, &[]),
    ("include Sidekiq::Worker", QueueFramework::Sidekiq, &[]),
    ("include Sidekiq::Job", QueueFramework::Sidekiq, &[]),
    ("use Oban.Worker", QueueFramework::Oban, &[]),
    ("use Oban.Pro.Worker", QueueFramework::Oban, &[]),
    // `nc.subscribe` collides with Backbone events / Redis pubsub vars; gate.
    ("nc.subscribe", QueueFramework::Nats, &["nats", "NATS"]),
    ("channel.consume", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"]),
    ("KafkaConsumer", QueueFramework::Kafka, &[]),
    // `consumer.subscribe` is generic; require kafka library presence.
    ("consumer.subscribe", QueueFramework::Kafka, &["kafka", "kafkajs", "confluent"]),
    // Redis-as-queue consumer side. BLPOP/BRPOP block until message; LPOP/RPOP
    // are non-blocking pops. .NET driver uses ListLeftPop/ListRightPop.
    (".blpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"]),
    (".brpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"]),
    (".lpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"]),
    (".rpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"]),
    ("ListLeftPop(", QueueFramework::RedisList, &["StackExchange.Redis"]),
    ("ListLeftPopAsync(", QueueFramework::RedisList, &["StackExchange.Redis"]),
    ("ListRightPop(", QueueFramework::RedisList, &["StackExchange.Redis"]),
    ("ListRightPopAsync(", QueueFramework::RedisList, &["StackExchange.Redis"]),
];

const PRODUCER_PATTERNS: &[(&str, QueueFramework, &[&str])] = &[
    // `.delay(` collides with `setTimeout.delay`, jQuery `.delay`, Carrierwave,
    // and many JS animation libs; require Celery presence.
    (".delay(", QueueFramework::Celery, &["celery", "@celery", "@shared_task"]),
    (".apply_async(", QueueFramework::Celery, &[]),
    // `.send(` is wildly overloaded (`res.send`, `socket.send`, ...). Require
    // Dramatiq import — `import dramatiq` or `@dramatiq.actor`.
    (".send(", QueueFramework::Dramatiq, &["dramatiq"]),
    // `queue.add(` — generic var name; require BullMQ context.
    ("queue.add(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"]),
    // `new Queue(` — also generic; require BullMQ.
    ("new Queue(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"]),
    ("perform_async", QueueFramework::Sidekiq, &[]),
    ("perform_in", QueueFramework::Sidekiq, &[]),
    ("Oban.insert", QueueFramework::Oban, &[]),
    ("nc.publish", QueueFramework::Nats, &["nats", "NATS"]),
    ("channel.publish", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"]),
    ("channel.basic_publish", QueueFramework::RabbitMQ, &[]),
    ("producer.send", QueueFramework::Kafka, &["kafka", "kafkajs", "confluent"]),
    ("producer.produce", QueueFramework::Kafka, &["kafka", "confluent"]),
    // Redis-as-queue producer side. .lpush / .rpush both push items onto a
    // list; consumers BLPOP/BRPOP off the other end.
    (".lpush(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"]),
    (".rpush(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"]),
    ("ListLeftPush(", QueueFramework::RedisList, &["StackExchange.Redis"]),
    ("ListLeftPushAsync(", QueueFramework::RedisList, &["StackExchange.Redis"]),
    ("ListRightPush(", QueueFramework::RedisList, &["StackExchange.Redis"]),
    ("ListRightPushAsync(", QueueFramework::RedisList, &["StackExchange.Redis"]),
];

/// True when `signals` is empty (always pass) or any signal substring appears
/// in `source`. Lets distinct framework names gate their broad-needle patterns.
fn signals_present(source: &str, signals: &[&str]) -> bool {
    signals.is_empty() || signals.iter().any(|s| source.contains(s))
}

pub struct QueueNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

pub fn extract_queue_consumer_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> QueueNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    for (pattern, framework, signals) in CONSUMER_PATTERNS {
        if !source.contains(pattern) || !signals_present(source, signals) {
            continue;
        }
        let topic = extract_topic_near(source, pattern).unwrap_or_else(|| framework_tag(framework));
        let key = format!("{topic}:{framework:?}");
        if seen.insert(key) {
            let qname = format!("queue_consumer:{topic}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::QUEUE_CONSUMER, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![],
            });
            nav.record(id, &topic, &qname, node_kind::QUEUE_CONSUMER, Some(module_id));
        }
    }

    QueueNodes { nodes, nav }
}

pub fn extract_queue_producer_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> QueueNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    for (pattern, framework, signals) in PRODUCER_PATTERNS {
        if !source.contains(pattern) || !signals_present(source, signals) {
            continue;
        }
        let topic = extract_topic_near(source, pattern).unwrap_or_else(|| framework_tag(framework));
        let key = format!("{topic}:{framework:?}");
        if seen.insert(key) {
            let qname = format!("queue_producer:{topic}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::QUEUE_PRODUCER, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![],
            });
            nav.record(id, &topic, &qname, node_kind::QUEUE_PRODUCER, Some(module_id));
        }
    }

    QueueNodes { nodes, nav }
}

fn framework_tag(f: &QueueFramework) -> String {
    format!("{f:?}").to_lowercase()
}

fn extract_topic_near(source: &str, pattern: &str) -> Option<String> {
    let idx = source.find(pattern)?;
    let after = &source[idx + pattern.len()..];
    extract_first_string_literal(after)
}

fn extract_first_string_literal(s: &str) -> Option<String> {
    let trimmed = s.trim_start();
    let (quote, rest) = if let Some(rest) = trimmed.strip_prefix('\'') {
        ('\'', rest)
    } else if let Some(rest) = trimmed.strip_prefix('"') {
        ('"', rest)
    } else {
        return None;
    };
    let end = rest.find(quote)?;
    let lit = &rest[..end];
    if lit.is_empty() || lit.len() > 128 {
        return None;
    }
    Some(lit.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn detects_celery() {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test");
        let refs = extract_queue_consumers("@celery.task\ndef process():", id);
        assert!(refs.iter().any(|r| r.framework == QueueFramework::Celery));
    }

    #[test]
    fn detects_bullmq() {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test");
        let refs = extract_queue_consumers(
            "import { Worker } from 'bullmq';\nconst worker = new Worker('queue', handler);",
            id,
        );
        assert!(refs.iter().any(|r| r.framework == QueueFramework::BullMQ));
    }

    #[test]
    fn detects_sidekiq() {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test");
        let refs = extract_queue_consumers("include Sidekiq::Worker", id);
        assert!(refs.iter().any(|r| r.framework == QueueFramework::Sidekiq));
    }

    #[test]
    fn consumer_nodes_with_topic() {
        let source = "import { Worker } from 'bullmq';\nconst worker = new Worker('emails', handler);";
        let result = extract_queue_consumer_nodes(source, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        let qname = result.nav.qname_by_id.values().next().unwrap();
        assert_eq!(qname, "queue_consumer:emails");
    }

    #[test]
    fn producer_nodes() {
        let source = "from celery import Celery\nsend_email.delay('hello')";
        let result = extract_queue_producer_nodes(source, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nav.kind_by_id[&result.nodes[0].id], node_kind::QUEUE_PRODUCER);
    }

    #[test]
    fn express_res_send_does_not_match_dramatiq_producer() {
        // Express response handler — `.send(` is not a Dramatiq producer
        // because the file has no `dramatiq` import. Was the dominant false
        // positive in the 2026-05-05 substrate eval (16/18 producers were
        // HTTP response strings).
        let source = r#"
const express = require('express');
const app = express();
app.get('/', (req, res) => {
    res.send('Hello World');
});
app.get('/users', (req, res) => {
    res.send('<p>Users online: 42</p>');
});
"#;
        let result = extract_queue_producer_nodes(source, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "express res.send must not emit a Dramatiq producer (no dramatiq import)"
        );
    }

    #[test]
    fn dramatiq_send_emits_when_import_present() {
        // Same `.send(` pattern, but file imports dramatiq → emit.
        let source = r#"
import dramatiq

@dramatiq.actor
def greet(name):
    pass

greet.send('alice')
"#;
        let result = extract_queue_producer_nodes(source, module_id(), repo());
        assert!(!result.nodes.is_empty(), "dramatiq import unlocks .send pattern");
    }

    #[test]
    fn redis_list_as_queue_pair() {
        // Voting-app shape: Python pushes on `votes`, .NET pops from `votes`.
        // Same topic across services → SHARES_QUEUE on cross-graph join.
        let producer = "import redis\nr = redis.Redis()\nr.rpush('votes', vote)";
        let pr = extract_queue_producer_nodes(producer, module_id(), repo());
        assert_eq!(pr.nodes.len(), 1);
        let pq = pr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(pq, "queue_producer:votes");

        let consumer = r#"using StackExchange.Redis;
var entry = db.ListLeftPop("votes");"#;
        let cr = extract_queue_consumer_nodes(consumer, module_id(), repo());
        assert_eq!(cr.nodes.len(), 1);
        let cq = cr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(cq, "queue_consumer:votes");
    }

    #[test]
    fn jquery_delay_does_not_match_celery_producer() {
        let source = "$('#el').fadeIn().delay(500).fadeOut();";
        let result = extract_queue_producer_nodes(source, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "jQuery .delay() must not emit a Celery producer (no celery import)"
        );
    }

    #[test]
    fn kafka_producer_and_consumer() {
        let consumer = "import { KafkaConsumer } from 'kafkajs';\nconst c = new KafkaConsumer();\nc.subscribe('user-events')";
        let cr = extract_queue_consumer_nodes(consumer, module_id(), repo());
        assert!(!cr.nodes.is_empty());

        let producer = "import { Kafka } from 'kafkajs';\nproducer.send({ topic: 'user-events' })";
        let pr = extract_queue_producer_nodes(producer, module_id(), repo());
        assert!(!pr.nodes.is_empty());
    }
}
