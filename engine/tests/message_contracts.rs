//! A12.2 acceptance: `message_contracts` turns the queue nodes of a real
//! on-disk build into topic-keyed producer/consumer rows with a
//! match / mismatch / unknown verdict on the MESSAGE_TYPE cell (A12.1).
//!
//! Every case is a real repo written to a temp dir and built by the engine, so
//! the rows are exactly what `glia contracts` (A12.3) will print. Pairing comes
//! from the QUEUE_FLOWS edges `QueueStackResolver` emitted; this file checks
//! that the report neither invents a pair the resolver refused (framework
//! tags) nor loses one it made.

use std::path::{Path, PathBuf};

use repo_graph_engine::{MessageContractRow, generate_many, generate_one, message_contracts};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A Go NATS publisher of `pb.<ty>` on `topic`.
fn go_publisher(topic: &str, ty: &str) -> String {
    format!(
        "package svc\n\nimport (\n\t\"github.com/nats-io/nats.go\"\n\t\"google.golang.org/protobuf/proto\"\n\n\tpb \"example.com/demo/gen\"\n)\n\n\
         // Publish marshals a {ty} and publishes it.\n\
         func Publish(nc *nats.Conn, id string) error {{\n\
         \tdata, err := proto.Marshal(&pb.{ty}{{Id: id}})\n\
         \tif err != nil {{\n\t\treturn err\n\t}}\n\
         \treturn nc.Publish(\"{topic}\", data)\n}}\n"
    )
}

/// A Go NATS subscriber that decodes `topic` as `pb.<ty>`. `*nats.Msg` is on
/// the A12.1 deny list, so the handler's own type never wins.
fn go_subscriber(topic: &str, ty: &str) -> String {
    format!(
        "package worker\n\nimport (\n\t\"github.com/nats-io/nats.go\"\n\t\"google.golang.org/protobuf/proto\"\n\n\tpb \"example.com/demo/gen\"\n)\n\n\
         // Subscribe decodes every message as a {ty}.\n\
         func Subscribe(nc *nats.Conn) (*nats.Subscription, error) {{\n\
         \treturn nc.Subscribe(\"{topic}\", func(m *nats.Msg) {{\n\
         \t\tevt := &pb.{ty}{{}}\n\
         \t\t_ = proto.Unmarshal(m.Data, evt)\n\
         \t}})\n}}\n"
    )
}

/// A C# Kafka publisher whose topic is a FIELD, not a literal — the extractor
/// can only mint the `queue_producer:unresolved:kafka` framework tag for it.
const CS_TAG_PRODUCER: &str = "using System.Threading.Tasks;\nusing Confluent.Kafka;\nusing Demo.Events;\n\n\
namespace Demo.Ordering;\n\npublic class OrderPublisher\n{\n\
    private readonly IProducer<Null, OrderCreated> _producer;\n\
    private readonly string _topic;\n\n\
    public async Task PublishAsync(OrderCreated evt)\n    {\n\
        await _producer.ProduceAsync(_topic, new Message<Null, OrderCreated> { Value = evt });\n\
    }\n}\n";

/// The consumer half of the same shape: `Subscribe(_topic)` → the
/// `queue_consumer:unresolved:kafka` tag.
const CS_TAG_CONSUMER: &str = "using Confluent.Kafka;\nusing Demo.Events;\n\n\
namespace Demo.Shipping;\n\npublic class OrderConsumer\n{\n\
    private readonly IConsumer<Ignore, OrderCreated> _consumer;\n\
    private readonly string _topic;\n\n\
    public void Run()\n    {\n\
        _consumer.Subscribe(_topic);\n\
        var result = _consumer.Consume();\n\
    }\n}\n";

fn repo_dir(tmp: &tempfile::TempDir, name: &str) -> PathBuf {
    let p = tmp.path().join(name);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn rows_one(root: &Path) -> Vec<MessageContractRow> {
    message_contracts(&generate_one(root.to_str().unwrap()).unwrap().merged)
}

#[test]
fn go_nats_same_type_is_one_match_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo_dir(&tmp, "shop");
    write(&root, "server/publisher.go", &go_publisher("orders", "OrderCreated"));
    write(&root, "client/consumer.go", &go_subscriber("orders", "OrderCreated"));

    let rows = rows_one(&root);
    assert_eq!(rows.len(), 1, "one producer + one consumer = one row: {rows:#?}");
    let r = &rows[0];
    assert_eq!(r.topic, "orders");
    assert!(!r.topic_is_tag);
    assert!(!r.pattern);
    assert_eq!(r.status, "match");
    assert_eq!(r.confidence, "strong");
    let p = r.producer.as_ref().expect("producer side");
    let c = r.consumer.as_ref().expect("consumer side");
    assert_eq!(p.qname, "queue_producer:orders");
    assert_eq!(c.qname, "queue_consumer:orders");
    assert_eq!(p.message_type.as_deref(), Some("OrderCreated"));
    assert_eq!(p.message_type_raw.as_deref(), Some("pb.OrderCreated"));
    assert_eq!(p.form.as_deref(), Some("struct_literal"));
    assert_eq!(p.window.as_deref(), Some("near"));
    assert_eq!(c.message_type.as_deref(), Some("OrderCreated"));
    // Located: the queue node's own POSITION (A2.8), not the module fallback.
    assert_eq!(p.file.as_deref(), Some("server/publisher.go"));
    assert_eq!(c.file.as_deref(), Some("client/consumer.go"));
    assert!(p.line.is_some() && c.line.is_some());
    assert!(p.module.is_some(), "the parent MODULE qname is carried");
}

#[test]
fn go_nats_differing_types_across_repos_is_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = repo_dir(&tmp, "svc");
    let worker = repo_dir(&tmp, "worker");
    write(&svc, "publisher.go", &go_publisher("shipments", "ShipmentCreated"));
    write(&worker, "consumer.go", &go_subscriber("shipments", "ShipmentDispatched"));

    let paths = vec![svc.to_str().unwrap().to_string(), worker.to_str().unwrap().to_string()];
    let rows = message_contracts(&generate_many(&paths).unwrap().merged);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let r = &rows[0];
    assert_eq!(r.topic, "shipments");
    assert_eq!(r.status, "mismatch");
    assert_eq!(r.confidence, "strong");
    let (p, c) = (r.producer.as_ref().unwrap(), r.consumer.as_ref().unwrap());
    assert_eq!(p.message_type.as_deref(), Some("ShipmentCreated"));
    assert_eq!(c.message_type.as_deref(), Some("ShipmentDispatched"));
    assert_ne!(p.repo_id, c.repo_id, "the pair crosses a repo boundary");
}

#[test]
fn csharp_framework_tag_is_one_unpaired_unknown_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo_dir(&tmp, "ordering");
    write(&root, "Publisher.cs", CS_TAG_PRODUCER);

    let rows = rows_one(&root);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let r = &rows[0];
    assert!(r.topic_is_tag, "topic {} must be flagged as a tag", r.topic);
    assert_eq!(r.topic, "unresolved:kafka");
    assert_eq!(r.status, "unknown");
    assert_eq!(r.confidence, "none");
    assert!(r.consumer.is_none());
    assert!(r.note.is_some_and(|n| n.contains("framework tag")), "{:?}", r.note);
    // The type is still reported — only the PAIRING is withheld.
    let p = r.producer.as_ref().unwrap();
    assert_eq!(p.message_type.as_deref(), Some("OrderCreated"));
}

/// The falsifiable fan-out guard: 3 tag producers × 4 tag consumers must be 7
/// one-sided rows, never the 12-row cross product (nor 12 + 7).
#[test]
fn tag_topics_never_cross_product() {
    let tmp = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for i in 0..3 {
        let d = repo_dir(&tmp, &format!("pub{i}"));
        write(&d, "Publisher.cs", CS_TAG_PRODUCER);
        paths.push(d.to_str().unwrap().to_string());
    }
    for i in 0..4 {
        let d = repo_dir(&tmp, &format!("sub{i}"));
        write(&d, "Consumer.cs", CS_TAG_CONSUMER);
        paths.push(d.to_str().unwrap().to_string());
    }
    let rows = message_contracts(&generate_many(&paths).unwrap().merged);
    assert_eq!(rows.len(), 7, "3 + 4 one-sided rows, not 3 x 4: {rows:#?}");
    assert!(rows.iter().all(|r| r.topic_is_tag && r.status == "unknown"));
    assert!(rows.iter().all(|r| r.producer.is_none() != r.consumer.is_none()));
    assert_eq!(rows.iter().filter(|r| r.producer.is_some()).count(), 3);
    assert_eq!(rows.iter().filter(|r| r.consumer.is_some()).count(), 4);
}

#[test]
fn producer_only_literal_topic_names_the_missing_counterpart() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo_dir(&tmp, "audit");
    write(&root, "publisher.go", &go_publisher("audit", "AuditEntry"));

    let rows = rows_one(&root);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let r = &rows[0];
    assert_eq!(r.topic, "audit");
    assert!(!r.topic_is_tag);
    assert_eq!(r.status, "unknown");
    assert!(r.consumer.is_none());
    assert_eq!(r.note, Some("no counterpart for this topic"));
    assert_eq!(
        r.producer.as_ref().unwrap().message_type.as_deref(),
        Some("AuditEntry")
    );
}

/// A primitive payload (`Message<Null, string>`) is usually a serialised body,
/// so `string` against `OrderCreated` is NOT evidence of a broken contract —
/// the report must not accuse it.
#[test]
fn primitive_payload_is_unknown_not_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo_dir(&tmp, "kafka");
    write(
        &root,
        "pub/Publisher.cs",
        "using System.Threading.Tasks;\nusing Confluent.Kafka;\n\nnamespace Demo.Ordering;\n\n\
         public class OrderPublisher\n{\n    private readonly IProducer<Null, string> _producer;\n\n\
         public async Task PublishAsync(string json)\n    {\n\
         await _producer.ProduceAsync(\"orders\", new Message<Null, string> { Value = json });\n    }\n}\n",
    );
    write(
        &root,
        "sub/Consumer.cs",
        "using Confluent.Kafka;\nusing Demo.Events;\n\nnamespace Demo.Shipping;\n\n\
         public class OrderConsumer\n{\n    private readonly IConsumer<Ignore, OrderCreated> _consumer;\n\n\
         public void Run()\n    {\n        _consumer.Subscribe(\"orders\");\n\
         ConsumeResult<Ignore, OrderCreated> result = _consumer.Consume();\n    }\n}\n",
    );

    let rows = rows_one(&root);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let r = &rows[0];
    assert_eq!(r.topic, "orders");
    assert_eq!(r.producer.as_ref().unwrap().message_type.as_deref(), Some("string"));
    assert_eq!(r.consumer.as_ref().unwrap().message_type.as_deref(), Some("OrderCreated"));
    assert_eq!(r.status, "unknown", "a primitive side is not comparable");
    assert!(r.note.is_some_and(|n| n.contains("primitive")), "{:?}", r.note);
}

#[test]
fn report_is_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo_dir(&tmp, "mixed");
    write(&root, "server/publisher.go", &go_publisher("orders", "OrderCreated"));
    write(&root, "client/consumer.go", &go_subscriber("orders", "OrderCreated"));
    write(&root, "server/audit.go", &go_publisher("audit", "AuditEntry"));
    write(&root, "billing/Publisher.cs", CS_TAG_PRODUCER);

    let merged = generate_one(root.to_str().unwrap()).unwrap().merged;
    let a = serde_json::to_string(&message_contracts(&merged)).unwrap();
    let b = serde_json::to_string(&message_contracts(&merged)).unwrap();
    assert_eq!(a, b, "two calls over one graph serialise identically");

    // And across two independent builds (fresh HashMap seeds in the navs).
    let merged2 = generate_one(root.to_str().unwrap()).unwrap().merged;
    let c = serde_json::to_string(&message_contracts(&merged2)).unwrap();
    assert_eq!(a, c, "two builds of one repo serialise identically");

    let rows = message_contracts(&merged);
    let topics: Vec<&str> = rows.iter().map(|r| r.topic.as_str()).collect();
    assert_eq!(topics, vec!["audit", "orders", "unresolved:kafka"], "topic-sorted");
}
