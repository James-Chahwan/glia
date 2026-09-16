use std::sync::OnceLock;

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

use crate::queue_topic::{self, TopicRule};

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
    // --- pre-allocated for the rest of batch A2; no table rows yet, so these
    // --- are inert until A2.4 / A2.6 / A2.9 add needles for them.
    /// AWS SQS (`sqs.sendMessage`, `SendMessageRequest`, ...).
    Sqs,
    /// AWS SNS fan-out (`sns.publish`, `PublishRequest`, ...).
    Sns,
    /// Google Cloud Pub/Sub (`topic.publish`, `subscription.on`, ...).
    PubSub,
    /// Azure Service Bus (`ServiceBusSender`, `ServiceBusProcessor`, ...).
    AzureServiceBus,
    /// MQTT brokers (`client.publish`, `client.subscribe` over mqtt.js/paho).
    Mqtt,
    /// Redis pub/sub proper (PUBLISH/SUBSCRIBE), distinct from `RedisList`.
    RedisPubSub,
    /// JMS / ActiveMQ / Artemis (`jmsTemplate.convertAndSend`, `@JmsListener`).
    Jms,
}

impl QueueFramework {
    /// Broker family, for grouping frameworks that share a wire identity (an
    /// SQS queue URL and an SQS ARN name the same queue). Stable lowercase
    /// tags — also what [`framework_tag`] falls back to.
    pub fn family(&self) -> &'static str {
        match self {
            QueueFramework::Celery => "celery",
            QueueFramework::Dramatiq => "dramatiq",
            QueueFramework::BullMQ => "bullmq",
            QueueFramework::Sidekiq => "sidekiq",
            QueueFramework::Oban => "oban",
            QueueFramework::Nats => "nats",
            QueueFramework::RabbitMQ => "rabbitmq",
            QueueFramework::Kafka => "kafka",
            QueueFramework::RedisList | QueueFramework::RedisPubSub => "redis",
            QueueFramework::Sqs => "sqs",
            QueueFramework::Sns => "sns",
            QueueFramework::PubSub => "pubsub",
            QueueFramework::AzureServiceBus => "azureservicebus",
            QueueFramework::Mqtt => "mqtt",
            QueueFramework::Jms => "jms",
        }
    }
}

pub fn extract_queue_consumers(source: &str, from: NodeId) -> Vec<QueueConsumer> {
    let mut consumers = Vec::new();
    for (pattern, framework, signals, _rule) in CONSUMER_PATTERNS {
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

/// (needle, framework, framework-presence signals, topic rule). The signals list gates
/// emission: if NONE of the substrings appears in the same source file, the
/// pattern is skipped — this stops e.g. Express `res.send('Hello')` from
/// being mis-classified as a Dramatiq producer. An empty signals list means
/// the needle is unique enough to stand alone (Sidekiq's `perform_async`,
/// `Oban.insert`, etc.).
///
/// The 4th element says HOW that needle spells its topic — see
/// [`crate::queue_topic::TopicRule`]. `ArgLiteral` is the historical behaviour
/// (first quoted literal in positional arg #0); `KeyedOrArg` additionally reads
/// the kafkajs object form `send({ topic: 'x' })` and the Go struct field
/// `kafka.Message{Topic: "x"}`, which the old scanner could not see at all.
const CONSUMER_PATTERNS: &[(&str, QueueFramework, &[&str], TopicRule)] = &[
    ("@celery.task", QueueFramework::Celery, &[], TopicRule::ArgLiteral),
    ("@shared_task", QueueFramework::Celery, &[], TopicRule::ArgLiteral),
    ("@dramatiq.actor", QueueFramework::Dramatiq, &[], TopicRule::ArgLiteral),
    // `new Worker(` is a common JS shape (web workers, BullMQ, etc.); require
    // BullMQ presence.
    ("new Worker(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    ("BullModule", QueueFramework::BullMQ, &[], TopicRule::ArgLiteral),
    ("include Sidekiq::Worker", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("include Sidekiq::Job", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("use Oban.Worker", QueueFramework::Oban, &[], TopicRule::ArgLiteral),
    ("use Oban.Pro.Worker", QueueFramework::Oban, &[], TopicRule::ArgLiteral),
    // `nc.subscribe` collides with Backbone events / Redis pubsub vars; gate.
    ("nc.subscribe", QueueFramework::Nats, &["nats", "NATS"], TopicRule::ArgLiteral),
    // Go's nats.go exports Capitalized APIs (`nc.Subscribe`, `nc.QueueSubscribe`);
    // the lowercase JS needles never match Go source, so Go queues went blind.
    ("nc.Subscribe", QueueFramework::Nats, &["nats", "NATS"], TopicRule::ArgLiteral),
    ("nc.QueueSubscribe", QueueFramework::Nats, &["nats", "NATS"], TopicRule::ArgLiteral),
    ("channel.consume", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"], TopicRule::ArgLiteral),
    ("KafkaConsumer", QueueFramework::Kafka, &[], TopicRule::ArgLiteral),
    // `consumer.subscribe` is generic; require kafka library presence.
    ("consumer.subscribe", QueueFramework::Kafka, &["kafka", "kafkajs", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    // Go Kafka consumers: segmentio `reader.ReadMessage`, confluent `consumer.ReadMessage`.
    ("reader.ReadMessage", QueueFramework::Kafka, &["kafka", "segmentio"], TopicRule::ArgLiteral),
    ("consumer.ReadMessage", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::ArgLiteral),
    // Redis-as-queue consumer side. BLPOP/BRPOP block until message; LPOP/RPOP
    // are non-blocking pops. .NET driver uses ListLeftPop/ListRightPop.
    (".blpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"], TopicRule::ArgLiteral),
    (".brpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"], TopicRule::ArgLiteral),
    (".lpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"], TopicRule::ArgLiteral),
    (".rpop(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"], TopicRule::ArgLiteral),
    ("ListLeftPop(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
    ("ListLeftPopAsync(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
    ("ListRightPop(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
    ("ListRightPopAsync(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
];

const PRODUCER_PATTERNS: &[(&str, QueueFramework, &[&str], TopicRule)] = &[
    // `.delay(` collides with `setTimeout.delay`, jQuery `.delay`, Carrierwave,
    // and many JS animation libs; require Celery presence.
    (".delay(", QueueFramework::Celery, &["celery", "@celery", "@shared_task"], TopicRule::ArgLiteral),
    (".apply_async(", QueueFramework::Celery, &[], TopicRule::ArgLiteral),
    // `.send(` is wildly overloaded (`res.send`, `socket.send`, ...). Require
    // Dramatiq import — `import dramatiq` or `@dramatiq.actor`.
    (".send(", QueueFramework::Dramatiq, &["dramatiq"], TopicRule::ArgLiteral),
    // `queue.add(` — generic var name; require BullMQ context.
    ("queue.add(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    // `new Queue(` — also generic; require BullMQ.
    ("new Queue(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    ("perform_async", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("perform_in", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("Oban.insert", QueueFramework::Oban, &[], TopicRule::ArgLiteral),
    ("nc.publish", QueueFramework::Nats, &["nats", "NATS"], TopicRule::ArgLiteral),
    // Go's nats.go exports Capitalized `nc.Publish`; the lowercase JS needle
    // never matches Go source, so Go NATS producers went blind.
    ("nc.Publish", QueueFramework::Nats, &["nats", "NATS"], TopicRule::ArgLiteral),
    ("channel.publish", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"], TopicRule::ArgLiteral),
    ("channel.basic_publish", QueueFramework::RabbitMQ, &[], TopicRule::ArgLiteral),
    ("producer.send", QueueFramework::Kafka, &["kafka", "kafkajs", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    ("producer.produce", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::ArgLiteral),
    // Go Kafka producers: confluent `producer.Produce`, segmentio `writer.WriteMessages`.
    ("producer.Produce", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    ("writer.WriteMessages", QueueFramework::Kafka, &["kafka", "segmentio"], TopicRule::KeyedOrArg(&["topic"])),
    // Redis-as-queue producer side. .lpush / .rpush both push items onto a
    // list; consumers BLPOP/BRPOP off the other end.
    (".lpush(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"], TopicRule::ArgLiteral),
    (".rpush(", QueueFramework::RedisList, &["redis", "Redis", "ioredis"], TopicRule::ArgLiteral),
    ("ListLeftPush(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
    ("ListLeftPushAsync(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
    ("ListRightPush(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
    ("ListRightPushAsync(", QueueFramework::RedisList, &["StackExchange.Redis"], TopicRule::ArgLiteral),
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

/// Queue-consumer nodes for one file — one node per DISTINCT (topic, framework).
///
/// `path` is the file the source came from; it is only used by the
/// `GLIA_QUEUE_DEBUG` marker today (A2.8 attaches it to the node).
pub fn extract_queue_consumer_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> QueueNodes {
    emit_queue_nodes(
        source,
        path,
        module_id,
        repo,
        CONSUMER_PATTERNS,
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:",
    )
}

/// Queue-producer nodes for one file — one node per DISTINCT (topic, framework).
pub fn extract_queue_producer_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> QueueNodes {
    emit_queue_nodes(
        source,
        path,
        module_id,
        repo,
        PRODUCER_PATTERNS,
        node_kind::QUEUE_PRODUCER,
        "queue_producer:",
    )
}

/// Shared emit loop for both sides.
///
/// Was: one topic per needle, read from the FIRST occurrence of that needle in
/// the file. Now: every occurrence is scanned and each distinct topic gets its
/// own node, so a file that publishes to `orders` and `payments` stops hiding
/// one of them. The framework-tag fallback is emitted only when NO occurrence
/// of that needle named a topic.
fn emit_queue_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    patterns: &[(&str, QueueFramework, &[&str], TopicRule)],
    kind: repo_graph_core::NodeKindId,
    prefix: &str,
) -> QueueNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    for (pattern, framework, signals, rule) in patterns {
        if !source.contains(pattern) || !signals_present(source, signals) {
            continue;
        }
        let hits = queue_topic::scan(source, pattern, *rule);
        let topics: Vec<String> = hits.into_iter().filter_map(|h| h.topic).collect();
        if debug_enabled() && !topics.is_empty() {
            eprintln!(
                "[queues] scan needle='{pattern}' rule={rule:?} hits={} path={path} topics={}",
                topics.len(),
                topics.join(",")
            );
        }
        for topic in &topics {
            push_node(
                &mut nodes, &mut nav, &mut seen, topic, framework, module_id, repo, kind, prefix,
            );
        }
        if topics.is_empty() {
            push_node(
                &mut nodes,
                &mut nav,
                &mut seen,
                &framework_tag(framework),
                framework,
                module_id,
                repo,
                kind,
                prefix,
            );
        }
    }

    QueueNodes { nodes, nav }
}

#[allow(clippy::too_many_arguments)]
fn push_node(
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
    seen: &mut std::collections::HashSet<String>,
    topic: &str,
    framework: &QueueFramework,
    module_id: NodeId,
    repo: RepoId,
    kind: repo_graph_core::NodeKindId,
    prefix: &str,
) -> bool {
    if !seen.insert(format!("{topic}:{framework:?}")) {
        return false;
    }
    let qname = format!("{prefix}{topic}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
    nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![],
    });
    nav.record(id, topic, &qname, kind, Some(module_id));
    true
}

/// Identity-free fallback when no occurrence of a needle named a topic.
/// Unchanged spelling (`Debug`-lowercased) so existing tag qnames keep working;
/// [`QueueFramework::family`] is deliberately NOT used here — it folds
/// `RedisList`/`RedisPubSub` together and would silently rename a live qname.
fn framework_tag(f: &QueueFramework) -> String {
    format!("{f:?}").to_lowercase()
}

/// `GLIA_QUEUE_DEBUG=1` turns on the `[queues] scan needle=` marker, read once.
fn debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_QUEUE_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
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

    const PATH: &str = "src/test.rs";

    fn qnames(r: &QueueNodes) -> Vec<String> {
        let mut v: Vec<String> = r.nav.qname_by_id.values().cloned().collect();
        v.sort();
        v
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
        let result = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        let qname = result.nav.qname_by_id.values().next().unwrap();
        assert_eq!(qname, "queue_consumer:emails");
    }

    #[test]
    fn producer_nodes() {
        let source = "from celery import Celery\nsend_email.delay('hello')";
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
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
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
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
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert!(!result.nodes.is_empty(), "dramatiq import unlocks .send pattern");
    }

    #[test]
    fn redis_list_as_queue_pair() {
        // Voting-app shape: Python pushes on `votes`, .NET pops from `votes`.
        // Same topic across services → SHARES_QUEUE on cross-graph join.
        let producer = "import redis\nr = redis.Redis()\nr.rpush('votes', vote)";
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(pr.nodes.len(), 1);
        let pq = pr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(pq, "queue_producer:votes");

        let consumer = r#"using StackExchange.Redis;
var entry = db.ListLeftPop("votes");"#;
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(cr.nodes.len(), 1);
        let cq = cr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(cq, "queue_consumer:votes");
    }

    #[test]
    fn jquery_delay_does_not_match_celery_producer() {
        let source = "$('#el').fadeIn().delay(500).fadeOut();";
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "jQuery .delay() must not emit a Celery producer (no celery import)"
        );
    }

    #[test]
    fn go_nats_capitalized_producer_and_consumer_with_topic() {
        // Go's nats.go exports Capitalized `nc.Publish` / `nc.Subscribe`, and the
        // topic is the first call argument after `(` — earlier lowercase-only
        // needles + a literal scanner that stopped at `(` left Go queues blind.
        let producer = r#"
import "github.com/nats-io/nats.go"

func PublishOrder(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders", payload)
}
"#;
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(pr.nodes.len(), 1);
        assert_eq!(pr.nav.kind_by_id[&pr.nodes[0].id], node_kind::QUEUE_PRODUCER);
        let pq = pr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(pq, "queue_producer:orders");

        let consumer = r#"
import "github.com/nats-io/nats.go"

func SubscribeOrders(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("orders", handle)
}
"#;
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(cr.nodes.len(), 1);
        assert_eq!(cr.nav.kind_by_id[&cr.nodes[0].id], node_kind::QUEUE_CONSUMER);
        let cq = cr.nav.qname_by_id.values().next().unwrap();
        // Topic must be the subject "orders", not the framework tag "nats".
        assert_eq!(cq, "queue_consumer:orders");
    }

    #[test]
    fn kafka_producer_and_consumer() {
        // BREAKING (A2.1): the producer half used to collapse to
        // `queue_producer:kafka` because the object form was unreadable; it now
        // carries the real topic. The consumer half still reads as the tag: the
        // only needle that fires is `KafkaConsumer`, whose occurrences (an
        // import and a bare `new KafkaConsumer()`) name no topic — the topic
        // lives on `c.subscribe(...)`, and `c` is not the `consumer.` receiver
        // the table matches. A2.x can widen that needle; this asserts today.
        let consumer = "import { KafkaConsumer } from 'kafkajs';\nconst c = new KafkaConsumer();\nc.subscribe('user-events')";
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:kafka".to_string()]);

        let producer = "import { Kafka } from 'kafkajs';\nproducer.send({ topic: 'user-events' })";
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:user-events".to_string()]);
    }

    // ---- A2.1: every occurrence, real argument forms ----------------------

    #[test]
    fn multi_topic_one_file() {
        // THE packet's reason to exist: `source.find(needle)` saw only the
        // FIRST send, so `payments` was invisible to the graph.
        let source = r#"
import { Kafka } from 'kafkajs';
const producer = kafka.producer();

export async function publishOrder(o) {
  await producer.send({ topic: 'orders', messages: [{ value: o }] });
}
export async function publishPayment(p) {
  await producer.send({ topic: 'payments', messages: [{ value: p }] });
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(pr.nodes.len(), 2);
        assert_eq!(
            qnames(&pr),
            vec![
                "queue_producer:orders".to_string(),
                "queue_producer:payments".to_string()
            ]
        );
    }

    #[test]
    fn kafkajs_object_form() {
        let source =
            "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'orders', fromBeginning: true });";
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn java_arrays_aslist() {
        let source = r#"
import org.apache.kafka.clients.consumer.KafkaConsumer;
consumer.subscribe(Arrays.asList("orders"));
"#;
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert!(
            qnames(&cr).contains(&"queue_consumer:orders".to_string()),
            "Arrays.asList(\"orders\") must resolve to the topic, got {:?}",
            qnames(&cr)
        );
    }

    #[test]
    fn go_struct_topic_field() {
        let source = r#"
import "github.com/segmentio/kafka-go"

func Publish(w *kafka.Writer, v []byte) error {
	return writer.WriteMessages(ctx, kafka.Message{Topic: "orders", Value: v})
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn backtick_literal() {
        let source = "import { Queue } from 'bullmq';\nqueue.add(`emails`, job);";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:emails".to_string()]);
    }

    #[test]
    fn payload_literal_in_arg_one_is_not_a_topic() {
        // PRECISION REGRESSION GUARD: fails the moment the region scan is
        // widened past positional arg #0.
        let source = "import { Kafka } from 'kafkajs';\nproducer.send(topicVar, \"payload\");";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(
            qnames(&pr),
            vec!["queue_producer:kafka".to_string()],
            "an unreadable topic must fall back to the framework tag, never to the payload"
        );
    }

    #[test]
    fn tag_fallback_is_emitted_once_per_framework() {
        // Three `nc.Publish` calls, none with a literal subject: exactly one
        // tag node, not three.
        let source = r#"
import "github.com/nats-io/nats.go"
nc.Publish(subjectA, a)
nc.Publish(subjectB, b)
nc.Publish(subjectC, c)
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:nats".to_string()]);
    }
}
