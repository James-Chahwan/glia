use std::collections::{BTreeSet, HashSet};
use std::sync::OnceLock;

use glia_code_domain::{
    CodeNav, FileParse, GRAPH_TYPE, cell_type, edge_category, evidence, node_kind,
};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, NodeKindId, RepoId};

use crate::anchor::{self, Anchor};
use crate::code_guard::LazyGuard;
use crate::marker_swap;
use crate::queue_topic::{self, TopicForm, TopicRule};

pub struct QueueConsumer {
    pub from: NodeId,
    pub framework: QueueFramework,
    pub identifier: String,
}

/// Qname marker for the identity-free framework-tag fallback:
/// `queue_producer:unresolved:kafka`, never `queue_producer:kafka`.
///
/// A node carrying this prefix is a COVERAGE SIGNAL — "this file talks to
/// Kafka, topic unknown" — and NEVER an identity. That distinction is the whole
/// point: two unrelated services whose topics both failed to parse used to fall
/// back to the same bare `kafka` tag, and `QueueStackResolver` joined tag to tag
/// exactly as if it were a real topic, so every such repo got a QUEUE_FLOWS edge
/// to every other such repo. `blast_radius` traverses those edges and
/// `cross_stack_trace` labels them as a real mechanism, so the false dependency
/// propagated into answers.
///
/// `graph` references THIS constant rather than a second copy of the literal —
/// the extractor and the resolver must never be able to disagree about the
/// spelling, because the failure mode of a disagreement is silent (the resolver
/// simply resumes pairing tags).
pub const UNRESOLVED_PREFIX: &str = "unresolved:";

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
    // --- pre-allocated for the rest of batch A2. Sqs/Sns/PubSub/
    // --- AzureServiceBus have rows since A2.6, Jms a consumer row since A2.4
    // --- and a producer row since A2.9, Mqtt/RedisPubSub rows since A2.9.
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
    // One allocation per call, reused by every gate below — see
    // [`signals_present`] for why the gate reads a lowercased copy.
    let lower = source.to_ascii_lowercase();
    for (pattern, framework, signals, _rule) in CONSUMER_PATTERNS {
        if source.contains(pattern) && signals_present(&lower, signals) {
            consumers.push(QueueConsumer {
                from,
                framework: framework.clone(),
                identifier: pattern.to_string(),
            });
        }
    }
    consumers
}

// ---- A2.5: TASK-QUEUE IDENTITY IS NOT A BROKER TOPIC ----------------------
// For Celery / Dramatiq / Sidekiq / Oban the join key is the TASK, and the
// first string argument is a PAYLOAD. `send_email.delay("welcome@example.com")`
// minted `queue_producer:welcome@example.com` — a topic named after user data,
// which then pairs with nothing — while the worker side (`@shared_task`,
// `include Sidekiq::Worker`) found no literal at all and collapsed to the
// framework tag, so the producer and the consumer of the SAME task never met
// while every celery repo paired with every other celery repo.
//
// The rows below therefore read an IDENTITY rather than a literal: the receiver
// before the call (`TopicRule::Receiver`), the definition under the decorator
// (`TopicRule::DeclaredSymbol`), or the enclosing class
// (`TopicRule::EnclosingSymbol`). This is the ONE place where the qname is
// deliberately NOT a broker topic, and it is why `queue_consumer:HardWorker`
// and `queue_consumer:critical` can both be right for one class: the first is
// the job, the second is the broker queue `sidekiq_options` names.
// ---------------------------------------------------------------------------
/// (needle, framework, framework-presence signals, topic rule). The signals list gates
/// emission: if NONE of the substrings appears in the same source file
/// (case-INSENSITIVELY — every signal literal here MUST be lowercase, see
/// [`signals_present`]), the pattern is skipped — this stops e.g. Express `res.send('Hello')` from
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
    // A2.5: the task is the DEF under the decorator, never an argument of it.
    ("@celery.task", QueueFramework::Celery, &[], TopicRule::DeclaredSymbol),
    ("@shared_task", QueueFramework::Celery, &[], TopicRule::DeclaredSymbol),
    ("@dramatiq.actor", QueueFramework::Dramatiq, &[], TopicRule::DeclaredSymbol),
    // `new Worker(` is a common JS shape (web workers, BullMQ, etc.); require
    // BullMQ presence.
    ("new Worker(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    ("BullModule", QueueFramework::BullMQ, &[], TopicRule::ArgLiteral),
    // A2.5: a mixin names no task — the ENCLOSING class/module is the job.
    ("include Sidekiq::Worker", QueueFramework::Sidekiq, &[], TopicRule::EnclosingSymbol),
    ("include Sidekiq::Job", QueueFramework::Sidekiq, &[], TopicRule::EnclosingSymbol),
    ("use Oban.Worker", QueueFramework::Oban, &[], TopicRule::EnclosingSymbol),
    ("use Oban.Pro.Worker", QueueFramework::Oban, &[], TopicRule::EnclosingSymbol),
    // A2.5: an explicitly named Sidekiq queue, usually written WITHOUT
    // parentheses (`sidekiq_options queue: 'critical'`) — the keyed rule falls
    // back to the rest of the line for exactly this shape. One class can
    // legitimately carry this node AND its EnclosingSymbol one: the first is
    // the broker queue, the second is the job.
    ("sidekiq_options", QueueFramework::Sidekiq, &[], TopicRule::Keyed(&["queue"])),
    // `nc.subscribe` collides with Backbone events / Redis pubsub vars; gate.
    ("nc.subscribe", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    // Go's nats.go exports Capitalized APIs (`nc.Subscribe`, `nc.QueueSubscribe`);
    // the lowercase JS needles never match Go source, so Go queues went blind.
    ("nc.Subscribe", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    ("nc.QueueSubscribe", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    ("channel.consume", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"], TopicRule::ArgLiteral),
    ("KafkaConsumer", QueueFramework::Kafka, &[], TopicRule::ArgLiteral),
    // `consumer.subscribe` is generic; require kafka library presence.
    ("consumer.subscribe", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    // Go Kafka consumers: segmentio `reader.ReadMessage`, confluent `consumer.ReadMessage`.
    ("reader.ReadMessage", QueueFramework::Kafka, &["kafka", "segmentio"], TopicRule::ArgLiteral),
    ("consumer.ReadMessage", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::ArgLiteral),
    // Redis-as-queue consumer side. BLPOP/BRPOP block until message; LPOP/RPOP
    // are non-blocking pops. .NET driver uses ListLeftPop/ListRightPop.
    (".blpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".brpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".lpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".rpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    ("ListLeftPop(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListLeftPopAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPop(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPopAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    // ---- A2.2: receiver-agnostic needles ---------------------------------
    // A leading-dot needle matches ANY receiver name, so C#'s `consumer.`,
    // Go's `r.` and Rust's `consumer.` all hit the same row. The LIBRARY gate
    // (never the bare framework word) is what keeps Rx/NATS/RxJS out.
    // C# — Confluent.Kafka: `consumer.Subscribe("orders")`.
    (".Subscribe(", QueueFramework::Kafka, &["confluent.kafka"], TopicRule::ArgLiteral),
    // Java/Kotlin — Spring: `@KafkaListener(topics = "orders")`.
    // A2.4: `topicPattern = "orders.*"` is read too, and QueueStackResolver
    // matches it as a Kafka-dialect wildcard (always Weak). `topic` stays for
    // `topicPartitions = @TopicPartition(topic = "orders")`; word edges keep it
    // from matching inside `topicPattern`.
    ("@KafkaListener", QueueFramework::Kafka, &["springframework.kafka"], TopicRule::Keyed(&["topics", "topic", "topicpattern"])),
    // Go — segmentio/kafka-go: the topic is a ReaderConfig struct field.
    ("kafka.NewReader(", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    ("kafka.ReaderConfig{", QueueFramework::Kafka, &["kafka-go"], TopicRule::Keyed(&["topic"])),
    // Go — `r.ReadMessage(ctx)` proves the consumer is live but names no topic.
    // NoIdentity is the one rule that emits NOTHING rather than falling back to
    // a framework tag, so it cannot shadow the ReaderConfig row above.
    (".ReadMessage(", QueueFramework::Kafka, &["kafka"], TopicRule::NoIdentity),
    // Scala — Alpakka / akka-stream-kafka: `Subscriptions.topics("orders")`.
    ("Subscriptions.topics(", QueueFramework::Kafka, &["akka.kafka"], TopicRule::ArgLiteral),
    // Rust — rdkafka: `consumer.subscribe(&["orders"])`.
    (".subscribe(&[", QueueFramework::Kafka, &["rdkafka"], TopicRule::ArgLiteral),
    // Python — pika / amqp: `ch.basic_consume(queue="orders", ...)`,
    // receiver-free. CL.1: `queue_declare(` is a DECLARE_PATTERNS row now.
    ("basic_consume(", QueueFramework::RabbitMQ, &["pika", "amqp"], TopicRule::KeyedOrArg(&["queue"])),
    // C# — RabbitMQ.Client v6 `channel.BasicConsume(queue: "orders",
    // autoAck: true, consumer: c)` (named or positional) and v7
    // `BasicConsumeAsync`. CL.1: before this row the C# consumer was found
    // only because the tutorial shape also DECLARES the queue.
    (".BasicConsume(", QueueFramework::RabbitMQ, &["rabbitmq.client"], TopicRule::KeyedOrArg(&["queue"])),
    (".BasicConsumeAsync(", QueueFramework::RabbitMQ, &["rabbitmq.client"], TopicRule::KeyedOrArg(&["queue"])),
    // ---- A2.6: cloud brokers ---------------------------------------------
    // The identity arrives as a queue URL, an ARN or a GCP resource path, and
    // `queue_topic::fold_topic` reduces each to the bare name — so both sides
    // join on `orders` whichever spelling they used. `@SqsListener` and
    // `[ServiceBusTrigger]` are A2.4's rows.
    // Python — boto3: `sqs.receive_message(QueueUrl="https://sqs.../orders")`.
    (".receive_message(", QueueFramework::Sqs, &["boto3", "botocore"], TopicRule::Keyed(&["queueurl"])),
    // JS — AWS SDK v3: `new ReceiveMessageCommand({ QueueUrl: "..." })`.
    ("ReceiveMessageCommand(", QueueFramework::Sqs, &["@aws-sdk/client-sqs"], TopicRule::Keyed(&["queueurl"])),
    // JS — SDK v2 / v3 aggregated client `sqs.receiveMessage({ QueueUrl })`;
    // Java — SDK v2 builder `.queueUrl("...")`, v1 `com.amazonaws`.
    (".receiveMessage(", QueueFramework::Sqs, &["aws-sdk", "awssdk.services.sqs", "amazonaws.services.sqs"], TopicRule::Keyed(&["queueurl"])),
    // C# — AWSSDK.SQS: `ReceiveMessageAsync(new ReceiveMessageRequest { QueueUrl = "..." })`.
    (".ReceiveMessageAsync(", QueueFramework::Sqs, &["amazon.sqs", "awssdk"], TopicRule::Keyed(&["queueurl"])),
    // GCP — JS `pubsub.subscription('orders-worker')`, Python
    // `subscriber.subscription_path("proj", "orders-worker")` (arg #0 is the
    // project). A subscription is named independently of its topic, so these
    // pair only when both are spelled alike — a declared COVERAGE_CAVEATS gap.
    (".subscription(", QueueFramework::PubSub, &["@google-cloud/pubsub"], TopicRule::ArgLiteral),
    (".subscription_path(", QueueFramework::PubSub, &["google.cloud"], TopicRule::ArgIndex(1)),
    // Azure Service Bus — C# `client.CreateProcessor("orders", options)`: for a
    // topic, arg #0 is the TOPIC and arg #1 the subscription, so arg #0 joins
    // the sender either way.
    ("CreateProcessor(", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::ArgLiteral),
    ("CreateReceiver(", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::ArgLiteral),
    // JS — @azure/service-bus: `sbClient.createReceiver("orders")`.
    (".createReceiver(", QueueFramework::AzureServiceBus, &["@azure/service-bus"], TopicRule::ArgLiteral),
    // Python — azure-servicebus: `get_queue_receiver(queue_name="orders")`. The
    // subscription receiver reads its TOPIC so it joins `get_topic_sender`.
    ("get_queue_receiver(", QueueFramework::AzureServiceBus, &["azure.servicebus"], TopicRule::KeyedOrArg(&["queue_name"])),
    ("get_subscription_receiver(", QueueFramework::AzureServiceBus, &["azure.servicebus"], TopicRule::KeyedOrArg(&["topic_name"])),
    // ---- A2.4: annotation-driven consumers --------------------------------
    // The handler declares its queue in its own annotation / attribute, so
    // each row reads that annotation's argument region. `@KafkaListener` is
    // the A2.2 row above. Every gate names the LIBRARY PACKAGE, never a word
    // the needle already holds: `@SqsListener(` lowercases to text containing
    // "sqs", so an `["sqs"]` gate would pass on the needle alone
    // (`annotation_gates_are_not_satisfied_by_their_own_needle` asserts it).
    // Java/Kotlin — Spring AMQP: `@RabbitListener(queues = "orders")`. The
    // `bindings = @QueueBinding(value = @Queue(value = "orders"))` form reads
    // through `value`: the first `value =` holds `@Queue(`, not a literal, so
    // the scan moves on to the queue's own.
    ("@RabbitListener(", QueueFramework::RabbitMQ, &["springframework.amqp"], TopicRule::Keyed(&["queues", "value"])),
    // Java/Kotlin — Spring JMS: `@JmsListener(destination = "orders")`.
    ("@JmsListener(", QueueFramework::Jms, &["springframework.jms"], TopicRule::Keyed(&["destination"])),
    // Java/Kotlin — Spring Cloud AWS 3.x (`io.awspring`) and 2.x
    // (`org.springframework.cloud.aws`): `@SqsListener("orders")`,
    // `@SqsListener(queueNames = "orders")`, or a queue URL, which folds.
    // KNOWN IMPRECISION: an array value (`queueNames = {"a"}`) is unreadable
    // to the keyed scan, so the fallback reads arg #0 — right when arg #0 IS
    // that array, wrong if another string attribute (`id = "x"`) precedes it.
    ("@SqsListener(", QueueFramework::Sqs, &["awspring", "springframework.cloud.aws"], TopicRule::KeyedOrArg(&["value", "queuenames"])),
    // TS — NestJS: `@Processor('emails')` on a WorkerHost (`@nestjs/bullmq`)
    // or a Bull consumer (`@nestjs/bull`). `Processor` is a common decorator
    // name, so the gate is the bull package, never the decorator.
    ("@Processor(", QueueFramework::BullMQ, &["@nestjs/bull", "bullmq"], TopicRule::ArgLiteral),
    // TS — @golevelup/nestjs-rabbitmq:
    // `@RabbitSubscribe({ exchange: 'orders', routingKey: 'order.created', queue: 'billing' })`.
    // The queue joins a direct producer; the routing key is the fallback.
    ("@RabbitSubscribe(", QueueFramework::RabbitMQ, &["golevelup", "rabbitmq"], TopicRule::Keyed(&["queue", "routingkey"])),
    // C# — Azure Functions, in-process (`Microsoft.Azure.WebJobs`) and
    // isolated worker (`Microsoft.Azure.Functions.Worker`):
    // `[ServiceBusTrigger("orders", Connection = "Sb")]`. For a topic trigger
    // arg #0 is the TOPIC and arg #1 the subscription, so arg #0 joins the
    // sender either way — the `CreateProcessor(` precedent above.
    ("[ServiceBusTrigger(", QueueFramework::AzureServiceBus, &["webjobs", "azure.functions.worker", "azure.messaging.servicebus"], TopicRule::ArgLiteral),
    // ---- CL.2: Go broker clients -------------------------------------------
    // Go exports Capitalised APIs and several take `ctx` first, so no row above
    // reads a Go call. `.Consume(`, `.Subscription(`, `.Topic(` and `.Publish(`
    // are common Go method names: every gate is the library's IMPORT PATH.
    // streadway/amqp + rabbitmq/amqp091-go: `ch.Consume(queue, consumer, ..)`.
    // The tutorial's `ch.Consume(q.Name, ..)` names no queue; CL.1's declare
    // pass binds it to the file's one `ch.QueueDeclare("orders", ..)`.
    (".Consume(", QueueFramework::RabbitMQ, &["streadway/amqp", "amqp091-go"], TopicRule::ArgLiteral),
    (".ConsumeWithContext(", QueueFramework::RabbitMQ, &["amqp091-go"], TopicRule::ArgIndex(1)),
    // azservicebus: `client.NewReceiverForQueue("orders", nil)`. A subscription
    // receiver reads its TOPIC (arg #0), so it joins the sender — the
    // `CreateProcessor(` precedent.
    (".NewReceiverForQueue(", QueueFramework::AzureServiceBus, &["azservicebus"], TopicRule::ArgLiteral),
    (".NewReceiverForSubscription(", QueueFramework::AzureServiceBus, &["azservicebus"], TopicRule::ArgLiteral),
    // cloud.google.com/go/pubsub (v1 and v2 paths): v1 `client.Subscription("s")`,
    // v2 `client.Subscriber("s")`. The JS row's COVERAGE_CAVEATS gap holds: a
    // subscription pairs only when it is spelled like its topic.
    (".Subscription(", QueueFramework::PubSub, &["cloud.google.com/go/pubsub"], TopicRule::ArgLiteral),
    (".Subscriber(", QueueFramework::PubSub, &["cloud.google.com/go/pubsub"], TopicRule::ArgLiteral),
    // go-redis (github.com/redis/go-redis, github.com/go-redis/redis): ctx is
    // arg #0, so `LPop(ctx, key)` reads arg #1 and `BLPop(ctx, timeout,
    // keys..)` arg #2, the first key.
    (".BLPop(", QueueFramework::RedisList, &["go-redis"], TopicRule::ArgIndex(2)),
    (".BRPop(", QueueFramework::RedisList, &["go-redis"], TopicRule::ArgIndex(2)),
    (".LPop(", QueueFramework::RedisList, &["go-redis"], TopicRule::ArgIndex(1)),
    (".RPop(", QueueFramework::RedisList, &["go-redis"], TopicRule::ArgIndex(1)),
    // aws-sdk-go v1 / v2: `client.ReceiveMessage(ctx, &sqs.ReceiveMessageInput{
    // QueueUrl: aws.String("https://sqs.../orders")})`. The keyed scan steps
    // into `aws.String(` (`queue_topic::POINTER_HELPERS`) and the URL folds.
    // Gated on the SQS SERVICE package, like the SNS rows, so another AWS
    // service's call of the same name is not a queue.
    (".ReceiveMessage(", QueueFramework::Sqs, &["aws-sdk-go/service/sqs", "aws-sdk-go-v2/service/sqs"], TopicRule::Keyed(&["queueurl"])),
    // confluent-kafka-go: `c.SubscribeTopics([]string{"orders"}, nil)` (the
    // list's first topic) and `c.SubscribeTopic("orders", nil)`.
    (".SubscribeTopics(", QueueFramework::Kafka, &["confluent-kafka-go"], TopicRule::ArgLiteral),
    (".SubscribeTopic(", QueueFramework::Kafka, &["confluent-kafka-go"], TopicRule::ArgLiteral),
    // ---- CL.3: JVM / .NET broker clients ------------------------------------
    // Every row sits BEFORE the A2.9 block, so a generic `.subscribe(` verb row
    // yields to a call one of these already read. `.receiver()` and the
    // `Template.` / `.leftPop(` spellings are common words: every gate is the
    // library's PACKAGE.
    // Java / Scala / Kotlin — azure-messaging-servicebus names the queue on a
    // builder METHOD after the client kind: `new ServiceBusClientBuilder()
    // .connectionString(c).receiver().queueName("orders").buildClient()`,
    // read along the chain to the statement's end (`TopicRule::Chain`). A
    // topic receiver (`.topicName("t").subscriptionName("s")`) reads its
    // TOPIC, so it joins the sender: the `CreateProcessor(` precedent. The C#
    // namespace lowercases to the same gate, but the C# API has no
    // `.receiver()`.
    (".receiver()", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::Chain(&["queuename", "topicname"])),
    (".processor()", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::Chain(&["queuename", "topicname"])),
    (".sessionReceiver()", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::Chain(&["queuename", "topicname"])),
    (".sessionProcessor()", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::Chain(&["queuename", "topicname"])),
    // Spring Cloud GCP (`com.google.cloud.spring.pubsub`, before 2.x
    // `org.springframework.cloud.gcp.pubsub`): `pubSubTemplate.subscribe(
    // "orders", handler)`. Arg #0 is a SUBSCRIPTION: the JS `.subscription(`
    // row's COVERAGE_CAVEATS gap holds.
    ("Template.subscribe(", QueueFramework::PubSub, &["cloud.spring.pubsub", "cloud.gcp.pubsub"], TopicRule::ArgLiteral),
    // Spring Data Redis lists: `redisTemplate.opsForList().rightPop("orders",
    // timeout)`, the key first.
    (".leftPop(", QueueFramework::RedisList, &["springframework.data.redis"], TopicRule::ArgLiteral),
    (".rightPop(", QueueFramework::RedisList, &["springframework.data.redis"], TopicRule::ArgLiteral),
    // AWS SDK for Java v2, the developer-guide shape: the request is built on
    // its OWN statement (`ReceiveMessageRequest.builder().queueUrl("..")
    // ..build()`) and `sqs.receiveMessage(request)` names no queue, so CL.1's
    // rule (ii) drops that call's tag once this row named one.
    ("ReceiveMessageRequest.builder()", QueueFramework::Sqs, &["awssdk.services.sqs"], TopicRule::Chain(&["queueurl"])),
    // C# — NATS.Net v2 (`NATS.Client.Core`, `NATS.Net`): `nc.SubscribeAsync<
    // Order>("orders")`. No trailing `(`: the type arguments sit between the
    // name and the call, and `queue_topic::arg_region` steps over them. The
    // HEAD `nc.Subscribe` row prefix-matches the same call and reads nothing;
    // CL.1's rule (i) drops its tag beside this row's node.
    (".SubscribeAsync", QueueFramework::Nats, &["nats.client", "nats.net"], TopicRule::ArgLiteral),
    // C# — MQTTnet: `client.SubscribeAsync("sensors/temp")`, an options
    // builder's `.WithTopicFilter("x")` / `.WithTopicFilter(f =>
    // f.WithTopic("x"))` (its arg #0's first literal), and `new
    // MqttTopicFilterBuilder().WithTopic("x")`. None is a shared verb
    // ([`is_generic_verb_row`]), so each reads its topic normally.
    ("MqttTopicFilterBuilder()", QueueFramework::Mqtt, &["mqttnet"], TopicRule::Chain(&["withtopic"])),
    (".SubscribeAsync(", QueueFramework::Mqtt, &["mqttnet"], TopicRule::ArgLiteral),
    (".WithTopicFilter(", QueueFramework::Mqtt, &["mqttnet"], TopicRule::ArgLiteral),
    // C# — Google.Cloud.PubSub.V1: `SubscriptionName.FromProjectSubscription(
    // "shop", "orders")`, arg #0 the project. A subscription pairs only when
    // spelled like its topic (the `.subscription_path(` caveat).
    ("SubscriptionName.FromProjectSubscription(", QueueFramework::PubSub, &["google.cloud.pubsub"], TopicRule::ArgIndex(1)),
    // ---- A2.9: broker pub/sub that used to live in eventbus.rs ----------
    // These verbs are the broadest needles in the table, so every row below is
    // a GENERIC-VERB row ([`is_generic_verb_row`]): it yields to any earlier
    // row that already claimed the call site, and it reads a channel only when
    // the literal LEADS argument #0 — an Rx `.subscribe(x => log('hi'))` in a
    // file that happens to import redis must not mint `queue_consumer:hi`.
    // eventbus.rs suppresses its own `.subscribe(` / `.on(` twin in the same
    // files (its gate is derived from these signals — [`broker_signal_present`]).
    // Redis pub/sub — redis-py `p.subscribe('ch')`, node-redis / ioredis
    // `sub.subscribe('ch')`, Ruby `redis.subscribe('ch')`. `redis` also covers
    // `ioredis`. `psubscribe` is a GLOB pattern; the resolver has no Redis
    // dialect yet, so it pairs only literally.
    (".subscribe(", QueueFramework::RedisPubSub, &["redis"], TopicRule::ArgLiteral),
    (".psubscribe(", QueueFramework::RedisPubSub, &["redis"], TopicRule::ArgLiteral),
    // MQTT — paho (Python / Java) and mqtt.js: `client.subscribe('sensors/temp')`.
    (".subscribe(", QueueFramework::Mqtt, &["mqtt", "paho"], TopicRule::ArgLiteral),
    // Go — eclipse/paho.mqtt.golang exports Capitalised APIs.
    (".Subscribe(", QueueFramework::Mqtt, &["paho.mqtt"], TopicRule::ArgLiteral),
];

const PRODUCER_PATTERNS: &[(&str, QueueFramework, &[&str], TopicRule)] = &[
    // `.delay(` collides with `setTimeout.delay`, jQuery `.delay`, Carrierwave,
    // and many JS animation libs; require Celery presence.
    // A2.5: `send_email.delay("welcome@example.com")` — the receiver is the
    // task, the argument is the payload.
    // CL.1: an IMPORT_GATED_ROWS row — a Python file that fails the gate
    // still fires on a task it imports by name (`from tasks import x`).
    (".delay(", QueueFramework::Celery, &["celery", "@shared_task"], TopicRule::Receiver),
    (".apply_async(", QueueFramework::Celery, &[], TopicRule::Receiver),
    // `.send(` is wildly overloaded (`res.send`, `socket.send`, ...). Require
    // Dramatiq import — `import dramatiq` or `@dramatiq.actor`.
    (".send(", QueueFramework::Dramatiq, &["dramatiq"], TopicRule::Receiver),
    // `queue.add(` — generic var name; require BullMQ context. CL.1:
    // NoIdentity — arg #0 is the JOB name, dispatch metadata the worker reads
    // as `job.name`, never a queue; the queue is `new Queue('x')` /
    // `@InjectQueue('x')`. Liveness only, so it mints no tag either.
    ("queue.add(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::NoIdentity),
    // `new Queue(` — also generic; require BullMQ.
    ("new Queue(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    // TS — NestJS producers inject the queue by name
    // (`@InjectQueue('audio') private q: Queue`) and call `this.q.add(..)`.
    // The gate also admits `@nestjs/bullmq`.
    ("@InjectQueue(", QueueFramework::BullMQ, &["@nestjs/bull"], TopicRule::ArgLiteral),
    // A2.5: `HardWorker.perform_async(order.id)` — receiver, not the id.
    ("perform_async", QueueFramework::Sidekiq, &[], TopicRule::Receiver),
    ("perform_in", QueueFramework::Sidekiq, &[], TopicRule::Receiver),
    // A2.5: nothing precedes `Oban.insert`; the worker is the callee of its
    // first argument, `Oban.insert(EmailWorker.new(%{}))`.
    ("Oban.insert", QueueFramework::Oban, &[], TopicRule::ArgReceiver),
    ("nc.publish", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    // Go's nats.go exports Capitalized `nc.Publish`; the lowercase JS needle
    // never matches Go source, so Go NATS producers went blind.
    ("nc.Publish", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    // amqplib `channel.publish(exchange, routingKey, content)`. CL.1: the
    // routing key (arg #1) names the queue on the default and direct
    // exchanges; an EMPTY key (fanout) falls back to the exchange (arg #0),
    // the identity this row read before.
    ("channel.publish", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"], TopicRule::ArgIndexOr(1, 0)),
    // amqplib's default-exchange shortcut `channel.sendToQueue('jobs', buf)`.
    (".sendToQueue(", QueueFramework::RabbitMQ, &["amqplib"], TopicRule::ArgLiteral),
    // pika's positional arg #0 is the exchange; the `basic_publish(` Keyed row
    // below reads the routing key of the SAME call, which explains this row's
    // empty read (CL.1's overlap rule), so the pair mints one node.
    ("channel.basic_publish", QueueFramework::RabbitMQ, &[], TopicRule::ArgLiteral),
    ("producer.send", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    ("producer.produce", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::ArgLiteral),
    // Go Kafka producers: confluent `producer.Produce`, segmentio `writer.WriteMessages`.
    // A2.2: the trailing `(` is REQUIRED — without it this needle also swallows
    // C#'s `_producer.ProduceAsync(`, whose topic sits where this rule cannot
    // read it, manufacturing a `queue_producer:unresolved:kafka` tag beside the
    // real node.
    ("producer.Produce(", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    // CL.2: Keyed, like `.WriteMessages(`. kafka-go's arg #0 is always `ctx`,
    // so the old KeyedOrArg fallback only ever recorded the expression `ctx`,
    // which kept the tag alive beside a topic the Writer literal named.
    ("writer.WriteMessages", QueueFramework::Kafka, &["kafka", "segmentio"], TopicRule::Keyed(&["topic"])),
    // Redis-as-queue producer side. .lpush / .rpush both push items onto a
    // list; consumers BLPOP/BRPOP off the other end.
    (".lpush(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".rpush(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    ("ListLeftPush(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListLeftPushAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPush(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPushAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    // ---- A2.2: receiver-agnostic needles ---------------------------------
    // C# — Confluent.Kafka: `_producer.ProduceAsync("orders", msg)`.
    (".ProduceAsync(", QueueFramework::Kafka, &["confluent.kafka"], TopicRule::ArgLiteral),
    (".Produce(", QueueFramework::Kafka, &["confluent.kafka"], TopicRule::ArgLiteral),
    // Java/Kotlin — Spring: `kafkaTemplate.send("orders", payload)`. Carrying
    // `Template.` keeps this off a bare `.send(` while matching any spelling of
    // the field (kafkaTemplate / KafkaTemplate / ordersTemplate).
    ("Template.send(", QueueFramework::Kafka, &["springframework.kafka"], TopicRule::ArgLiteral),
    // Java/Scala — plain client, diamond form: `new ProducerRecord<>("orders", v)`.
    // KNOWN MISS: the explicit-generics form `new ProducerRecord<String,String>(`.
    // CL.3 taught `queue_topic::arg_region` to step over a `<...>` list, but
    // only after a MEMBER-CALL needle (one holding a `.`, `.SubscribeAsync<T>(`):
    // after a dotless TYPE needle the list opens a constructor whose argument
    // is config (`new KafkaConsumer<>(props)`). A `ProducerRecord` row needs
    // that guard lifted for its own needle (0.5.2).
    ("ProducerRecord<>(", QueueFramework::Kafka, &["kafka"], TopicRule::ArgLiteral),
    // Go — segmentio/kafka-go: `w.WriteMessages(ctx, kafka.Message{Topic: "x"})`.
    (".WriteMessages(", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    ("kafka.NewWriter(", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    // Rust — rdkafka: `FutureRecord::to("orders")`.
    ("FutureRecord::to(", QueueFramework::Kafka, &["rdkafka"], TopicRule::ArgLiteral),
    // PHP — rdkafka: the topic is named on `$producer->newTopic("orders")`;
    // `$topic->produce(PARTITION, flags, $payload)` is liveness only: arg #1 is
    // the msgflags int and NO argument ever holds the topic, which is bound to
    // the `$topic` object. NoIdentity (not ArgIndex(1), as first drafted) —
    // otherwise this row's empty topic list falls back to a
    // `queue_producer:unresolved:kafka` tag standing beside the real node the
    // `newTopic(` row just emitted.
    ("newTopic(", QueueFramework::Kafka, &["rdkafka"], TopicRule::ArgLiteral),
    ("->produce(", QueueFramework::Kafka, &["rdkafka"], TopicRule::NoIdentity),
    // Ruby — WaterDrop / Karafka: `produce_async(topic: "orders", payload: p)`.
    (".produce_async(", QueueFramework::Kafka, &["waterdrop", "karafka", "rdkafka"], TopicRule::Keyed(&["topic"])),
    // C / C++ — librdkafka: `rd_kafka_topic_new(rk, "orders", NULL)`.
    ("rd_kafka_topic_new(", QueueFramework::Kafka, &["rdkafka", "librdkafka"], TopicRule::ArgIndex(1)),
    // Clojure — `(send! producer {:topic "orders" :value v})`. KNOWN MISS: the
    // separator-less EDN map is unreadable to the keyed scanner, so this row is
    // framework-tag liveness until the scanner learns `{:k v}`.
    ("send!", QueueFramework::Kafka, &["kafka"], TopicRule::Keyed(&["topic"])),
    // Java/Kotlin — Spring AMQP: `rabbitTemplate.convertAndSend("orders", msg)`.
    ("Template.convertAndSend(", QueueFramework::RabbitMQ, &["springframework.amqp"], TopicRule::ArgLiteral),
    // C# — RabbitMQ.Client: `BasicPublish(exchange, routingKey, props, body)`.
    (".BasicPublish(", QueueFramework::RabbitMQ, &["rabbitmq.client"], TopicRule::ArgIndex(1)),
    // v7: `BasicPublishAsync(exchange: "", routingKey: "orders", body: b)` —
    // the routing key is arg #1, named or positional.
    (".BasicPublishAsync(", QueueFramework::RabbitMQ, &["rabbitmq.client"], TopicRule::ArgIndex(1)),
    // Elixir — AMQP: `publish(chan, exchange, routing_key, payload)` — arg #2 is
    // the routing key; arg #1 is the exchange, which is usually "".
    ("AMQP.Basic.publish(", QueueFramework::RabbitMQ, &["amqp"], TopicRule::ArgIndex(2)),
    // Python — pika, receiver-free: `ch.basic_publish(routing_key="orders")`.
    // Positional arg #0 is deliberately NOT read: for pika it is the exchange.
    ("basic_publish(", QueueFramework::RabbitMQ, &["pika", "amqp"], TopicRule::Keyed(&["routing_key", "queue"])),
    // ---- A2.6: cloud brokers (see the CONSUMER_PATTERNS block) ------------
    // Python — boto3: `sqs.send_message(QueueUrl="https://sqs.../orders")`.
    (".send_message(", QueueFramework::Sqs, &["boto3", "botocore"], TopicRule::Keyed(&["queueurl", "queuename"])),
    // JS — AWS SDK v3: `new SendMessageCommand({ QueueUrl: "..." })`.
    ("SendMessageCommand(", QueueFramework::Sqs, &["@aws-sdk/client-sqs"], TopicRule::Keyed(&["queueurl"])),
    // C# — AWSSDK.SQS: `SendMessageAsync(new SendMessageRequest { QueueUrl = "..." })`.
    (".SendMessageAsync(", QueueFramework::Sqs, &["amazon.sqs", "awssdk"], TopicRule::Keyed(&["queueurl"])),
    // JS — SDK v2 / v3 aggregated `sqs.sendMessage({ QueueUrl })`; Java — SDK v2
    // builder `.queueUrl("...")`. The real Java package is
    // `software.amazon.awssdk.services.sqs`, hence the `services.` gate.
    (".sendMessage(", QueueFramework::Sqs, &["aws-sdk", "awssdk.services.sqs", "amazonaws.services.sqs"], TopicRule::Keyed(&["queueurl"])),
    // SNS — boto3 / JS / Java: `sns.publish(TopicArn="arn:aws:sns:...:orders")`.
    // BROAD needle; since A2.9 eventbus.rs no longer mints an EVENT_EMITTER
    // twin in a file these signals gate. C# spells it `PublishAsync`, so it
    // never matches.
    (".publish(", QueueFramework::Sns, &["boto3", "aws-sdk", "amazon.simplenotification", "awssdk.services.sns", "amazonaws.services.sns"], TopicRule::Keyed(&["topicarn"])),
    // JS — AWS SDK v3: `new PublishCommand({ TopicArn: "..." })`.
    ("PublishCommand(", QueueFramework::Sns, &["@aws-sdk/client-sns"], TopicRule::Keyed(&["topicarn"])),
    // GCP — JS `pubsub.topic('orders').publishMessage(...)`; Python
    // `publisher.topic_path("proj", "orders")` (arg #0 is the project).
    // KNOWN IMPRECISION: `.topic(` is a reference, not a publish, so a
    // subscriber written `pubsub.topic('orders').subscription(...)` also
    // mints the producer node.
    (".topic(", QueueFramework::PubSub, &["@google-cloud/pubsub"], TopicRule::ArgLiteral),
    (".topic_path(", QueueFramework::PubSub, &["google.cloud"], TopicRule::ArgIndex(1)),
    // Azure Service Bus — C# `client.CreateSender("orders")`, JS
    // `sbClient.createSender("orders")`, Python
    // `get_queue_sender(queue_name=...)` / `get_topic_sender(topic_name=...)`.
    // No row for a bare `ServiceBusSender` type mention: it names no queue,
    // and a NoIdentity row there would emit nothing at all.
    ("CreateSender(", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::ArgLiteral),
    (".createSender(", QueueFramework::AzureServiceBus, &["@azure/service-bus"], TopicRule::ArgLiteral),
    ("get_queue_sender(", QueueFramework::AzureServiceBus, &["azure.servicebus"], TopicRule::KeyedOrArg(&["queue_name"])),
    ("get_topic_sender(", QueueFramework::AzureServiceBus, &["azure.servicebus"], TopicRule::KeyedOrArg(&["topic_name"])),
    // ---- CL.2: Go broker clients (see the CONSUMER_PATTERNS block) ---------
    // Every row here sits BEFORE the A2.9 block: the MQTT `.Publish(` row there
    // is a generic verb that yields to a call an earlier row already read.
    // streadway/amqp + amqp091-go: `ch.Publish(exchange, key, mandatory,
    // immediate, msg)` reads the routing key; the context-first spellings read
    // it one slot later. An EMPTY key (fanout) falls back to the exchange,
    // amqplib's CL.1 rule.
    (".Publish(", QueueFramework::RabbitMQ, &["streadway/amqp", "amqp091-go"], TopicRule::ArgIndexOr(1, 0)),
    (".PublishWithContext(", QueueFramework::RabbitMQ, &["streadway/amqp", "amqp091-go"], TopicRule::ArgIndexOr(2, 1)),
    (".PublishWithDeferredConfirm(", QueueFramework::RabbitMQ, &["amqp091-go"], TopicRule::ArgIndexOr(1, 0)),
    (".PublishWithDeferredConfirmWithContext(", QueueFramework::RabbitMQ, &["amqp091-go"], TopicRule::ArgIndexOr(2, 1)),
    // aws-sdk-go v1 / v2 SNS: `client.Publish(ctx, &sns.PublishInput{TopicArn:
    // aws.String("arn:aws:sns:..:orders")})`, gated on the SNS service package.
    (".Publish(", QueueFramework::Sns, &["aws-sdk-go/service/sns", "aws-sdk-go-v2/service/sns"], TopicRule::Keyed(&["topicarn"])),
    // aws-sdk-go v1 / v2 SQS: `client.SendMessage(ctx, &sqs.SendMessageInput{
    // QueueUrl: aws.String("https://sqs.../orders")})`.
    (".SendMessage(", QueueFramework::Sqs, &["aws-sdk-go/service/sqs", "aws-sdk-go-v2/service/sqs"], TopicRule::Keyed(&["queueurl"])),
    // cloud.google.com/go/pubsub: v1 `client.Topic("orders")`, v2
    // `client.Publisher("orders")`. KNOWN IMPRECISION, the JS `.topic(` row's:
    // `.Topic(` is a reference, not a publish, so a subscriber that names its
    // topic (`pubsub.SubscriptionConfig{Topic: client.Topic("orders")}`) mints
    // the producer node too.
    (".Topic(", QueueFramework::PubSub, &["cloud.google.com/go/pubsub"], TopicRule::ArgLiteral),
    (".Publisher(", QueueFramework::PubSub, &["cloud.google.com/go/pubsub"], TopicRule::ArgLiteral),
    // azservicebus: `client.NewSender("orders", nil)`, a queue or a topic.
    (".NewSender(", QueueFramework::AzureServiceBus, &["azservicebus"], TopicRule::ArgLiteral),
    // go-redis: `rdb.LPush(ctx, key, values..)`, the key behind ctx.
    (".LPush(", QueueFramework::RedisList, &["go-redis"], TopicRule::ArgIndex(1)),
    (".RPush(", QueueFramework::RedisList, &["go-redis"], TopicRule::ArgIndex(1)),
    // segmentio/kafka-go: the topic is a field of the WRITER, `&kafka.Writer{
    // Addr: .., Topic: "orders"}` or `kafka.NewWriter(kafka.WriterConfig{..})`
    // (kafka-go rejects a Topic set on both the Writer and the Message). The
    // needle ends in `{`, so the keyed scan reads the literal's body. A
    // `WriteMessages(ctx, kafka.Message{Value: v})` through that Writer names
    // no topic and carries no expression, so CL.1's rule (ii) drops its tag.
    ("kafka.Writer{", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    ("kafka.WriterConfig{", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    // ---- CL.3: JVM / .NET broker clients (see the CONSUMER_PATTERNS block) -
    // Before the A2.9 block, so the generic `.publish(` verb rows yield to a
    // call these read.
    // azure-messaging-servicebus: `new ServiceBusClientBuilder()
    // .connectionString(c).sender().queueName("orders").buildClient()`.
    (".sender()", QueueFramework::AzureServiceBus, &["azure.messaging.servicebus"], TopicRule::Chain(&["queuename", "topicname"])),
    // Spring Cloud GCP: `pubSubTemplate.publish("orders", payload)`.
    ("Template.publish(", QueueFramework::PubSub, &["cloud.spring.pubsub", "cloud.gcp.pubsub"], TopicRule::ArgLiteral),
    // Spring Data Redis lists: `redisTemplate.opsForList().leftPush("orders", p)`.
    (".leftPush(", QueueFramework::RedisList, &["springframework.data.redis"], TopicRule::ArgLiteral),
    (".rightPush(", QueueFramework::RedisList, &["springframework.data.redis"], TopicRule::ArgLiteral),
    // AWS SDK for Java v2 request builders. Built inside the call
    // (`sqs.sendMessage(SendMessageRequest.builder()..build())`) the A2.6
    // `.sendMessage(` row reads the same queue and the two sites join one
    // node; built on its own statement, only this row reads it and CL.1's
    // rule (ii) drops the call's tag.
    ("SendMessageRequest.builder()", QueueFramework::Sqs, &["awssdk.services.sqs"], TopicRule::Chain(&["queueurl"])),
    ("PublishRequest.builder()", QueueFramework::Sns, &["awssdk.services.sns"], TopicRule::Chain(&["topicarn"])),
    // C# — NATS.Net v2: `nc.PublishAsync("orders", body)` and the typed
    // `nc.PublishAsync<Order>("orders", o)`, hence no trailing `(`. The HEAD
    // `nc.Publish` row prefix-matches and reads nothing; CL.1's rule (i)
    // drops its tag beside this row's node.
    (".PublishAsync", QueueFramework::Nats, &["nats.client", "nats.net"], TopicRule::ArgLiteral),
    // C# — MQTTnet: `new MqttApplicationMessageBuilder().WithTopic("x")
    // .WithPayload(p).Build()`, then `client.PublishAsync(msg)`.
    ("MqttApplicationMessageBuilder()", QueueFramework::Mqtt, &["mqttnet"], TopicRule::Chain(&["withtopic"])),
    // C# — Google.Cloud.PubSub.V1: `TopicName.FromProjectTopic("shop",
    // "orders")`, arg #0 the project.
    ("TopicName.FromProjectTopic(", QueueFramework::PubSub, &["google.cloud.pubsub"], TopicRule::ArgIndex(1)),
    // ---- A2.9: broker pub/sub (see the CONSUMER_PATTERNS block) -----------
    // Redis pub/sub — `r.publish('notifications', payload)`. Placed AFTER the
    // NATS / RabbitMQ / SNS rows: a file importing both is attributed to the
    // more specific row, which claims the call site first.
    (".publish(", QueueFramework::RedisPubSub, &["redis"], TopicRule::ArgLiteral),
    // MQTT — paho / mqtt.js `client.publish('sensors/temp', payload)`, and Go
    // paho's `client.Publish("sensors/temp", qos, retained, payload)`.
    (".publish(", QueueFramework::Mqtt, &["mqtt", "paho"], TopicRule::ArgLiteral),
    (".Publish(", QueueFramework::Mqtt, &["paho.mqtt"], TopicRule::ArgLiteral),
    // Java/Kotlin — Spring JMS: `jmsTemplate.convertAndSend("orders", msg)`,
    // the producer half of the A2.4 `@JmsListener(` row. Same needle as the
    // Spring AMQP row above; a file importing both is attributed to AMQP.
    ("Template.convertAndSend(", QueueFramework::Jms, &["springframework.jms"], TopicRule::ArgLiteral),
];

// ---- CL.1: DECLARING A QUEUE IS NOT CONSUMING IT -----------------------------
// `queue_declare(` / `.QueueDeclare(` used to be CONSUMER rows, so the RabbitMQ
// tutorial's producer (`send.py` / `Send.cs`), which declares the queue it
// publishes to, minted a `queue_consumer:<q>` beside its own producer and
// QueueStackResolver paired the two: a false self-flow. A declaration names a
// queue the file USES, which is exactly what an unnamed consume needs: Go's
// `q, _ := ch.QueueDeclare("orders", ..)` + `ch.Consume(q.Name, ..)`, pika's
// `queue_declare(queue="jobs")` + `basic_consume(queue=qname, ..)`.
// ---------------------------------------------------------------------------
/// Queue DECLARATIONS, scanned on the consumer side only and never a consumer
/// site by themselves. When a file declares EXACTLY ONE queue of a framework,
/// each consumer-row hit of that framework that named no topic (an "unnamed
/// consume") becomes a consumer site of the declared queue, at the CONSUME
/// call's offset; zero or two-plus declared queues bind nothing (ambiguous),
/// and the unnamed consume keeps HEAD's framework tag. Same tuple shape and
/// gate rules as [`CONSUMER_PATTERNS`].
const DECLARE_PATTERNS: &[(&str, QueueFramework, &[&str], TopicRule)] = &[
    // Python — pika / amqp: `ch.queue_declare(queue="orders")`, receiver-free.
    ("queue_declare(", QueueFramework::RabbitMQ, &["pika", "amqp"], TopicRule::KeyedOrArg(&["queue"])),
    // C# — RabbitMQ.Client v6 `channel.QueueDeclare(queue: "orders", ...)`, v7
    // async. CL.2: Go's streadway/amqp and amqp091-go spell it the same,
    // positionally: `q, err := ch.QueueDeclare("orders", durable, ..)`.
    (".QueueDeclare(", QueueFramework::RabbitMQ, &["rabbitmq.client", "streadway/amqp", "amqp091-go"], TopicRule::KeyedOrArg(&["queue"])),
    (".QueueDeclareAsync(", QueueFramework::RabbitMQ, &["rabbitmq.client"], TopicRule::KeyedOrArg(&["queue"])),
];

/// CL.1: rows whose library gate a Python file may miss while still calling
/// the task. The canonical Celery caller (`from tasks import process_order` +
/// `process_order.delay(order_id)`) never mentions `celery`. When such a row's
/// gate fails in a `.py` / `.pyw` file, a hit is kept iff its
/// [`TopicRule::Receiver`] chain is a BARE name (no `.`) that a
/// `from <mod> import ...` line of the same file binds
/// ([`python_imported_names`]). `pygame.time.delay(100)` (a dotted chain) and
/// jQuery's `.delay(` (not Python) stay out. KNOWN IMPRECISION: `from x
/// import time` + `time.delay(..)` mints a Celery producer `time`.
/// `.apply_async(` needs no entry: it is ungated.
const IMPORT_GATED_ROWS: &[&str] = &[".delay("];

/// True when `signals` is empty (always pass) or any signal substring appears in
/// `lower_source`. Lets distinct framework names gate their broad-needle patterns.
///
/// `lower_source` MUST already be `to_ascii_lowercase`d, and every signal literal
/// in both tables MUST be spelled lowercase (`signal_lists_are_lowercase` asserts
/// it) — that is what makes the GATE case-insensitive. It used to compare the raw
/// source, so `using Confluent.Kafka;` failed a `["kafka", "confluent"]` gate and
/// a whole C# Kafka file emitted nothing at all.
///
/// The NEEDLES stay case-sensitive: API names are (`Produce` is not `produce`).
fn signals_present(lower_source: &str, signals: &[&str]) -> bool {
    signals.is_empty() || signals.iter().any(|s| lower_source.contains(s))
}

/// A2.9: true for a message BROKER, false for a task queue. Exhaustive on
/// purpose — a new variant must decide which side of eventbus.rs's broker
/// gate its signals belong on.
fn is_broker(f: &QueueFramework) -> bool {
    match f {
        QueueFramework::Nats
        | QueueFramework::RabbitMQ
        | QueueFramework::Kafka
        | QueueFramework::RedisList
        | QueueFramework::RedisPubSub
        | QueueFramework::Sqs
        | QueueFramework::Sns
        | QueueFramework::PubSub
        | QueueFramework::AzureServiceBus
        | QueueFramework::Mqtt
        | QueueFramework::Jms => true,
        // BullMQ workers are real EventEmitters (`worker.on('completed')`).
        QueueFramework::Celery
        | QueueFramework::Dramatiq
        | QueueFramework::BullMQ
        | QueueFramework::Sidekiq
        | QueueFramework::Oban => false,
    }
}

/// Every library signal that gates a broker row in either table, deduped, in
/// table order. Derived, never re-typed: see [`broker_signal_present`].
fn broker_signals() -> &'static [&'static str] {
    static SIGNALS: OnceLock<Vec<&'static str>> = OnceLock::new();
    SIGNALS.get_or_init(|| {
        let mut out: Vec<&'static str> = Vec::new();
        for (_, framework, signals, _) in CONSUMER_PATTERNS
            .iter()
            .chain(PRODUCER_PATTERNS)
            .chain(DECLARE_PATTERNS)
        {
            if !is_broker(framework) {
                continue;
            }
            for s in signals.iter().copied() {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        out
    })
}

/// A2.9: does this (already lowercased) file import a message-broker client?
/// eventbus.rs asks before minting an EVENT_* node from a verb brokers share
/// with in-process buses (`publish(`, `.subscribe(`, `.on(`); a yes means the
/// call is broker traffic, which this module owns.
pub(crate) fn broker_signal_present(lower_source: &str) -> bool {
    broker_signals().iter().any(|s| lower_source.contains(s))
}

/// A2.9: the bare pub/sub verbs shared with Rx, in-process buses and every
/// other broker.
const GENERIC_VERB_NEEDLES: &[&str] = &[
    ".publish(",
    ".subscribe(",
    ".psubscribe(",
    ".Publish(",
    ".Subscribe(",
];

/// A2.9: rows whose needle is a bare pub/sub verb (`.publish(` /
/// `.subscribe(`) on a Redis pub/sub or MQTT row. CL.3: keyed by the NEEDLE
/// as well as the framework, so an MQTTnet builder row
/// (`MqttApplicationMessageBuilder()`, `.SubscribeAsync(`) is no shared verb:
/// it reads its topic normally instead of losing it to the literal-must-lead
/// guard, and it never yields. Every RedisPubSub / Mqtt row before CL.3 was a
/// verb row, so their behaviour is unchanged.
fn is_generic_verb_row(needle: &str, f: &QueueFramework) -> bool {
    matches!(f, QueueFramework::RedisPubSub | QueueFramework::Mqtt)
        && GENERIC_VERB_NEEDLES.contains(&needle)
}

/// A2.9: rows that give way when an EARLIER row already read the same call
/// site, so one call never mints two nodes of different frameworks.
fn yields_to_earlier_rows(needle: &str, f: &QueueFramework) -> bool {
    is_generic_verb_row(needle, f) || matches!(f, QueueFramework::Jms)
}

/// True when the argument region after `after` opens with a quoted literal,
/// or with an array whose first element is one (`subscribe(['a', 'b'])`).
fn literal_leads(source: &str, after: usize) -> bool {
    let rest = source.get(after..).unwrap_or("").trim_start();
    let rest = rest.strip_prefix('[').map_or(rest, str::trim_start);
    matches!(rest.as_bytes().first(), Some(b'\'' | b'"' | b'`'))
}

pub struct QueueNodes {
    pub nodes: Vec<Node>,
    /// A2.8: one `module -> node` CONTAINS edge per emitted node.
    ///
    /// Queue nodes used to have NO in-repo edge at all, so nothing linked a
    /// topic back to the file that publishes to it: `CodeNav.parent_of` was the
    /// only link, and it is a SINGLE pointer that `merge_nav` overwrites, so
    /// when two files published the same topic one of them simply vanished.
    ///
    /// CONTAINS is deliberate rather than a semantic category: it is excluded
    /// from `CODE_TABLES.carry_edges`, so linking the publishing file does NOT fan
    /// the blast radius back out through every symbol in that file.
    /// `QUEUE_FLOWS` stays the semantic path.
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// LE.4c: one (node, 0-indexed line) per call site that minted or re-used
    /// a topic node, uncapped (unlike the [`MAX_SITES`] provenance list), so a
    /// topic published from two functions of one file anchors in both.
    /// [`anchor::attach`] turns them into the owner edges: the enclosing
    /// function USES a QUEUE_PRODUCER, a QUEUE_CONSUMER is HANDLED_BY it. The
    /// node's POSITION and module CONTAINS above are kept as they are.
    pub anchors: Vec<Anchor>,
    /// LA.33: the handler each consumer passes as a callback, one per
    /// (consumer, handler), in site order. Consumer side only (empty for
    /// producers); [`bind_consumer_callbacks`] turns them into HANDLED_BY
    /// edges and refs once the file's spans are all in.
    pub callbacks: Vec<ConsumerCallback>,
}

/// Call sites recorded per (topic, framework) per file. A generated file can
/// call the same publish helper hundreds of times; the CODE cell is provenance,
/// not an index, and 16 names every place a human would actually open.
const MAX_SITES: usize = 16;

/// One (topic, framework) accumulated across every needle in ONE file, before
/// it becomes a `Node`.
///
/// The old code deduped with a `HashSet` and dropped the duplicate outright, so
/// the second and later call sites for a topic were lost. They are the
/// provenance this packet exists to keep, so dedup now MERGES.
struct Pending {
    id: NodeId,
    topic: String,
    qname: String,
    framework: QueueFramework,
    confidence: Confidence,
    /// 0-indexed lines, first-seen order, deduped, capped at [`MAX_SITES`].
    lines: Vec<usize>,
    /// A12.1: the needle's byte offset for each entry of `lines`, same order.
    /// The MESSAGE_TYPE scan looks for the payload type around these.
    offsets: Vec<usize>,
    /// LE.4c: the 0-indexed line of EVERY site recorded for this node, in
    /// scan order, not capped or deduped (`anchor::attach` sorts and dedups).
    anchor_lines: Vec<u32>,
    /// CL.1: where the node sits in the file's output — `(pending index, 1,
    /// 0)` for a named node, `(slot, 0, row)` for a framework tag, whose
    /// `slot` is the pending length when its row ran. Sorting on it puts a
    /// deferred tag exactly where the per-row fallback used to create it.
    rank: (usize, u8, usize),
}

/// Queue-consumer nodes for one file — one node per DISTINCT (topic, framework).
///
/// `path` is the file the source came from: it names the POSITION cell and
/// every entry of the CODE cell's `sites` array.
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
        DECLARE_PATTERNS,
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:",
        None,
        &mut ConstFoldCounts::default(),
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
        &[],
        node_kind::QUEUE_PRODUCER,
        "queue_producer:",
        None,
        &mut ConstFoldCounts::default(),
    )
}

/// LA.4 (A11.7): turns the identifier in a topic slot into the value it
/// holds, or `None`. The engine closes it over the file's own const table and
/// the repo's.
type TopicResolver<'a> = &'a dyn Fn(&str) -> Option<String>;

/// LA.4 (A11.7): what the post-cache const fold read in one file.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConstFoldCounts {
    /// Occurrences whose identifier expression resolved to a topic.
    pub folded: usize,
    /// Occurrences that recorded an identifier expression the resolver could
    /// not turn into a topic (a parameter, a runtime variable, an env read,
    /// an ambiguous constant, a lower-case binding in another file).
    pub unresolved: usize,
}

/// LA.4: both sides of one file's queue nodes, re-emitted with a resolver.
pub struct ConstFold {
    pub consumers: QueueNodes,
    pub producers: QueueNodes,
    pub counts: ConstFoldCounts,
    /// LE.4c: the file the nodes were read from, which [`replace_queue_nodes`]
    /// hands [`anchor::attach`] when it re-anchors the folded nodes.
    pub path: String,
}

/// LA.4 (A11.7): the file's queue nodes, both sides, emitted exactly as
/// [`extract_queue_consumer_nodes`] + [`extract_queue_producer_nodes`] emit
/// them, except that an occurrence naming its topic by an identifier becomes
/// a site when `resolve` turns that identifier into a topic.
///
/// Resolution needs the whole repo's const table, which is not a function of
/// this file, so the engine calls this AFTER the parse cache (never inside the
/// per-file extractors) and swaps the file's queue nodes with
/// [`replace_queue_nodes`] only when `counts.folded > 0`.
pub fn extract_queue_nodes_with_consts(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> ConstFold {
    let mut counts = ConstFoldCounts::default();
    let consumers = emit_queue_nodes(
        source,
        path,
        module_id,
        repo,
        CONSUMER_PATTERNS,
        DECLARE_PATTERNS,
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:",
        Some(resolve),
        &mut counts,
    );
    let producers = emit_queue_nodes(
        source,
        path,
        module_id,
        repo,
        PRODUCER_PATTERNS,
        &[],
        node_kind::QUEUE_PRODUCER,
        "queue_producer:",
        Some(resolve),
        &mut counts,
    );
    ConstFold {
        consumers,
        producers,
        counts,
        path: path.to_string(),
    }
}

/// LA.4: swap every QUEUE_CONSUMER / QUEUE_PRODUCER node of one file's parse
/// for `fold`'s, consumers then producers. Only this module mints those kinds.
///
/// The swap itself is kind-agnostic and shared with CB.3b's event fold
/// ([`marker_swap::swap`]): the old nodes, their module CONTAINS edges, nav
/// entries, owner edges and every edge / ref naming a gone id (a sentinel
/// whose sites all folded) go; the fold's nodes, CONTAINS edges and child ids
/// go back IN PLACE, so a folded file's parse is laid out exactly as the
/// per-file pass lays out a literal-topic file; the raw IMPORTS cell is
/// re-attached when the old nodes carried it; [`anchor::attach`] re-anchors
/// the fold's nodes (LE.4c), its edges stamped `extractor:anchor` rule
/// `const_fold`. What stays here is the queue half: the old set, and (LA.33)
/// [`bind_consumer_callbacks`] re-binding the fold's consumer callbacks once
/// the nodes are anchored — the direct edges land with the new owner edges,
/// and a same-id consumer's surviving refs are not doubled.
pub fn replace_queue_nodes(fp: &mut FileParse, module_id: NodeId, lang: &str, fold: ConstFold) {
    let is_queue =
        |k: &NodeKindId| *k == node_kind::QUEUE_CONSUMER || *k == node_kind::QUEUE_PRODUCER;
    let old: HashSet<NodeId> = fp
        .nav
        .kind_by_id
        .iter()
        .filter(|(_, k)| is_queue(k))
        .map(|(id, _)| *id)
        .collect();
    let ConstFold {
        consumers,
        producers,
        path,
        ..
    } = fold;
    let callbacks = consumers.callbacks;
    let mut nodes = consumers.nodes;
    nodes.extend(producers.nodes);
    let mut edges = consumers.edges;
    edges.extend(producers.edges);
    let mut anchors = consumers.anchors;
    anchors.extend(producers.anchors);
    let fresh = marker_swap::MarkerNodes {
        nodes,
        edges,
        navs: vec![consumers.nav, producers.nav],
        anchors,
    };
    marker_swap::swap(fp, module_id, lang, &old, fresh, &path, |fp| {
        bind_consumer_callbacks(fp, module_id, &callbacks);
    });
}

/// CL.1: one needle occurrence that survived its row's filters — what the
/// declare binding and the deferred-tag pass read after the row loop.
struct HitSpan<'p> {
    /// Byte span of the needle occurrence. Both ends come from
    /// `match_indices` plus the needle's length, so overlap tests compare
    /// byte offsets and never slice.
    start: usize,
    end: usize,
    needle: &'p str,
    rule: &'p TopicRule,
    framework: &'p QueueFramework,
    /// LA.33: the row's handler rule (consumer side only).
    handler: Option<HandlerRule>,
    /// Pending index of the node this hit's site joined, when it named one.
    named: Option<usize>,
    /// The row is `NoIdentity`: a liveness call, never a site and never a tag.
    no_identity: bool,
    /// The hit recorded an identifier expression in its topic slot.
    has_expr: bool,
    /// Pending index of the declared queue the declare pass bound it to.
    declared: Option<usize>,
    /// The row's index in its table: callbacks are ordered by it.
    row: usize,
}

/// CL.1: a row whose every hit read no topic. The per-row fallback minted its
/// framework tag on the spot; it is now recorded after the declare pass, and
/// only when one of its hits stays unexplained ([`explain_hit`]).
struct DeferredTag<'p> {
    row: usize,
    needle: &'p str,
    rule: &'p TopicRule,
    framework: &'p QueueFramework,
    handler: Option<HandlerRule>,
    /// `pending.len()` when the row ran: where the tag used to be created.
    slot: usize,
    /// Indices into the span list, in hit order (never empty).
    hits: Vec<usize>,
}

/// CL.1: why a deferred tag's hit needs no tag, and which node (if any)
/// inherits its callbacks.
#[derive(Clone, Copy)]
enum Explained {
    /// The same call is read by another row of the same framework that named
    /// a topic (`Some`) or is a `NoIdentity` liveness row (`None`).
    Overlap(Option<usize>),
    /// A call with no identifier expression in a file that names a queue of
    /// the same framework: the single such node, or `None` when several.
    Named(Option<usize>),
    /// The declare pass bound it to the file's one declared queue (its
    /// callbacks went to that node at binding time).
    Declare,
}

impl Explained {
    fn by(self) -> &'static str {
        match self {
            Explained::Overlap(_) => "overlap",
            Explained::Named(_) => "named",
            Explained::Declare => "declare",
        }
    }
}

/// CL.1 rule (i): the pending index of a hit of the SAME framework that named
/// a topic and whose needle span overlaps `spans[i]`'s (two rows reading one
/// call: pika `channel.basic_publish` + `basic_publish(`), `Some(None)` when
/// only a `NoIdentity` row's hit overlaps (segmentio `reader.ReadMessage` +
/// `.ReadMessage(`), else `None`. A named overlap wins over a liveness one.
fn overlap_of(spans: &[HitSpan<'_>], i: usize) -> Option<Option<usize>> {
    let s = spans.get(i)?;
    let mut liveness = false;
    for (j, o) in spans.iter().enumerate() {
        if j == i || o.framework != s.framework || !(o.start < s.end && s.start < o.end) {
            continue;
        }
        if let Some(n) = o.named.or(o.declared) {
            return Some(Some(n));
        }
        liveness |= o.no_identity;
    }
    liveness.then_some(None)
}

/// CL.1: is the deferred tag's hit `spans[i]` explained? Rules, in order:
/// (iii) the declare pass bound it, (i) [`overlap_of`], (ii) it carries no
/// identifier expression and `named` (the file's named nodes of its
/// framework, taken before any tag was recorded) is non-empty.
fn explain_hit(spans: &[HitSpan<'_>], i: usize, named: &[usize]) -> Option<Explained> {
    let s = spans.get(i)?;
    if s.declared.is_some() {
        return Some(Explained::Declare);
    }
    if let Some(o) = overlap_of(spans, i) {
        return Some(Explained::Overlap(o));
    }
    if !s.has_expr && !named.is_empty() {
        let single = if let [only] = named {
            Some(*only)
        } else {
            None
        };
        return Some(Explained::Named(single));
    }
    None
}

/// LA.4 (A11.7): the topic one hit names — its literal, else its identifier
/// expression folded through `folds` (a fold that names nothing counts as
/// unresolved).
fn read_hit(
    h: &queue_topic::TopicHit,
    folds: Option<TopicResolver<'_>>,
    counts: &mut ConstFoldCounts,
    framework: &QueueFramework,
    path: &str,
) -> Option<(String, TopicForm)> {
    match (&h.topic, folds, h.expr.as_deref()) {
        (Some(t), _, _) => Some((t.clone(), h.form)),
        (None, Some(resolve), Some(expr)) => {
            let folded = resolve(expr).and_then(|v| queue_topic::fold_topic(&v));
            match &folded {
                Some((t, _)) => {
                    counts.folded += 1;
                    if debug_enabled() {
                        eprintln!(
                            "[queues] const-fold expr={expr} topic={t} framework={framework:?} file={path}"
                        );
                    }
                }
                None => counts.unresolved += 1,
            }
            folded
        }
        _ => None,
    }
}

/// LA.33 + CL.1: [`push_callbacks`], remembering the row each new callback
/// came from so the file's callbacks can be put back in row order.
fn push_row_callbacks(
    callbacks: &mut Vec<ConsumerCallback>,
    rows: &mut Vec<usize>,
    seen: &mut HashSet<(NodeId, HandlerExpr)>,
    consumer: NodeId,
    found: Vec<(u32, HandlerExpr, bool)>,
    row: usize,
) {
    push_callbacks(callbacks, seen, consumer, found);
    rows.resize(callbacks.len(), row);
}

/// CL.1: a Python source file, where an [`IMPORT_GATED_ROWS`] row may fire
/// without its library gate.
fn is_python_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".py") || lower.ends_with(".pyw")
}

/// CL.1: the receiver chain right before `offset` when it is a BARE name (no
/// `.`): `process_order` of `process_order.delay(`, never `pygame.time`.
fn bare_receiver(source: &str, offset: usize) -> Option<&str> {
    let head = source.get(..offset)?;
    let b = head.as_bytes();
    let mut i = b.len();
    // Only ASCII bytes are stepped over, so `i` stays on a char boundary.
    while i > 0 && is_chain_byte(b[i - 1]) {
        i -= 1;
    }
    let chain = head.get(i..)?;
    (!chain.is_empty() && !chain.contains('.')).then_some(chain)
}

/// CL.1: a Python identifier (`[A-Za-z_][A-Za-z0-9_]*`, Unicode letters too).
fn is_python_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// CL.1: every name a `from <mod> import ...` line of this Python file binds:
/// comma lists, `a as b` binds `b`, a parenthesised list may span lines, a
/// trailing `\\` continues one; `import *` binds nothing and a plain
/// `import x` is not read (its calls are dotted, `x.task.delay(`).
fn python_imported_names(source: &str) -> BTreeSet<String> {
    let strip = |l: &str| l.split('#').next().unwrap_or("").to_string();
    let mut out = BTreeSet::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim_start().strip_prefix("from ") else {
            continue;
        };
        let rest = strip(rest);
        let Some(at) = rest.find(" import") else {
            continue;
        };
        let mut list = rest.get(at + " import".len()..).unwrap_or("").to_string();
        if !list.starts_with([' ', '(']) {
            continue;
        }
        if list.trim_start().starts_with('(') {
            while !list.contains(')') {
                let Some(next) = lines.next() else { break };
                list.push(' ');
                list.push_str(&strip(next));
            }
            list = list.replace(['(', ')'], " ");
        } else {
            while list.trim_end().ends_with('\\') {
                let cut = list.trim_end().len() - 1;
                list.truncate(cut);
                let Some(next) = lines.next() else { break };
                list.push(' ');
                list.push_str(&strip(next));
            }
        }
        for item in list.split(',') {
            let words: Vec<&str> = item.split_whitespace().collect();
            let name = match words.as_slice() {
                [n] => *n,
                [_, "as", alias] => *alias,
                _ => continue,
            };
            if is_python_ident(name) {
                out.insert(name.to_string());
            }
        }
    }
    out
}

/// Shared emit loop for both sides.
///
/// Was: one topic per needle, read from the FIRST occurrence of that needle in
/// the file. Now: every occurrence is scanned and each distinct topic gets its
/// own node, so a file that publishes to `orders` and `payments` stops hiding
/// one of them.
///
/// CL.1 — THE FRAMEWORK TAG IS MINTED ONLY FOR AN UNEXPLAINED TOPIC-LESS CALL.
/// The fallback used to be decided PER ROW: a row none of whose hits named a
/// topic minted `unresolved:<framework>` on the spot, so a second row reading
/// the SAME call (pika `channel.basic_publish` beside `basic_publish(`), or a
/// liveness row in a file another row had already named, stood a sentinel
/// beside the real node. Such a row is now DEFERRED, and after the last row
/// (and, consumer side, the [`DECLARE_PATTERNS`] pass) each of its hits is
/// explained by [`explain_hit`]. A deferred tag whose hits are ALL explained
/// is not recorded, and its callbacks move to the node that explained each
/// hit; one unexplained hit records it exactly as before (Weak, the row's
/// first hit's site, every hit's callback). The rule only ever REMOVES a tag
/// the per-row fallback minted, and a surviving tag lands at the row it used
/// to be created at (`Pending::rank`), so a file whose tags all survive emits
/// what it did before. A per-file "a named node silences every tag of its
/// framework" rule was weighed and rejected: it would hide a genuinely
/// unresolved second topic (`producer.produce(topicVar, ..)` beside a named
/// `producer.send`).
///
/// CL.1 — DECLARE BINDING (consumer side, `declares` non-empty): see
/// [`DECLARE_PATTERNS`]. An unnamed consume is a consumer-row hit that named
/// no topic, is not a `NoIdentity` row's and overlaps no hit that named one.
///
/// LA.4 (A11.7): `resolve` is `None` on the per-file (cached) path. The
/// engine's post-cache const fold passes a resolver: an occurrence that read
/// no literal but recorded an identifier expression
/// ([`queue_topic::TopicHit::expr`]) becomes a site when the resolver names a
/// value AND [`queue_topic::fold_topic`] accepts it — so a constant holding an
/// SQS URL folds exactly as the literal would. Never on a generic-verb row
/// (those keep their `literal_leads` guard) and never for `NoIdentity` or an
/// A2.5 identity rule. `counts` tallies folded and unresolved expressions.
///
/// fired_on (CL.1, `GLIA_QUEUE_DEBUG=1`):
///   `[queues] tag-suppressed needle='<n>' framework=<F> side=<consumer|producer> by=<overlap|named|declare> file=<path>`
///   `[queues] declare-bound queue=<q> framework=<F> consume='<needle>' consumes=<k> file=<path>`
///   `[queues] declare-unbound declared=<d> framework=<F> file=<path>`
///   `[queues] import-gated needle='.delay(' receiver=<sym> file=<path>`
#[allow(clippy::too_many_arguments)]
fn emit_queue_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    patterns: &[(&str, QueueFramework, &[&str], TopicRule)],
    declares: &[(&str, QueueFramework, &[&str], TopicRule)],
    kind: glia_core::NodeKindId,
    prefix: &str,
    resolve: Option<TopicResolver<'_>>,
    counts: &mut ConstFoldCounts,
) -> QueueNodes {
    let mut pending: Vec<Pending> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // A2.9: byte spans of every needle occurrence an earlier row read.
    let mut claimed: Vec<(usize, usize)> = Vec::new();
    // ONE allocation per emit call (two per file, both sides) — cheap beside the
    // tree-sitter parse that already ran, and it is what makes the gate
    // case-insensitive for every row at once.
    let lower = source.to_ascii_lowercase();
    // LA.33: consumer callbacks, per (consumer, handler), in site order;
    // CL.1: `callback_rows[i]` is the row `callbacks[i]` came from.
    let mut callbacks: Vec<ConsumerCallback> = Vec::new();
    let mut callback_rows: Vec<usize> = Vec::new();
    let mut callback_seen: HashSet<(NodeId, HandlerExpr)> = HashSet::new();
    // CJ.1a: in a Rust / Python file an occurrence starting in a string
    // literal or comment is no site, so a refused needle mints neither a
    // topic node nor its `unresolved:*` tag. Lexed on the first candidate.
    let mut guard = LazyGuard::new(path, source);
    // CL.1: every surviving hit, and the rows whose tag is deferred.
    let mut spans: Vec<HitSpan<'_>> = Vec::new();
    let mut deferred: Vec<DeferredTag<'_>> = Vec::new();
    let mut imported: Option<BTreeSet<String>> = None;
    let side = if kind == node_kind::QUEUE_CONSUMER {
        "consumer"
    } else {
        "producer"
    };

    for (row, (pattern, framework, signals, rule)) in patterns.iter().enumerate() {
        if !source.contains(pattern) {
            continue;
        }
        // CL.1: a Python file that fails an import-gated row's library gate
        // still fires on a task it imports by name.
        let import_gated = if signals_present(&lower, signals) {
            false
        } else if IMPORT_GATED_ROWS.contains(pattern) && is_python_path(path) {
            true
        } else {
            continue;
        };
        let handler = handler_rule(pattern).filter(|_| kind == node_kind::QUEUE_CONSUMER);
        let mut hits = queue_topic::scan_guarded(source, pattern, *rule, &mut guard);
        if import_gated {
            let names = imported.get_or_insert_with(|| python_imported_names(source));
            hits.retain(|h| {
                let Some(recv) = bare_receiver(source, h.offset) else {
                    return false;
                };
                let keep = names.contains(recv) && h.topic.as_deref() == Some(recv);
                if keep && debug_enabled() {
                    eprintln!(
                        "[queues] import-gated needle='{pattern}' receiver={recv} file={path}"
                    );
                }
                keep
            });
        }
        if yields_to_earlier_rows(pattern, framework) {
            let len = pattern.len();
            hits.retain(|h| {
                !claimed
                    .iter()
                    .any(|&(s, e)| h.offset < e && s < h.offset + len)
            });
        }
        if is_generic_verb_row(pattern, framework) {
            for h in &mut hits {
                if !literal_leads(source, h.offset + pattern.len()) {
                    h.topic = None;
                }
            }
        }
        if hits.is_empty() {
            continue;
        }
        claimed.extend(hits.iter().map(|h| (h.offset, h.offset + pattern.len())));
        // A2.8: keep each hit's OFFSET next to its topic. A topic read at byte
        // 402 is a call SITE at line 11, and that is the only provenance a
        // queue node has ever been able to carry. `line_of` is 0-indexed, the
        // tree-sitter convention every other span in the graph uses.
        // A2.6: the literal's shape (url/arn/path) rides along for the
        // `[queues] cloud broker=` marker.
        // A12.1: the byte offset rides along too — it anchors the MESSAGE_TYPE scan.
        // LA.4: the resolver only ever sees a row that may name a topic by an
        // identifier; everything else reads exactly as it always has.
        let folds = resolve.filter(|_| {
            !is_generic_verb_row(pattern, framework)
                && !is_identity_rule(rule)
                && !matches!(rule, TopicRule::NoIdentity)
        });
        let reads: Vec<Option<(String, TopicForm)>> = hits
            .iter()
            .map(|h| read_hit(h, folds, counts, framework, path))
            .collect();
        let topics: Vec<&str> = reads.iter().flatten().map(|(t, _)| t.as_str()).collect();
        if debug_enabled() && !topics.is_empty() {
            eprintln!(
                "[queues] scan needle='{pattern}' rule={rule:?} hits={} path={path} topics={}",
                topics.len(),
                topics.join(",")
            );
        }
        // A2.5 marker — the one place a qname is a SYMBOL, not a broker topic,
        // so it gets its own grep-able line:
        //   GLIA_QUEUE_DEBUG=1 ... 2>&1 | grep '\[queues\] taskq rule='
        if debug_enabled() && is_identity_rule(rule) {
            for sym in &topics {
                eprintln!(
                    "[queues] taskq rule={rule:?} symbol={sym} framework={framework:?} file={path}"
                );
            }
        }
        let no_identity = matches!(rule, TopicRule::NoIdentity);
        let first_span = spans.len();
        for (h, read) in hits.iter().zip(&reads) {
            let named = match read {
                Some((topic, form)) => {
                    let line = queue_topic::line_of(source, h.offset);
                    if record_site(
                        &mut pending,
                        &mut seen,
                        topic,
                        framework,
                        repo,
                        kind,
                        prefix,
                        Confidence::Medium,
                        (line, h.offset),
                    ) {
                        fired_on(pattern, rule, framework, topic, path);
                        cloud_fired_on(framework, topic, *form, path);
                    }
                    let idx = seen.get(&format!("{topic}:{framework:?}")).copied();
                    // LA.33: this site's handler belongs to the node it just joined.
                    if let (Some(hr), Some(id)) =
                        (handler, idx.and_then(|i| pending.get(i)).map(|p| p.id))
                    {
                        let found = handlers_at(source, h.offset, pattern, hr);
                        push_row_callbacks(
                            &mut callbacks,
                            &mut callback_rows,
                            &mut callback_seen,
                            id,
                            found,
                            row,
                        );
                    }
                    idx
                }
                None => None,
            };
            spans.push(HitSpan {
                start: h.offset,
                end: h.offset + pattern.len(),
                needle: pattern,
                rule,
                framework,
                handler,
                named,
                no_identity,
                has_expr: h.expr.is_some(),
                declared: None,
                row,
            });
        }
        // A needle whose rule is `NoIdentity` NEVER names a topic (it is a
        // liveness signal — Go's `r.ReadMessage(ctx)`), so falling back to a
        // topic-less framework tag here would manufacture exactly the all-to-all
        // tag pairing A2.1 removed. Every other rule keeps the fallback, which
        // CL.1 defers until every row has run.
        if topics.is_empty() && !no_identity {
            deferred.push(DeferredTag {
                row,
                needle: pattern,
                rule,
                framework,
                handler,
                slot: pending.len(),
                hits: (first_span..spans.len()).collect(),
            });
        }
    }

    bind_declared_queues(
        DeclareCtx {
            source,
            path,
            repo,
            kind,
            prefix,
            resolve,
            lower: &lower,
        },
        declares,
        &mut DeclareState {
            pending: &mut pending,
            seen: &mut seen,
            spans: &mut spans,
            callbacks: &mut callbacks,
            callback_rows: &mut callback_rows,
            callback_seen: &mut callback_seen,
            guard: &mut guard,
            counts,
        },
    );

    // CL.1: the deferred tags, in row order. `named` is taken BEFORE any tag
    // is recorded, so a tag never explains another.
    let named_len = pending.len();
    for tag in &deferred {
        let named: Vec<usize> = pending
            .iter()
            .take(named_len)
            .enumerate()
            .filter(|(_, p)| p.framework == *tag.framework)
            .map(|(i, _)| i)
            .collect();
        let why: Vec<Option<Explained>> = tag
            .hits
            .iter()
            .map(|&i| explain_hit(&spans, i, &named))
            .collect();
        if why.iter().any(Option::is_none) {
            // A2.3: `Weak`, not `Medium` — the tag proves the framework is
            // live in this file and nothing else. The `seen` key is
            // `{topic}:{framework:?}`, so one tag per (framework, direction)
            // per file however many rows of that framework deferred one. Its
            // site is the row's first occurrence, the one thing that WAS read.
            let label = framework_tag(tag.framework);
            let Some(first) = tag.hits.first().and_then(|&i| spans.get(i)) else {
                continue;
            };
            let site = (queue_topic::line_of(source, first.start), first.start);
            if record_site(
                &mut pending,
                &mut seen,
                &label,
                tag.framework,
                repo,
                kind,
                prefix,
                Confidence::Weak,
                site,
            ) {
                if let Some(p) = pending.last_mut() {
                    p.rank = (tag.slot, 0, tag.row);
                }
                fired_on(tag.needle, tag.rule, tag.framework, &label, path);
            }
            // LA.33: every topic-less occurrence is the sentinel's; its
            // handler is a queue handler whatever the topic.
            if let (Some(h), Some(id)) = (
                tag.handler,
                pending_id(&pending, &seen, &label, tag.framework),
            ) {
                for &i in &tag.hits {
                    let Some(s) = spans.get(i) else { continue };
                    let found = handlers_at(source, s.start, tag.needle, h);
                    push_row_callbacks(
                        &mut callbacks,
                        &mut callback_rows,
                        &mut callback_seen,
                        id,
                        found,
                        tag.row,
                    );
                }
            }
            continue;
        }
        // Every hit is explained: no tag. Each hit's callbacks move to the
        // node that explained it (a declare-bound hit's already did), or are
        // dropped and counted when no single node did.
        let mut dropped = 0usize;
        let mut reasons: Vec<&str> = Vec::new();
        for (&i, why) in tag.hits.iter().zip(why.iter().flatten()) {
            if !reasons.contains(&why.by()) {
                reasons.push(why.by());
            }
            let (Some(h), Some(s)) = (tag.handler, spans.get(i)) else {
                continue;
            };
            let target = match *why {
                Explained::Declare => continue,
                Explained::Overlap(t) | Explained::Named(t) => t,
            };
            let found = handlers_at(source, s.start, tag.needle, h);
            match target.and_then(|t| pending.get(t)).map(|p| p.id) {
                Some(id) => push_row_callbacks(
                    &mut callbacks,
                    &mut callback_rows,
                    &mut callback_seen,
                    id,
                    found,
                    tag.row,
                ),
                None => dropped += found.len(),
            }
        }
        if debug_enabled() {
            let extra = if dropped > 0 {
                format!(" callbacks_dropped={dropped}")
            } else {
                String::new()
            };
            eprintln!(
                "[queues] tag-suppressed needle='{}' framework={:?} side={side} by={} file={path}{extra}",
                tag.needle,
                tag.framework,
                reasons.join(",")
            );
        }
    }

    // CL.1: back into the order the per-row fallback produced — a deferred
    // tag before the nodes created after its row, callbacks by row. Both
    // sorts are stable and keyed on table positions, never on a HashMap.
    if pending.windows(2).any(|w| w[0].rank > w[1].rank) {
        pending.sort_by_key(|p| p.rank);
    }
    if callback_rows.windows(2).any(|w| w[0] > w[1]) {
        let mut rowed: Vec<(usize, ConsumerCallback)> =
            callback_rows.into_iter().zip(callbacks).collect();
        rowed.sort_by_key(|(r, _)| *r);
        callbacks = rowed.into_iter().map(|(_, c)| c).collect();
    }

    // The post-cache const fold re-reads the file through this function with
    // the same path, so it refuses exactly what the per-file pass refused; it
    // stays quiet so each file reports once per parse.
    if resolve.is_none() {
        guard.report(if kind == node_kind::QUEUE_CONSUMER {
            "queue_consumer"
        } else {
            "queue_producer"
        });
    }
    let mut out = finish(pending, source, path, module_id, repo, kind);
    out.callbacks = callbacks;
    out
}

/// CL.1: what [`bind_declared_queues`] reads.
struct DeclareCtx<'a> {
    source: &'a str,
    path: &'a str,
    repo: RepoId,
    kind: glia_core::NodeKindId,
    prefix: &'a str,
    resolve: Option<TopicResolver<'a>>,
    lower: &'a str,
}

/// CL.1: what [`bind_declared_queues`] writes — the emit loop's accumulators.
struct DeclareState<'s, 'p, 'g> {
    pending: &'s mut Vec<Pending>,
    seen: &'s mut std::collections::HashMap<String, usize>,
    spans: &'s mut Vec<HitSpan<'p>>,
    callbacks: &'s mut Vec<ConsumerCallback>,
    callback_rows: &'s mut Vec<usize>,
    callback_seen: &'s mut HashSet<(NodeId, HandlerExpr)>,
    guard: &'s mut LazyGuard<'g>,
    counts: &'s mut ConstFoldCounts,
}

/// CL.1: the [`DECLARE_PATTERNS`] pass. Per framework with a declare row:
/// when the file declares EXACTLY ONE queue (a declaration whose queue reads
/// no name counts too, so `queue_declare(queue='')` beside
/// `queue_declare(queue='orders')` is ambiguous), every unnamed consume of
/// that framework becomes a consumer site of it at the consume call's offset
/// (LE.4c anchors it to the consuming function), its callbacks follow, and
/// its span records the binding, which explains it to the deferred-tag pass.
/// Zero or two-plus declared queues bind nothing.
fn bind_declared_queues(
    ctx: DeclareCtx<'_>,
    declares: &[(&str, QueueFramework, &[&str], TopicRule)],
    st: &mut DeclareState<'_, '_, '_>,
) {
    // (framework, named queues in first-seen order, unnamed declarations).
    let mut declared: Vec<(&QueueFramework, Vec<String>, usize)> = Vec::new();
    for (pattern, framework, signals, rule) in declares {
        let slot = match declared.iter().position(|(f, _, _)| *f == framework) {
            Some(i) => i,
            None => {
                declared.push((framework, Vec::new(), 0));
                declared.len() - 1
            }
        };
        if !ctx.source.contains(pattern) || !signals_present(ctx.lower, signals) {
            continue;
        }
        let folds = ctx
            .resolve
            .filter(|_| !is_identity_rule(rule) && !matches!(rule, TopicRule::NoIdentity));
        for h in queue_topic::scan_guarded(ctx.source, pattern, *rule, st.guard) {
            let read = read_hit(&h, folds, st.counts, framework, ctx.path);
            let Some((_, queues, unnamed)) = declared.get_mut(slot) else {
                continue;
            };
            match read {
                Some((q, _)) if !queues.contains(&q) => queues.push(q),
                Some(_) => {}
                None => *unnamed += 1,
            }
        }
    }
    for (framework, queues, unnamed) in &declared {
        let consumes: Vec<usize> = (0..st.spans.len())
            .filter(|&i| {
                st.spans.get(i).is_some_and(|s| {
                    s.framework == *framework && s.named.is_none() && !s.no_identity
                }) && !matches!(overlap_of(st.spans, i), Some(Some(_)))
            })
            .collect();
        if consumes.is_empty() {
            continue;
        }
        let count = queues.len() + unnamed;
        let queue = match queues.as_slice() {
            [q] if count == 1 => q,
            _ => {
                if debug_enabled() {
                    eprintln!(
                        "[queues] declare-unbound declared={count} framework={framework:?} file={}",
                        ctx.path
                    );
                }
                continue;
            }
        };
        let mut per_needle: Vec<(&str, usize)> = Vec::new();
        for i in consumes {
            let Some(s) = st.spans.get(i) else { continue };
            let (start, needle, rule, handler, row) = (s.start, s.needle, s.rule, s.handler, s.row);
            let line = queue_topic::line_of(ctx.source, start);
            if record_site(
                st.pending,
                st.seen,
                queue,
                framework,
                ctx.repo,
                ctx.kind,
                ctx.prefix,
                Confidence::Medium,
                (line, start),
            ) {
                fired_on(needle, rule, framework, queue, ctx.path);
            }
            let idx = st.seen.get(&format!("{queue}:{framework:?}")).copied();
            if let Some(s) = st.spans.get_mut(i) {
                s.declared = idx;
            }
            if let (Some(h), Some(id)) =
                (handler, idx.and_then(|x| st.pending.get(x)).map(|p| p.id))
            {
                let found = handlers_at(ctx.source, start, needle, h);
                push_row_callbacks(
                    st.callbacks,
                    st.callback_rows,
                    st.callback_seen,
                    id,
                    found,
                    row,
                );
            }
            match per_needle.iter_mut().find(|(n, _)| *n == needle) {
                Some((_, k)) => *k += 1,
                None => per_needle.push((needle, 1)),
            }
        }
        if debug_enabled() {
            for (needle, k) in per_needle {
                eprintln!(
                    "[queues] declare-bound queue={queue} framework={framework:?} consume='{needle}' consumes={k} file={}",
                    ctx.path
                );
            }
        }
    }
}

/// The id [`record_site`] gave (topic, framework) in this file.
fn pending_id(
    pending: &[Pending],
    seen: &std::collections::HashMap<String, usize>,
    topic: &str,
    framework: &QueueFramework,
) -> Option<NodeId> {
    let idx = *seen.get(&format!("{topic}:{framework:?}"))?;
    pending.get(idx).map(|p| p.id)
}

/// Record one call site for a (topic, framework).
///
/// Returns true only the FIRST time that pair is seen in this file, so
/// [`fired_on`] still fires once per NODE rather than once per site. Every
/// later occurrence merges its line into the existing entry.
#[allow(clippy::too_many_arguments)]
fn record_site(
    pending: &mut Vec<Pending>,
    seen: &mut std::collections::HashMap<String, usize>,
    topic: &str,
    framework: &QueueFramework,
    repo: RepoId,
    kind: glia_core::NodeKindId,
    prefix: &str,
    confidence: Confidence,
    (line, offset): (usize, usize),
) -> bool {
    let key = format!("{topic}:{framework:?}");
    // LE.4c: every site anchors, capped or not. `line` is the site's
    // POSITION / CODE line, the count of `\n` before `offset`.
    let anchor_line = u32::try_from(line).unwrap_or(u32::MAX);
    if let Some(&idx) = seen.get(&key) {
        let p = &mut pending[idx];
        p.anchor_lines.push(anchor_line);
        if p.lines.len() < MAX_SITES && !p.lines.contains(&line) {
            p.lines.push(line);
            p.offsets.push(offset);
        }
        return false;
    }
    let qname = format!("{prefix}{topic}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
    seen.insert(key, pending.len());
    pending.push(Pending {
        id,
        topic: topic.to_string(),
        qname,
        framework: framework.clone(),
        confidence,
        lines: vec![line],
        offsets: vec![offset],
        anchor_lines: vec![anchor_line],
        rank: (pending.len(), 1, 0),
    });
    true
}

/// Turn the per-file accumulation into nodes + cells + module edges.
///
/// fired_on marker (A2.8):
///   `GLIA_QUEUE_DEBUG=1 ... 2>&1 | grep '\[queues\] position sites='`
///
/// A12.1: `source` is the file text, read once more per node for the
/// MESSAGE_TYPE cell (see [`message_type_cell`]).
fn finish(
    pending: Vec<Pending>,
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    kind: glia_core::NodeKindId,
) -> QueueNodes {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut anchors = Vec::new();
    let mut nav = CodeNav::default();
    let file = queue_topic::escape_json(path);

    for p in pending {
        // POSITION names the FIRST site. Every reader of a POSITION cell
        // (`locate_node`, `position_file`, `projection_text::node_position`)
        // takes the first one, and after `merge_parses` a topic published from
        // two files carries one POSITION cell PER FILE — so "first" has to mean
        // something stable, and it does: earliest site in the earliest-parsed
        // file that publishes the topic.
        let first = p.lines.first().copied().unwrap_or(0);
        let sites = p
            .lines
            .iter()
            .map(|l| format!(r#"{{"file":"{file}","line":{l}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        // CODE, not a new CellTypeId: the payload is JSON, and
        // `projection_text::extract_code_cell` only reads `CellPayload::Text`,
        // so this never leaks into dense text as if it were source. `cron.rs`
        // sets the same precedent.
        let code = format!(
            r#"{{"framework":"{:?}","family":"{}","sites":[{sites}]}}"#,
            p.framework,
            p.framework.family()
        );
        if debug_enabled() {
            eprintln!(
                "[queues] position sites={} first_line={first} qname={} file={path}",
                p.lines.len(),
                p.qname
            );
        }
        let mut cells = vec![
            Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(
                    r#"{{"file":"{file}","start_line":{first},"end_line":{first}}}"#
                )),
            },
            Cell {
                kind: cell_type::CODE,
                payload: CellPayload::Json(code),
            },
        ];
        // A12.1: appended AFTER the A2.8 pair, so every "first POSITION"
        // reader is unaffected.
        cells.extend(message_type_cell(source, &p.offsets));
        nodes.push(Node {
            id: p.id,
            repo,
            confidence: p.confidence,
            cells,
        });
        nav.record(p.id, &p.topic, &p.qname, kind, Some(module_id));
        edges.push(Edge {
            from: module_id,
            to: p.id,
            category: edge_category::CONTAINS,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        anchors.extend(
            p.anchor_lines
                .iter()
                .map(|&line| Anchor { node: p.id, line }),
        );
    }

    QueueNodes {
        nodes,
        edges,
        nav,
        anchors,
        callbacks: Vec::new(),
    }
}

/// True for the A2.5 rules that read a task SYMBOL rather than a broker topic.
fn is_identity_rule(rule: &TopicRule) -> bool {
    matches!(
        rule,
        TopicRule::Receiver
            | TopicRule::ArgReceiver
            | TopicRule::DeclaredSymbol
            | TopicRule::EnclosingSymbol
    )
}

/// Identity-free fallback when no occurrence of a needle named a topic.
///
/// BREAKING (A2.3): prefixed with [`UNRESOLVED_PREFIX`], so the tag is
/// structurally unpairable — `queue_producer:unresolved:kafka`, not
/// `queue_producer:kafka`. The bare spelling was indistinguishable from a repo
/// that genuinely publishes to a topic literally named `kafka`, which is what
/// let the resolver pair it. [`QueueFramework::family`] is deliberately NOT used
/// for the suffix — it folds `RedisList`/`RedisPubSub` together and would
/// silently merge two distinct coverage signals.
fn framework_tag(f: &QueueFramework) -> String {
    format!("{UNRESOLVED_PREFIX}{}", format!("{f:?}").to_lowercase())
}

/// A12.1: is `topic` (the part of a queue qname after `queue_producer:` /
/// `queue_consumer:`) a [`framework_tag`] rather than a real topic?
///
/// The packet spec asked for a hand-listed `FRAMEWORK_TAGS` table. Since A2.3
/// every tag carries [`UNRESOLVED_PREFIX`], which no legal topic can
/// (`unresolved_prefix_is_not_a_legal_topic_shape`), so the prefix IS the
/// test — the same one `QueueStackResolver` uses — and there is no list that
/// could drift from the enum. A12.2 calls this rather than re-deriving it.
pub fn is_framework_tag(topic: &str) -> bool {
    topic.starts_with(UNRESOLVED_PREFIX)
}

// ---- LA.33: consumer callbacks -> HANDLED_BY the handler ------------------
// A QUEUE_CONSUMER was HANDLED_BY only the function holding its subscribe call
// (LE.4c's owner edge). The handler a consumer passes as a callback -
// `consumer.run({ eachMessage: onPayment })`, `channel.consume(q, onOrder)`,
// `nc.Subscribe(subj, onOrder)` - was read by no one: the needle already found
// the call and `queue_topic` already split its arguments, but the handler
// argument was thrown away, so no path ran from a consumer into the code that
// processes its messages.
//
// PARSERS EXTRACT, THE GRAPH RESOLVES. The extractor records WHICH expression
// is the handler ([`HandlerExpr`]); [`bind_consumer_callbacks`] turns a plain
// or member name into a HANDLED_BY `UnresolvedRef` that the graph builder's
// `resolve_refs` binds through the file's import bindings, the module's
// symbols and its unique-global HANDLED_BY fallbacks (the Go ROUTE handler
// precedent), so a handler imported from another file resolves and nothing
// cross-file is guessed here. Only `this.x` / `self.x`, and a method value on
// the enclosing method's own type (`w.handle` inside `(w *Worker) Start`), are
// bound in-file: the enclosing class is a fact of this file.
//
// fired_on (per file, printed by the engine):
//   `... 2>&1 | grep '\[queue-callbacks\] consumers='`
// ---------------------------------------------------------------------------

/// Where a consumer needle's handler sits. Keyed by the SAME needle strings as
/// [`CONSUMER_PATTERNS`] (`handler_rules_name_consumer_needles` asserts it).
#[derive(Debug, Clone, Copy)]
enum HandlerRule {
    /// Positional argument N of the needle's call.
    ArgIndex(usize),
    /// `key: value` / `key=value` in the call's argument region (a top-level
    /// keyword argument, or a member of a top-level object argument; keys
    /// tried in order), falling back to positional argument N.
    KeyedOrArg(&'static [&'static str], usize),
    /// kafkajs: the handler is not an argument of `subscribe` but of the SAME
    /// receiver's `run({ eachMessage })`, anywhere in the file.
    RunCallback(&'static [&'static str]),
}

const HANDLER_RULES: &[(&str, HandlerRule)] = &[
    // kafkajs: `consumer.subscribe({ topic })` + `consumer.run({ eachMessage })`.
    ("consumer.subscribe", HandlerRule::RunCallback(&["eachMessage", "eachBatch"])),
    // amqplib: `channel.consume(queue, onMessage, options)`.
    ("channel.consume", HandlerRule::ArgIndex(1)),
    // BullMQ: `new Worker(queue, processor, options)`.
    ("new Worker(", HandlerRule::ArgIndex(1)),
    // nats.js v2 `nc.subscribe(subject, { callback: (err, msg) => .. })`; v1
    // passed the callback positionally, `nc.subscribe(subject, onMsg)`.
    ("nc.subscribe", HandlerRule::KeyedOrArg(&["callback"], 1)),
    // nats.go: `nc.Subscribe(subj, cb)`, `nc.QueueSubscribe(subj, queue, cb)`.
    ("nc.Subscribe", HandlerRule::ArgIndex(1)),
    ("nc.QueueSubscribe", HandlerRule::ArgIndex(2)),
    // pika 1.x: `basic_consume(queue, on_message_callback, ...)`, keyword or positional.
    ("basic_consume(", HandlerRule::KeyedOrArg(&["on_message_callback"], 1)),
];

fn handler_rule(needle: &str) -> Option<HandlerRule> {
    HANDLER_RULES
        .iter()
        .find(|(n, _)| *n == needle)
        .map(|(_, r)| *r)
}

/// The handler expression a consumer passes, as written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HandlerExpr {
    /// `onOrder` - a function name (local, imported or package-level).
    Name(String),
    /// `handlers.onOrder`, Go's method value `w.handle`.
    Member { base: String, name: String },
    /// `this.handle` / `self.on_message` (a trailing `.bind(..)` stripped).
    SelfMember(String),
}

/// One consumer's handler, read at a call site of that consumer's needle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerCallback {
    pub consumer: NodeId,
    /// 0-indexed line of the expression naming the handler (the needle's
    /// line, or the kafkajs `run(` call's).
    pub line: u32,
    pub handler: HandlerExpr,
    /// The handler was the one call inside an inline function
    /// (`(job) => sendEmail(job)`), not a reference.
    pub inline: bool,
}

/// What [`bind_consumer_callbacks`] did with one file's callbacks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CallbackStats {
    /// Distinct consumers that had at least one callback.
    pub consumers: usize,
    /// Callbacks bound in-file to a METHOD (`this.x`, a method value).
    pub direct: usize,
    /// Callbacks handed to the graph builder as HANDLED_BY `UnresolvedRef`s.
    pub refs: usize,
    /// The share of `direct + refs` read out of a one-call inline function.
    pub inline: usize,
    /// `this.x` / method values with no such method on the enclosing type.
    pub unbound: usize,
}

impl CallbackStats {
    pub fn bound(&self) -> usize {
        self.direct + self.refs
    }

    /// The per-file fired_on line, or `None` when the file had no callback.
    pub fn marker(&self, path: &str) -> Option<String> {
        (self.consumers > 0).then(|| {
            format!(
                "[queue-callbacks] consumers={} bound={} (direct={} refs={} inline={}) unbound={} path={path}",
                self.consumers,
                self.bound(),
                self.direct,
                self.refs,
                self.inline,
                self.unbound
            )
        })
    }
}

/// Bytes of a receiver chain / callee: `consumer`, `this.consumer`, `w.handle`.
fn is_chain_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'.'
}

/// Words that are never a handler name on their own.
const NOT_A_HANDLER: &[&str] = &[
    "this", "self", "super", "null", "undefined", "nil", "None", "true", "false", "True",
    "False", "function", "async", "await", "return", "lambda", "func", "new",
];

/// One identifier: `[A-Za-z_$][A-Za-z0-9_$]*`.
fn is_ident(s: &str) -> bool {
    let b = s.as_bytes();
    matches!(b.first(), Some(c) if c.is_ascii_alphabetic() || *c == b'_' || *c == b'$')
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
}

/// `word` followed by a non-identifier byte (or the end) at the start of `s`;
/// returns the rest after it.
fn strip_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(word)?;
    match rest.as_bytes().first() {
        Some(c) if c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$' => None,
        _ => Some(rest),
    }
}

/// The body of a bracket opened just before `s`, when its close is really
/// there (an unterminated or over-long region is `None`).
fn closed_region(s: &str) -> Option<&str> {
    let inner = queue_topic::region_body(s)?;
    let close = s.as_bytes().get(inner.len())?;
    (matches!(close, b')' | b'}' | b']') && inner.len() < queue_topic::MAX_REGION)
        .then_some(inner)
}

/// `handler.bind(anything)` -> `handler`; anything else unchanged.
fn strip_bind(t: &str) -> &str {
    let Some(at) = t.rfind(".bind(") else {
        return t;
    };
    let Some(rest) = t.get(at + ".bind(".len()..) else {
        return t;
    };
    match closed_region(rest) {
        Some(inner) if inner.len() + 1 == rest.len() => t.get(..at).map_or(t, str::trim_end),
        _ => t,
    }
}

/// A name or a one-dot member chain as a [`HandlerExpr`].
fn reference(t: &str) -> Option<HandlerExpr> {
    let t = strip_bind(t.trim());
    let mut parts = t.split('.');
    let first = parts.next()?;
    let second = parts.next();
    if parts.next().is_some() || !is_ident(first) {
        return None;
    }
    let Some(name) = second else {
        return (!NOT_A_HANDLER.contains(&first)).then(|| HandlerExpr::Name(first.to_string()));
    };
    if !is_ident(name) || NOT_A_HANDLER.contains(&name) {
        return None;
    }
    if first == "this" || first == "self" {
        return Some(HandlerExpr::SelfMember(name.to_string()));
    }
    (!NOT_A_HANDLER.contains(&first)).then(|| HandlerExpr::Member {
        base: first.to_string(),
        name: name.to_string(),
    })
}

/// The callee of `s` when `s` is exactly ONE call statement:
/// `[return] [await] callee(args)[;]`. A chained call (`f(x).then(g)`) or a
/// second statement leaves text after the call and is `None`.
fn single_call(s: &str) -> Option<&str> {
    let mut s = s.trim();
    if let Some(rest) = strip_word(s, "return") {
        s = rest.trim_start();
    }
    if let Some(rest) = strip_word(s, "await") {
        s = rest.trim_start();
    }
    let b = s.as_bytes();
    let len = b.iter().take_while(|c| is_chain_byte(**c)).count();
    if len == 0 || b.get(len) != Some(&b'(') {
        return None;
    }
    let args = s.get(len + 1..)?;
    let inner = closed_region(args)?;
    let rest = args.get(inner.len() + 1..)?;
    rest.trim()
        .trim_end_matches(';')
        .trim()
        .is_empty()
        .then(|| s.get(..len))
        .flatten()
}

/// The single call inside a block body `{ .. }` that ends the text.
fn block_call(s: &str) -> Option<&str> {
    let s = s.trim_start().strip_prefix('{')?;
    let inner = closed_region(s)?;
    let rest = s.get(inner.len() + 1..)?;
    if !rest.trim().is_empty() {
        return None;
    }
    single_call(inner)
}

/// After a parameter list's `(`: the text following its matching `)`.
fn after_params(s: &str) -> Option<&str> {
    let s = s.strip_prefix('(')?;
    let inner = closed_region(s)?;
    s.get(inner.len() + 1..)
}

/// The callee of an inline function whose body is exactly one call: JS
/// arrows (`[async] (p) => [await] f(p)`, `p => f(p)`, block bodies with one
/// `[await|return] f(p);`), JS `function (p) { f(p); }`, a Go func literal
/// `func(m *nats.Msg) { f(m) }` and a Python `lambda ch, m, p, b: f(m)`.
fn inline_callee(t: &str) -> Option<&str> {
    let t = t.trim();
    // Python lambda: params end at the first `:`.
    if let Some(rest) = strip_word(t, "lambda") {
        let colon = rest.find(':')?;
        return single_call(rest.get(colon + 1..)?);
    }
    let t = strip_word(t, "async").map_or(t, str::trim_start);
    // JS `function [name] (params) { .. }`.
    if let Some(rest) = strip_word(t, "function") {
        let rest = rest.trim_start().trim_start_matches('*').trim_start();
        let name_len = rest
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
            .count();
        let rest = rest.get(name_len..)?.trim_start();
        return block_call(after_params(rest)?);
    }
    // Go `func(params) { .. }`.
    if let Some(rest) = strip_word(t, "func") {
        return block_call(after_params(rest.trim_start())?);
    }
    // JS arrow.
    let rest = if t.starts_with('(') {
        after_params(t)?
    } else {
        let n = t
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
            .count();
        if n == 0 {
            return None;
        }
        t.get(n..)?
    };
    let body = rest.trim_start().strip_prefix("=>")?.trim_start();
    if body.starts_with('{') {
        block_call(body)
    } else {
        single_call(body)
    }
}

/// The handler an argument / value expression names, and whether it came out
/// of a one-call inline function. `None` for anything else (multi-statement
/// bodies, chained calls, computed values): LE.4c's owner edge then stays the
/// consumer's only link.
fn handler_expr(text: &str) -> Option<(HandlerExpr, bool)> {
    if let Some(h) = reference(text) {
        return Some((h, false));
    }
    inline_callee(text)
        .and_then(reference)
        .map(|h| (h, true))
}

/// [`queue_topic::split_args`], with a Python lambda's own commas put back:
/// `f(q, lambda ch, m, p, b: g(m))` has two arguments, not five.
fn call_args(region: &str) -> Vec<&str> {
    let raw = queue_topic::split_args(region);
    let base = region.as_ptr() as usize;
    let span = |a: &str| {
        let start = (a.as_ptr() as usize).saturating_sub(base);
        (start, start + a.len())
    };
    let opens_lambda = |a: &str| {
        let v = a.trim_start();
        let v = v.find('=').map_or(v, |eq| v.get(eq + 1..).unwrap_or(v).trim_start());
        strip_word(v, "lambda").is_some_and(|rest| !rest.contains(':'))
    };
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        let (start, mut end) = span(raw[i]);
        if opens_lambda(raw[i]) {
            let mut j = i + 1;
            while j < raw.len() {
                end = span(raw[j]).1;
                j += 1;
                if raw[j - 1].contains(':') {
                    break;
                }
            }
            i = j;
        } else {
            i += 1;
        }
        if let Some(arg) = region.get(start..end) {
            out.push(arg);
        }
    }
    out
}

/// The value of `key: value` / `key=value` when `member` is that member
/// (`'key': value` too). Keys compare ASCII-case-insensitively, like
/// `queue_topic`'s keyed lookup; `==`, `=>` and `::` are not separators.
fn member_value<'a>(member: &'a str, key: &str) -> Option<&'a str> {
    let m = member.trim();
    let (m, quote) = match m.as_bytes().first() {
        Some(q @ (b'\'' | b'"' | b'`')) => (m.get(1..)?, Some(*q)),
        _ => (m, None),
    };
    let head = m.get(..key.len())?;
    if !head.eq_ignore_ascii_case(key) {
        return None;
    }
    let mut rest = m.get(key.len()..)?;
    match quote {
        Some(q) => rest = rest.strip_prefix(q as char)?,
        None => {
            if rest
                .as_bytes()
                .first()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
            {
                return None;
            }
        }
    }
    let rest = rest.trim_start();
    let b = rest.as_bytes();
    let sep_ok = match b.first() {
        Some(b':') => b.get(1) != Some(&b':'),
        Some(b'=') => !matches!(b.get(1), Some(b'=' | b'>')),
        _ => false,
    };
    sep_ok.then(|| rest.get(1..)).flatten().map(str::trim)
}

/// The value keyed by one of `keys` in a call's argument region: a top-level
/// keyword argument, or a member of a top-level object argument.
fn keyed_value<'a>(region: &'a str, keys: &[&str]) -> Option<&'a str> {
    let args = call_args(region);
    for key in keys {
        for arg in &args {
            if let Some(v) = member_value(arg, key) {
                return Some(v);
            }
            let Some(obj) = arg.trim().strip_prefix('{') else {
                continue;
            };
            let Some(inner) = closed_region(obj) else {
                continue;
            };
            if let Some(v) = call_args(inner).iter().find_map(|m| member_value(m, key)) {
                return Some(v);
            }
        }
    }
    None
}

/// Every `(line, handler, inline)` the call at `offset` names under `rule`.
fn handlers_at(
    source: &str,
    offset: usize,
    needle: &str,
    rule: HandlerRule,
) -> Vec<(u32, HandlerExpr, bool)> {
    let after = offset.saturating_add(needle.len());
    let line = anchor::line_of(source, offset);
    let region = || queue_topic::arg_region(source, after, needle);
    let arg = |n: usize| region().and_then(|r| call_args(r).get(n).copied());
    let read = match rule {
        HandlerRule::ArgIndex(n) => arg(n),
        HandlerRule::KeyedOrArg(keys, n) => region()
            .and_then(|r| keyed_value(r, keys))
            .or_else(|| arg(n)),
        HandlerRule::RunCallback(keys) => return run_callbacks(source, offset, needle, keys),
    };
    read.and_then(handler_expr)
        .map(|(h, inline)| vec![(line, h, inline)])
        .unwrap_or_default()
}

/// kafkajs: the receiver chain before the needle's last `.` (`consumer`,
/// `this.consumer`), then every `<chain>.run(` in the file whose chain starts
/// at an identifier boundary - each `eachMessage` / `eachBatch` value.
fn run_callbacks(
    source: &str,
    offset: usize,
    needle: &str,
    keys: &[&str],
) -> Vec<(u32, HandlerExpr, bool)> {
    let mut out = Vec::new();
    let b = source.as_bytes();
    // The needle's own receiver (`consumer` of `consumer.subscribe`) plus
    // whatever chain precedes it (`this.`).
    let Some(dot) = needle.rfind('.').map(|d| offset + d) else {
        return out;
    };
    let mut start = dot;
    while start > 0 && b.get(start - 1).is_some_and(|c| is_chain_byte(*c)) {
        start -= 1;
    }
    let Some(chain) = source.get(start..dot).filter(|c| !c.is_empty()) else {
        return out;
    };
    let run = format!("{chain}.run(");
    for (at, _) in source
        .match_indices(&run)
        .take(queue_topic::MAX_HITS_PER_NEEDLE)
    {
        if at > 0 && b.get(at - 1).is_some_and(|c| is_chain_byte(*c)) {
            continue;
        }
        let Some(region) = queue_topic::arg_region(source, at + run.len(), "run(") else {
            continue;
        };
        let line = anchor::line_of(source, at);
        for key in keys {
            if let Some((h, inline)) = keyed_value(region, &[key]).and_then(handler_expr) {
                out.push((line, h, inline));
            }
        }
    }
    out
}

/// Record `found` for `consumer`, deduped per (consumer, handler), first
/// site wins.
fn push_callbacks(
    callbacks: &mut Vec<ConsumerCallback>,
    seen: &mut HashSet<(NodeId, HandlerExpr)>,
    consumer: NodeId,
    found: Vec<(u32, HandlerExpr, bool)>,
) {
    for (line, handler, inline) in found {
        if seen.insert((consumer, handler.clone())) {
            callbacks.push(ConsumerCallback {
                consumer,
                line,
                handler,
                inline,
            });
        }
    }
}

/// Is `name` a name this file's imports bind (an alias, an imported symbol,
/// a whole module's first or last path segment)? Over-approximate on purpose:
/// a name that might be an import goes to the graph's resolver as a ref.
fn import_binds(fp: &FileParse, name: &str) -> bool {
    use glia_code_domain::ImportTarget;
    fp.imports.iter().any(|i| match &i.target {
        ImportTarget::Symbol {
            name: sym, alias, ..
        } => alias.as_deref().unwrap_or(sym) == name,
        ImportTarget::Module { path, alias } => match alias {
            Some(a) => a == name,
            None => {
                let segs: Vec<&str> = path
                    .split(['/', ':', '.', '\\', '"', '\''])
                    .filter(|s| !s.is_empty())
                    .collect();
                segs.first() == Some(&name)
                    || path
                        .rsplit(['/', ':', '\\'])
                        .next()
                        .and_then(|last| last.split('.').next())
                        == Some(name)
            }
        },
    })
}

/// Parent kinds a `this.x` / method value resolves against.
const OWNER_TYPES: &[NodeKindId] = &[node_kind::CLASS, node_kind::STRUCT];

/// The METHOD `name` of the type enclosing the function that holds `line`:
/// the innermost METHOD / FUNCTION span, then up its nav parents to the first
/// CLASS / STRUCT, then that type's METHOD child named `name`.
fn own_method(fp: &FileParse, idx: &anchor::OwnerIndex, line: u32, name: &str) -> Option<NodeId> {
    let mut at = anchor::owner_of_line(idx, line)?;
    let mut ty = None;
    for _ in 0..8 {
        let parent = *fp.nav.parent_of.get(&at)?;
        if fp
            .nav
            .kind_by_id
            .get(&parent)
            .is_some_and(|k| OWNER_TYPES.contains(k))
        {
            ty = Some(parent);
            break;
        }
        at = parent;
    }
    fp.nav.children_of.get(&ty?)?.iter().copied().find(|c| {
        fp.nav.kind_by_id.get(c) == Some(&node_kind::METHOD)
            && fp.nav.name_by_id.get(c).is_some_and(|n| n == name)
    })
}

/// Bind one file's consumer callbacks (LA.33).
///
/// - `this.x` / `self.x` -> `consumer -HANDLED_BY-> <enclosing type>::x`, direct;
/// - `base.name` where `base` is not an import binding and the enclosing
///   type declares a METHOD `name` (Go's method value `w.handle`) -> the same
///   direct edge; any other member -> a HANDLED_BY `UnresolvedRef`
///   `Attribute { base, name }`;
/// - a name -> a HANDLED_BY `UnresolvedRef` `Bare(name)`, which the graph
///   builder binds through the import bindings, the module's symbols, then
///   the unique-global HANDLED_BY fallback.
///
/// Direct edges carry `extractor:queue_callbacks` EVIDENCE (rule `self` or
/// `method_value`) at the callback's line in the module's file. Edges dedupe
/// by (from, to, category) and refs by (from, qualifier, category) against
/// what `fp` already holds, so a second call (the post-cache const fold
/// re-binding a same-id consumer) never doubles one. Output order is callback
/// order.
pub fn bind_consumer_callbacks(
    fp: &mut FileParse,
    module_id: NodeId,
    callbacks: &[ConsumerCallback],
) -> CallbackStats {
    use glia_code_domain::{CallQualifier, UnresolvedRef};
    let mut stats = CallbackStats::default();
    if callbacks.is_empty() {
        return stats;
    }
    let idx = anchor::build_owner_index(&fp.nodes, &fp.nav);
    let file = fp
        .nodes
        .iter()
        .find(|n| n.id == module_id)
        .and_then(|n| evidence::locate(&n.cells))
        .map(|(f, _)| f);
    let mut edges: HashSet<(NodeId, NodeId)> = fp
        .edges
        .iter()
        .filter(|e| e.category == edge_category::HANDLED_BY)
        .map(|e| (e.from, e.to))
        .collect();
    let mut consumers: HashSet<NodeId> = HashSet::new();
    for cb in callbacks {
        consumers.insert(cb.consumer);
        let direct = match &cb.handler {
            HandlerExpr::SelfMember(x) => own_method(fp, &idx, cb.line, x).map(|m| (m, "self")),
            HandlerExpr::Member { base, name } if !import_binds(fp, base) => {
                own_method(fp, &idx, cb.line, name).map(|m| (m, "method_value"))
            }
            _ => None,
        };
        let qualifier = match (&cb.handler, direct) {
            (_, Some((to, rule))) => {
                if edges.insert((cb.consumer, to)) {
                    let ev = evidence::Evidence::emitter("extractor:queue_callbacks").rule(rule);
                    let ev = match &file {
                        Some(f) => ev.at(f.clone(), cb.line),
                        None => ev.line(cb.line),
                    };
                    fp.edges.push(
                        Edge::new(cb.consumer, to, edge_category::HANDLED_BY, Confidence::Medium)
                            .with_cell(ev.to_cell()),
                    );
                }
                stats.direct += 1;
                if cb.inline {
                    stats.inline += 1;
                }
                continue;
            }
            (HandlerExpr::SelfMember(_), None) => {
                stats.unbound += 1;
                continue;
            }
            (HandlerExpr::Member { base, name }, None) => CallQualifier::Attribute {
                base: base.clone(),
                name: name.clone(),
            },
            (HandlerExpr::Name(n), None) => CallQualifier::Bare(n.clone()),
        };
        let dup = fp.refs.iter().any(|r| {
            r.from == cb.consumer
                && r.category == edge_category::HANDLED_BY
                && r.qualifier == qualifier
        });
        if !dup {
            fp.refs.push(UnresolvedRef {
                from: cb.consumer,
                from_module: module_id,
                qualifier,
                category: edge_category::HANDLED_BY,
                line: cb.line,
            });
        }
        stats.refs += 1;
        if cb.inline {
            stats.inline += 1;
        }
    }
    stats.consumers = consumers.len();
    stats
}

// ---- A12.1: MESSAGE_TYPE cell --------------------------------------------
// A queue node pairs on its topic string and nothing else, so glia could not
// tell a producer sending `OrderCreated` from a consumer parsing
// `OrderPlaced`. Each node now carries the payload type read off the source
// around its call sites, as a `cell_type::MESSAGE_TYPE` JSON cell, keys in
// this fixed order:
//
//   {"type":"OrderCreated","raw":"pb.OrderCreated",
//    "form":"generic|struct_literal","window":"near|file"}
//
// `type` is the normalised simple name, so Go `pb.OrderCreated`, C#
// `Events.OrderCreated` and Java `OrderCreated` compare equal; `raw` is the
// text as written. The cell names a TYPE; `node_kind::MESSAGE_TYPE` (A10.5) is
// the separate node a `.proto` declares, and nothing here links the two.
// ---------------------------------------------------------------------------

/// Bytes scanned before / after a call site.
const MSG_BACK: usize = 600;
const MSG_FWD: usize = 400;
/// A generic argument list longer than this is not a type (sanity cap).
const MAX_TYPE_ARG: usize = 512;
const MAX_TYPE_NAME: usize = 64;

/// Broker envelope types whose LAST type argument is the payload (the Kafka
/// `<Key, Value>` convention). The `<` is part of the needle so a bare
/// `Message` identifier never matches, and [`scan_generic`] adds a left word
/// boundary so `IMessage<` / `BrokeredMessage<` do not either.
const GENERIC_WRAPPERS: &[&str] = &[
    "Message<",
    "ConsumerRecord<",
    "ProducerRecord<",
    "ConsumeResult<",
    "KafkaTemplate<",
    "KafkaProducer<",
    "KafkaConsumer<",
    "IProducer<",
    "IConsumer<",
    "ProducerBuilder<",
    "ConsumerBuilder<",
];

/// Go packages whose struct literals are broker / stdlib plumbing, never a
/// payload: `&kafka.Message{Topic: ...}` must not be read as the message type.
/// Heuristic — extend as fixtures accumulate.
const GO_PKG_DENY: &[&str] = &[
    "nats", "jetstream", "kafka", "sarama", "amqp", "amqp091", "mqtt", "sqs", "sns", "pubsub",
    "azservicebus", "aws", "types", "redis", "bytes", "http", "sync", "time", "strings", "errors",
    "sql", "json", "url", "os", "io", "fmt", "context", "tls", "log", "slog",
];

/// Envelope / client type names that are never the payload, whatever package.
const GO_TYPE_DENY: &[&str] = &[
    "Message", "Msg", "ProducerMessage", "ConsumerMessage", "Publishing", "Delivery", "Header",
    "Reader", "Writer", "Config", "Client", "Conn", "Options", "Server", "Request", "Response",
];

/// The payload type found near a queue call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageType {
    /// Normalised simple name — the cross-language join key.
    pub simple: String,
    /// The type text as written (`pb.OrderCreated`, `Map<String, Item>`).
    pub raw: String,
    /// `"generic"` or `"struct_literal"`.
    pub form: &'static str,
    /// `"near"` (within the call-site window) or `"file"` (whole-file
    /// fallback, generic form only — consumers should weigh it lower).
    pub window: &'static str,
}

/// A scanner hit: (byte position, raw text, simple name).
type TypeHit = (usize, String, String);

/// Payload type near the FIRST occurrence of `pattern` in `source` — the
/// single-needle entry point; the emit loop uses every recorded site.
pub fn extract_message_type_near(source: &str, pattern: &str) -> Option<MessageType> {
    let at = source.find(pattern)?;
    message_type_at(source, &[at])
}

/// Payload type for a node whose call sites sit at byte `anchors` (first-seen
/// order). Each site's window is tried in turn, generic before struct literal;
/// within a window the hit NEAREST the call site wins, so a file publishing
/// two topics gives each its own type. Only then the whole file, generic form
/// only: a Go struct literal anywhere in a file (`&cobra.Command{`) is far too
/// weak a signal without a call site beside it.
fn message_type_at(source: &str, anchors: &[usize]) -> Option<MessageType> {
    let found = |hit: TypeHit, form: &'static str, window: &'static str| MessageType {
        simple: hit.2,
        raw: hit.1,
        form,
        window,
    };
    for &at in anchors {
        let at = at.min(source.len());
        let mut lo = at.saturating_sub(MSG_BACK);
        while lo > 0 && !source.is_char_boundary(lo) {
            lo -= 1;
        }
        let mut hi = at.saturating_add(MSG_FWD).min(source.len());
        while hi < source.len() && !source.is_char_boundary(hi) {
            hi += 1;
        }
        let Some(s) = source.get(lo..hi) else {
            continue;
        };
        let rel = at - lo; // lo <= at by construction
        if let Some(hit) = nearest(scan_generic(s), rel) {
            return Some(found(hit, "generic", "near"));
        }
        if let Some(hit) = nearest(scan_go_struct(s), rel) {
            return Some(found(hit, "struct_literal", "near"));
        }
    }
    let at = anchors.first().copied().unwrap_or(0);
    nearest(scan_generic(source), at).map(|hit| found(hit, "generic", "file"))
}

/// The hit closest to `anchor`; ties go to the earlier one, so the answer is a
/// function of the text alone, never of scan order.
fn nearest(hits: Vec<TypeHit>, anchor: usize) -> Option<TypeHit> {
    hits.into_iter().min_by_key(|h| (h.0.abs_diff(anchor), h.0))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Every `Wrapper<..., T>` in `s`, as (position, raw `T`, simple `T`).
fn scan_generic(s: &str) -> Vec<TypeHit> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    for w in GENERIC_WRAPPERS {
        for (i, _) in s.match_indices(w) {
            let glued = i
                .checked_sub(1)
                .and_then(|j| bytes.get(j))
                .is_some_and(|b| is_ident_byte(*b));
            if glued {
                continue;
            }
            let Some(raw) = s.get(i + w.len()..).and_then(last_type_arg) else {
                continue;
            };
            if let Some(simple) = simple_type(&raw) {
                out.push((i, raw, simple));
            }
        }
    }
    out
}

/// The last top-level argument of a generic list whose `<` was just consumed:
/// `Null, Map<String, List<Item>>>` -> `Map<String, List<Item>>`. Never crosses
/// a line or a statement. Every split point is an ASCII byte, so each
/// `get(a..b)` lands on a char boundary — `get` rather than indexing anyway,
/// because CODE_RULES forbids the panicking form.
fn last_type_arg(s: &str) -> Option<String> {
    let mut depth = 1usize;
    let mut seg = 0usize;
    for (i, b) in s.bytes().enumerate() {
        if i > MAX_TYPE_ARG {
            return None;
        }
        match b {
            b'<' => depth += 1,
            b'>' => {
                // depth >= 1 here: it starts at 1 and we return on reaching 0.
                depth -= 1;
                if depth == 0 {
                    return s.get(seg..i).map(|t| t.trim().to_string());
                }
            }
            b',' if depth == 1 => seg = i + 1,
            b'\n' | b';' | b'(' | b')' | b'{' | b'}' | b'"' => return None,
            _ => {}
        }
    }
    None
}

/// Every Go `pkg.Type{` composite literal in `s`, outermost only: once one is
/// accepted the scan resumes after its closing brace, so the `pb.Item{}` inside
/// `&pb.OrderCreated{Items: []pb.Item{}}` never out-ranks its container.
fn scan_go_struct(s: &str) -> Vec<TypeHit> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = bytes
        .get(i..)
        .and_then(|rest| rest.iter().position(|b| *b == b'{'))
    {
        let open = i + off;
        let mut start = open;
        while start > 0
            && bytes
                .get(start - 1)
                .is_some_and(|b| is_ident_byte(*b) || *b == b'.')
        {
            start -= 1;
        }
        match go_struct_type(s, start, open) {
            Some((raw, simple)) => {
                out.push((start, raw, simple));
                i = matching_brace(bytes, open).map_or(bytes.len(), |close| close + 1);
            }
            None => i = open + 1,
        }
    }
    out
}

/// `s[start..open]` as a payload `pkg.Type`, or None. Go shape only: exactly
/// one dot, a lowercase package, an exported (uppercase) type, and a
/// non-identifier byte before it (`&`, whitespace, `(`, `,`, `=`, or the
/// window start) — which also rejects `[]pb.Item{` element literals.
fn go_struct_type(s: &str, start: usize, open: usize) -> Option<(String, String)> {
    let ident = s.get(start..open)?;
    let before_ok = match start.checked_sub(1).and_then(|j| s.as_bytes().get(j)) {
        None => true,
        Some(b) => matches!(b, b'&' | b'(' | b',' | b'=') || b.is_ascii_whitespace(),
    };
    let (pkg, ty) = ident.split_once('.')?;
    if !before_ok || ty.contains('.') {
        return None;
    }
    let pkg_ok = pkg.as_bytes().first().is_some_and(u8::is_ascii_lowercase);
    let ty_ok = ty.as_bytes().first().is_some_and(u8::is_ascii_uppercase);
    if !pkg_ok || !ty_ok || GO_PKG_DENY.contains(&pkg) || GO_TYPE_DENY.contains(&ty) {
        return None;
    }
    Some((ident.to_string(), simple_type(ty)?))
}

/// Index of the `}` closing the `{` at `open`, if it is inside `bytes`.
fn matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// Normalise a written type to its simple name: drop generic arguments, a
/// nullable `?` and array `[]`, then keep the segment after the last `.` or
/// `::`. `Null` / `Ignore` / `string` are NOT rejected — by the Kafka
/// convention the last argument IS the value type, and dropping them would
/// silently lose `Message<Null, string>`; A12.2 can weigh them.
fn simple_type(raw: &str) -> Option<String> {
    let t = raw.split('<').next().unwrap_or(raw).trim();
    let t = t.trim_end_matches(['?', '[', ']']).trim_end();
    let t = t.rsplit(['.', ':']).next().unwrap_or(t);
    let first_ok = t
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_');
    let ok = first_ok && t.len() <= MAX_TYPE_NAME && t.bytes().all(is_ident_byte);
    ok.then(|| t.to_string())
}

/// The MESSAGE_TYPE cell for a node whose call sites sit at `anchors`.
fn message_type_cell(source: &str, anchors: &[usize]) -> Option<Cell> {
    let mt = message_type_at(source, anchors)?;
    if debug_enabled() {
        eprintln!(
            "[queues] msgtype type={} form={} window={} raw={}",
            mt.simple, mt.form, mt.window, mt.raw
        );
    }
    // Written by hand in a FIXED key order: the workspace unifies serde_json's
    // `preserve_order` feature on for some builds and not others, and a
    // `json!` map would serialise in whichever order that build chose — the
    // `.gmap` bytes must not depend on it. Each value still goes through
    // serde_json, which escapes control characters `escape_json` does not.
    let q = |s: &str| serde_json::to_string(s).ok();
    let payload = format!(
        r#"{{"type":{},"raw":{},"form":{},"window":{}}}"#,
        q(&mt.simple)?,
        q(&mt.raw)?,
        q(mt.form)?,
        q(mt.window)?
    );
    Some(Cell {
        kind: cell_type::MESSAGE_TYPE,
        payload: CellPayload::Json(payload),
    })
}

/// Grep-able proof that a needle passed its gate and produced a node.
/// `GLIA_QUEUE_DEBUG=1 cargo test -p glia-code-extractors -- --nocapture
///  2>&1 | grep "\\[queues\\] needle '"`
///
/// A2.4: a node minted by an annotation / attribute row gets a second,
/// dedicated line (the unresolved tag included, so a listener whose queue is a
/// constant still shows up):
///   `GLIA_QUEUE_DEBUG=1 ... 2>&1 | grep "\\[queues\\] annotation '"`
fn fired_on(
    needle: &str,
    rule: &TopicRule,
    framework: &QueueFramework,
    topic: &str,
    path: &str,
) {
    if !debug_enabled() {
        return;
    }
    eprintln!("[queues] needle '{needle}' framework={framework:?} gate=ok topic={topic} file={path}");
    if is_annotation_row(needle, rule) {
        eprintln!(
            "[queues] annotation '{needle}' topic={topic} framework={framework:?} file={path}"
        );
    }
}

/// True for a row whose needle is a JVM/TS annotation (`@SqsListener(`) or a
/// C# attribute (`[ServiceBusTrigger(`). The A2.5 task decorators
/// (`@shared_task`) are excluded: they read a SYMBOL, not a queue, and carry
/// their own `[queues] taskq` marker.
fn is_annotation_row(needle: &str, rule: &TopicRule) -> bool {
    needle.starts_with(['@', '[']) && !is_identity_rule(rule)
}

/// A2.6 marker — one line per cloud-broker NODE, naming the literal shape
/// (`arn|url|path|literal`) its topic was folded from:
///   `GLIA_QUEUE_DEBUG=1 ... 2>&1 | grep '\[queues\] cloud broker='`
fn cloud_fired_on(framework: &QueueFramework, topic: &str, form: TopicForm, path: &str) {
    if debug_enabled() && is_cloud_broker(framework) {
        eprintln!(
            "[queues] cloud broker={framework:?} topic={topic} from={} file={path}",
            form.as_str()
        );
    }
}

/// The brokers whose identity can arrive as a URL / ARN / resource path.
fn is_cloud_broker(f: &QueueFramework) -> bool {
    matches!(
        f,
        QueueFramework::Sqs
            | QueueFramework::Sns
            | QueueFramework::PubSub
            | QueueFramework::AzureServiceBus
    )
}

/// `GLIA_QUEUE_DEBUG=1` turns on the `[queues] scan needle=` marker, read once.
/// `pub(crate)`: eventbus.rs's `[queues] broker-event suppressed` line (A2.9)
/// shares the switch.
pub(crate) fn debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_QUEUE_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::attach_imports_cell;

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
        // BREAKING (A2.5): was `queue_producer:hello` — the PAYLOAD. This test
        // asserted only the kind, so the phantom topic was invisible to it.
        assert_eq!(qnames(&result), vec!["queue_producer:send_email".to_string()]);
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
        // BREAKING (A2.5): was `queue_producer:alice` — the PAYLOAD.
        assert_eq!(qnames(&result), vec!["queue_producer:greet".to_string()]);
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
        // `queue_producer:unresolved:kafka` because the object form was unreadable; it now
        // carries the real topic. The consumer half still reads as the tag: the
        // only needle that fires is `KafkaConsumer`, whose occurrences (an
        // import and a bare `new KafkaConsumer()`) name no topic — the topic
        // lives on `c.subscribe(...)`, and `c` is not the `consumer.` receiver
        // the table matches. A2.x can widen that needle; this asserts today.
        let consumer = "import { KafkaConsumer } from 'kafkajs';\nconst c = new KafkaConsumer();\nc.subscribe('user-events')";
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:unresolved:kafka".to_string()]);

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
        // CL.1: was over `queue.add(`, whose arg #0 is a BullMQ job name and
        // no longer a topic; the subject here is the backtick reader.
        let source = "import { Queue } from 'bullmq';\nconst q = new Queue(`emails`, opts);";
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
            vec!["queue_producer:unresolved:kafka".to_string()],
            "an unreadable topic must fall back to the framework tag, never to the payload"
        );
    }

    #[test]
    fn unparseable_topic_emits_weak_unresolved_node() {
        // A2.3: the topic is an env var, so nothing readable names it. The node
        // must still exist (it is the coverage signal "this file talks to Kafka,
        // topic unknown") but it must be spelled `unresolved:` so the resolver
        // can refuse to pair it, and it must be Weak, not Medium.
        let source = "import { Kafka } from 'kafkajs';\nconst topic = process.env.T;\nproducer.send({ topic, messages: [] });";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(
            qnames(&pr),
            vec!["queue_producer:unresolved:kafka".to_string()],
            "an unreadable topic must be a self-declaring sentinel, not a bare framework name"
        );
        assert_eq!(pr.nodes.len(), 1);
        assert_eq!(
            pr.nodes[0].confidence,
            Confidence::Weak,
            "the tag proves the framework is live, nothing more"
        );
        // The topic-bearing sibling stays Medium — this is the contrast the
        // confidence split exists to express.
        let named = "import { Kafka } from 'kafkajs';\nproducer.send({ topic: 'orders' });";
        let ok = extract_queue_producer_nodes(named, PATH, module_id(), repo());
        assert_eq!(qnames(&ok), vec!["queue_producer:orders".to_string()]);
        assert_eq!(ok.nodes[0].confidence, Confidence::Medium);
    }

    #[test]
    fn unresolved_prefix_is_not_a_legal_topic_shape() {
        // Guards the one way the sentinel could collide with a real topic: the
        // resolver keys on this exact prefix, so it must stay in lockstep with
        // what `framework_tag` actually emits.
        assert!(framework_tag(&QueueFramework::Kafka).starts_with(UNRESOLVED_PREFIX));
        assert_eq!(framework_tag(&QueueFramework::Kafka), "unresolved:kafka");
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
        assert_eq!(qnames(&pr), vec!["queue_producer:unresolved:nats".to_string()]);
    }

    // ---- A2.2: receiver-agnostic needles + case-insensitive gates ---------

    #[test]
    fn signal_lists_are_lowercase() {
        // The gate compares against a `to_ascii_lowercase`d copy of the source,
        // so ONE upper-case letter in a signal literal is a permanently dead
        // gate — the row can never fire again. Asserted, not commented.
        for (needle, _, signals, _) in CONSUMER_PATTERNS
            .iter()
            .chain(PRODUCER_PATTERNS)
            .chain(DECLARE_PATTERNS)
        {
            for s in *signals {
                assert_eq!(
                    s.to_string(),
                    s.to_ascii_lowercase(),
                    "signal {s:?} gating needle {needle:?} must be spelled lowercase"
                );
            }
        }
    }

    #[test]
    fn csharp_confluent_gate_is_case_insensitive() {
        // THE packet's reason to exist. The only Kafka evidence in either file
        // is `using Confluent.Kafka;` — there is no lowercase "kafka" anywhere
        // (no bootstrap.servers host). Under the old case-SENSITIVE gate both
        // halves of this service emitted NOTHING AT ALL.
        let producer = r#"using System.Threading.Tasks;
using Confluent.Kafka;

public sealed class OrderProducer
{
    private readonly IProducer<Null, string> _producer;

    public async Task PublishAsync(string payload)
    {
        await _producer.ProduceAsync("orders", new Message<Null, string> { Value = payload });
    }
}"#;
        assert!(!producer.contains("kafka"), "fixture must carry no lowercase 'kafka'");
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);

        let consumer = r#"using System.Threading;
using Confluent.Kafka;

public sealed class OrderConsumer
{
    public void Run(ConsumerConfig config, CancellationToken ct)
    {
        var consumer = new ConsumerBuilder<Ignore, string>(config).Build();
        consumer.Subscribe("orders");
        var result = consumer.Consume(ct);
    }
}"#;
        assert!(!consumer.contains("kafka"), "fixture must carry no lowercase 'kafka'");
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn rx_subscribe_without_kafka_gate_emits_nothing() {
        // PRECISION GUARD for the receiver-agnostic `.Subscribe(` needle: the
        // gate is the ONLY thing keeping Rx/ReactiveX out. Fails the moment the
        // `confluent.kafka` signal list is dropped or widened to "kafka".
        let source = r#"using System;
using System.Reactive.Linq;

public sealed class Ticker
{
    public IDisposable Start(IObservable<long> source)
    {
        return source.Subscribe("ignored");
    }
}"#;
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert!(
            cr.nodes.is_empty(),
            "Rx .Subscribe( must not emit a Kafka consumer, got {:?}",
            qnames(&cr)
        );
    }

    #[test]
    fn spring_kafka_template_producer() {
        // `kafkaTemplate.send(` matched NO needle before: the table's row was
        // the receiver-bound `producer.send`.
        let source = r#"
import org.springframework.kafka.core.KafkaTemplate;

public class OrderProducer {
    private final KafkaTemplate<String, String> kafkaTemplate;

    public void publish(String payload) {
        kafkaTemplate.send("orders", payload);
    }
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn go_segmentio_writer_and_reader() {
        // Idiomatic short receivers `w` / `r` — the old needles were spelled
        // `writer.WriteMessages` / `reader.ReadMessage` and saw nothing here.
        let producer = r#"
import "github.com/segmentio/kafka-go"

func Publish(ctx context.Context, w *kafka.Writer, body []byte) error {
	return w.WriteMessages(ctx, kafka.Message{Topic: "orders", Value: body})
}
"#;
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);

        let consumer = r#"
import "github.com/segmentio/kafka-go"

func Consume(ctx context.Context) error {
	r := kafka.NewReader(kafka.ReaderConfig{
		Brokers: []string{"localhost:9092"},
		Topic:   "orders",
		GroupID: "svc",
	})
	m, err := r.ReadMessage(ctx)
	_ = m
	return err
}
"#;
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        // `.ReadMessage(` is NoIdentity: it proves the consumer is live but
        // names no topic, so it must NOT add a `queue_consumer:unresolved:kafka` tag
        // beside the real topic the ReaderConfig row already named.
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn rust_rdkafka_future_record() {
        let source = r#"
use rdkafka::producer::{FutureProducer, FutureRecord};

async fn publish(p: &FutureProducer, payload: &str) {
    let _ = p.send(FutureRecord::to("orders").payload(payload), Timeout::Never).await;
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn php_rdkafka_new_topic() {
        // `RdKafka\Producer` lowercases to `rdkafka\producer`, so this is a
        // second case-insensitivity proof. `->produce(` is NoIdentity, so the
        // real topic stands alone with no `queue_producer:unresolved:kafka` tag beside it.
        let source = r#"<?php
$producer = new RdKafka\Producer();
$topic = $producer->newTopic("orders");
$topic->produce(RD_KAFKA_PARTITION_UA, 0, $payload);
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    // ---- A2.5: the task, not the first string argument --------------------

    #[test]
    fn celery_delay_uses_receiver_not_payload() {
        // THE packet's reason to exist: the argument is an email ADDRESS.
        let source = "from celery import Celery\nsend_email.delay(\"welcome@example.com\")\n";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:send_email".to_string()]);
        assert!(
            !qnames(&pr).iter().any(|q| q.contains("welcome")),
            "a payload literal must never become a topic, got {:?}",
            qnames(&pr)
        );
    }

    #[test]
    fn shared_task_uses_def_name() {
        let source =
            "from celery import shared_task\n\n@shared_task\ndef send_email(address):\n    pass\n";
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:send_email".to_string()]);
    }

    #[test]
    fn decorator_stack_skips_to_def() {
        let source = "from celery import shared_task\n\n@shared_task\n@retry(max_retries=3)\ndef send_email(address):\n    pass\n";
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:send_email".to_string()]);
    }

    #[test]
    fn celery_producer_and_consumer_pair_on_the_task() {
        // The two halves live in different services; the ONLY thing that can
        // join them is the task name. Before A2.5 the producer said
        // `welcome@example.com` and the consumer said `unresolved:celery`.
        let producer = "from celery import Celery\nsend_email.delay(\"welcome@example.com\")\n";
        let consumer =
            "from celery import shared_task\n@shared_task\ndef send_email(address):\n    pass\n";
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(
            qnames(&pr)[0].trim_start_matches("queue_producer:"),
            qnames(&cr)[0].trim_start_matches("queue_consumer:"),
            "producer and consumer of one task must share a join key"
        );
    }

    #[test]
    fn sidekiq_perform_async_uses_class() {
        let source = "class OrdersController\n  def create\n    HardWorker.perform_async(order.id)\n  end\nend\n";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:HardWorker".to_string()]);
    }

    /// CJ.1a: the self-scan-literal-needles fixture's scanner/src/table.rs: a
    /// needle table, doc comments and a test's embedded sample source.
    const NEEDLE_TABLE_RS: &str = r#"//! A scanner's own needle table: NATS `nc.publish("orders", data)` and the
//! node `emitter.emit('user.created', u)` bus, read by the queue scanner.

/// Needles the scanner looks for.
pub const NEEDLES: &[&str] = &["nc.publish(", "channel.basic_publish(", "emitter.emit("];

/// The library signals that gate them.
pub const SIGNALS: &[&str] = &["nats", "amqp", "events"];

/// Sidekiq rows read the receiver before the call.
pub fn sidekiq_perform_async_uses_class() -> bool {
    NEEDLES.is_empty()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_nats_publish() {
        let src = "import { connect } from 'nats';\nnc.publish(\"orders\", data);\n";
        assert!(src.contains("nc.publish("));
        let bus = "const emitter = new EventEmitter();\nemitter.emit('user.created', u);\n";
        assert!(bus.contains("emit("));
    }
}
"#;

    #[test]
    fn literal_and_comment_needles_mint_nothing_in_rust_and_python() {
        let both = |src: &str, path: &str| {
            let mut v = qnames(&extract_queue_producer_nodes(src, path, module_id(), repo()));
            v.extend(qnames(&extract_queue_consumer_nodes(src, path, module_id(), repo())));
            v
        };
        assert_eq!(both(NEEDLE_TABLE_RS, "scanner/src/table.rs"), Vec::<String>::new());
        // Other languages have no guard: the same text read as TypeScript
        // mints exactly what an unguarded read mints, phantoms included.
        let ts = both(NEEDLE_TABLE_RS, "scanner/src/table.ts");
        assert_eq!(ts, both(NEEDLE_TABLE_RS, ""));
        assert!(ts.contains(&"queue_producer:orders".to_string()), "{ts:?}");

        let worker = "use rdkafka::consumer::{Consumer, StreamConsumer};\n\npub fn start(consumer: &StreamConsumer) {\n    consumer.subscribe(&[\"payments\"]).expect(\"subscribe\");\n}\n";
        let cr = extract_queue_consumer_nodes(worker, "worker/src/main.rs", module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:payments".to_string()]);

        let probe = "import pika\n\nSAMPLE = \"channel.basic_publish(exchange='', routing_key='refunds', body=b)\"\n\n\ndef send_invoice(channel, body):\n    channel.basic_publish(exchange='', routing_key='invoices', body=body)\n";
        let py = qnames(&extract_queue_producer_nodes(probe, "tools/probe.py", module_id(), repo()));
        assert!(py.contains(&"queue_producer:invoices".to_string()), "{py:?}");
        assert!(!py.contains(&"queue_producer:refunds".to_string()), "{py:?}");
        let unguarded = qnames(&extract_queue_producer_nodes(probe, "tools/probe.txt", module_id(), repo()));
        assert!(unguarded.contains(&"queue_producer:refunds".to_string()), "{unguarded:?}");
    }

    #[test]
    fn bare_word_receiver_needs_a_word_start() {
        let pr = |src: &str| qnames(&extract_queue_producer_nodes(src, "app/x.rb", module_id(), repo()));
        assert_eq!(pr("fn sidekiq_perform_async_uses_class() {}\n"), Vec::<String>::new());
        assert_eq!(pr("HardWorker.perform_async(1)\n"), vec!["queue_producer:HardWorker".to_string()]);
        assert_eq!(pr("HardWorker.perform_in(5.minutes, 1)\n"), vec!["queue_producer:HardWorker".to_string()]);
        assert_eq!(pr("MyJob.perform_inline(1)\n"), Vec::<String>::new());
        assert_eq!(pr("MyJob.perform_async_bulk(1)\n"), Vec::<String>::new());
    }

    #[test]
    fn sidekiq_worker_uses_enclosing_class() {
        // Two nodes for one class, deliberately: the JOB (the class) and the
        // broker QUEUE that `sidekiq_options` names, written without parens.
        let source = "class HardWorker\n  include Sidekiq::Worker\n  sidekiq_options queue: 'critical', retry: 3\n\n  def perform(id); end\nend\n";
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(
            qnames(&cr),
            vec![
                "queue_consumer:HardWorker".to_string(),
                "queue_consumer:critical".to_string()
            ]
        );
        assert!(
            !qnames(&cr).iter().any(|q| q.contains(UNRESOLVED_PREFIX)),
            "the job IS named; no coverage sentinel should stand beside it"
        );
    }

    #[test]
    fn oban_insert_names_the_worker() {
        let source = "Oban.insert(EmailWorker.new(%{to: \"a@b.com\"}))\n";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:EmailWorker".to_string()]);

        let worker = "defmodule MyApp.EmailWorker do\n  use Oban.Worker\n\n  def perform(job), do: :ok\nend\n";
        let cr = extract_queue_consumer_nodes(worker, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:EmailWorker".to_string()]);
    }

    #[test]
    fn an_unnamed_task_still_falls_back_to_the_sentinel() {
        // The identity rules return None rather than guessing, and A2.3's
        // unpairable coverage signal is what must show up then.
        // The receiver is an INDEX EXPRESSION, so no symbol precedes the call.
        let anonymous = "from celery import Celery\nhandlers[kind].apply_async(payload)\n";
        let ar = extract_queue_producer_nodes(anonymous, PATH, module_id(), repo());
        assert_eq!(
            qnames(&ar),
            vec!["queue_producer:unresolved:celery".to_string()]
        );
    }

    // ---- A2.8: POSITION + per-site provenance + module edge ---------------

    fn payload(c: &Cell) -> &str {
        match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => s.as_str(),
            _ => "",
        }
    }

    fn cell_of(n: &Node, kind: glia_core::CellTypeId) -> &Cell {
        n.cells
            .iter()
            .find(|c| c.kind == kind)
            .expect("cell present on queue node")
    }

    #[test]
    fn producer_node_carries_position_and_sites() {
        // Two `nc.Publish("orders", …)` calls in ONE file: ONE node, TWO sites.
        // Before A2.8 the node was `cells: vec![]` and the second call site was
        // dropped by the `seen` HashSet, so nothing in the graph could say
        // WHERE the topic was published from.
        let source = concat!(
            "package main\n",                                       // line 0
            "\n",                                                   // line 1
            "import \"github.com/nats-io/nats.go\"\n",              // line 2
            "\n",                                                   // line 3
            "func A(nc *nats.Conn) { nc.Publish(\"orders\", nil) }\n", // line 4
            "\n",                                                   // line 5
            "func B(nc *nats.Conn) { nc.Publish(\"orders\", nil) }\n", // line 6
        );
        let r = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(r.nodes.len(), 1, "two sites, one topic => one node");

        // 0-indexed, tree-sitter convention — the FIRST site, not the last.
        assert_eq!(
            payload(cell_of(&r.nodes[0], cell_type::POSITION)),
            r#"{"file":"src/test.rs","start_line":4,"end_line":4}"#
        );
        // Both sites survive dedup; this is the cell A2.7 reads for `family`.
        assert_eq!(
            payload(cell_of(&r.nodes[0], cell_type::CODE)),
            concat!(
                r#"{"framework":"Nats","family":"nats","sites":["#,
                r#"{"file":"src/test.rs","line":4},"#,
                r#"{"file":"src/test.rs","line":6}]}"#
            )
        );

        // One CONTAINS edge module → topic. CONTAINS, not a semantic category,
        // because `CODE_TABLES.carry_edges` excludes it.
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.edges[0].from, module_id());
        assert_eq!(r.edges[0].to, r.nodes[0].id);
        assert_eq!(r.edges[0].category, edge_category::CONTAINS);
    }

    #[test]
    fn framework_tag_fallback_is_still_located() {
        // The unpairable coverage sentinel (A2.3) is a real observation about a
        // real file, so it gets a position too — otherwise `glia coverage`
        // reports a Kafka signal it cannot point at.
        let source = "import { KafkaConsumer } from 'kafkajs';\nconst c = new KafkaConsumer();\n";
        let r = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&r), vec!["queue_consumer:unresolved:kafka".to_string()]);
        assert_eq!(
            payload(cell_of(&r.nodes[0], cell_type::POSITION)),
            r#"{"file":"src/test.rs","start_line":0,"end_line":0}"#
        );
    }

    #[test]
    fn position_path_is_json_escaped() {
        // Windows separators and quotes must not break the payload — the
        // engine parses it with serde_json in `locate_node`.
        let source = "import redis\nr = redis.Redis()\nr.rpush('votes', vote)\n";
        let r = extract_queue_producer_nodes(source, r#"src\a"b.py"#, module_id(), repo());
        assert_eq!(
            payload(cell_of(&r.nodes[0], cell_type::POSITION)),
            r#"{"file":"src\\a\"b.py","start_line":2,"end_line":2}"#
        );
    }

    // ---- A2.6: cloud brokers + URL/ARN/path folding -----------------------

    fn framework_of(r: &QueueNodes) -> String {
        payload(cell_of(&r.nodes[0], cell_type::CODE)).to_string()
    }

    #[test]
    fn sqs_queue_url_folds_to_name() {
        // THE packet's reason to exist: both sides name the queue by URL, and
        // they only join if both fold to the same bare name.
        let producer = "import boto3\nsqs = boto3.client(\"sqs\")\nsqs.send_message(\n    QueueUrl=\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\",\n    MessageBody=body,\n)\n";
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
        assert!(framework_of(&pr).contains(r#""framework":"Sqs","family":"sqs""#));

        let consumer = "import boto3\nsqs.receive_message(QueueUrl=\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\", MaxNumberOfMessages=10)\n";
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);

        // JS v3 command objects and C# request initialisers read the same key.
        let js = "import { SQSClient, SendMessageCommand } from \"@aws-sdk/client-sqs\";\nawait client.send(new SendMessageCommand({ QueueUrl: \"http://localhost:4566/000000000000/orders\", MessageBody: b }));\n";
        let jr = extract_queue_producer_nodes(js, PATH, module_id(), repo());
        assert_eq!(qnames(&jr), vec!["queue_producer:orders".to_string()]);
        let cs = "using Amazon.SQS;\nawait _sqs.ReceiveMessageAsync(new ReceiveMessageRequest { QueueUrl = \"https://sqs.eu-west-1.amazonaws.com/1/orders\" });\n";
        let csr = extract_queue_consumer_nodes(cs, PATH, module_id(), repo());
        assert_eq!(qnames(&csr), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn java_sdk_v2_builder_reads_queue_url() {
        // `.queueUrl("...")` is a builder METHOD, not `key = value`; the gate is
        // the real v2 package, `software.amazon.awssdk.services.sqs`.
        let source = r#"
import software.amazon.awssdk.services.sqs.SqsClient;
sqs.sendMessage(SendMessageRequest.builder()
    .queueUrl("https://sqs.us-east-1.amazonaws.com/123456789012/orders")
    .messageBody(body).build());
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn sns_arn_folds_to_topic() {
        let py = "import boto3\nsns = boto3.client(\"sns\")\nsns.publish(TopicArn=\"arn:aws:sns:us-east-1:123456789012:orders\", Message=m)\n";
        let pr = extract_queue_producer_nodes(py, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
        assert!(framework_of(&pr).contains(r#""framework":"Sns","family":"sns""#));

        let js = "import { SNSClient, PublishCommand } from '@aws-sdk/client-sns';\nawait sns.send(new PublishCommand({ TopicArn: 'arn:aws:sns:eu-west-1:1:orders', Message: m }));\n";
        let jr = extract_queue_producer_nodes(js, PATH, module_id(), repo());
        assert_eq!(qnames(&jr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn pubsub_topic_path_arg_index() {
        // arg #0 is the PROJECT; reading it would mint `queue_producer:my-project`.
        let py = "from google.cloud import pubsub_v1\npublisher = pubsub_v1.PublisherClient()\ntopic_path = publisher.topic_path(\"my-project\", \"orders\")\n";
        let pr = extract_queue_producer_nodes(py, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);

        let sub = "from google.cloud import pubsub_v1\npath = subscriber.subscription_path(\"my-project\", \"orders-worker\")\n";
        let sr = extract_queue_consumer_nodes(sub, PATH, module_id(), repo());
        assert_eq!(qnames(&sr), vec!["queue_consumer:orders-worker".to_string()]);

        // JS: the topic and the subscription are distinct names by design.
        let js_pub = "import { PubSub } from '@google-cloud/pubsub';\nawait pubsub.topic('orders').publishMessage({ data });\n";
        let jp = extract_queue_producer_nodes(js_pub, PATH, module_id(), repo());
        assert_eq!(qnames(&jp), vec!["queue_producer:orders".to_string()]);
        let js_sub = "import { PubSub } from '@google-cloud/pubsub';\nconst sub = pubsub.subscription('orders-worker');\n";
        assert!(extract_queue_producer_nodes(js_sub, PATH, module_id(), repo()).nodes.is_empty());
        let js = extract_queue_consumer_nodes(js_sub, PATH, module_id(), repo());
        assert_eq!(qnames(&js), vec!["queue_consumer:orders-worker".to_string()]);

        // A fully-qualified resource path folds like a URL.
        let fq = "import { PubSub } from '@google-cloud/pubsub';\npubsub.topic('projects/my-project/topics/orders');\n";
        let fr = extract_queue_producer_nodes(fq, PATH, module_id(), repo());
        assert_eq!(qnames(&fr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn azure_create_sender_literal() {
        let cs = "using Azure.Messaging.ServiceBus;\nServiceBusSender sender = client.CreateSender(\"orders\");\n";
        let pr = extract_queue_producer_nodes(cs, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
        assert!(framework_of(&pr).contains(r#""framework":"AzureServiceBus""#));

        let proc_ = "using Azure.Messaging.ServiceBus;\nvar processor = client.CreateProcessor(\"orders\", \"worker\", new ServiceBusProcessorOptions());\n";
        let cr = extract_queue_consumer_nodes(proc_, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);

        // Python azure-servicebus: keyword form, and the subscription receiver
        // joins on its TOPIC, not the subscription name.
        let py = "from azure.servicebus import ServiceBusClient\nwith client.get_queue_sender(queue_name=\"orders\") as s:\n    pass\n";
        let ps = extract_queue_producer_nodes(py, PATH, module_id(), repo());
        assert_eq!(qnames(&ps), vec!["queue_producer:orders".to_string()]);
        let py_sub = "from azure.servicebus import ServiceBusClient\nr = client.get_subscription_receiver(topic_name=\"orders\", subscription_name=\"worker\")\n";
        let pc = extract_queue_consumer_nodes(py_sub, PATH, module_id(), repo());
        assert_eq!(qnames(&pc), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn cloud_needles_without_their_sdk_emit_nothing() {
        // PRECISION GUARD: `.sendMessage(` / `.send_message(` / `.topic(` /
        // `.publish(` are everyday method names. The SDK gate is all that keeps
        // a browser extension, a Telegram bot or an MQTT client out.
        for (src, what) in [
            ("chrome.runtime.sendMessage({ QueueUrl: 'x' });", "chrome sendMessage"),
            ("bot.send_message(chat_id, \"hello\")", "telegram send_message"),
            ("const t = mqttClient.topic('orders');", "non-GCP .topic("),
            ("client.publish(\"sensors/temp\", payload)", "mqtt publish"),
            ("var s = factory.CreateSender(\"orders\");", "non-Azure CreateSender"),
        ] {
            let pr = extract_queue_producer_nodes(src, PATH, module_id(), repo());
            let cr = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
            assert!(
                pr.nodes.is_empty() && cr.nodes.is_empty(),
                "{what} must not emit a cloud-broker node, got {:?} / {:?}",
                qnames(&pr),
                qnames(&cr)
            );
        }
    }

    #[test]
    fn interpolated_queue_name_falls_back_to_the_sentinel() {
        // The host/account may be interpolated — the NAME may not. A placeholder
        // name is a variable, so it must read as the unresolved coverage signal
        // rather than minting `queue_producer:{name}`.
        let named = "import boto3\nsqs.send_message(QueueUrl=f\"https://sqs.{region}.amazonaws.com/{account}/orders\", MessageBody=b)\n";
        let nr = extract_queue_producer_nodes(named, PATH, module_id(), repo());
        assert_eq!(qnames(&nr), vec!["queue_producer:orders".to_string()]);

        let unnamed = "import boto3\nsqs.send_message(QueueUrl=f\"https://sqs.{region}.amazonaws.com/{account}/{name}\", MessageBody=b)\n";
        let ur = extract_queue_producer_nodes(unnamed, PATH, module_id(), repo());
        assert_eq!(qnames(&ur), vec!["queue_producer:unresolved:sqs".to_string()]);
    }

    // ---- A2.4: annotation-driven consumers --------------------------------

    fn consumers(src: &str) -> Vec<String> {
        qnames(&extract_queue_consumer_nodes(src, PATH, module_id(), repo()))
    }

    #[test]
    fn kafka_listener_topics_attr() {
        // `groupId` is a consumer group — it must never become the topic, in
        // either attribute order.
        for src in [
            "import org.springframework.kafka.annotation.KafkaListener;\n@KafkaListener(topics = \"orders\", groupId = \"billing\")\npublic void on(String p) {}\n",
            "import org.springframework.kafka.annotation.KafkaListener;\n@KafkaListener(groupId = \"billing\", topics = \"orders\")\npublic void on(String p) {}\n",
        ] {
            assert_eq!(consumers(src), vec!["queue_consumer:orders".to_string()]);
        }
        // A pattern subscription is read verbatim; the resolver matches it in
        // the Kafka dialect. Was `queue_consumer:unresolved:kafka`.
        let pattern = "import org.springframework.kafka.annotation.KafkaListener;\n@KafkaListener(topicPattern = \"orders.*\", groupId = \"audit\")\n";
        assert_eq!(consumers(pattern), vec!["queue_consumer:orders.*".to_string()]);
        // A property placeholder is kept: services reading the same property
        // share the join key. KNOWN QUIRK (pre-existing, `queue_topic.rs`):
        // `trim_noise` strips the trailing `}`, so the key is `${app.topic` —
        // stable on both sides, but mangled. Update when that trim is fixed.
        let placeholder = "import org.springframework.kafka.annotation.KafkaListener;\n@KafkaListener(topics = \"${app.topic}\")\n";
        assert_eq!(consumers(placeholder), vec!["queue_consumer:${app.topic".to_string()]);
    }

    #[test]
    fn kafka_listener_array_topics_is_a_known_miss() {
        // The packet spec assumed the keyed scan returns the first literal of
        // `topics = {"a", "b"}`. It does not: `read_literal` stops at `{`, so the
        // listener reads as the coverage sentinel. Flips when `queue_topic`
        // learns array values; update this assertion then.
        let src = "import org.springframework.kafka.annotation.KafkaListener;\n@KafkaListener(topics = {\"orders\", \"payments\"}, groupId = \"billing\")\n";
        assert_eq!(consumers(src), vec!["queue_consumer:unresolved:kafka".to_string()]);
    }

    #[test]
    fn rabbit_listener_queues_attr() {
        let src = "import org.springframework.amqp.rabbit.annotation.RabbitListener;\n@RabbitListener(queues = \"orders\")\npublic void on(String p) {}\n";
        let r = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(qnames(&r), vec!["queue_consumer:orders".to_string()]);
        assert!(framework_of(&r).contains(r#""framework":"RabbitMQ","family":"rabbitmq""#));

        // Declarative binding: the first `value =` holds `@Queue(`, the queue's
        // own `value` holds the name; the exchange must not win.
        let bindings = r#"import org.springframework.amqp.rabbit.annotation.*;
@RabbitListener(bindings = @QueueBinding(
    value = @Queue(value = "orders", durable = "true"),
    exchange = @Exchange(value = "shop"),
    key = "order.created"))
public void on(String p) {}
"#;
        assert_eq!(consumers(bindings), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn jms_listener_destination_attr() {
        let src = "import org.springframework.jms.annotation.JmsListener;\n@JmsListener(destination = \"orders\", containerFactory = \"factory\")\npublic void on(String p) {}\n";
        let r = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(qnames(&r), vec!["queue_consumer:orders".to_string()]);
        assert!(framework_of(&r).contains(r#""framework":"Jms","family":"jms""#));
    }

    #[test]
    fn sqs_listener_positional() {
        for src in [
            // Spring Cloud AWS 3.x, positional.
            "import io.awspring.cloud.sqs.annotation.SqsListener;\n@SqsListener(\"orders\")\npublic void on(String p) {}\n",
            // 3.x, named attribute.
            "import io.awspring.cloud.sqs.annotation.SqsListener;\n@SqsListener(queueNames = \"orders\", maxConcurrentMessages = \"10\")\n",
            // 2.x package, `value =` with a queue URL that folds.
            "import org.springframework.cloud.aws.messaging.listener.annotation.SqsListener;\n@SqsListener(value = \"https://sqs.us-east-1.amazonaws.com/123456789012/orders\")\n",
        ] {
            let r = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
            assert_eq!(qnames(&r), vec!["queue_consumer:orders".to_string()], "{src}");
            assert!(framework_of(&r).contains(r#""framework":"Sqs","family":"sqs""#));
        }
    }

    #[test]
    fn nest_processor_queue_name() {
        let bullmq = "import { Processor, WorkerHost } from '@nestjs/bullmq';\n\n@Processor('emails')\nexport class EmailsProcessor extends WorkerHost {\n  async process(job) {}\n}\n";
        let r = extract_queue_consumer_nodes(bullmq, PATH, module_id(), repo());
        assert_eq!(qnames(&r), vec!["queue_consumer:emails".to_string()]);
        assert!(framework_of(&r).contains(r#""framework":"BullMQ","family":"bullmq""#));

        // Legacy @nestjs/bull; `@Process('welcome')` names a JOB and must not
        // become a queue.
        let bull = "import { Processor, Process } from '@nestjs/bull';\n\n@Processor('emails')\nexport class EmailsConsumer {\n  @Process('welcome')\n  async welcome(job) {}\n}\n";
        assert_eq!(consumers(bull), vec!["queue_consumer:emails".to_string()]);
    }

    #[test]
    fn rabbit_subscribe_queue_option() {
        let src = "import { RabbitSubscribe } from '@golevelup/nestjs-rabbitmq';\n\n@RabbitSubscribe({ exchange: 'shop', routingKey: 'order.created', queue: 'billing-orders' })\npublic async onOrder(msg: {}) {}\n";
        assert_eq!(consumers(src), vec!["queue_consumer:billing-orders".to_string()]);
    }

    #[test]
    fn servicebus_trigger_attr() {
        // In-process model.
        let inproc = r#"using Microsoft.Azure.WebJobs;

public static class OrderFunction
{
    [FunctionName("OrderFunction")]
    public static void Run([ServiceBusTrigger("orders", Connection = "ServiceBus")] string body) { }
}"#;
        let r = extract_queue_consumer_nodes(inproc, PATH, module_id(), repo());
        assert_eq!(qnames(&r), vec!["queue_consumer:orders".to_string()]);
        assert!(framework_of(&r).contains(r#""framework":"AzureServiceBus""#));

        // Isolated worker, topic trigger: arg #0 is the TOPIC, arg #1 the
        // subscription — the topic is the join key.
        let isolated = r#"using Microsoft.Azure.Functions.Worker;

public class AuditFunction
{
    [Function("Audit")]
    public void Run([ServiceBusTrigger("orders", "audit", Connection = "ServiceBus")] string body) { }
}"#;
        assert_eq!(consumers(isolated), vec!["queue_consumer:orders".to_string()]);

        // A constant queue name reads as the unpairable sentinel, never as the
        // `Connection` setting that follows it.
        let constant = "using Microsoft.Azure.WebJobs;\npublic static void Run([ServiceBusTrigger(Queues.Orders, Connection = \"ServiceBus\")] string body) { }\n";
        assert_eq!(
            consumers(constant),
            vec!["queue_consumer:unresolved:azureservicebus".to_string()]
        );
    }

    #[test]
    fn annotation_without_gate_is_ignored() {
        // PRECISION GUARD: each annotation name also exists outside its
        // messaging library, and the package gate is the only thing keeping
        // those out.
        for (src, what) in [
            (
                "import { Processor } from '@acme/pipeline';\n@Processor('emails')\nexport class Step {}\n",
                "non-bull @Processor",
            ),
            (
                "import com.acme.messaging.SqsListener;\n@SqsListener(\"orders\")\npublic void on(String p) {}\n",
                "home-grown @SqsListener",
            ),
            (
                "import com.acme.RabbitListener;\n@RabbitListener(queues = \"orders\")\n",
                "home-grown @RabbitListener",
            ),
            (
                "import com.acme.JmsListener;\n@JmsListener(destination = \"orders\")\n",
                "home-grown @JmsListener",
            ),
            (
                "using Acme.Triggers;\npublic void Run([ServiceBusTrigger(\"orders\")] string body) { }\n",
                "home-grown [ServiceBusTrigger]",
            ),
        ] {
            let cr = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
            let pr = extract_queue_producer_nodes(src, PATH, module_id(), repo());
            assert!(
                cr.nodes.is_empty() && pr.nodes.is_empty(),
                "{what} must not emit a queue node, got {:?} / {:?}",
                qnames(&cr),
                qnames(&pr)
            );
        }
    }

    #[test]
    fn annotation_gates_are_not_satisfied_by_their_own_needle() {
        // `@SqsListener(` lowercases to `@sqslistener(`, which already contains
        // "sqs" — a gate spelled that way passes on the needle alone and gates
        // nothing. Asserted for every annotation row, not commented.
        let rows: Vec<_> = CONSUMER_PATTERNS
            .iter()
            .filter(|(needle, _, _, rule)| is_annotation_row(needle, rule))
            .collect();
        assert!(rows.len() >= 7, "annotation rows missing: {}", rows.len());
        for (needle, _, signals, _) in rows {
            assert!(!signals.is_empty(), "annotation row {needle:?} must be gated");
            let lower = needle.to_ascii_lowercase();
            for s in *signals {
                assert!(
                    !lower.contains(s),
                    "signal {s:?} is a substring of its own needle {needle:?}"
                );
            }
        }
    }

    // ---- A2.9: broker pub/sub rows ----------------------------------------

    fn producers(src: &str) -> Vec<String> {
        qnames(&extract_queue_producer_nodes(src, PATH, module_id(), repo()))
    }

    #[test]
    fn redis_pubsub_channel_becomes_queue_nodes() {
        let publisher = "import redis\nr = redis.Redis()\nr.publish(\"notifications\", json.dumps(p))\n";
        let pr = extract_queue_producer_nodes(publisher, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:notifications".to_string()]);
        assert!(framework_of(&pr).contains(r#""framework":"RedisPubSub""#));

        let subscriber = "import redis\np = r.pubsub()\np.subscribe(\"notifications\")\np.psubscribe('news.*')\n";
        assert_eq!(
            consumers(subscriber),
            vec![
                "queue_consumer:news.*".to_string(),
                "queue_consumer:notifications".to_string()
            ]
        );
        // ioredis spells the gate differently and still passes it.
        assert_eq!(
            consumers("import Redis from 'ioredis';\nsub.subscribe('notifications');"),
            vec!["queue_consumer:notifications".to_string()]
        );
    }

    #[test]
    fn mqtt_topic_becomes_queue_nodes() {
        for (publisher, subscriber, what) in [
            (
                "import paho.mqtt.client as mqtt\nclient.publish(\"sensors/temp\", payload)\n",
                "import paho.mqtt.client as mqtt\nclient.subscribe(\"sensors/temp\")\n",
                "python paho",
            ),
            (
                "import mqtt from 'mqtt';\nclient.publish('sensors/temp', String(v));",
                "import mqtt from 'mqtt';\nclient.on('connect', () => { client.subscribe(['sensors/temp']); });",
                "mqtt.js",
            ),
            (
                "import mqtt \"github.com/eclipse/paho.mqtt.golang\"\ntoken := client.Publish(\"sensors/temp\", 0, false, payload)\n",
                "import mqtt \"github.com/eclipse/paho.mqtt.golang\"\ntoken := client.Subscribe(\"sensors/temp\", 0, handler)\n",
                "go paho",
            ),
        ] {
            let pr = extract_queue_producer_nodes(publisher, PATH, module_id(), repo());
            assert_eq!(qnames(&pr), vec!["queue_producer:sensors/temp".to_string()], "{what}");
            assert!(framework_of(&pr).contains(r#""framework":"Mqtt""#), "{what}");
            assert_eq!(consumers(subscriber), vec!["queue_consumer:sensors/temp".to_string()], "{what}");
        }
    }

    #[test]
    fn generic_verb_rows_never_read_a_callback_literal() {
        // PRECISION GUARD: an Rx subscription in a file that imports redis. The
        // only literal is inside the callback, so it must not become a channel.
        let rx = "import Redis from 'ioredis';\nthis.events$.subscribe((e) => log('hi', e));";
        assert_eq!(
            consumers(rx),
            vec!["queue_consumer:unresolved:redispubsub".to_string()],
            "a non-literal channel is the coverage sentinel, never the callback's literal"
        );
    }

    #[test]
    fn generic_verb_rows_yield_to_an_earlier_row() {
        // One call, one node: a NATS file that also imports redis must not
        // mint a RedisPubSub twin of the NATS producer or consumer.
        let src = "import { connect } from 'nats';\nimport Redis from 'ioredis';\nnc.publish('orders', b);\nnc.subscribe('orders');";
        let pr = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
        assert_eq!(pr.nodes.len(), 1);
        assert!(framework_of(&pr).contains(r#""framework":"Nats""#));
        let cr = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(cr.nodes.len(), 1, "{:?}", qnames(&cr));
        assert!(framework_of(&cr).contains(r#""framework":"Nats""#));
        // A kafkajs subscriber in a redis-importing file stays Kafka-only.
        let kafka = "import { Kafka } from 'kafkajs';\nimport Redis from 'ioredis';\nawait consumer.subscribe({ topic: 'orders' });";
        let kr = extract_queue_consumer_nodes(kafka, PATH, module_id(), repo());
        assert_eq!(qnames(&kr), vec!["queue_consumer:orders".to_string()]);
        assert!(framework_of(&kr).contains(r#""framework":"Kafka""#));
    }

    #[test]
    fn broker_rows_stay_off_without_their_library() {
        // No broker import: the verbs belong to eventbus.rs, not here.
        for src in [
            "bus.publish('user.created', u);",
            "bus.subscribe('user.created', h);",
            "client.Publish(\"sensors/temp\", 0, false, p)",
        ] {
            assert!(producers(src).is_empty() && consumers(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn spring_jms_template_producer() {
        let src = "import org.springframework.jms.core.JmsTemplate;\npublic void send(String p) { jmsTemplate.convertAndSend(\"orders\", p); }";
        let pr = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
        assert!(framework_of(&pr).contains(r#""framework":"Jms""#));
        // A file importing Spring AMQP too is attributed to the earlier row, once.
        let both = format!("import org.springframework.amqp.rabbit.core.RabbitTemplate;\n{src}");
        let br = extract_queue_producer_nodes(&both, PATH, module_id(), repo());
        assert_eq!(br.nodes.len(), 1);
        assert!(framework_of(&br).contains(r#""framework":"RabbitMQ""#));
    }

    #[test]
    fn broker_signals_are_derived_and_exclude_task_queues() {
        let signals = broker_signals();
        for s in ["redis", "nats", "amqp", "kafka", "mqtt", "paho", "boto3", "awssdk.services.sqs", "google.cloud"] {
            assert!(signals.contains(&s), "broker signal {s:?} missing from {signals:?}");
        }
        for s in ["celery", "dramatiq", "bullmq", "@shared_task", "pubsub"] {
            assert!(!signals.contains(&s), "{s:?} is not a broker signal");
        }
        assert!(broker_signal_present("import paho.mqtt.client as mqtt"));
        assert!(!broker_signal_present("from celery import celery"));
    }

    // ---- A12.1: MESSAGE_TYPE ----------------------------------------------

    fn msg_type(n: &Node) -> Option<serde_json::Value> {
        n.cells
            .iter()
            .find(|c| c.kind == cell_type::MESSAGE_TYPE)
            .and_then(|c| serde_json::from_str(payload(c)).ok())
    }

    #[test]
    fn msgtype_go_struct_literal_near_publish() {
        // (a) the Go marshal-then-publish shape.
        let src = "import \"github.com/nats-io/nats.go\"\n\
                   func P(nc *nats.Conn, id string) error {\n\
                   \tdata, _ := proto.Marshal(&pb.OrderCreated{Id: id})\n\
                   \treturn nc.Publish(\"orders\", data)\n}\n";
        let mt = extract_message_type_near(src, "nc.Publish").expect("type found");
        assert_eq!(
            (mt.simple.as_str(), mt.raw.as_str(), mt.form, mt.window),
            ("OrderCreated", "pb.OrderCreated", "struct_literal", "near")
        );
        // End to end: the cell rides on the node, after the A2.8 pair.
        let r = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert_eq!(r.nodes.len(), 1);
        let kinds: Vec<_> = r.nodes[0].cells.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![cell_type::POSITION, cell_type::CODE, cell_type::MESSAGE_TYPE]);
        assert_eq!(
            payload(cell_of(&r.nodes[0], cell_type::MESSAGE_TYPE)),
            r#"{"type":"OrderCreated","raw":"pb.OrderCreated","form":"struct_literal","window":"near"}"#
        );
    }

    #[test]
    fn msgtype_go_envelope_is_denied() {
        // (b) `kafka.Message{Topic: ...}` is the envelope, never the payload;
        // a `*nats.Msg` handler parameter must not win over the real type.
        let src = "w.WriteMessages(ctx, &kafka.Message{Topic: \"x\", Value: data})";
        assert_eq!(extract_message_type_near(src, ".WriteMessages("), None);
        let consumer = "import \"github.com/nats-io/nats.go\"\n\
                        func S(nc *nats.Conn) {\n\
                        \tnc.Subscribe(\"orders\", func(m *nats.Msg) {\n\
                        \t\tevt := &pb.OrderCreated{}\n\
                        \t\t_ = proto.Unmarshal(m.Data, evt)\n\t})\n}\n";
        let r = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(qnames(&r), vec!["queue_consumer:orders".to_string()]);
        assert_eq!(msg_type(&r.nodes[0]).expect("typed")["type"], "OrderCreated");
        // Outermost literal only: the nested `pb.Item{}` never out-ranks it.
        let nested = "x := &pb.OrderCreated{Item: pb.Item{}}\nnc.Publish(\"o\", x)";
        let mt = extract_message_type_near(nested, "nc.Publish").expect("type found");
        assert_eq!(mt.simple, "OrderCreated");
    }

    #[test]
    fn msgtype_csharp_generic_producer() {
        // (c) the Confluent generic form, canonical `<K, V>` spacing (A15.8).
        let src = "using Confluent.Kafka;\n\
                   class P {\n\
                   \tprivate readonly IProducer<Null, OrderCreated> _producer;\n\
                   \tasync Task Go(OrderCreated e) {\n\
                   \t\tawait _producer.ProduceAsync(\"orders\", new Message<Null, OrderCreated> { Value = e });\n\
                   \t}\n}\n";
        let r = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert_eq!(qnames(&r), vec!["queue_producer:orders".to_string()]);
        let v = msg_type(&r.nodes[0]).expect("typed");
        assert_eq!((v["type"].as_str(), v["form"].as_str()), (Some("OrderCreated"), Some("generic")));
        // The Kafka convention keeps a primitive value type rather than dropping it.
        let plain = "IProducer<string, string> p; p.Produce(\"orders\", m);";
        assert_eq!(extract_message_type_near(plain, ".Produce(").map(|m| m.simple), Some("string".into()));
    }

    #[test]
    fn msgtype_java_consumer_record() {
        // (d) the Java path — unit-only: `@KafkaListener` reads the topic,
        // the handler parameter carries the type.
        let src = "@KafkaListener(topics = \"orders\")\n\
                   public void on(ConsumerRecord<String, com.acme.OrderCreated> rec) {}";
        let mt = extract_message_type_near(src, "@KafkaListener").expect("type found");
        assert_eq!((mt.simple.as_str(), mt.raw.as_str()), ("OrderCreated", "com.acme.OrderCreated"));
        // A diamond names nothing, and `IMessage<` is not `Message<`.
        assert_eq!(extract_message_type_near("new ProducerRecord<>(\"o\", v)", "ProducerRecord"), None);
        assert_eq!(extract_message_type_near("IMessage<Foo> m; bus.Send(m)", "bus.Send"), None);
    }

    #[test]
    fn msgtype_nested_generics_and_char_boundaries() {
        // (e) balanced `<>`: the LAST top-level argument, reduced to `Map`.
        let src = "Message<Null, Map<String, List<Item>>> m; producer.Produce(\"o\", m);";
        let mt = extract_message_type_near(src, "producer.Produce(").expect("type found");
        assert_eq!((mt.simple.as_str(), mt.raw.as_str()), ("Map", "Map<String, List<Item>>"));
        // Multi-byte text straddling both window edges must never panic.
        let pad = "é".repeat(700);
        let wide = format!("{pad}IProducer<Null, Évènement> p; p.Produce(\"o\", m);{pad}");
        let mt = extract_message_type_near(&wide, ".Produce(");
        assert_eq!(mt, None, "a non-ASCII type name is not an identifier");
        let ok = format!("{pad}IProducer<Null, Ok> p; p.Produce(\"o\", m);{pad}");
        assert_eq!(extract_message_type_near(&ok, ".Produce(").map(|m| m.simple), Some("Ok".into()));
        // Unterminated / multi-line argument lists bail instead of reading on.
        assert_eq!(extract_message_type_near("Message<Null,\n Foo> m; x.Produce(", "x.Produce("), None);
    }

    #[test]
    fn msgtype_each_topic_gets_its_nearest_type_and_file_fallback() {
        // Two topics, one file: each node reads the type beside ITS call.
        let src = "using Confluent.Kafka;\n\
                   await p.ProduceAsync(\"orders\", new Message<Null, OrderCreated> { Value = a });\n\
                   await p.ProduceAsync(\"payments\", new Message<Null, PaymentTaken> { Value = b });\n";
        let r = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        let mut got: Vec<(String, String)> = r
            .nodes
            .iter()
            .map(|n| {
                let v = msg_type(n).expect("typed");
                (r.nav.qname_by_id[&n.id].clone(), v["type"].as_str().unwrap_or("").to_string())
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("queue_producer:orders".to_string(), "OrderCreated".to_string()),
                ("queue_producer:payments".to_string(), "PaymentTaken".to_string()),
            ]
        );
        // Beyond the window: a generic type elsewhere in the file is kept but
        // marked `window: file`; a Go struct literal that far away is not.
        let far = format!(
            "using Confluent.Kafka;\nIProducer<Null, OrderCreated> p;\n{}\np.ProduceAsync(\"orders\", m);\n",
            "// filler\n".repeat(80)
        );
        let v = msg_type(&extract_queue_producer_nodes(&far, PATH, module_id(), repo()).nodes[0])
            .expect("file-window type");
        assert_eq!((v["type"].as_str(), v["window"].as_str()), (Some("OrderCreated"), Some("file")));
        let go_far = format!(
            "import \"github.com/nats-io/nats.go\"\nvar x = &pb.OrderCreated{{}}\n{}\nnc.Publish(\"orders\", d)\n",
            "// filler\n".repeat(80)
        );
        let r = extract_queue_producer_nodes(&go_far, PATH, module_id(), repo());
        assert_eq!(r.nodes[0].cells.len(), 2, "no type => no MESSAGE_TYPE cell");
    }

    #[test]
    fn every_framework_tag_is_a_framework_tag() {
        // (f) `is_framework_tag` is derived from the same prefix `framework_tag`
        // writes, so the two cannot drift. The match is exhaustive on purpose:
        // a new variant fails to compile here until it is listed.
        use QueueFramework::*;
        let all = [
            Celery, Dramatiq, BullMQ, Sidekiq, Oban, Nats, RabbitMQ, Kafka, RedisList, Sqs, Sns,
            PubSub, AzureServiceBus, Mqtt, RedisPubSub, Jms,
        ];
        for f in &all {
            match f {
                Celery | Dramatiq | BullMQ | Sidekiq | Oban | Nats | RabbitMQ | Kafka
                | RedisList | Sqs | Sns | PubSub | AzureServiceBus | Mqtt | RedisPubSub | Jms => {}
            }
            assert!(is_framework_tag(&framework_tag(f)), "{f:?}");
        }
        // A bare framework word is a legal topic since A2.3, not a tag.
        for topic in ["orders", "kafka", "nats"] {
            assert!(!is_framework_tag(topic), "{topic}");
        }
    }

    // ---- LA.4 (A11.7): post-cache const fold ------------------------------

    /// A resolver over a fixed `expr -> value` table, like the engine's closure
    /// over the file's own table and the repo's.
    fn resolver(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |e: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == e)
                .map(|(_, v)| (*v).to_string())
        }
    }

    fn folded(src: &str, pairs: &'static [(&'static str, &'static str)]) -> ConstFold {
        extract_queue_nodes_with_consts(src, PATH, module_id(), repo(), &resolver(pairs))
    }

    fn position(n: &Node) -> String {
        payload(cell_of(n, cell_type::POSITION)).to_string()
    }

    #[test]
    fn const_resolver_folds_keyed_positional_and_annotation_sites() {
        // kafkajs object form, keyed.
        let ts = "import { Kafka } from 'kafkajs';\n\nawait producer.send({ topic: ORDERS_TOPIC, messages });\n";
        let f = folded(ts, &[("ORDERS_TOPIC", "orders")]);
        assert_eq!(
            qnames(&f.producers),
            vec!["queue_producer:orders".to_string()]
        );
        assert!(f.consumers.nodes.is_empty());
        assert_eq!(
            f.counts,
            ConstFoldCounts {
                folded: 1,
                unresolved: 0
            }
        );
        // The folded node keeps its call-site provenance and a Medium rank.
        assert!(position(&f.producers.nodes[0]).contains(r#""start_line":2"#));
        assert_eq!(f.producers.nodes[0].confidence, Confidence::Medium);

        // Spring `kafkaTemplate.send(TOPIC, payload)`, positional.
        let java = "import org.springframework.kafka.core.KafkaTemplate;\nclass P { void p() { kafkaTemplate.send(TOPIC, payload); } }\n";
        let f = folded(java, &[("TOPIC", "orders")]);
        assert_eq!(
            qnames(&f.producers),
            vec!["queue_producer:orders".to_string()]
        );

        // `@KafkaListener(topics = Topics.ORDERS)`, the annotation's keyed slot.
        let listener = "import org.springframework.kafka.annotation.KafkaListener;\nclass L {\n  @KafkaListener(topics = Topics.ORDERS, groupId = \"billing\")\n  public void on(String p) {}\n}\n";
        let f = folded(listener, &[("Topics.ORDERS", "orders")]);
        assert_eq!(
            qnames(&f.consumers),
            vec!["queue_consumer:orders".to_string()]
        );
        assert!(f.producers.nodes.is_empty());

        // Go NATS `nc.Publish(SubjectOrders, data)`, a same-file PascalCase const.
        let go =
            "import \"github.com/nats-io/nats.go\"\nfunc p() { nc.Publish(SubjectOrders, data) }\n";
        let f = folded(go, &[("SubjectOrders", "orders")]);
        assert_eq!(
            qnames(&f.producers),
            vec!["queue_producer:orders".to_string()]
        );

        // A constant holding an SQS URL folds exactly as the literal would.
        let py = "import boto3\nsqs.send_message(QueueUrl=ORDERS_QUEUE_URL, MessageBody=b)\n";
        let f = folded(
            py,
            &[(
                "ORDERS_QUEUE_URL",
                "https://sqs.us-east-1.amazonaws.com/123456789012/orders",
            )],
        );
        assert_eq!(
            qnames(&f.producers),
            vec!["queue_producer:orders".to_string()]
        );

        // Every one of these reads as the sentinel on the per-file path.
        for (src, want) in [
            (ts, "queue_producer:unresolved:kafka"),
            (java, "queue_producer:unresolved:kafka"),
            (go, "queue_producer:unresolved:nats"),
        ] {
            assert_eq!(producers(src), vec![want.to_string()], "{src}");
        }
        assert_eq!(
            consumers(listener),
            vec!["queue_consumer:unresolved:kafka".to_string()]
        );
    }

    #[test]
    fn mixed_literal_and_constant_sites_in_one_file() {
        let src = "import { Kafka } from 'kafkajs';\nawait producer.send({ topic: 'audit', messages });\nawait producer.send({ topic: ORDERS_TOPIC, messages });\n";
        // Per-file: the literal site names a topic, so the constant site is
        // simply lost — no sentinel, no orders.
        assert_eq!(producers(src), vec!["queue_producer:audit".to_string()]);
        let f = folded(src, &[("ORDERS_TOPIC", "orders")]);
        assert_eq!(
            qnames(&f.producers),
            vec![
                "queue_producer:audit".to_string(),
                "queue_producer:orders".to_string()
            ]
        );
        assert_eq!(
            f.counts,
            ConstFoldCounts {
                folded: 1,
                unresolved: 0
            }
        );
    }

    #[test]
    fn generic_verb_rows_never_resolve() {
        // `.subscribe(` / `.publish(` are shared with Rx and in-process buses;
        // their topic must LEAD the call as a literal, and a resolver cannot
        // relax that guard.
        let src = "const redis = require('redis');\nsub.subscribe(CHANNEL);\npub.publish(CHANNEL, msg);\n";
        let f = folded(src, &[("CHANNEL", "orders")]);
        assert_eq!(f.counts, ConstFoldCounts::default());
        assert!(!qnames(&f.consumers).iter().any(|q| q.ends_with(":orders")));
        assert!(!qnames(&f.producers).iter().any(|q| q.ends_with(":orders")));
        assert_eq!(qnames(&f.consumers), consumers(src));
        assert_eq!(qnames(&f.producers), producers(src));
    }

    #[test]
    fn unresolvable_expr_keeps_the_sentinel() {
        let src =
            "import { Kafka } from 'kafkajs';\nawait producer.send({ topic: topic, messages });\n";
        let f = folded(src, &[("OTHER", "orders")]);
        assert_eq!(
            qnames(&f.producers),
            vec!["queue_producer:unresolved:kafka".to_string()]
        );
        assert_eq!(
            f.counts,
            ConstFoldCounts {
                folded: 0,
                unresolved: 1
            }
        );
        assert_eq!(f.producers.nodes[0].confidence, Confidence::Weak);
        // A value the topic fold rejects (a placeholder queue name) is no topic.
        let sqs = "import boto3\nsqs.send_message(QueueUrl=QUEUE_URL, MessageBody=b)\n";
        let f = folded(
            sqs,
            &[(
                "QUEUE_URL",
                "https://sqs.us-east-1.amazonaws.com/1/${QUEUE}",
            )],
        );
        assert_eq!(
            qnames(&f.producers),
            vec!["queue_producer:unresolved:sqs".to_string()]
        );
        assert_eq!(
            f.counts,
            ConstFoldCounts {
                folded: 0,
                unresolved: 1
            }
        );
    }

    /// One file's parse as the engine assembles it: a language-parser MODULE
    /// and FUNCTION with an edge, then the queue extractors (consumers, then
    /// producers), then a later extractor's node and edge, then the A5.8
    /// anchor pass over the queue anchors (LE.4c); with `imports`, the
    /// router's raw IMPORTS cell on every node.
    fn file_parse(src: &str, imports: bool) -> FileParse {
        file_parse_spanned(src, imports, None)
    }

    /// [`file_parse`] with the FUNCTION located at `span` (0-indexed lines),
    /// so the anchor pass finds an owner for the queue sites inside it.
    fn file_parse_spanned(src: &str, imports: bool, span: Option<(u32, u32)>) -> FileParse {
        let module = module_id();
        let func = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "test::publish");
        let later = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CRON_JOB, "cron:nightly");
        let bare = |id| Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: Vec::new(),
        };
        let mut located = bare(func);
        if let Some((start, end)) = span {
            located.cells.push(Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(
                    r#"{{"file":"{PATH}","start_line":{start},"end_line":{end}}}"#
                )),
            });
        }
        let contains = |to| Edge {
            from: module,
            to,
            category: edge_category::CONTAINS,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        };
        let mut fp = FileParse {
            nodes: vec![bare(module), located],
            edges: vec![contains(func)],
            ..Default::default()
        };
        fp.nav
            .record(module, "test", "test", node_kind::MODULE, None);
        fp.nav.record(
            func,
            "publish",
            "test::publish",
            node_kind::FUNCTION,
            Some(module),
        );
        let mut anchors = Vec::new();
        for out in [
            extract_queue_consumer_nodes(src, PATH, module, repo()),
            extract_queue_producer_nodes(src, PATH, module, repo()),
        ] {
            fp.nodes.extend(out.nodes);
            fp.edges.extend(out.edges);
            anchors.extend(out.anchors);
            merge(&mut fp.nav, out.nav);
        }
        fp.nodes.push(bare(later));
        fp.edges.push(contains(later));
        fp.nav.record(
            later,
            "nightly",
            "cron:nightly",
            node_kind::CRON_JOB,
            Some(module),
        );
        anchor::attach(&mut fp, PATH, module, &mut anchors);
        if imports {
            fp.imports.push(glia_code_domain::ImportStmt {
                from_module: "test".into(),
                target: glia_code_domain::ImportTarget::Module {
                    path: "kafkajs".into(),
                    alias: None,
                },
                line: 0,
            });
            attach_imports_cell(&mut fp, "typescript");
        }
        fp
    }

    /// The engine's `merge_nav`, for the test's hand-assembled parse.
    fn merge(dst: &mut CodeNav, src: CodeNav) {
        dst.name_by_id.extend(src.name_by_id);
        dst.qname_by_id.extend(src.qname_by_id);
        dst.kind_by_id.extend(src.kind_by_id);
        dst.parent_of.extend(src.parent_of);
        for (k, v) in src.children_of {
            dst.children_of.entry(k).or_default().extend(v);
        }
    }

    /// The constant spelling and the literal spelling of the same file; the
    /// lines match, so a fold of one must lay out exactly as the other.
    const CONST_SRC: &str = "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'payments' });\nawait producer.send({ topic: ORDERS_TOPIC, messages });\n";
    const LITERAL_SRC: &str = "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'payments' });\nawait producer.send({ topic: 'orders', messages });\n";

    fn fold_in_place(imports: bool) -> (FileParse, FileParse) {
        let mut fp = file_parse(CONST_SRC, imports);
        let fold = folded(CONST_SRC, &[("ORDERS_TOPIC", "orders")]);
        assert_eq!(fold.counts.folded, 1);
        replace_queue_nodes(&mut fp, module_id(), "typescript", fold);
        (fp, file_parse(LITERAL_SRC, imports))
    }

    #[test]
    fn replace_queue_nodes_swaps_nodes_edges_and_nav() {
        let before = file_parse(CONST_SRC, false);
        let sentinel = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::QUEUE_PRODUCER,
            "queue_producer:unresolved:kafka",
        );
        assert!(before.nav.kind_by_id.contains_key(&sentinel));

        let (fp, literal) = fold_in_place(false);
        // Nodes, edges and child order match the literal-topic parse exactly:
        // the replacements went back where the removed queue nodes were.
        assert_eq!(fp.nodes, literal.nodes);
        assert_eq!(fp.edges, literal.edges);
        assert_eq!(fp.nav.children_of, literal.nav.children_of);
        assert_eq!(fp.nav.qname_by_id, literal.nav.qname_by_id);
        assert_eq!(fp.nav.name_by_id, literal.nav.name_by_id);
        assert_eq!(fp.nav.kind_by_id, literal.nav.kind_by_id);
        assert_eq!(fp.nav.parent_of, literal.nav.parent_of);
        // The sentinel is gone from every index; nothing else was touched.
        assert!(!fp.nodes.iter().any(|n| n.id == sentinel));
        assert!(!fp.edges.iter().any(|e| e.to == sentinel));
        assert!(!fp.nav.parent_of.contains_key(&sentinel));
        assert_eq!(fp.nodes.first(), before.nodes.first());
        assert_eq!(fp.nodes.last(), before.nodes.last());
        assert_eq!(fp.edges.last(), before.edges.last());
    }

    #[test]
    fn replace_queue_nodes_keeps_the_imports_cell() {
        let (fp, literal) = fold_in_place(true);
        assert_eq!(fp.nodes, literal.nodes);
        let orders = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::QUEUE_PRODUCER,
            "queue_producer:orders",
        );
        let node = fp
            .nodes
            .iter()
            .find(|n| n.id == orders)
            .expect("folded node");
        let imports: Vec<_> = node
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::IMPORTS)
            .collect();
        assert_eq!(imports.len(), 1, "exactly one IMPORTS cell");
        assert_eq!(node.cells.last().map(|c| c.kind), Some(cell_type::IMPORTS));
        assert_eq!(payload(imports[0]), r#"["kafkajs"]"#);
        // A parse that never carried the cell does not gain one.
        let (bare, _) = fold_in_place(false);
        assert!(
            bare.nodes
                .iter()
                .all(|n| n.cells.iter().all(|c| c.kind != cell_type::IMPORTS))
        );
    }

    // ---- LE.4c: queue markers anchored to their functions ----------------

    fn anchors_of(r: &QueueNodes) -> Vec<(String, u32)> {
        r.anchors
            .iter()
            .map(|a| {
                let q = r.nav.qname_by_id.get(&a.node).cloned().unwrap_or_default();
                (q, a.line)
            })
            .collect()
    }

    #[test]
    fn every_queue_site_is_an_anchor() {
        // `orders` sent from two functions, `payments` once, one subscribe.
        let src = "import { Kafka } from 'kafkajs';\nasync function a() { await producer.send({ topic: 'orders', messages }); }\nasync function b() {\n  await producer.send({ topic: 'payments', messages });\n  await producer.send({ topic: 'orders', messages });\n}\nasync function c() { await consumer.subscribe({ topic: 'payments' }); }\n";
        let p = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            anchors_of(&p),
            vec![
                ("queue_producer:orders".to_string(), 1),
                ("queue_producer:orders".to_string(), 4),
                ("queue_producer:payments".to_string(), 3),
            ],
            "node order, then site order: both `orders` sites anchor"
        );
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            anchors_of(&c),
            vec![("queue_consumer:payments".to_string(), 6)]
        );
        // The A2.8 provenance is unchanged: one module CONTAINS per node.
        assert_eq!(p.edges.len(), 2);
        assert!(
            p.edges
                .iter()
                .all(|e| e.from == module_id() && e.category == edge_category::CONTAINS)
        );
    }

    #[test]
    fn anchors_are_not_capped_like_the_provenance_sites() {
        let mut src = String::from("import { Kafka } from 'kafkajs';\n");
        for _ in 0..(MAX_SITES + 4) {
            src.push_str("await producer.send({ topic: 'orders', messages });\n");
        }
        let p = extract_queue_producer_nodes(&src, PATH, module_id(), repo());
        assert_eq!(p.anchors.len(), MAX_SITES + 4);
        let code = payload(cell_of(&p.nodes[0], cell_type::CODE)).to_string();
        assert_eq!(code.matches("\"line\":").count(), MAX_SITES);
    }

    #[test]
    fn the_framework_tag_anchors_at_the_site_that_minted_it() {
        let src = "import { Kafka } from 'kafkajs';\n\nawait producer.send({ topic: topic, messages });\n";
        let p = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            anchors_of(&p),
            vec![("queue_producer:unresolved:kafka".to_string(), 2)]
        );
    }

    #[test]
    fn replace_queue_nodes_re_anchors_the_folded_nodes() {
        // The FUNCTION spans both sites (lines 1 and 2), so the per-file parse
        // has `payments -HANDLED_BY-> publish` and `publish -USES->` the
        // sentinel; the fold must drop the sentinel's owner edge and anchor
        // `orders` in its place.
        let func = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "test::publish");
        let sentinel = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::QUEUE_PRODUCER,
            "queue_producer:unresolved:kafka",
        );
        let orders = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::QUEUE_PRODUCER,
            "queue_producer:orders",
        );
        let payments = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::QUEUE_CONSUMER,
            "queue_consumer:payments",
        );
        let mut fp = file_parse_spanned(CONST_SRC, false, Some((1, 2)));
        assert!(
            fp.edges
                .iter()
                .any(|e| (e.from, e.to, e.category) == (func, sentinel, edge_category::USES))
        );
        let fold = folded(CONST_SRC, &[("ORDERS_TOPIC", "orders")]);
        assert_eq!(fold.path, PATH);
        replace_queue_nodes(&mut fp, module_id(), "typescript", fold);
        let literal = file_parse_spanned(LITERAL_SRC, false, Some((1, 2)));

        type Triple = (NodeId, NodeId, glia_core::EdgeCategoryId);
        let triples = |fp: &FileParse| -> Vec<Triple> {
            fp.edges
                .iter()
                .map(|e| (e.from, e.to, e.category))
                .collect()
        };
        assert_eq!(
            triples(&fp),
            triples(&literal),
            "laid out as the literal file"
        );
        assert!(triples(&fp).contains(&(func, orders, edge_category::USES)));
        assert!(triples(&fp).contains(&(payments, func, edge_category::HANDLED_BY)));
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.from == sentinel || e.to == sentinel)
        );
        assert_eq!(fp.nodes, literal.nodes);

        // The re-anchored owner edges carry the post-cache anchor stamp; the
        // literal parse's are unstamped in this hand-built harness.
        let owner: Vec<evidence::Evidence> = fp
            .edges
            .iter()
            .filter(|e| e.category != edge_category::CONTAINS)
            .filter_map(evidence::Evidence::of)
            .collect();
        assert_eq!(owner.len(), 2);
        assert!(
            owner
                .iter()
                .all(|ev| ev.emitter == "extractor:anchor"
                    && ev.rule.as_deref() == Some("const_fold"))
        );
        assert_eq!(anchor::census(&fp), anchor::census(&literal));
    }

    // ---- LA.33: consumer callbacks -> HANDLED_BY the handler --------------

    fn name(h: &str) -> HandlerExpr {
        HandlerExpr::Name(h.to_string())
    }

    fn member(base: &str, h: &str) -> HandlerExpr {
        HandlerExpr::Member {
            base: base.to_string(),
            name: h.to_string(),
        }
    }

    /// `(consumer qname, line, handler, inline)` per callback.
    fn callbacks_of(r: &QueueNodes) -> Vec<(String, u32, HandlerExpr, bool)> {
        r.callbacks
            .iter()
            .map(|c| {
                let q = r.nav.qname_by_id.get(&c.consumer).cloned().unwrap_or_default();
                (q, c.line, c.handler.clone(), c.inline)
            })
            .collect()
    }

    fn cb(q: &str, line: u32, h: HandlerExpr, inline: bool) -> (String, u32, HandlerExpr, bool) {
        (q.to_string(), line, h, inline)
    }

    #[test]
    fn handler_rules_name_consumer_needles() {
        for (needle, _) in HANDLER_RULES {
            assert!(
                CONSUMER_PATTERNS.iter().any(|(n, ..)| n == needle),
                "{needle} is not a CONSUMER_PATTERNS needle"
            );
            assert!(
                !PRODUCER_PATTERNS.iter().any(|(n, ..)| n == needle),
                "{needle} is a producer needle"
            );
        }
        // A producer never carries callbacks, even when a consumer needle's
        // text appears in its file.
        let src = "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'a' });\nawait consumer.run({ eachMessage: h });\nawait producer.send({ topic: 'b', messages });\n";
        let p = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert!(p.callbacks.is_empty());
    }

    #[test]
    fn handler_expr_forms() {
        let bare = |t: &str| handler_expr(t).map(|(h, _)| h);
        let inline = |t: &str| handler_expr(t).filter(|(_, i)| *i).map(|(h, _)| h);
        assert_eq!(handler_expr(" onOrder "), Some((name("onOrder"), false)));
        assert_eq!(bare("handlers.onOrder"), Some(member("handlers", "onOrder")));
        assert_eq!(bare("w.handle"), Some(member("w", "handle")));
        assert_eq!(
            bare("this.handle"),
            Some(HandlerExpr::SelfMember("handle".into()))
        );
        assert_eq!(
            bare("self.on_message"),
            Some(HandlerExpr::SelfMember("on_message".into()))
        );
        assert_eq!(
            handler_expr("this.handle.bind(this)"),
            Some((HandlerExpr::SelfMember("handle".into()), false))
        );
        // One-call inline functions bind their callee, marked inline.
        for t in [
            "(job) => sendEmail(job)",
            "async (job) => await sendEmail(job)",
            "job => sendEmail(job)",
            "async job => sendEmail(job, 1)",
            "({ message }) => { sendEmail(message); }",
            "async (p) => { await sendEmail(p) }",
            "(p) => { return sendEmail(p); }",
            "function (p) { sendEmail(p); }",
            "async function named(p) {\n  return await sendEmail(p);\n}",
            "func(m *nats.Msg) { sendEmail(m) }",
            "lambda ch, method, props, body: sendEmail(body)",
        ] {
            assert_eq!(inline(t), Some(name("sendEmail")), "{t}");
        }
        assert_eq!(
            inline("(m) => this.handle(m)"),
            Some(HandlerExpr::SelfMember("handle".into()))
        );
        assert_eq!(
            inline("func(m *nats.Msg) { w.handle(m) }"),
            Some(member("w", "handle"))
        );
        // Anything else stays unread.
        for t in [
            "(p) => { a(p); b(p); }",
            "(p) => {\n  a(p)\n  b(p)\n}",
            "(p) => a(p).then(b)",
            "(p) => a.b.c(p)",
            "a.b.c",
            "handlers[name]",
            "makeHandler(cfg)",
            "'orders'",
            "{ noAck: true }",
            "this",
            "null",
            "(p) => {",
            "lambda: ",
            "π => f(π)",
        ] {
            assert_eq!(handler_expr(t), None, "{t}");
        }
    }

    #[test]
    fn kafkajs_run_pairs_by_receiver_chain() {
        // Two receivers in ONE file: `consumer` and `this.consumer` pair with
        // their own run() only.
        let src = "import { Kafka } from 'kafkajs';\n\
await consumer.subscribe({ topic: 'payments' });\n\
await consumer.run({ eachMessage: onPayment });\n\
class R {\n\
  async start() {\n\
    await this.consumer.subscribe({ topic: 'refunds' });\n\
    await this.consumer.run({ eachBatch: this.onBatch.bind(this) });\n\
  }\n\
}\n\
await myconsumer.run({ eachMessage: notMine });\n";
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:payments", 2, name("onPayment"), false),
                cb(
                    "queue_consumer:refunds",
                    6,
                    HandlerExpr::SelfMember("onBatch".into()),
                    false
                ),
            ]
        );
        // One run() serves every topic its receiver subscribed, and a topic
        // subscribed twice keeps one callback.
        let src = "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'a' });\nawait consumer.subscribe({ topic: 'b' });\nawait consumer.subscribe({ topic: 'a' });\nawait consumer.run({\n  eachMessage: async ({ message }) => handle(message),\n});\n";
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:a", 4, name("handle"), true),
                cb("queue_consumer:b", 4, name("handle"), true),
            ]
        );
    }

    #[test]
    fn amqplib_worker_and_nats_arg_indexes() {
        let src = "import amqp from 'amqplib';\nimport { Worker } from 'bullmq';\nimport { connect } from 'nats';\nchannel.consume('orders', onOrder, { noAck: true });\nchannel.consume('audit', (msg) => { audit(msg); record(msg); });\nnew Worker('emails', (job) => sendEmail(job), { connection });\nnc.subscribe('events', { callback: (err, msg) => onEvent(msg) });\nnc.subscribe('plain');\nnc.subscribe('v1', onV1);\nnc.subscribe('opts', { queue: 'workers' });\n";
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:emails", 5, name("sendEmail"), true),
                cb("queue_consumer:events", 6, name("onEvent"), true),
                cb("queue_consumer:v1", 8, name("onV1"), false),
                cb("queue_consumer:orders", 3, name("onOrder"), false),
            ],
            "table order (BullMQ, NATS, RabbitMQ), then site order; the two-statement arrow, the callback-less subscribe and an options-only object read nothing"
        );
        let go = "import \"github.com/nats-io/nats.go\"\nfunc main() {\n\tnc.Subscribe(\"orders\", onOrder)\n\tnc.QueueSubscribe(\"audit\", \"workers\", func(m *nats.Msg) { onAudit(m) })\n\tw.nc.Subscribe(\"refunds\", w.handle)\n}\n";
        let c = extract_queue_consumer_nodes(go, PATH, module_id(), repo());
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:orders", 2, name("onOrder"), false),
                cb("queue_consumer:refunds", 4, member("w", "handle"), false),
                cb("queue_consumer:audit", 3, name("onAudit"), true),
            ]
        );
    }

    #[test]
    fn pika_keyed_and_positional() {
        let src = "import pika\nch.basic_consume(queue='orders', on_message_callback=on_order, auto_ack=True)\nch.basic_consume('audit', lambda ch, method, props, body: on_audit(body))\nch.basic_consume(queue='jobs', on_message_callback=self.on_job)\n";
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:orders", 1, name("on_order"), false),
                cb("queue_consumer:audit", 2, name("on_audit"), true),
                cb(
                    "queue_consumer:jobs",
                    3,
                    HandlerExpr::SelfMember("on_job".into()),
                    false
                ),
            ]
        );
        // A keyword lambda keeps its own commas too.
        let src = "import pika\nch.basic_consume(queue='orders', on_message_callback=lambda c, m, p, b: on_order(b), auto_ack=True)\n";
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            callbacks_of(&c),
            vec![cb("queue_consumer:orders", 1, name("on_order"), true)]
        );
    }

    #[test]
    fn a_sentinel_consumer_keeps_every_sites_callback() {
        let src = "import amqp from 'amqplib';\nchannel.consume(queueA, onA);\nchannel.consume(queueB, onB);\n";
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:unresolved:rabbitmq", 1, name("onA"), false),
                cb("queue_consumer:unresolved:rabbitmq", 2, name("onB"), false),
            ]
        );
    }

    /// POSITION span cell on `id`.
    fn spanned(id: NodeId, span: (u32, u32)) -> Node {
        Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(
                    r#"{{"file":"{PATH}","start_line":{},"end_line":{}}}"#,
                    span.0, span.1
                )),
            }],
        }
    }

    /// A MODULE holding a CLASS `Svc` (lines `class`) whose METHODs are
    /// `methods` (name, span), then the file's queue nodes, anchored, with
    /// the consumers' callbacks bound exactly as the engine binds them.
    fn class_parse(src: &str, class: (u32, u32), methods: &[(&str, (u32, u32))]) -> FileParse {
        let module = module_id();
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "test::Svc");
        let mut fp = FileParse {
            nodes: vec![spanned(module, (0, 100)), spanned(class_id, class)],
            ..Default::default()
        };
        fp.nav.record(module, "test", "test", node_kind::MODULE, None);
        fp.nav
            .record(class_id, "Svc", "test::Svc", node_kind::CLASS, Some(module));
        for (m, span) in methods {
            let q = format!("test::Svc::{m}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, &q);
            fp.nodes.push(spanned(id, *span));
            fp.nav.record(id, m, &q, node_kind::METHOD, Some(class_id));
        }
        let mut anchors = Vec::new();
        let mut callbacks = Vec::new();
        for out in [
            extract_queue_consumer_nodes(src, PATH, module, repo()),
            extract_queue_producer_nodes(src, PATH, module, repo()),
        ] {
            fp.nodes.extend(out.nodes);
            fp.edges.extend(out.edges);
            anchors.extend(out.anchors);
            callbacks.extend(out.callbacks);
            merge(&mut fp.nav, out.nav);
        }
        anchor::attach(&mut fp, PATH, module, &mut anchors);
        bind_consumer_callbacks(&mut fp, module, &callbacks);
        fp
    }

    fn qid(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn handled(fp: &FileParse, from: NodeId) -> Vec<NodeId> {
        fp.edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::HANDLED_BY)
            .map(|e| e.to)
            .collect()
    }

    fn refs_from(fp: &FileParse, from: NodeId) -> Vec<glia_code_domain::CallQualifier> {
        fp.refs
            .iter()
            .filter(|r| r.from == from && r.category == edge_category::HANDLED_BY)
            .map(|r| r.qualifier.clone())
            .collect()
    }

    #[test]
    fn bind_self_member_uses_the_owner_class() {
        let src = "import { Kafka } from 'kafkajs';\nclass Svc {\n  async start() {\n    await this.consumer.subscribe({ topic: 'refunds' });\n    await this.consumer.run({ eachMessage: this.handle.bind(this) });\n  }\n  async handle(p) { return p; }\n}\nawait other.subscribe({ topic: 'x' });\n";
        let fp = class_parse(src, (1, 7), &[("start", (2, 5)), ("handle", (6, 6))]);
        let refunds = qid(node_kind::QUEUE_CONSUMER, "queue_consumer:refunds");
        let start = qid(node_kind::METHOD, "test::Svc::start");
        let handle = qid(node_kind::METHOD, "test::Svc::handle");
        assert_eq!(
            handled(&fp, refunds),
            vec![start, handle],
            "LE.4c's owner edge, then the callback"
        );
        assert!(refs_from(&fp, refunds).is_empty());
        let edge = fp
            .edges
            .iter()
            .find(|e| e.from == refunds && e.to == handle)
            .expect("callback edge");
        assert_eq!(edge.confidence, Confidence::Medium);
        let ev = evidence::Evidence::of(edge).expect("evidence");
        assert_eq!(ev.emitter, "extractor:queue_callbacks");
        assert_eq!(ev.rule.as_deref(), Some("self"));
        assert_eq!((ev.file.as_deref(), ev.line), (Some(PATH), Some(4)));

        // `this.x` with no such method on the class binds nothing.
        let src = "import { Kafka } from 'kafkajs';\nclass Svc {\n  async start() {\n    await this.consumer.subscribe({ topic: 'refunds' });\n    await this.consumer.run({ eachMessage: this.missing });\n  }\n}\n";
        let mut fp = class_parse(src, (1, 6), &[("start", (2, 5))]);
        assert_eq!(handled(&fp, refunds), vec![start]);
        assert!(refs_from(&fp, refunds).is_empty());
        let cbs = extract_queue_consumer_nodes(src, PATH, module_id(), repo()).callbacks;
        let stats = bind_consumer_callbacks(&mut fp, module_id(), &cbs);
        assert_eq!(
            stats,
            CallbackStats {
                consumers: 1,
                unbound: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn member_on_import_binding_becomes_a_ref() {
        use glia_code_domain::{CallQualifier, ImportStmt, ImportTarget};
        // `handlers` is an import binding, so `handlers.onOrder` goes to the
        // resolver even though the enclosing class declares an `onOrder`;
        // `w.onOrder` is not, so it binds to the class's own method.
        let src = "import amqp from 'amqplib';\nclass Svc {\n  start() {\n    channel.consume('orders', handlers.onOrder);\n    channel.consume('audit', w.onOrder);\n    channel.consume('jobs', onJob);\n  }\n  onOrder(m) {}\n}\n";
        let mut fp = class_parse(src, (1, 8), &[("start", (2, 6)), ("onOrder", (7, 7))]);
        let orders = qid(node_kind::QUEUE_CONSUMER, "queue_consumer:orders");
        let audit = qid(node_kind::QUEUE_CONSUMER, "queue_consumer:audit");
        let jobs = qid(node_kind::QUEUE_CONSUMER, "queue_consumer:jobs");
        let own = qid(node_kind::METHOD, "test::Svc::onOrder");
        // Without the import, `handlers.onOrder` is a method value too.
        assert!(handled(&fp, orders).contains(&own));

        fp.imports.push(ImportStmt {
            from_module: "test".into(),
            target: ImportTarget::Module {
                path: "./handlers".into(),
                alias: Some("handlers".into()),
            },
            line: 0,
        });
        fp.edges
            .retain(|e| !(e.category == edge_category::HANDLED_BY && e.to == own));
        let cbs = extract_queue_consumer_nodes(src, PATH, module_id(), repo()).callbacks;
        let stats = bind_consumer_callbacks(&mut fp, module_id(), &cbs);
        assert!(!handled(&fp, orders).contains(&own));
        assert_eq!(
            refs_from(&fp, orders),
            vec![CallQualifier::Attribute {
                base: "handlers".into(),
                name: "onOrder".into()
            }]
        );
        assert!(handled(&fp, audit).contains(&own));
        let ev = fp
            .edges
            .iter()
            .find(|e| e.from == audit && e.to == own)
            .and_then(evidence::Evidence::of)
            .expect("evidence");
        assert_eq!(ev.rule.as_deref(), Some("method_value"));
        assert_eq!(refs_from(&fp, jobs), vec![CallQualifier::Bare("onJob".into())]);
        let r = fp.refs.iter().find(|r| r.from == jobs).expect("ref");
        assert_eq!((r.from_module, r.line), (module_id(), 5));
        assert_eq!(
            stats,
            CallbackStats {
                consumers: 3,
                direct: 1,
                refs: 2,
                inline: 0,
                unbound: 0
            }
        );
        // A second bind of the same callbacks adds nothing.
        let (edges, refs) = (fp.edges.len(), fp.refs.len());
        bind_consumer_callbacks(&mut fp, module_id(), &cbs);
        assert_eq!((fp.edges.len(), fp.refs.len()), (edges, refs));
    }

    #[test]
    fn replace_queue_nodes_leaves_no_dangling_edge_or_ref() {
        // `payments` is a literal kafkajs consumer (same id before and after
        // the fold) with a `this.onPayment` callback; the amqplib consumer
        // names its queue by a constant, so the per-file parse mints the
        // `unresolved:rabbitmq` sentinel, which the fold replaces by `orders`.
        let src = "import { Kafka } from 'kafkajs';\nimport amqp from 'amqplib';\nclass Svc {\n  async start() {\n    await consumer.subscribe({ topic: 'payments' });\n    await consumer.run({ eachMessage: this.onPayment });\n    channel.consume(ORDERS_QUEUE, onOrder);\n  }\n  onPayment(p) {}\n}\n";
        let mut fp = class_parse(src, (2, 9), &[("start", (3, 7)), ("onPayment", (8, 8))]);
        let sentinel = qid(
            node_kind::QUEUE_CONSUMER,
            "queue_consumer:unresolved:rabbitmq",
        );
        let payments = qid(node_kind::QUEUE_CONSUMER, "queue_consumer:payments");
        let orders = qid(node_kind::QUEUE_CONSUMER, "queue_consumer:orders");
        let start = qid(node_kind::METHOD, "test::Svc::start");
        let on_payment = qid(node_kind::METHOD, "test::Svc::onPayment");
        let on_order = glia_code_domain::CallQualifier::Bare("onOrder".into());
        assert_eq!(handled(&fp, sentinel), vec![start]);
        assert_eq!(refs_from(&fp, sentinel), vec![on_order.clone()]);
        assert_eq!(handled(&fp, payments), vec![start, on_payment]);

        let fold = folded(src, &[("ORDERS_QUEUE", "orders")]);
        assert_eq!(fold.counts.folded, 1);
        replace_queue_nodes(&mut fp, module_id(), "typescript", fold);

        assert!(
            fp.edges.iter().all(|e| e.from != sentinel && e.to != sentinel),
            "no edge names the sentinel"
        );
        assert!(
            fp.refs.iter().all(|r| r.from != sentinel),
            "no ref names the sentinel"
        );
        assert!(!fp.nav.kind_by_id.contains_key(&sentinel));
        // The literal consumer keeps exactly one HANDLED_BY per target.
        assert_eq!(handled(&fp, payments), vec![start, on_payment]);
        let cb_edge = fp
            .edges
            .iter()
            .find(|e| e.from == payments && e.to == on_payment)
            .and_then(evidence::Evidence::of)
            .expect("evidence");
        assert_eq!(cb_edge.emitter, "extractor:queue_callbacks");
        // The folded consumer carries the owner edge and the callback ref.
        assert_eq!(handled(&fp, orders), vec![start]);
        assert_eq!(refs_from(&fp, orders), vec![on_order]);
        assert_eq!(fp.refs.len(), 1);
    }

    // ---- CL.1: queue row semantics -----------------------------------------

    /// The file's queue qnames in EMITTED order (not sorted).
    fn ordered(r: &QueueNodes) -> Vec<String> {
        r.nodes
            .iter()
            .filter_map(|n| r.nav.qname_by_id.get(&n.id).cloned())
            .collect()
    }

    fn at(src: &str, path: &str) -> (Vec<String>, Vec<String>) {
        (
            qnames(&extract_queue_consumer_nodes(
                src,
                path,
                module_id(),
                repo(),
            )),
            qnames(&extract_queue_producer_nodes(
                src,
                path,
                module_id(),
                repo(),
            )),
        )
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn declaring_a_queue_is_not_consuming_it() {
        // matrix/python/amqp's producer: it declares the queue it publishes to.
        let pika = "import pika\n\nchannel = connection.channel()\nchannel.queue_declare(queue=\"orders\")\n\n\ndef publish_order(body):\n    channel.basic_publish(exchange=\"\", routing_key=\"orders\", body=body)\n";
        assert_eq!(
            at(pika, "server/producer.py"),
            (vec![], strs(&["queue_producer:orders"]))
        );
        // matrix/csharp/amqp's producer, RabbitMQ.Client v6.
        let send = "using RabbitMQ.Client;\nclass P {\n  void Publish(string p) {\n    _channel.QueueDeclare(queue: \"orders\", durable: true, exclusive: false, autoDelete: false, arguments: null);\n    _channel.BasicPublish(exchange: \"\", routingKey: \"orders\", basicProperties: null, body: b);\n  }\n}\n";
        assert_eq!(
            at(send, "server/Producer.cs"),
            (vec![], strs(&["queue_producer:orders"]))
        );
        // The consumer is found by `.BasicConsume(` now, not by the declare.
        let recv = "using RabbitMQ.Client;\nclass C {\n  void Run() {\n    _channel.QueueDeclare(queue: \"orders\", durable: true, exclusive: false, autoDelete: false, arguments: null);\n    var consumer = new EventingBasicConsumer(_channel);\n    _channel.BasicConsume(queue: \"orders\", autoAck: true, consumer: consumer);\n  }\n}\n";
        let c = extract_queue_consumer_nodes(recv, "client/Consumer.cs", module_id(), repo());
        assert_eq!(qnames(&c), strs(&["queue_consumer:orders"]));
        let lines: Vec<u32> = c.anchors.iter().map(|a| a.line).collect();
        assert_eq!(
            lines,
            vec![5],
            "the consume call is the one site, never the declare"
        );
        // A consumer of a queue declared elsewhere, positional, and v7 async.
        let elsewhere =
            "using RabbitMQ.Client;\n_channel.BasicConsume(\"orders\", true, consumer);\n";
        assert_eq!(at(elsewhere, "W.cs").0, strs(&["queue_consumer:orders"]));
        let v7 = "using RabbitMQ.Client;\nawait channel.QueueDeclareAsync(queue: \"hello\", durable: false);\nawait channel.BasicConsumeAsync(\"hello\", autoAck: true, consumer: consumer);\nawait channel.BasicPublishAsync(exchange: string.Empty, routingKey: \"hello\", body: body);\n";
        assert_eq!(
            at(v7, "V7.cs"),
            (
                strs(&["queue_consumer:hello"]),
                strs(&["queue_producer:hello"])
            )
        );
    }

    #[test]
    fn declared_queue_names_an_unnamed_consume() {
        let src = "import pika\nchannel.queue_declare(queue=\"jobs\")\n\ndef run(qname):\n    channel.basic_consume(queue=qname, on_message_callback=cb)\n";
        let c = extract_queue_consumer_nodes(src, "w.py", module_id(), repo());
        assert_eq!(qnames(&c), strs(&["queue_consumer:jobs"]));
        // The site is the CONSUME call (line 4), so LE.4c anchors it to `run`.
        let lines: Vec<u32> = c.anchors.iter().map(|a| a.line).collect();
        assert_eq!(lines, vec![4]);
        assert_eq!(
            callbacks_of(&c),
            vec![cb("queue_consumer:jobs", 4, name("cb"), false)]
        );
        // Two declared queues: ambiguous, nothing is bound and the tag stays.
        let two = "import pika\nchannel.queue_declare(queue=\"a\")\nchannel.queue_declare(queue=\"b\")\nchannel.basic_consume(queue=qname, on_message_callback=cb)\n";
        let c = extract_queue_consumer_nodes(two, "w.py", module_id(), repo());
        assert_eq!(qnames(&c), strs(&["queue_consumer:unresolved:rabbitmq"]));
        assert_eq!(
            callbacks_of(&c),
            vec![cb(
                "queue_consumer:unresolved:rabbitmq",
                3,
                name("cb"),
                false
            )]
        );
        // A server-named declaration (`queue=''`) is a declared queue too.
        let anon = "import pika\nresult = channel.queue_declare(queue='', exclusive=True)\nchannel.queue_declare(queue='orders')\nchannel.basic_consume(queue=result.method.queue, on_message_callback=cb)\n";
        assert_eq!(
            at(anon, "w.py").0,
            strs(&["queue_consumer:unresolved:rabbitmq"])
        );
        // A consume that names its queue is not rebound to the declared one.
        let named = "import pika\nchannel.queue_declare(queue='jobs')\nchannel.basic_consume(queue='audit', on_message_callback=cb)\n";
        assert_eq!(at(named, "w.py").0, strs(&["queue_consumer:audit"]));
    }

    #[test]
    fn same_call_read_by_two_rows_mints_no_tag() {
        // `channel.basic_publish` (ArgLiteral over arg #0, pika's exchange)
        // and `basic_publish(` (Keyed routing_key) read ONE call.
        let src = "import pika\n\nchannel.queue_declare(queue=\"orders\")\n\n\ndef publish_order(body):\n    channel.basic_publish(exchange=\"\", routing_key=\"orders\", body=body)\n";
        let p = extract_queue_producer_nodes(src, "server/producer.py", module_id(), repo());
        assert_eq!(ordered(&p), strs(&["queue_producer:orders"]));
        // One node, one site: the anchor list holds the call's line once.
        let lines: Vec<u32> = p.anchors.iter().map(|a| a.line).collect();
        assert_eq!(lines, vec![6]);
    }

    #[test]
    fn liveness_call_beside_a_named_node_mints_no_tag() {
        // `new KafkaConsumer()` carries no identifier expression; the file
        // names its Kafka topic on `consumer.subscribe`.
        let src = "import { Kafka } from 'kafkajs';\nconst c = new KafkaConsumer();\nawait consumer.subscribe({ topic: 'orders' });\n";
        let c = extract_queue_consumer_nodes(src, PATH, module_id(), repo());
        assert_eq!(ordered(&c), strs(&["queue_consumer:orders"]));
        // Without the named node the liveness call is unexplained: tag kept.
        let alone = "import { Kafka } from 'kafkajs';\nconst c = new KafkaConsumer();\n";
        assert_eq!(consumers(alone), strs(&["queue_consumer:unresolved:kafka"]));
        // A named node of ANOTHER framework explains nothing.
        let other = "import { Kafka } from 'kafkajs';\nimport { connect } from 'nats';\nconst c = new KafkaConsumer();\nnc.subscribe('orders', onOrder);\n";
        assert_eq!(
            consumers(other),
            strs(&["queue_consumer:orders", "queue_consumer:unresolved:kafka"])
        );
    }

    #[test]
    fn unexplained_hit_keeps_its_tag() {
        // GUARD (passes before and after CL.1): `producer.produce(topicVar, ..)`
        // names its topic by an expression nothing resolved, so the tag stays
        // beside the named `orders` — and every node keeps its old position:
        // the NATS tag's row runs before the Kafka rows.
        let src = "import { connect } from 'nats';\nimport { Kafka } from 'kafkajs';\nnc.publish(subject, data);\nawait producer.send({ topic: 'orders', messages });\nawait producer.produce(topicVar, msg);\n";
        let p = extract_queue_producer_nodes(src, PATH, module_id(), repo());
        assert_eq!(
            ordered(&p),
            strs(&[
                "queue_producer:unresolved:nats",
                "queue_producer:orders",
                "queue_producer:unresolved:kafka",
            ])
        );
        let tag = p
            .nodes
            .iter()
            .find(|n| {
                p.nav
                    .qname_by_id
                    .get(&n.id)
                    .is_some_and(|q| q.ends_with(":unresolved:kafka"))
            })
            .expect("kafka tag");
        assert_eq!(tag.confidence, Confidence::Weak);
    }

    #[test]
    fn amqplib_publish_reads_the_routing_key() {
        let p = |body: &str| producers(&format!("import amqp from 'amqplib';\n{body}\n"));
        assert_eq!(
            p("channel.publish('shop', 'orders', b);"),
            strs(&["queue_producer:orders"])
        );
        // Fanout: an empty routing key keeps the exchange identity.
        assert_eq!(
            p("channel.publish('logs', '', b);"),
            strs(&["queue_producer:logs"])
        );
        // The default-exchange shortcut.
        assert_eq!(
            p("channel.sendToQueue('jobs', b);"),
            strs(&["queue_producer:jobs"])
        );
        // matrix/typescript/amqp's producer: never `queue_producer:shop`.
        let src = "import amqp from 'amqplib';\n\nexport async function publishOrder(payload: string): Promise<void> {\n  const channel = await conn.createChannel();\n  await channel.assertExchange('shop', 'direct');\n  channel.publish('shop', 'orders', Buffer.from(payload));\n}\n";
        assert_eq!(
            at(src, "server/producer.ts"),
            (vec![], strs(&["queue_producer:orders"]))
        );
    }

    #[test]
    fn bullmq_job_name_is_not_a_topic() {
        // matrix/typescript/taskq's producer.
        let src = "import { Queue } from 'bullmq';\n\nconst queue = new Queue('orders', { connection: { host: 'localhost', port: 6379 } });\n\nexport async function checkout(orderId: string): Promise<void> {\n  await queue.add('process-order', { orderId });\n}\n";
        assert_eq!(producers(src), strs(&["queue_producer:orders"]));
        // `queue.add(` alone is liveness: no job-name node and no tag.
        let only =
            "import { Queue } from 'bullmq';\nawait queue.add('process-order', { orderId });\n";
        assert_eq!(producers(only), Vec::<String>::new());
    }

    #[test]
    fn inject_queue_names_the_producer() {
        for import in ["@nestjs/bull", "@nestjs/bullmq"] {
            let src = format!(
                "import {{ InjectQueue }} from '{import}';\n@Injectable()\nexport class AudioService {{\n  constructor(@InjectQueue('audio') private audioQueue: Queue) {{}}\n  async transcode() {{ await this.audioQueue.add('transcode', {{ file: 'a.mp3' }}); }}\n}}\n"
            );
            assert_eq!(producers(&src), strs(&["queue_producer:audio"]), "{import}");
        }
        // Without a bull import the decorator is someone else's.
        assert_eq!(
            producers("@InjectQueue('audio') q: Queue;\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn celery_delay_on_an_imported_task() {
        let py = |src: &str| {
            qnames(&extract_queue_producer_nodes(
                src,
                "server/producer.py",
                module_id(),
                repo(),
            ))
        };
        // matrix/python/taskq's caller never mentions celery.
        let caller = "from tasks import process_order\n\n\ndef checkout(order_id):\n    process_order.delay(order_id)\n    return {\"queued\": True}\n";
        assert_eq!(py(caller), strs(&["queue_producer:process_order"]));
        // A dotted receiver is someone else's `delay`.
        assert_eq!(
            py("import pygame\npygame.time.delay(1)\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            py("import tasks\ntasks.process_order.delay(1)\n"),
            Vec::<String>::new()
        );
        // A bare name no from-import binds, and a star import, bind nothing.
        assert_eq!(py("def f(x):\n    x.delay(1)\n"), Vec::<String>::new());
        assert_eq!(
            py("from tasks import *\nprocess_order.delay(1)\n"),
            Vec::<String>::new()
        );
        // Parenthesised multi-line lists, aliases and continuations.
        let multi = "from app.tasks import (\n    send_email,  # mail\n    process_order as po,\n)\nfrom app.more import a, \\\n    rebuild\npo.delay(1)\nsend_email.delay(2)\nrebuild.delay(3)\n";
        assert_eq!(
            py(multi),
            strs(&[
                "queue_producer:po",
                "queue_producer:rebuild",
                "queue_producer:send_email"
            ])
        );
        // Only Python files: the same text in a JS file is jQuery-shaped.
        assert_eq!(
            qnames(&extract_queue_producer_nodes(
                caller,
                "server/producer.js",
                module_id(),
                repo()
            )),
            Vec::<String>::new()
        );
        // A gated file reads exactly as before.
        assert_eq!(
            py("from celery import Celery\nsend_email.delay('x')\n"),
            strs(&["queue_producer:send_email"])
        );
        assert_eq!(
            python_imported_names(
                "from a import b as c, d\nfrom .x import (e,\n f)\nimport g\nfrom h import *\n"
            ),
            ["c", "d", "e", "f"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn suppressed_tag_callbacks_follow_the_explaining_node() {
        // `basic_consume(on_message_callback=cb)` names no queue and carries no
        // identifier expression; amqplib's `channel.consume` names `audit` in
        // the same file, so the tag is explained (rule ii) and `cb` follows
        // the single named RabbitMQ consumer.
        let src = "import amqp\nchannel.consume('audit', onAudit)\nch.basic_consume(on_message_callback=cb)\n";
        let c = extract_queue_consumer_nodes(src, "w.py", module_id(), repo());
        assert_eq!(qnames(&c), strs(&["queue_consumer:audit"]));
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:audit", 1, name("onAudit"), false),
                cb("queue_consumer:audit", 2, name("cb"), false),
            ]
        );
        // Two named consumers: no single owner, so `cb` is dropped, not guessed.
        let src = "import amqp\nchannel.consume('audit', onAudit)\nchannel.consume('orders', onOrder)\nch.basic_consume(on_message_callback=cb)\n";
        let c = extract_queue_consumer_nodes(src, "w.py", module_id(), repo());
        assert_eq!(
            qnames(&c),
            strs(&["queue_consumer:audit", "queue_consumer:orders"])
        );
        assert_eq!(
            callbacks_of(&c),
            vec![
                cb("queue_consumer:audit", 1, name("onAudit"), false),
                cb("queue_consumer:orders", 2, name("onOrder"), false),
            ]
        );
    }

    // ---- CL.2: Go broker rows ----------------------------------------------

    const GO_PATH: &str = "svc/main.go";

    fn go_p(src: &str) -> Vec<String> {
        qnames(&extract_queue_producer_nodes(src, GO_PATH, module_id(), repo()))
    }

    fn go_c(src: &str) -> Vec<String> {
        qnames(&extract_queue_consumer_nodes(src, GO_PATH, module_id(), repo()))
    }

    /// `package main`, one import, and `body` inside `func f()`.
    fn go_file(import: &str, body: &str) -> String {
        format!("package main\n\nimport \"{import}\"\n\nfunc f() {{\n\t{body}\n}}\n")
    }

    /// matrix/go/amqp's producer and consumer.
    const GO_AMQP_PRODUCER: &str = "package main\n\nimport \"github.com/streadway/amqp\"\n\nfunc PublishOrder(ch *amqp.Channel, body []byte) error {\n\treturn ch.Publish(\"\", \"orders\", false, false, amqp.Publishing{\n\t\tContentType: \"application/json\",\n\t\tBody:        body,\n\t})\n}\n";
    const GO_AMQP_CONSUMER: &str = "package main\n\nimport \"github.com/streadway/amqp\"\n\nfunc ConsumeOrders(ch *amqp.Channel) error {\n\tq, err := ch.QueueDeclare(\"orders\", true, false, false, false, nil)\n\tif err != nil {\n\t\treturn err\n\t}\n\tmsgs, err := ch.Consume(q.Name, \"\", true, false, false, false, nil)\n\tif err != nil {\n\t\treturn err\n\t}\n\tfor d := range msgs {\n\t\thandle(d.Body)\n\t}\n\treturn nil\n}\n\nfunc handle(b []byte) {}\n";

    #[test]
    fn go_streadway_publish_reads_the_routing_key() {
        assert_eq!(
            at(GO_AMQP_PRODUCER, "server/producer.go"),
            (vec![], strs(&["queue_producer:orders"]))
        );
        // amqp091-go, the maintained fork, and its context-first spellings:
        // the routing key is arg #2 behind ctx and the exchange.
        let amqp091 = "github.com/rabbitmq/amqp091-go";
        for call in [
            "ch.Publish(\"shop\", \"orders\", false, false, msg)",
            "ch.PublishWithContext(ctx, \"\", \"orders\", false, false, msg)",
            "ch.PublishWithDeferredConfirm(\"shop\", \"orders\", false, false, msg)",
            "ch.PublishWithDeferredConfirmWithContext(ctx, \"shop\", \"orders\", false, false, msg)",
        ] {
            assert_eq!(
                go_p(&go_file(amqp091, call)),
                strs(&["queue_producer:orders"]),
                "{call}"
            );
        }
        // Fanout: an empty routing key keeps the exchange identity, the
        // amqplib rule (CL.1).
        assert_eq!(
            go_p(&go_file(amqp091, "ch.Publish(\"logs\", \"\", false, false, msg)")),
            strs(&["queue_producer:logs"])
        );
        // A file that also imports paho reads `.Publish(` ONCE, as RabbitMQ:
        // the generic-verb MQTT row yields to the earlier amqp row.
        let both = "package main\n\nimport (\n\t\"github.com/streadway/amqp\"\n\tmqtt \"github.com/eclipse/paho.mqtt.golang\"\n)\n\nfunc f() {\n\tch.Publish(\"\", \"orders\", false, false, msg)\n}\n";
        let r = extract_queue_producer_nodes(both, GO_PATH, module_id(), repo());
        assert_eq!(qnames(&r), strs(&["queue_producer:orders"]));
        assert!(framework_of(&r).contains(r#""framework":"RabbitMQ""#));
        // Without the library, `.Publish(` is somebody else's.
        assert_eq!(
            go_p(&go_file("example.com/bus", "b.Publish(\"\", \"orders\")")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn go_amqp_declared_queue_names_q_name_consume() {
        let c = extract_queue_consumer_nodes(
            GO_AMQP_CONSUMER,
            "client/consumer.go",
            module_id(),
            repo(),
        );
        assert_eq!(qnames(&c), strs(&["queue_consumer:orders"]));
        // The site is the CONSUME call (line 9), never the declare.
        let lines: Vec<u32> = c.anchors.iter().map(|a| a.line).collect();
        assert_eq!(lines, vec![9]);
        // The tutorial producer declares its queue: that consumes nothing.
        let amqp091 = "github.com/rabbitmq/amqp091-go";
        assert_eq!(
            go_c(&go_file(
                amqp091,
                "q, _ := ch.QueueDeclare(\"orders\", false, false, false, false, nil)"
            )),
            Vec::<String>::new()
        );
        // A literal consume, and amqp091's context-first spelling.
        assert_eq!(
            go_c(&go_file(
                amqp091,
                "ch.Consume(\"audit\", \"\", true, false, false, false, nil)\n\tch.ConsumeWithContext(ctx, \"orders\", \"\", true, false, false, false, nil)"
            )),
            strs(&["queue_consumer:audit", "queue_consumer:orders"])
        );
    }

    #[test]
    fn go_azservicebus_sender_and_receivers() {
        let sdk = "github.com/Azure/azure-sdk-for-go/sdk/messaging/azservicebus";
        assert_eq!(
            go_p(&go_file(sdk, "sender, err := client.NewSender(\"orders\", nil)")),
            strs(&["queue_producer:orders"])
        );
        assert_eq!(
            go_c(&go_file(
                sdk,
                "receiver, err := client.NewReceiverForQueue(\"orders\", nil)"
            )),
            strs(&["queue_consumer:orders"])
        );
        // A subscription receiver reads its TOPIC (arg #0), so it joins the
        // sender — the C# `CreateProcessor(` precedent.
        assert_eq!(
            go_c(&go_file(
                sdk,
                "r, err := client.NewReceiverForSubscription(\"orders\", \"billing\", nil)"
            )),
            strs(&["queue_consumer:orders"])
        );
        assert_eq!(
            go_p(&go_file("example.com/mq", "client.NewSender(\"orders\", nil)")),
            Vec::<String>::new()
        );
        // matrix/go/azure_sb's sender: `sender.SendMessage(` is no SQS call,
        // and the `azservicebus.Message` envelope is no MESSAGE_TYPE.
        let sender = "package main\n\nimport (\n\t\"context\"\n\n\t\"github.com/Azure/azure-sdk-for-go/sdk/messaging/azservicebus\"\n)\n\nfunc Send(ctx context.Context, client *azservicebus.Client, body []byte) error {\n\tsender, err := client.NewSender(\"orders\", nil)\n\tif err != nil {\n\t\treturn err\n\t}\n\treturn sender.SendMessage(ctx, &azservicebus.Message{Body: body}, nil)\n}\n";
        let r = extract_queue_producer_nodes(sender, "server/sender.go", module_id(), repo());
        assert_eq!(qnames(&r), strs(&["queue_producer:orders"]));
        assert!(framework_of(&r).contains(r#""framework":"AzureServiceBus""#));
        assert_eq!(msg_type(&r.nodes[0]), None);
        let opts = go_file(
            sdk,
            "r, err := client.NewReceiverForQueue(\"orders\", &azservicebus.ReceiverOptions{ReceiveMode: azservicebus.ReceiveModePeekLock})",
        );
        let c = extract_queue_consumer_nodes(&opts, GO_PATH, module_id(), repo());
        assert_eq!(msg_type(&c.nodes[0]), None, "client options are no payload");
    }

    #[test]
    fn go_gcp_pubsub_topic_and_subscription() {
        for import in ["cloud.google.com/go/pubsub", "cloud.google.com/go/pubsub/v2"] {
            let p = |body: &str| go_p(&go_file(import, body));
            let c = |body: &str| go_c(&go_file(import, body));
            assert_eq!(
                p("topic := client.Topic(\"orders\")"),
                strs(&["queue_producer:orders"]),
                "{import}"
            );
            assert_eq!(
                c("sub := client.Subscription(\"orders\")"),
                strs(&["queue_consumer:orders"]),
                "{import}"
            );
            // v2 spells them Publisher / Subscriber.
            assert_eq!(
                p("pub := client.Publisher(\"orders\")"),
                strs(&["queue_producer:orders"]),
                "{import}"
            );
            assert_eq!(
                c("sub := client.Subscriber(\"orders\")"),
                strs(&["queue_consumer:orders"]),
                "{import}"
            );
        }
        // `.Topic(` outside a pubsub file is somebody else's.
        assert_eq!(
            go_p(&go_file("example.com/mq", "t := c.Topic(\"orders\")")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn go_redis_lists_read_the_key_after_ctx() {
        for import in ["github.com/redis/go-redis/v9", "github.com/go-redis/redis/v8"] {
            for push in [
                "rdb.LPush(ctx, \"orders\", payload)",
                "rdb.RPush(ctx, \"orders\", a, b)",
            ] {
                assert_eq!(
                    go_p(&go_file(import, push)),
                    strs(&["queue_producer:orders"]),
                    "{import} {push}"
                );
            }
            for pop in [
                "rdb.LPop(ctx, \"orders\")",
                "rdb.RPop(ctx, \"orders\")",
                "rdb.BLPop(ctx, 0, \"orders\")",
                "rdb.BRPop(ctx, 5*time.Second, \"orders\")",
            ] {
                assert_eq!(
                    go_c(&go_file(import, pop)),
                    strs(&["queue_consumer:orders"]),
                    "{import} {pop}"
                );
            }
        }
        let v9 = "github.com/redis/go-redis/v9";
        // The pushed VALUE is never the key: a key held in a variable is the
        // unresolved signal, not `queue_producer:hello`.
        assert_eq!(
            go_p(&go_file(v9, "rdb.LPush(ctx, key, \"hello\")")),
            strs(&["queue_producer:unresolved:redislist"])
        );
        // `.LPush(` in a file without go-redis mints nothing, and redigo's
        // `redis` import is not go-redis.
        for import in ["example.com/list", "github.com/gomodule/redigo/redis"] {
            assert_eq!(
                go_p(&go_file(import, "l.LPush(ctx, \"orders\", p)")),
                Vec::<String>::new(),
                "{import}"
            );
        }
    }

    /// matrix/go/sqs_sns's producer.
    const GO_SQS_PRODUCER: &str = "package main\n\nimport (\n\t\"context\"\n\n\t\"github.com/aws/aws-sdk-go-v2/aws\"\n\t\"github.com/aws/aws-sdk-go-v2/service/sqs\"\n)\n\nfunc PublishOrder(ctx context.Context, client *sqs.Client, body string) error {\n\t_, err := client.SendMessage(ctx, &sqs.SendMessageInput{\n\t\tQueueUrl:    aws.String(\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\"),\n\t\tMessageBody: aws.String(body),\n\t})\n\treturn err\n}\n";

    #[test]
    fn go_aws_sdk_sqs_url_through_aws_string() {
        let pr = extract_queue_producer_nodes(
            GO_SQS_PRODUCER,
            "server/producer.go",
            module_id(),
            repo(),
        );
        assert_eq!(qnames(&pr), strs(&["queue_producer:orders"]));
        assert!(framework_of(&pr).contains(r#""framework":"Sqs","family":"sqs""#));
        let consumer = "package main\n\nimport (\n\t\"context\"\n\t\"github.com/aws/aws-sdk-go-v2/aws\"\n\t\"github.com/aws/aws-sdk-go-v2/service/sqs\"\n)\n\nfunc PollOrders(ctx context.Context, client *sqs.Client) error {\n\t_, err := client.ReceiveMessage(ctx, &sqs.ReceiveMessageInput{\n\t\tQueueUrl: aws.String(\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\"),\n\t})\n\treturn err\n}\n";
        assert_eq!(go_c(consumer), strs(&["queue_consumer:orders"]));
        // SDK v1: no ctx argument, same helper.
        let v1 = "package main\n\nimport (\n\t\"github.com/aws/aws-sdk-go/aws\"\n\t\"github.com/aws/aws-sdk-go/service/sqs\"\n)\n\nfunc f() {\n\tsvc.SendMessage(&sqs.SendMessageInput{QueueUrl: aws.String(\"https://sqs.eu-west-1.amazonaws.com/1/orders\"), MessageBody: aws.String(b)})\n}\n";
        assert_eq!(go_p(v1), strs(&["queue_producer:orders"]));
        // A URL held in a variable hands its NAME to the LA.4 const fold.
        let held = GO_SQS_PRODUCER.replace(
            "aws.String(\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\")",
            "aws.String(ordersURL)",
        );
        assert_eq!(go_p(&held), strs(&["queue_producer:unresolved:sqs"]));
        let fold = extract_queue_nodes_with_consts(
            &held,
            "server/producer.go",
            module_id(),
            repo(),
            &|e: &str| (e == "ordersURL").then(|| "https://sqs.us-east-1.amazonaws.com/1/orders".to_string()),
        );
        assert_eq!(qnames(&fold.producers), strs(&["queue_producer:orders"]));
        assert_eq!(fold.counts.folded, 1);
        // Another AWS service's `SendMessage(` (Amazon Connect participant)
        // in a file without the SQS service package.
        assert_eq!(
            go_p(&go_file(
                "github.com/aws/aws-sdk-go-v2/service/connectparticipant",
                "c.SendMessage(ctx, &connectparticipant.SendMessageInput{ConnectionToken: aws.String(tok)})"
            )),
            Vec::<String>::new()
        );
    }

    #[test]
    fn go_sns_topic_arn_through_aws_string() {
        for import in [
            "github.com/aws/aws-sdk-go/service/sns",
            "github.com/aws/aws-sdk-go-v2/service/sns",
        ] {
            let src = go_file(
                import,
                "client.Publish(ctx, &sns.PublishInput{\n\t\tTopicArn: aws.String(\"arn:aws:sns:us-east-1:123456789012:orders\"),\n\t\tMessage:  aws.String(body),\n\t})",
            );
            let r = extract_queue_producer_nodes(&src, GO_PATH, module_id(), repo());
            assert_eq!(qnames(&r), strs(&["queue_producer:orders"]), "{import}");
            assert!(
                framework_of(&r).contains(r#""framework":"Sns","family":"sns""#),
                "{import}"
            );
        }
        // An SQS-only file's `.Publish(` is not SNS.
        assert_eq!(
            go_p(&go_file(
                "github.com/aws/aws-sdk-go-v2/service/sqs",
                "bus.Publish(ctx, &Event{TopicArn: aws.String(\"arn:aws:sns:us-east-1:1:orders\")})"
            )),
            Vec::<String>::new()
        );
    }

    #[test]
    fn kafka_go_writer_struct_names_the_topic_and_drops_both_tags() {
        // matrix/go/kafka: kafka-go rejects a Topic on both the Writer and the
        // Message, so the topic lives on the Writer literal alone.
        let producer = "package main\n\nimport (\n\t\"context\"\n\n\t\"github.com/segmentio/kafka-go\"\n)\n\nvar writer = &kafka.Writer{\n\tAddr:  kafka.TCP(\"localhost:9092\"),\n\tTopic: \"orders\",\n}\n\nfunc PublishOrder(ctx context.Context, payload []byte) error {\n\treturn writer.WriteMessages(ctx, kafka.Message{Value: payload})\n}\n";
        let consumer = "package main\n\nimport (\n\t\"context\"\n\n\t\"github.com/segmentio/kafka-go\"\n)\n\nfunc ConsumeOrders(ctx context.Context) error {\n\treader := kafka.NewReader(kafka.ReaderConfig{Brokers: []string{\"localhost:9092\"}, Topic: \"orders\", GroupID: \"workers\"})\n\tfor {\n\t\tm, err := reader.ReadMessage(ctx)\n\t\tif err != nil {\n\t\t\treturn err\n\t\t}\n\t\thandle(m.Value)\n\t}\n}\n\nfunc handle(v []byte) {}\n";
        assert_eq!(
            at(producer, "server/producer.go"),
            (vec![], strs(&["queue_producer:orders"]))
        );
        assert_eq!(
            at(consumer, "client/consumer.go"),
            (strs(&["queue_consumer:orders"]), vec![])
        );
        // `kafka.NewWriter(kafka.WriterConfig{..})`, the pre-0.4 spelling.
        let kgo = "github.com/segmentio/kafka-go";
        assert_eq!(
            go_p(&go_file(
                kgo,
                "w := kafka.NewWriter(kafka.WriterConfig{Brokers: brokers, Topic: \"orders\"})\n\tw.WriteMessages(ctx, kafka.Message{Value: v})"
            )),
            strs(&["queue_producer:orders"])
        );
        // A Writer naming no topic beside a message that does: one node.
        assert_eq!(
            go_p(&go_file(
                kgo,
                "w := &kafka.Writer{Addr: kafka.TCP(\"localhost:9092\")}\n\tw.WriteMessages(ctx, kafka.Message{Topic: \"orders\", Value: v})"
            )),
            strs(&["queue_producer:orders"])
        );
        // Nothing in the file names the topic: the tag still says kafka is
        // live here, once.
        assert_eq!(
            go_p(&go_file(
                kgo,
                "writer.WriteMessages(ctx, kafka.Message{Value: v})"
            )),
            strs(&["queue_producer:unresolved:kafka"])
        );
    }

    #[test]
    fn confluent_go_subscribe_topics() {
        let lib = "github.com/confluentinc/confluent-kafka-go/v2/kafka";
        for sub in [
            "c.SubscribeTopics([]string{\"orders\"}, nil)",
            "c.SubscribeTopic(\"orders\", nil)",
        ] {
            let body = format!("{sub}\n\tfor {{\n\t\tmsg, err := consumer.ReadMessage(time.Second)\n\t\t_, _ = msg, err\n\t}}");
            assert_eq!(
                go_c(&go_file(lib, &body)),
                strs(&["queue_consumer:orders"]),
                "{sub}"
            );
        }
        // Without the Go module, `.SubscribeTopics(` mints nothing.
        assert_eq!(
            go_c(&go_file("example.com/k", "c.SubscribeTopics([]string{\"orders\"}, nil)")),
            Vec::<String>::new()
        );
    }

    // ---- CL.3: JVM / .NET broker rows ---------------------------------------

    fn jvm_p(src: &str) -> Vec<String> {
        qnames(&extract_queue_producer_nodes(src, "server/Producer.java", module_id(), repo()))
    }

    fn jvm_c(src: &str) -> Vec<String> {
        qnames(&extract_queue_consumer_nodes(src, "client/Consumer.java", module_id(), repo()))
    }

    /// matrix/java/azure_sb's sender and receiver.
    const JAVA_SB_SENDER: &str = "package com.example;\n\nimport com.azure.messaging.servicebus.ServiceBusClientBuilder;\nimport com.azure.messaging.servicebus.ServiceBusMessage;\nimport com.azure.messaging.servicebus.ServiceBusSenderClient;\n\npublic class OrderSender {\n    public void send(String conn, String body) {\n        ServiceBusSenderClient sender = new ServiceBusClientBuilder().connectionString(conn)\n            .sender().queueName(\"orders\").buildClient();\n        sender.sendMessage(new ServiceBusMessage(body));\n    }\n}\n";
    const JAVA_SB_RECEIVER: &str = "package com.example;\n\nimport com.azure.messaging.servicebus.ServiceBusClientBuilder;\nimport com.azure.messaging.servicebus.ServiceBusReceiverClient;\n\npublic class OrderReceiver {\n    public ServiceBusReceiverClient build(String conn) {\n        return new ServiceBusClientBuilder().connectionString(conn)\n            .receiver().queueName(\"orders\").buildClient();\n    }\n}\n";

    #[test]
    fn java_azure_sb_builders() {
        let p = extract_queue_producer_nodes(JAVA_SB_SENDER, "server/OrderSender.java", module_id(), repo());
        assert_eq!(qnames(&p), strs(&["queue_producer:orders"]));
        assert!(framework_of(&p).contains(r#""framework":"AzureServiceBus""#));
        // The site is the `.sender()` call's line (0-indexed 9).
        assert_eq!(p.anchors.iter().map(|a| a.line).collect::<Vec<_>>(), vec![9]);
        assert_eq!(jvm_c(JAVA_SB_RECEIVER), strs(&["queue_consumer:orders"]));
        let sb = "import com.azure.messaging.servicebus.*;\nclass C {\n  void f() {\n";
        // The processor client, a lambda's `;` inside its chain.
        let processor = format!("{sb}    ServiceBusProcessorClient p = new ServiceBusClientBuilder()\n        .connectionString(c)\n        .processor()\n        .queueName(\"orders\")\n        .processMessage(ctx -> {{ handle(ctx); }})\n        .processError(e -> {{ log(e); }})\n        .buildProcessorClient();\n  }}\n}}\n");
        assert_eq!(jvm_c(&processor), strs(&["queue_consumer:orders"]));
        // A topic receiver reads its TOPIC, so it joins the topic's sender.
        let topic = format!("{sb}    b.receiver().topicName(\"orders\").subscriptionName(\"billing\").buildClient();\n  }}\n}}\n");
        assert_eq!(jvm_c(&topic), strs(&["queue_consumer:orders"]));
        let topic_sender = format!("{sb}    b.sender().topicName(\"orders\").buildClient();\n  }}\n}}\n");
        assert_eq!(jvm_p(&topic_sender), strs(&["queue_producer:orders"]));
        // Session-enabled queues.
        let session = format!("{sb}    b.sessionReceiver().queueName(\"orders\").buildClient();\n    b.sessionProcessor().queueName(\"audit\").processMessage(h).buildProcessorClient();\n  }}\n}}\n");
        assert_eq!(jvm_c(&session), strs(&["queue_consumer:audit", "queue_consumer:orders"]));
        // Scala calls the same Java builder; its statement ends at the newline.
        let scala = "package shop\n\nimport com.azure.messaging.servicebus.{ServiceBusClientBuilder, ServiceBusMessage}\n\nobject OrderSender {\n  def send(conn: String, body: String): Unit =\n    new ServiceBusClientBuilder().connectionString(conn).sender().queueName(\"orders\").buildClient().sendMessage(new ServiceBusMessage(body))\n}\n";
        assert_eq!(
            qnames(&extract_queue_producer_nodes(scala, "server/OrderSender.scala", module_id(), repo())),
            strs(&["queue_producer:orders"])
        );
        // A queue held in a variable is the unresolved signal; a constant folds.
        let held = format!("{sb}    b.sender().queueName(queueName).buildClient();\n  }}\n}}\n");
        assert_eq!(jvm_p(&held), strs(&["queue_producer:unresolved:azureservicebus"]));
        let fold = extract_queue_nodes_with_consts(&held, "server/P.java", module_id(), repo(), &|e: &str| {
            (e == "queueName").then(|| "orders".to_string())
        });
        assert_eq!(qnames(&fold.producers), strs(&["queue_producer:orders"]));
        assert_eq!(fold.counts.folded, 1);
        // `.sender()` / `.receiver()` without the Azure package are anybody's.
        let other = "import com.example.mail.Envelope;\nclass C { void f() { e.sender().queueName(\"orders\"); e.receiver().queueName(\"orders\"); } }\n";
        assert_eq!(jvm_p(other), Vec::<String>::new());
        assert_eq!(jvm_c(other), Vec::<String>::new());
    }

    #[test]
    fn java_spring_gcp_pubsub_template() {
        let publisher = "package com.example;\n\nimport com.google.cloud.spring.pubsub.core.PubSubTemplate;\n\npublic class OrderPublisher {\n    private final PubSubTemplate pubSubTemplate;\n\n    public OrderPublisher(PubSubTemplate pubSubTemplate) {\n        this.pubSubTemplate = pubSubTemplate;\n    }\n\n    public void publish(String body) {\n        pubSubTemplate.publish(\"orders\", body);\n    }\n}\n";
        let listener = "package com.example;\n\nimport com.google.cloud.spring.pubsub.core.PubSubTemplate;\n\npublic class OrderListener {\n    public OrderListener(PubSubTemplate pubSubTemplate) {\n        pubSubTemplate.subscribe(\"orders\", message -> message.ack());\n    }\n}\n";
        let p = extract_queue_producer_nodes(publisher, "server/OrderPublisher.java", module_id(), repo());
        assert_eq!(qnames(&p), strs(&["queue_producer:orders"]));
        assert!(framework_of(&p).contains(r#""framework":"PubSub""#));
        assert_eq!(jvm_c(listener), strs(&["queue_consumer:orders"]));
        // The pre-2.x namespace.
        let old = publisher.replace("com.google.cloud.spring.pubsub", "org.springframework.cloud.gcp.pubsub");
        assert_eq!(jvm_p(&old), strs(&["queue_producer:orders"]));
        // A Redis import beside it: the generic `.publish(` verb row yields to
        // the template row, so the call mints ONE PubSub node.
        let both = publisher.replace(
            "import com.google",
            "import org.springframework.data.redis.core.StringRedisTemplate;\nimport com.google",
        );
        let r = extract_queue_producer_nodes(&both, "server/OrderPublisher.java", module_id(), repo());
        assert_eq!(qnames(&r), strs(&["queue_producer:orders"]));
        assert!(framework_of(&r).contains(r#""framework":"PubSub""#));
        // Without Spring Cloud GCP, `template.publish(` is somebody else's.
        assert_eq!(
            jvm_p("import com.example.Bus;\nclass C { void f() { eventTemplate.publish(\"orders\", b); } }\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn java_spring_data_redis_lists() {
        let producer = "package com.shop.messaging;\n\nimport org.springframework.data.redis.core.StringRedisTemplate;\nimport org.springframework.stereotype.Service;\n\n@Service\npublic class OrderProducer {\n    private final StringRedisTemplate redisTemplate;\n\n    public OrderProducer(StringRedisTemplate redisTemplate) {\n        this.redisTemplate = redisTemplate;\n    }\n\n    public void publish(String payload) {\n        redisTemplate.opsForList().leftPush(\"orders\", payload);\n    }\n}\n";
        let consumer = "package com.shop.workers;\n\nimport java.time.Duration;\nimport org.springframework.data.redis.core.StringRedisTemplate;\nimport org.springframework.stereotype.Component;\n\n@Component\npublic class OrderConsumer {\n    private final StringRedisTemplate redisTemplate;\n\n    public OrderConsumer(StringRedisTemplate redisTemplate) {\n        this.redisTemplate = redisTemplate;\n    }\n\n    public void poll() {\n        String payload = redisTemplate.opsForList().rightPop(\"orders\", Duration.ofSeconds(5));\n        System.out.println(payload);\n    }\n}\n";
        let p = extract_queue_producer_nodes(producer, "server/OrderProducer.java", module_id(), repo());
        assert_eq!(qnames(&p), strs(&["queue_producer:orders"]));
        assert!(framework_of(&p).contains(r#""framework":"RedisList""#));
        let c = extract_queue_consumer_nodes(consumer, "client/OrderConsumer.java", module_id(), repo());
        assert_eq!(qnames(&c), strs(&["queue_consumer:orders"]));
        assert!(framework_of(&c).contains(r#""framework":"RedisList""#));
        let ops = "import org.springframework.data.redis.core.ListOperations;\nclass C {\n  void f() {\n    ops.rightPush(\"audit\", p);\n    ops.leftPop(\"jobs\");\n  }\n}\n";
        assert_eq!(jvm_p(ops), strs(&["queue_producer:audit"]));
        assert_eq!(jvm_c(ops), strs(&["queue_consumer:jobs"]));
        // A `java.util` deque-like call without Spring Data Redis mints nothing.
        let plain = "import java.util.Deque;\nclass C { void f() { d.leftPush(\"x\"); d.rightPop(\"x\"); } }\n";
        assert_eq!(jvm_p(plain), Vec::<String>::new());
        assert_eq!(jvm_c(plain), Vec::<String>::new());
    }

    /// matrix/java/sqs_sns's consumer: the AWS developer-guide receive shape.
    const JAVA_SQS_CONSUMER: &str = "package com.shop.workers;\n\nimport software.amazon.awssdk.services.sqs.SqsClient;\nimport software.amazon.awssdk.services.sqs.model.Message;\nimport software.amazon.awssdk.services.sqs.model.ReceiveMessageRequest;\n\npublic class OrderConsumer {\n    private final SqsClient sqs = SqsClient.create();\n\n    public void poll() {\n        ReceiveMessageRequest request = ReceiveMessageRequest.builder()\n                .queueUrl(\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\")\n                .maxNumberOfMessages(10)\n                .build();\n        for (Message m : sqs.receiveMessage(request).messages()) {\n            System.out.println(m.body());\n        }\n    }\n}\n";

    #[test]
    fn java_sqs_request_built_on_its_own_statement() {
        let c = extract_queue_consumer_nodes(JAVA_SQS_CONSUMER, "client/OrderConsumer.java", module_id(), repo());
        // Exactly the named node: `sqs.receiveMessage(request)` carries no
        // expression in a `queueUrl` slot, so CL.1's rule (ii) drops its tag.
        assert_eq!(qnames(&c), strs(&["queue_consumer:orders"]));
        assert!(framework_of(&c).contains(r#""framework":"Sqs","family":"sqs""#));
        // The producer builds its request INSIDE the call: the `.sendMessage(`
        // row and the builder row read one queue, one node.
        let producer = "package com.shop.messaging;\n\nimport software.amazon.awssdk.services.sqs.SqsClient;\nimport software.amazon.awssdk.services.sqs.model.SendMessageRequest;\n\npublic class OrderProducer {\n    private final SqsClient sqs = SqsClient.create();\n\n    public void publish(String payload) {\n        sqs.sendMessage(SendMessageRequest.builder()\n                .queueUrl(\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\")\n                .messageBody(payload)\n                .build());\n    }\n}\n";
        assert_eq!(jvm_p(producer), strs(&["queue_producer:orders"]));
        // Built on its own statement, the producer reads the same way.
        let own = "import software.amazon.awssdk.services.sqs.model.SendMessageRequest;\nclass P {\n  void f() {\n    SendMessageRequest r = SendMessageRequest.builder()\n        .queueUrl(\"https://sqs.eu-west-1.amazonaws.com/1/orders\")\n        .messageBody(b)\n        .build();\n    sqs.sendMessage(r);\n  }\n}\n";
        assert_eq!(jvm_p(own), strs(&["queue_producer:orders"]));
        // The request's queue held in a variable: the tag stays, one per side.
        let held = JAVA_SQS_CONSUMER.replace(
            "\"https://sqs.us-east-1.amazonaws.com/123456789012/orders\"",
            "queueUrl",
        );
        assert_eq!(jvm_c(&held), strs(&["queue_consumer:unresolved:sqs"]));
    }

    #[test]
    fn java_sns_publish_request_builder() {
        let src = "import software.amazon.awssdk.services.sns.SnsClient;\nimport software.amazon.awssdk.services.sns.model.PublishRequest;\n\nclass Notifier {\n  void notify(SnsClient sns, String m) {\n    PublishRequest request = PublishRequest.builder()\n        .message(m)\n        .topicArn(\"arn:aws:sns:us-east-1:123456789012:orders\")\n        .build();\n    sns.publish(request);\n  }\n}\n";
        let p = extract_queue_producer_nodes(src, "server/Notifier.java", module_id(), repo());
        assert_eq!(qnames(&p), strs(&["queue_producer:orders"]));
        assert!(framework_of(&p).contains(r#""framework":"Sns","family":"sns""#));
        // Without the SNS service package the builder is not a publish.
        let other = src.replace("services.sns", "services.ses");
        assert_eq!(jvm_p(&other), Vec::<String>::new());
    }

    const CS_NATS_PUBLISHER: &str = "using NATS.Client.Core;\n\npublic class Publisher\n{\n    public async Task Send(NatsConnection nc, string body)\n    {\n        await nc.PublishAsync(\"orders\", body);\n    }\n}\n";
    const CS_NATS_SUBSCRIBER: &str = "using NATS.Client.Core;\n\npublic class Subscriber\n{\n    public async Task Run(NatsConnection nc)\n    {\n        await foreach (var msg in nc.SubscribeAsync<string>(\"orders\"))\n        {\n            Console.WriteLine(msg.Data);\n        }\n    }\n}\n";

    #[test]
    fn csharp_nats_async_rows_name_both_sides_and_drop_the_prefix_tags() {
        // Exactly one node per side: the HEAD `nc.Publish` / `nc.Subscribe`
        // rows prefix-match the same calls, read nothing, and CL.1's rule (i)
        // drops their `unresolved:nats` tags beside the named node.
        let p = extract_queue_producer_nodes(CS_NATS_PUBLISHER, "server/Publisher.cs", module_id(), repo());
        assert_eq!(qnames(&p), strs(&["queue_producer:orders"]));
        assert!(framework_of(&p).contains(r#""framework":"Nats""#));
        let c = extract_queue_consumer_nodes(CS_NATS_SUBSCRIBER, "client/Subscriber.cs", module_id(), repo());
        assert_eq!(qnames(&c), strs(&["queue_consumer:orders"]));
        // The typed publish, any receiver, and the NATS.Net meta-package.
        let typed = CS_NATS_PUBLISHER
            .replace("nc.PublishAsync(\"orders\"", "js.PublishAsync<Order>(\"orders\"")
            .replace("NATS.Client.Core", "NATS.Net");
        assert_eq!(
            qnames(&extract_queue_producer_nodes(&typed, "server/Publisher.cs", module_id(), repo())),
            strs(&["queue_producer:orders"])
        );
        // A subject in a variable keeps ONE tag per side, as before.
        let held = CS_NATS_PUBLISHER.replace("\"orders\"", "subject");
        assert_eq!(
            qnames(&extract_queue_producer_nodes(&held, "server/Publisher.cs", module_id(), repo())),
            strs(&["queue_producer:unresolved:nats"])
        );
        // `PublishAsync` in a file without NATS is anybody's (the
        // xcut-queue-csharp-kafka method name, a Confluent file).
        let kafka = "using Confluent.Kafka;\npublic class P { public async Task PublishAsync(string s) { await _bus.PublishAsync(\"orders\", s); } }\n";
        assert_eq!(
            qnames(&extract_queue_producer_nodes(kafka, "server/P.cs", module_id(), repo())),
            Vec::<String>::new()
        );
    }

    const CS_MQTT_PUBLISHER: &str = "using MQTTnet;\nusing MQTTnet.Client;\n\npublic class SensorPublisher\n{\n    public async Task Send(IMqttClient client)\n    {\n        var msg = new MqttApplicationMessageBuilder().WithTopic(\"sensors/temp\").WithPayload(\"21\").Build();\n        await client.PublishAsync(msg);\n    }\n}\n";
    const CS_MQTT_SUBSCRIBER: &str = "using MQTTnet;\nusing MQTTnet.Client;\n\npublic class SensorSubscriber\n{\n    public async Task Listen(IMqttClient client)\n    {\n        await client.SubscribeAsync(\"sensors/temp\");\n    }\n}\n";

    #[test]
    fn csharp_mqttnet_builders_and_subscribe() {
        let p = extract_queue_producer_nodes(CS_MQTT_PUBLISHER, "server/SensorPublisher.cs", module_id(), repo());
        assert_eq!(qnames(&p), strs(&["queue_producer:sensors/temp"]));
        assert!(framework_of(&p).contains(r#""framework":"Mqtt""#));
        let c = extract_queue_consumer_nodes(CS_MQTT_SUBSCRIBER, "client/SensorSubscriber.cs", module_id(), repo());
        assert_eq!(qnames(&c), strs(&["queue_consumer:sensors/temp"]));
        let cs = |body: &str| {
            let src = format!("using MQTTnet;\n\nclass S\n{{\n    async Task F()\n    {{\n        {body}\n    }}\n}}\n");
            qnames(&extract_queue_consumer_nodes(&src, "client/S.cs", module_id(), repo()))
        };
        // The options builder, a filter lambda and the filter builder.
        assert_eq!(
            cs("var o = factory.CreateSubscribeOptionsBuilder().WithTopicFilter(\"a/b\").Build();"),
            strs(&["queue_consumer:a/b"])
        );
        assert_eq!(
            cs("var o = factory.CreateSubscribeOptionsBuilder().WithTopicFilter(f => { f.WithTopic(\"a/c\"); }).Build();"),
            strs(&["queue_consumer:a/c"])
        );
        assert_eq!(
            cs("await client.SubscribeAsync(new MqttTopicFilterBuilder().WithTopic(\"a/d\").Build());"),
            strs(&["queue_consumer:a/d"])
        );
        // Without MQTTnet the builder is somebody else's.
        let other = CS_MQTT_PUBLISHER.replace("MQTTnet", "Acme.Messaging");
        assert_eq!(
            qnames(&extract_queue_producer_nodes(&other, "server/S.cs", module_id(), repo())),
            Vec::<String>::new()
        );
    }

    #[test]
    fn csharp_gcp_pubsub_resource_names() {
        let publisher = "using Google.Cloud.PubSub.V1;\n\npublic class Publisher\n{\n    public async Task Send(string body)\n    {\n        var pub = await PublisherClient.CreateAsync(TopicName.FromProjectTopic(\"shop\", \"orders\"));\n        await pub.PublishAsync(body);\n    }\n}\n";
        let subscriber = "using Google.Cloud.PubSub.V1;\n\npublic class Subscriber\n{\n    public async Task Run()\n    {\n        var sub = await SubscriberClient.CreateAsync(SubscriptionName.FromProjectSubscription(\"shop\", \"orders\"));\n        await sub.StartAsync((msg, ct) => Task.FromResult(SubscriberClient.Reply.Ack));\n    }\n}\n";
        let p = extract_queue_producer_nodes(publisher, "server/Publisher.cs", module_id(), repo());
        assert_eq!(qnames(&p), strs(&["queue_producer:orders"]));
        assert!(framework_of(&p).contains(r#""framework":"PubSub""#));
        assert_eq!(
            qnames(&extract_queue_consumer_nodes(subscriber, "client/Subscriber.cs", module_id(), repo())),
            strs(&["queue_consumer:orders"])
        );
        // The project (arg #0) is never the name.
        let project_only = publisher.replace("\"shop\", \"orders\"", "\"shop\", topicId");
        assert_eq!(
            qnames(&extract_queue_producer_nodes(&project_only, "server/Publisher.cs", module_id(), repo())),
            strs(&["queue_producer:unresolved:pubsub"])
        );
    }

    #[test]
    fn mqtt_generic_guard_spares_builder_rows() {
        // Every RedisPubSub / Mqtt row of the bare verbs is still generic, as
        // at HEAD; the MQTTnet rows are not.
        for (needle, framework, _, _) in CONSUMER_PATTERNS.iter().chain(PRODUCER_PATTERNS) {
            let verb = GENERIC_VERB_NEEDLES.contains(needle);
            let pubsub = matches!(framework, QueueFramework::RedisPubSub | QueueFramework::Mqtt);
            assert_eq!(
                is_generic_verb_row(needle, framework),
                verb && pubsub,
                "{needle} {framework:?}"
            );
            if pubsub && !needle.contains("Mqtt") && !needle.ends_with("Async(") && *needle != ".WithTopicFilter(" {
                assert!(verb, "{needle} {framework:?} is a pre-CL.3 verb row");
            }
        }
        assert!(is_generic_verb_row(".publish(", &QueueFramework::Mqtt));
        assert!(is_generic_verb_row(".Subscribe(", &QueueFramework::Mqtt));
        assert!(!is_generic_verb_row("MqttApplicationMessageBuilder()", &QueueFramework::Mqtt));
        assert!(!is_generic_verb_row(".SubscribeAsync(", &QueueFramework::Mqtt));
        assert!(!yields_to_earlier_rows(".SubscribeAsync(", &QueueFramework::Mqtt));
        assert!(yields_to_earlier_rows("Template.convertAndSend(", &QueueFramework::Jms));
        // HEAD behaviour of a verb row: a payload-first Rx-style call in an
        // mqtt file mints no topic; the literal-led call still does.
        let js = "import mqtt from 'mqtt';\nclient.publish('sensors/temp', payload);\nstream.subscribe(x => log('hi'));\n";
        assert_eq!(producers(js), strs(&["queue_producer:sensors/temp"]));
        // The builder row reads a topic the literal-must-lead guard would have
        // erased (`.WithTopic(` is no literal).
        assert_eq!(
            qnames(&extract_queue_producer_nodes(CS_MQTT_PUBLISHER, "server/S.cs", module_id(), repo())),
            strs(&["queue_producer:sensors/temp"])
        );
    }
}
